#![no_std]
#![no_main]

mod artwork_pool;
mod display;
mod net;
mod touch;

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use app_config::{AppConfig, WifiConfig};
use app_core::{
    App, Button, Command, Event, HifiArtwork, HifiCommand, HifiPins, HifiStatus, NetworkStatus,
    RECOMMENDED_SCRATCH_PIXELS, RenderSession, Screen,
};
use app_runtime::hifi::{DriverError as HifiDriverError, HifiDriver};
use app_runtime::lpec::{
    ARTWORK_DECODE_BUFFER_BYTES, ARTWORK_HTTP_BUFFER_BYTES, Error as LpecError, LpecSession,
    load_artwork_with_buffers_into,
};
use app_runtime::net::Endpoint;
use app_runtime::{HifiController, RuntimeError};
use artwork_pool::ArtworkPool;
use board_waveshare_c6::{BOARD_NAME, DISPLAY_SIZE, peripherals};
use core::sync::atomic::{AtomicBool, Ordering};
use display::AmoledDisplay;
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::StackResources;
use embassy_sync::{
    blocking_mutex::raw::CriticalSectionRawMutex,
    channel::{Channel, TryReceiveError},
};
use embassy_time::{Duration as EmbassyDuration, Instant as EmbassyInstant, Timer};
use embedded_graphics::{draw_target::DrawTarget, pixelcolor::Rgb565, prelude::RgbColor};
use esp_backtrace as _;
use esp_hal::{
    gpio::{Input, InputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    interrupt::software::SoftwareInterruptControl,
    peripherals::{GPIO2, GPIO9, WIFI},
    ram,
    time::{Duration, Instant, Rate},
    timer::timg::TimerGroup,
};
use esp_println::println;
use esp_radio::wifi::{
    Config as WifiRadioConfig, ControllerConfig, DisconnectReason, Interfaces, WifiController,
    WifiError, ap::AccessPointInfo, event::EventInfo, scan::ScanConfig, sta::StationConfig,
};
use net::{
    ARTWORK_RX_BUFFER_BYTES, ARTWORK_TX_BUFFER_BYTES, FirmwareNetError, FirmwareNetwork,
    NetBuffers, TCP_RX_BUFFER_BYTES, TCP_TX_BUFFER_BYTES,
};
use static_cell::StaticCell;
use touch::{FT6146_ADDRESS, TouchController};

esp_bootloader_esp_idf::esp_app_desc!();

const TOUCH_POLL_MS: u64 = 50;
const HEARTBEAT_MS: u64 = 1_000;
const LPEC_EVENT_POLL_MS: u64 = 100;
const WIFI_CONNECT_ATTEMPTS: u8 = 5;
/// Bounce window: any falling edge within this of the last accepted press is
/// discarded. The Waveshare BOOT button is mechanically clean enough that
/// 50 ms is plenty without making the UI feel laggy.
const BUTTON_DEBOUNCE_MS: u64 = 50;
/// Hold duration on PWR that counts as a shutdown request. Anything shorter is
/// treated as an accidental tap and ignored.
const PWR_LONG_PRESS_MS: u64 = 1_500;
/// TCA9554 I2C address on this board.
const TCA9554_ADDR: u8 = 0x20;
/// TCA9554 Output Port register. Boot init writes `0xc0` here (EXIO6 BAT_EN +
/// EXIO7 PA_CTRL high). Releasing the battery latch means clearing bit 6 while
/// leaving the audio-amp gate (bit 7) alone.
const TCA9554_OUTPUT_REG: u8 = 0x01;
const TCA9554_OUTPUT_BAT_RELEASED: u8 = 0x80;
/// Contents of `config/local.env`, or of `config/local.env.example` when that
/// gitignored file is absent. `build.rs` decides which and says so when it
/// falls back.
const LOCAL_CONFIG: &str = include_str!(env!("LOCAL_CONFIG_PATH"));

static FIRMWARE_EVENTS: Channel<CriticalSectionRawMutex, FirmwareEvent, 1> = Channel::new();
static HIFI_REQUESTS: Channel<CriticalSectionRawMutex, HifiRequest, 4> = Channel::new();
static POWER_OFF_REQUESTED: AtomicBool = AtomicBool::new(false);

enum FirmwareEvent {
    App(Event),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HifiRequest {
    SetActive(bool),
    Command(HifiCommand),
}

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let config = esp_hal::Config::default();
    let peripherals = esp_hal::init(config);
    esp_alloc::heap_allocator!(#[ram(reclaimed)] size: 64 * 1024);
    // Bumped from 36 KB to 64 KB to give esp-radio, embassy-net, and the JPEG
    // artwork decoder comfortable headroom. The original budget was sitting
    // a few KB above failure; a 3 KB alloc inside zune_jpeg's APP2 (ICC
    // profile) parser was failing once `LpecSession::response_args` started
    // boxing its 16 KB args buffer. The C6's stack region is enormous (most
    // of the remaining RWDATA), so trading ~28 KB of stack for heap is free.
    esp_alloc::heap_allocator!(size: 64 * 1024);

    let app_config = firmware_config();

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    let sw_interrupt = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, sw_interrupt.software_interrupt0);

    println!("boot: {BOARD_NAME}");
    println!(
        "display: {} {}x{}",
        peripherals::DISPLAY_CONTROLLER,
        DISPLAY_SIZE.width,
        DISPLAY_SIZE.height
    );
    println!(
        "touch: {}, imu: {}, rtc: {}, gpio expander: {}",
        peripherals::TOUCH_CONTROLLER,
        peripherals::IMU,
        peripherals::RTC,
        peripherals::GPIO_EXPANDER
    );

    let mut app = App::new();
    let mut touch = TouchController::new();

    let mut i2c = match I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(100)),
    ) {
        Ok(i2c) => i2c.with_sda(peripherals.GPIO18).with_scl(peripherals.GPIO8),
        Err(error) => {
            println!("power: i2c init failed: {:?}", error);
            loop {
                wait_millis(1_000);
            }
        }
    };

    match i2c
        .write(0x20_u8, &[0x01, 0xc0])
        .and_then(|()| i2c.write(0x20_u8, &[0x03, 0x3f]))
        .and_then(|()| i2c.write(0x20_u8, &[0x01, 0xc0]))
    {
        Ok(()) => {
            println!("power: tca9554 pins 6/7 enabled");
            wait_millis(50);
        }
        Err(error) => println!("power: tca9554 enable failed: {:?}", error),
    }

    println!("touch: FT6146 polling at i2c address 0x{FT6146_ADDRESS:02x}");

    let mut display = match AmoledDisplay::new(
        peripherals.SPI2,
        peripherals.GPIO11,
        peripherals.GPIO4,
        peripherals.GPIO5,
        peripherals.GPIO6,
        peripherals.GPIO7,
        peripherals.GPIO10,
        peripherals.GPIO3,
    ) {
        Ok(display) => {
            println!("display: initialized");
            display
        }
        Err(error) => {
            println!("display: init failed: {}", error);
            loop {
                wait_millis(1_000);
            }
        }
    };

    if let Err(error) = display.clear(Rgb565::BLACK) {
        println!("display: initial clear failed: {}", error);
    }

    // One session per render target, living as long as the display does. It
    // starts empty, which matches the panel we just cleared.
    let mut session = RenderSession::new();

    // Scratch buffer for the painter's text-band fast path. Lives on the heap
    // — vec! goes straight to the allocator, avoiding a 30+ KB stack alloc.
    let mut scratch: Vec<Rgb565> = vec![Rgb565::BLACK; RECOMMENDED_SCRATCH_PIXELS];
    println!(
        "display: scratch buffer {} px ({} bytes)",
        scratch.len(),
        scratch.len() * 2
    );

    match app.render(&mut display, &mut scratch, &mut session) {
        Ok(()) => {
            println!("display: initial frame rendered");
            match display.set_brightness(0xff) {
                Ok(()) => println!("display: brightness enabled"),
                Err(error) => println!("display: brightness enable failed: {}", error),
            }
        }
        Err(_) => println!("display: initial render failed"),
    }

    spawner
        .spawn(boot_button_task(peripherals.GPIO9).expect("boot-button task should allocate once"));
    spawner
        .spawn(pwr_button_task(peripherals.GPIO2).expect("pwr-button task should allocate once"));

    let booted_at = Instant::now();
    let mut next_heartbeat_ms = 0;
    let mut consecutive_touch_errors = 0_u32;
    let mut hifi_screen_active = app.screen() == Screen::HifiControl;

    println!("app-core: initialized");

    if app_config.wifi.ssid.is_some() && app_config.wifi.password.is_some() {
        let render_requested = app
            .update(Event::NetworkStatus(NetworkStatus::Connecting))
            .render_requested;
        let _ = render_app(
            &mut app,
            &mut display,
            &mut scratch,
            &mut session,
            render_requested,
        );
        spawner.spawn(
            firmware_runtime_task(
                spawner,
                peripherals.WIFI,
                app_config.wifi.clone(),
                app_config.linn_lpec_endpoint,
            )
            .expect("wifi task should allocate once"),
        );
    } else {
        println!("wifi: missing WIFI_SSID/WIFI_PASSWORD");
    }

    loop {
        if POWER_OFF_REQUESTED.load(Ordering::Acquire) {
            println!("power: shutting down");
            if let Err(error) = display.set_brightness(0x00) {
                println!("power: brightness off failed: {}", error);
            }
            // Brief grace so the brightness write and logs make it out before
            // the rails collapse on battery.
            Timer::after(EmbassyDuration::from_millis(100)).await;
            match i2c.write(
                TCA9554_ADDR,
                &[TCA9554_OUTPUT_REG, TCA9554_OUTPUT_BAT_RELEASED],
            ) {
                Ok(()) => {
                    println!("power: battery latch released; on battery the device will power off")
                }
                Err(error) => println!("power: latch release failed: {:?}", error),
            }
            // On battery, execution stops here as VBAT drops out. On USB the
            // rails stay up — yield to the executor and wait visibly forever.
            loop {
                Timer::after(EmbassyDuration::from_secs(60)).await;
            }
        }

        let uptime_ms = booted_at.elapsed().as_millis() as u64;
        let mut render_requested = false;
        let mut frame_rendered = false;
        let mut pending_command = None;

        loop {
            match FIRMWARE_EVENTS.try_receive() {
                Ok(FirmwareEvent::App(event)) => {
                    render_requested |= app.update(event).render_requested;
                }
                Err(TryReceiveError::Empty) => break,
            }
        }

        match touch.poll(&mut i2c) {
            Ok(Some(event)) => {
                if consecutive_touch_errors >= 10 {
                    println!("touch: polling recovered");
                }
                consecutive_touch_errors = 0;
                if let Event::TouchDown(point) = event {
                    println!("touch: down x={} y={}", point.x, point.y);
                }
                let outcome = app.update(event);
                render_requested |= outcome.render_requested;
                pending_command = outcome.command;
            }
            Ok(None) => {
                if consecutive_touch_errors >= 10 {
                    println!("touch: polling recovered");
                }
                consecutive_touch_errors = 0;
            }
            Err(error) => {
                consecutive_touch_errors = consecutive_touch_errors.saturating_add(1);
                if consecutive_touch_errors == 10 || consecutive_touch_errors % 100 == 0 {
                    println!(
                        "touch: poll failed {} times: {:?}",
                        consecutive_touch_errors, error
                    );
                }
            }
        }

        let outcome = app.update(Event::Tick { uptime_ms });
        render_requested |= outcome.render_requested;
        sync_hifi_screen_focus(&app, &mut hifi_screen_active);

        frame_rendered |= render_app(
            &mut app,
            &mut display,
            &mut scratch,
            &mut session,
            render_requested,
        );

        if let Some(command) = pending_command {
            send_hifi_command(command);
        }

        if uptime_ms >= next_heartbeat_ms {
            println!(
                "heartbeat: uptime={}ms interactions={} redraw={}",
                app.uptime_ms(),
                app.interaction_count(),
                frame_rendered
            );
            next_heartbeat_ms = next_heartbeat_ms.saturating_add(HEARTBEAT_MS);
        }

        Timer::after(EmbassyDuration::from_millis(TOUCH_POLL_MS)).await;
    }
}

async fn connect_wifi<'d>(wifi: WIFI<'d>, config: &WifiConfig) -> WifiConnection<'d> {
    let (Some(ssid), Some(password)) = (&config.ssid, &config.password) else {
        println!("wifi: missing WIFI_SSID/WIFI_PASSWORD");
        return WifiConnection::MissingCredentials;
    };

    println!("wifi: connecting to configured SSID");
    let target_ssid = ssid.as_str();
    let base_station_config = StationConfig::default()
        .with_ssid(target_ssid)
        .with_password(password.as_str().into());
    let mut station_config = base_station_config.clone();

    let (mut controller, interfaces) = match esp_radio::wifi::new(
        wifi,
        ControllerConfig::default()
            .with_initial_config(WifiRadioConfig::Station(station_config.clone())),
    ) {
        Ok(wifi) => wifi,
        Err(error) => {
            println!("wifi: init failed: {:?}", error);
            return WifiConnection::Failed;
        }
    };

    let mut scan_fallback_used = false;

    for attempt in 1..=WIFI_CONNECT_ATTEMPTS {
        if let Err(error) = controller.set_config(&WifiRadioConfig::Station(station_config.clone()))
        {
            println!("wifi: station reconfigure failed: {:?}", error);
        }

        match controller.connect_async().await {
            Ok(info) => {
                println!(
                    "wifi: connected: channel={} auth={:?}",
                    info.channel, info.authmode
                );
                return WifiConnection::Connected {
                    controller,
                    interfaces,
                };
            }
            Err(error) => {
                log_wifi_error("connect failed", attempt, error);
                let _ = controller.disconnect_async().await;
                if attempt < WIFI_CONNECT_ATTEMPTS
                    && !scan_fallback_used
                    && should_scan_for_access_point(error)
                {
                    scan_fallback_used = true;
                    if let Some(access_point) =
                        scan_configured_access_point(&mut controller, target_ssid).await
                    {
                        station_config = station_config_for_access_point(
                            base_station_config.clone(),
                            &access_point,
                        );
                    } else {
                        station_config = base_station_config.clone();
                    }
                }
                Timer::after(EmbassyDuration::from_millis(250)).await;
            }
        }
    }

    WifiConnection::Failed
}

enum WifiConnection<'d> {
    Connected {
        controller: WifiController<'d>,
        interfaces: Interfaces<'d>,
    },
    MissingCredentials,
    Failed,
}

#[embassy_executor::task]
async fn boot_button_task(pin: GPIO9<'static>) {
    // GPIO9 = the BOOT key on the Waveshare ESP32-C6 Touch AMOLED 1.43.
    // Active low (button to GND), internal pull-up; interrupt-driven via
    // esp-hal's async GPIO support — no polling on the main loop.
    let mut input = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
    let mut last_press_ms: u64 = 0;
    loop {
        input.wait_for_falling_edge().await;
        let now_ms = EmbassyInstant::now().as_millis();
        if now_ms.saturating_sub(last_press_ms) < BUTTON_DEBOUNCE_MS {
            continue;
        }
        last_press_ms = now_ms;
        // This board has one software-readable key, so it maps to the "up one
        // level" control rather than any of the Kode Dot's pad directions.
        send_app_event(Event::ButtonPressed(Button::Back)).await;
    }
}

#[embassy_executor::task]
async fn pwr_button_task(pin: GPIO2<'static>) {
    // GPIO2 = PWR_KEY on the Waveshare ESP32-C6 Touch AMOLED 1.43. Active low
    // via the board's 10K pull-up to 3V3. Holding this is what turned the
    // device on in the first place, so the line may still be low when this
    // task starts running — wait for the initial release before arming
    // long-press detection, otherwise the boot press would self-trigger.
    let mut input = Input::new(pin, InputConfig::default().with_pull(Pull::Up));
    input.wait_for_high().await;
    println!("power: PWR armed");

    loop {
        input.wait_for_falling_edge().await;
        let press_start = EmbassyInstant::now();
        match select(
            input.wait_for_rising_edge(),
            Timer::after(EmbassyDuration::from_millis(PWR_LONG_PRESS_MS)),
        )
        .await
        {
            Either::First(()) => {
                let held = press_start.elapsed().as_millis();
                println!("power: PWR short press ({} ms), ignored", held);
            }
            Either::Second(()) => {
                if input.is_low() {
                    println!(
                        "power: PWR long press (>= {} ms), shutdown requested",
                        PWR_LONG_PRESS_MS
                    );
                    POWER_OFF_REQUESTED.store(true, Ordering::Release);
                    return;
                }
                println!("power: PWR long-press timer fired but line is high, ignoring");
            }
        }
    }
}

/// Listens for `StationDisconnected` and re-runs `connect_async`. Without this
/// the application sees no signal when the AP drops the station (roaming
/// hand-off, AP reboot, power-save renegotiation) and the network stack just
/// sits on a dead link until every operation has timed out.
#[embassy_executor::task]
async fn wifi_reconnect_task(mut controller: WifiController<'static>, config: WifiConfig) {
    let (Some(ssid), Some(password)) = (&config.ssid, &config.password) else {
        return;
    };
    let station_config = StationConfig::default()
        .with_ssid(ssid.as_str())
        .with_password(password.as_str().into());

    loop {
        // The subscriber borrows the controller immutably; its scope ends
        // before we touch the controller mutably below.
        wait_for_station_disconnect(&controller).await;
        println!("wifi: link dropped — attempting reconnect");
        // Small backoff so we don't hammer the controller if the AP just
        // bounced.
        Timer::after(EmbassyDuration::from_millis(500)).await;
        if let Err(error) = controller.set_config(&WifiRadioConfig::Station(station_config.clone()))
        {
            println!("wifi: reconnect reconfigure failed: {:?}", error);
            continue;
        }
        match controller.connect_async().await {
            Ok(info) => println!(
                "wifi: reconnected: channel={} auth={:?}",
                info.channel, info.authmode
            ),
            Err(error) => println!("wifi: reconnect failed: {:?}", error),
        }
    }
}

async fn wait_for_station_disconnect(controller: &WifiController<'_>) {
    let mut subscriber = match controller.subscribe() {
        Ok(subscriber) => subscriber,
        Err(error) => {
            // If the event-channel subscriber slots are full we can't drive
            // reconnects. Back off so this doesn't become a tight loop and
            // let the caller retry the connect path periodically.
            println!("wifi: reconnect subscribe failed: {:?}", error);
            Timer::after(EmbassyDuration::from_secs(30)).await;
            return;
        }
    };
    loop {
        if matches!(
            subscriber.next_event_pure().await,
            EventInfo::StationDisconnected { .. }
        ) {
            return;
        }
    }
}

#[embassy_executor::task]
async fn firmware_runtime_task(
    spawner: Spawner,
    wifi: WIFI<'static>,
    config: WifiConfig,
    endpoint: Endpoint,
) {
    match connect_wifi(wifi, &config).await {
        WifiConnection::Connected {
            controller,
            interfaces,
        } => {
            // Hand the controller to a dedicated reconnect task. Roaming
            // (or any AP-initiated disassociation) emits StationDisconnected;
            // without a listener nothing in app code notices and the next
            // operation just times out into the void.
            spawner.spawn(
                wifi_reconnect_task(controller, config.clone())
                    .expect("wifi reconnect task should allocate once"),
            );
            // Statically reserve every long-lived buffer the network stack and
            // artwork loader need. `StaticCell::init` panics if called twice,
            // which is fine — `firmware_runtime_task` is spawned exactly once.
            static NET_RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();
            static LPEC_RX: StaticCell<[u8; TCP_RX_BUFFER_BYTES]> = StaticCell::new();
            static LPEC_TX: StaticCell<[u8; TCP_TX_BUFFER_BYTES]> = StaticCell::new();
            static ARTWORK_RX: StaticCell<[u8; ARTWORK_RX_BUFFER_BYTES]> = StaticCell::new();
            static ARTWORK_TX: StaticCell<[u8; ARTWORK_TX_BUFFER_BYTES]> = StaticCell::new();
            static HTTP_BUFFER: StaticCell<[u8; ARTWORK_HTTP_BUFFER_BYTES]> = StaticCell::new();
            static DECODE_BUFFER: StaticCell<[u8; ARTWORK_DECODE_BUFFER_BYTES]> = StaticCell::new();

            let buffers = NetBuffers {
                resources: NET_RESOURCES.init(StackResources::new()),
                lpec_rx: LPEC_RX.init([0; TCP_RX_BUFFER_BYTES]),
                lpec_tx: LPEC_TX.init([0; TCP_TX_BUFFER_BYTES]),
                artwork_rx: ARTWORK_RX.init([0; ARTWORK_RX_BUFFER_BYTES]),
                artwork_tx: ARTWORK_TX.init([0; ARTWORK_TX_BUFFER_BYTES]),
            };
            let mut network = FirmwareNetwork::new(interfaces.station, buffers);
            let http_buffer = HTTP_BUFFER.init([0; ARTWORK_HTTP_BUFFER_BYTES]);
            let decode_buffer = DECODE_BUFFER.init([0; ARTWORK_DECODE_BUFFER_BYTES]);
            let artwork_pool = ArtworkPool::new();

            loop {
                match network.poll_config_up() {
                    Ok(true) => {
                        if let Some(config) = network.config_v4() {
                            println!("net: dhcp address {}", config.address);
                        }
                        send_app_event(Event::NetworkStatus(NetworkStatus::Online)).await;
                        let hifi = FirmwareHifi::new(
                            network,
                            endpoint,
                            http_buffer,
                            decode_buffer,
                            artwork_pool,
                        );
                        hifi_runtime_loop(HifiDriver::new(hifi, LPEC_EVENT_POLL_MS)).await;
                    }
                    Ok(false) => {
                        Timer::after(EmbassyDuration::from_millis(TOUCH_POLL_MS)).await;
                    }
                    Err(error) => {
                        println!("net: dhcp failed: {:?}", error);
                        send_app_event(Event::NetworkStatus(NetworkStatus::Offline)).await;
                        break;
                    }
                }
            }
        }
        WifiConnection::MissingCredentials | WifiConnection::Failed => {
            send_app_event(Event::NetworkStatus(NetworkStatus::Offline)).await;
        }
    }
}

async fn hifi_runtime_loop(mut driver: HifiDriver<FirmwareHifi<'static>>) -> ! {
    loop {
        while let Ok(request) = HIFI_REQUESTS.try_receive() {
            match request {
                HifiRequest::SetActive(active) => {
                    driver.set_active(active, EmbassyInstant::now().as_millis());
                }
                HifiRequest::Command(command) => {
                    apply_hifi_driver_result(driver.handle_command(command)).await;
                }
            }
        }

        let uptime_ms = EmbassyInstant::now().as_millis();
        apply_hifi_driver_result(driver.poll_status_if_due(uptime_ms)).await;
        apply_hifi_driver_result(driver.load_pending_artwork()).await;
        apply_hifi_driver_result(driver.fetch_pins_if_needed()).await;

        Timer::after(EmbassyDuration::from_millis(TOUCH_POLL_MS)).await;
    }
}

struct FirmwareHifi<'a> {
    network: FirmwareNetwork,
    endpoint: Endpoint,
    session: LpecSession,
    pending_status: Option<HifiStatus>,
    http_buffer: &'a mut [u8],
    decode_buffer: &'a mut [u8],
    artwork_pool: ArtworkPool,
}

#[derive(Debug)]
enum FirmwareHifiError {
    Idle,
    Net(FirmwareNetError),
    Lpec(LpecError<FirmwareNetError>),
}

impl<'a> FirmwareHifi<'a> {
    fn new(
        network: FirmwareNetwork,
        endpoint: Endpoint,
        http_buffer: &'a mut [u8],
        decode_buffer: &'a mut [u8],
        artwork_pool: ArtworkPool,
    ) -> Self {
        Self {
            network,
            endpoint,
            session: LpecSession::new(),
            pending_status: None,
            http_buffer,
            decode_buffer,
            artwork_pool,
        }
    }

    fn reset_lpec(&mut self) {
        self.session.reset();
        self.network.reset_lpec();
    }
}

impl HifiController for FirmwareHifi<'_> {
    type Error = FirmwareHifiError;

    fn handle_command(&mut self, command: HifiCommand) -> Result<(), Self::Error> {
        println!("linn: command {:?}", command);
        let result = {
            let mut stream = self
                .network
                .connect(self.endpoint)
                .map_err(FirmwareHifiError::Net)?;
            self.session.handle_command(&mut stream, command)
        };

        match result {
            Ok(status) => {
                self.pending_status = status;
                println!("linn: command sent");
                Ok(())
            }
            Err(error) => {
                println!("linn: command failed: {:?}", error);
                self.reset_lpec();
                Err(FirmwareHifiError::Lpec(error))
            }
        }
    }

    fn status(&mut self) -> Result<HifiStatus, Self::Error> {
        if let Some(status) = self.pending_status.take() {
            return Ok(status);
        }

        let result = {
            let mut stream = self
                .network
                .connect_events(self.endpoint)
                .map_err(FirmwareHifiError::Net)?;
            // The session expires optimistic skips against this, so it wants
            // wall-clock progress rather than anything app-relative.
            self.session
                .poll(&mut stream, EmbassyInstant::now().as_millis())
        };

        match result {
            Ok(Some(status)) => Ok(status),
            Ok(None) => self.session.live_status().ok_or(FirmwareHifiError::Idle),
            Err(LpecError::Connect(
                FirmwareNetError::ConnectTimeout | FirmwareNetError::ReadTimeout,
            )) => self.session.live_status().ok_or(FirmwareHifiError::Idle),
            Err(error) => {
                println!("linn: session poll failed: {:?}", error);
                self.reset_lpec();
                Err(FirmwareHifiError::Lpec(error))
            }
        }
    }

    fn artwork(&mut self, uri: &str) -> Result<HifiArtwork, Self::Error> {
        println!("linn: artwork load {}", uri);
        let pixels = self.artwork_pool.acquire();
        load_artwork_with_buffers_into(
            &mut self.network,
            uri,
            self.http_buffer,
            self.decode_buffer,
            pixels,
        )
        .map_err(FirmwareHifiError::Lpec)
    }

    fn pins(&mut self) -> Result<HifiPins, Self::Error> {
        let result = {
            let mut stream = self
                .network
                .connect(self.endpoint)
                .map_err(FirmwareHifiError::Net)?;
            self.session.fetch_pins(&mut stream)
        };

        match result {
            Ok(pins) => Ok(pins),
            Err(LpecError::Protocol(::linn_lpec::Error::Remote { code, description })) => {
                println!(
                    "linn: pins not available (code {}: {})",
                    code,
                    description.as_str()
                );
                Err(FirmwareHifiError::Idle)
            }
            Err(error) => {
                println!("linn: pins fetch failed: {:?}", error);
                // Same pattern as `status` and `handle_command`: a fetch that
                // failed mid-exchange has likely left the LPEC stream
                // mis-aligned with the device, so drop the socket and
                // re-subscribe on the next call instead of writing into a
                // desynced stream forever.
                self.reset_lpec();
                Err(FirmwareHifiError::Lpec(error))
            }
        }
    }

    fn mark_track_changed(&mut self) {
        self.pending_status = None;
        self.session.clear_track_metadata();
    }
}

async fn apply_hifi_driver_result(
    result: Result<Option<Event>, HifiDriverError<FirmwareHifiError>>,
) {
    match result {
        Ok(Some(event)) => send_app_event(event).await,
        Ok(None)
        | Err(HifiDriverError::Status(RuntimeError::Hifi(FirmwareHifiError::Idle)))
        | Err(HifiDriverError::Pins(RuntimeError::Hifi(FirmwareHifiError::Idle))) => {}
        Err(error) => log_hifi_driver_error(error),
    }
}

fn log_hifi_driver_error(error: HifiDriverError<FirmwareHifiError>) {
    match error {
        HifiDriverError::Command(RuntimeError::Hifi(error)) => {
            log_firmware_hifi_error("command", error);
        }
        HifiDriverError::Status(RuntimeError::Hifi(error)) => {
            log_firmware_hifi_error("status", error);
        }
        HifiDriverError::Artwork(RuntimeError::Hifi(error)) => {
            log_firmware_hifi_error("artwork", error);
        }
        HifiDriverError::Pins(RuntimeError::Hifi(error)) => {
            log_firmware_hifi_error("pins", error);
        }
    }
}

fn log_firmware_hifi_error(context: &str, error: FirmwareHifiError) {
    match error {
        FirmwareHifiError::Idle => {}
        FirmwareHifiError::Net(error) => {
            println!("linn: {} network error: {:?}", context, error);
        }
        FirmwareHifiError::Lpec(error) => {
            println!("linn: {} protocol error: {:?}", context, error);
        }
    }
}

fn render_app<D>(
    app: &mut App,
    display: &mut D,
    scratch: &mut [Rgb565],
    session: &mut RenderSession,
    render_requested: bool,
) -> bool
where
    D: DrawTarget<Color = Rgb565>,
{
    if !render_requested {
        return false;
    }

    // Screen-change clear is handled inside App::render now.
    match app.render(display, scratch, session) {
        Ok(()) => true,
        Err(_) => {
            println!("display: render failed");
            false
        }
    }
}

fn sync_hifi_screen_focus(app: &App, hifi_screen_active: &mut bool) {
    let active = app.screen() == Screen::HifiControl;
    if *hifi_screen_active == active {
        return;
    }

    *hifi_screen_active = active;
    send_hifi_request(HifiRequest::SetActive(active));
}

fn send_hifi_command(command: Command) {
    let Command::Hifi(command) = command;
    send_hifi_request(HifiRequest::Command(command));
}

fn send_hifi_request(request: HifiRequest) {
    if HIFI_REQUESTS.try_send(request).is_err() {
        println!("hifi: request dropped; runtime queue full");
    }
}

async fn send_app_event(event: Event) {
    FIRMWARE_EVENTS.send(FirmwareEvent::App(event)).await;
}

fn firmware_config() -> AppConfig {
    let mut config = AppConfig::parse_env(LOCAL_CONFIG).unwrap_or_default();
    if let Err(error) =
        config.apply_wifi_env(option_env!("WIFI_SSID"), option_env!("WIFI_PASSWORD"))
    {
        println!("wifi: compile-time env config ignored: {:?}", error);
    }
    config
}

async fn scan_configured_access_point(
    controller: &mut WifiController<'_>,
    target_ssid: &str,
) -> Option<AccessPointInfo> {
    println!("wifi: scanning visible SSIDs");
    let scan_config = ScanConfig::default().with_show_hidden(true).with_max(32);

    match controller.scan_async(&scan_config).await {
        Ok(access_points) => {
            let mut target = None;
            for access_point in access_points.iter() {
                if access_point.ssid.as_str() == target_ssid
                    && target.as_ref().is_none_or(|current: &AccessPointInfo| {
                        access_point.signal_strength > current.signal_strength
                    })
                {
                    target = Some(access_point.clone());
                }
            }
            println!(
                "wifi: scan saw {} network(s), configured SSID exact={}",
                access_points.len(),
                target.is_some()
            );
            if let Some(access_point) = &target {
                println!(
                    "wifi: configured SSID bssid={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x} channel={} rssi={} auth={:?}",
                    access_point.bssid[0],
                    access_point.bssid[1],
                    access_point.bssid[2],
                    access_point.bssid[3],
                    access_point.bssid[4],
                    access_point.bssid[5],
                    access_point.channel,
                    access_point.signal_strength,
                    access_point.auth_method
                );
            }
            target
        }
        Err(error) => {
            println!("wifi: scan failed: {:?}", error);
            None
        }
    }
}

fn station_config_for_access_point(
    config: StationConfig,
    access_point: &AccessPointInfo,
) -> StationConfig {
    let mut config = config
        .with_bssid(access_point.bssid)
        .with_channel(access_point.channel);
    if let Some(auth_method) = access_point.auth_method {
        config = config.with_auth_method(auth_method);
    }
    config
}

fn log_wifi_error(context: &str, attempt: u8, error: WifiError) {
    match error {
        WifiError::Disconnected(info) => {
            println!(
                "wifi: {context} attempt {attempt}/{WIFI_CONNECT_ATTEMPTS}: station disconnected: {:?}",
                info.reason
            );
        }
        error => println!(
            "wifi: {context} attempt {attempt}/{WIFI_CONNECT_ATTEMPTS}: {:?}",
            error
        ),
    }
}

fn should_scan_for_access_point(error: WifiError) -> bool {
    matches!(
        error,
        WifiError::Disconnected(info)
            if matches!(
                info.reason,
                DisconnectReason::NoAccessPointFound
                    | DisconnectReason::NoAccessPointFoundWithCompatibleSecurity
                    | DisconnectReason::NoAccessPointFoundInAuthmodeThreshold
                    | DisconnectReason::NoAccessPointFoundInRssiThreshold
            )
    )
}

fn wait_millis(ms: u64) {
    let start = Instant::now();
    let duration = Duration::from_millis(ms);

    while start.elapsed() < duration {
        core::hint::spin_loop();
    }
}
