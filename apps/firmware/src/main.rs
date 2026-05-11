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
    App, Button, Command, Event, HIFI_URI_LEN, HifiArtwork, HifiCommand, HifiPins, HifiStatus,
    NetworkStatus, PlaybackState, RECOMMENDED_SCRATCH_PIXELS, Screen,
};
use app_runtime::lpec::{
    ARTWORK_DECODE_BUFFER_BYTES, ARTWORK_HTTP_BUFFER_BYTES, Error as LpecError, LpecSession,
    load_artwork_with_buffers_into,
};
use app_runtime::net::Endpoint;
use artwork_pool::ArtworkPool;
use board_waveshare_c6::{BOARD_NAME, DISPLAY_SIZE, peripherals};
use display::AmoledDisplay;
use embassy_executor::Spawner;
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
    peripherals::{GPIO9, WIFI},
    ram,
    time::{Duration, Instant, Rate},
    timer::timg::TimerGroup,
};
use esp_println::println;
use esp_radio::wifi::{
    Config as WifiRadioConfig, ControllerConfig, DisconnectReason, Interfaces, WifiController,
    WifiError, ap::AccessPointInfo, scan::ScanConfig, sta::StationConfig,
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
const LOCAL_CONFIG: &str = include_str!("../../../config/local.env");

static FIRMWARE_EVENTS: Channel<CriticalSectionRawMutex, FirmwareEvent, 1> = Channel::new();
static HIFI_REQUESTS: Channel<CriticalSectionRawMutex, HifiRequest, 4> = Channel::new();

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
    esp_alloc::heap_allocator!(size: 36 * 1024);

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

    // Scratch buffer for the painter's text-band fast path. Lives on the heap
    // — vec! goes straight to the allocator, avoiding a 30+ KB stack alloc.
    let mut scratch: Vec<Rgb565> = vec![Rgb565::BLACK; RECOMMENDED_SCRATCH_PIXELS];
    println!(
        "display: scratch buffer {} px ({} bytes)",
        scratch.len(),
        scratch.len() * 2
    );

    match app.render(&mut display, &mut scratch) {
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

    let booted_at = Instant::now();
    let mut next_heartbeat_ms = 0;
    let mut consecutive_touch_errors = 0_u32;
    let mut hifi_screen_active = app.screen() == Screen::HifiControl;

    println!("app-core: initialized");

    if app_config.wifi.ssid.is_some() && app_config.wifi.password.is_some() {
        let render_requested = app
            .update(Event::NetworkStatus(NetworkStatus::Connecting))
            .render_requested;
        let _ = render_app(&mut app, &mut display, &mut scratch, render_requested);
        spawner.spawn(
            firmware_runtime_task(
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

        frame_rendered |= render_app(&mut app, &mut display, &mut scratch, render_requested);

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
        send_app_event(Event::ButtonPressed(Button::Boot)).await;
    }
}

#[embassy_executor::task]
async fn firmware_runtime_task(wifi: WIFI<'static>, config: WifiConfig, endpoint: Endpoint) {
    match connect_wifi(wifi, &config).await {
        WifiConnection::Connected {
            controller,
            interfaces,
        } => {
            let _controller = controller;
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
            let mut artwork_pool = ArtworkPool::new();

            loop {
                match network.poll_config_up() {
                    Ok(true) => {
                        if let Some(config) = network.config_v4() {
                            println!("net: dhcp address {}", config.address);
                        }
                        send_app_event(Event::NetworkStatus(NetworkStatus::Online)).await;
                        hifi_runtime_loop(
                            &mut network,
                            endpoint,
                            http_buffer,
                            decode_buffer,
                            &mut artwork_pool,
                        )
                        .await;
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

async fn hifi_runtime_loop(
    network: &mut FirmwareNetwork,
    endpoint: Endpoint,
    http_buffer: &mut [u8],
    decode_buffer: &mut [u8],
    artwork_pool: &mut ArtworkPool,
) -> ! {
    let mut hifi_active = false;
    let mut pins_fetched = false;
    let mut next_lpec_event_poll_at = EmbassyInstant::now();
    let mut lpec_session = LpecSession::new();
    let mut last_hifi_artwork_uri = heapless::String::<HIFI_URI_LEN>::new();
    let mut pending_hifi_artwork_uri = heapless::String::<HIFI_URI_LEN>::new();

    loop {
        let mut commands = heapless::Vec::<HifiCommand, 4>::new();
        while let Ok(request) = HIFI_REQUESTS.try_receive() {
            match request {
                HifiRequest::SetActive(active) => {
                    hifi_active = active;
                    next_lpec_event_poll_at = EmbassyInstant::now();
                }
                HifiRequest::Command(command) => {
                    let _ = commands.push(command);
                }
            }
        }

        if hifi_active {
            for command in commands {
                if let Some(status) =
                    handle_linn_command(network, endpoint, &mut lpec_session, command)
                {
                    send_hifi_status(
                        &mut last_hifi_artwork_uri,
                        &mut pending_hifi_artwork_uri,
                        status,
                    )
                    .await;
                }
                next_lpec_event_poll_at = EmbassyInstant::now();
            }
        } else if !commands.is_empty() {
            println!("linn: command ignored while HIFI screen is inactive");
        }

        let now = EmbassyInstant::now();
        if hifi_active && now >= next_lpec_event_poll_at {
            if let Some(status) = poll_linn_events(network, endpoint, &mut lpec_session) {
                send_hifi_status(
                    &mut last_hifi_artwork_uri,
                    &mut pending_hifi_artwork_uri,
                    status,
                )
                .await;
            }
            next_lpec_event_poll_at = now + EmbassyDuration::from_millis(LPEC_EVENT_POLL_MS);
        }

        if hifi_active && !pins_fetched {
            let pins = fetch_linn_pins(network, endpoint, &mut lpec_session);
            // Mark fetched after the first attempt regardless — retrying
            // a `JsonCorrupt` etc. on every iteration just spams the
            // device. Pins are optional, so failures must not destabilize
            // the status/volume subscription.
            pins_fetched = true;
            if let Some(pins) = pins {
                send_app_event(Event::HifiPins(pins)).await;
            }
            if let Some(status) = lpec_session.live_status() {
                send_hifi_status(
                    &mut last_hifi_artwork_uri,
                    &mut pending_hifi_artwork_uri,
                    status,
                )
                .await;
            }
        }

        if let Some(artwork) = load_pending_hifi_artwork(
            network,
            &mut last_hifi_artwork_uri,
            &mut pending_hifi_artwork_uri,
            http_buffer,
            decode_buffer,
            artwork_pool,
        ) {
            send_app_event(Event::HifiArtwork(artwork)).await;
        }

        Timer::after(EmbassyDuration::from_millis(TOUCH_POLL_MS)).await;
    }
}

fn render_app<D>(
    app: &mut App,
    display: &mut D,
    scratch: &mut [Rgb565],
    render_requested: bool,
) -> bool
where
    D: DrawTarget<Color = Rgb565>,
{
    if !render_requested {
        return false;
    }

    // Screen-change clear is handled inside App::render now.
    match app.render(display, scratch) {
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

async fn send_hifi_status(
    last_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    pending_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    status: HifiStatus,
) {
    let event = hifi_status_event(last_artwork_uri, pending_artwork_uri, status);
    send_app_event(event).await;
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

fn poll_linn_events(
    network: &mut FirmwareNetwork,
    endpoint: Endpoint,
    session: &mut LpecSession,
) -> Option<HifiStatus> {
    let started_at = Instant::now();
    let result = {
        let mut stream = match network.connect_events(endpoint) {
            Ok(stream) => stream,
            Err(FirmwareNetError::ConnectTimeout | FirmwareNetError::ReadTimeout) => return None,
            Err(error) => {
                println!(
                    "linn: event tcp connect failed after {}ms: {:?}",
                    started_at.elapsed().as_millis(),
                    error
                );
                return None;
            }
        };
        session.poll(&mut stream)
    };

    match result {
        Ok(Some(status)) => Some(status),
        Ok(None) | Err(LpecError::Connect(FirmwareNetError::ReadTimeout)) => None,
        Err(error) => {
            println!("linn: session poll failed: {:?}", error);
            session.reset();
            network.reset_lpec();
            None
        }
    }
}

fn fetch_linn_pins(
    network: &mut FirmwareNetwork,
    endpoint: Endpoint,
    session: &mut LpecSession,
) -> Option<HifiPins> {
    let mut stream = match network.connect(endpoint) {
        Ok(stream) => stream,
        Err(error) => {
            println!("linn: pins tcp connect failed: {:?}", error);
            return None;
        }
    };
    match session.fetch_pins(&mut stream) {
        Ok(pins) => Some(pins),
        Err(LpecError::Protocol(::linn_lpec::Error::Remote { code, description })) => {
            // Receiver said no — connection is still healthy, don't reset
            // (resetting would tear down the status/volume subscription).
            println!(
                "linn: pins not available (code {}: {})",
                code,
                description.as_str()
            );
            None
        }
        Err(error) => {
            println!("linn: pins fetch transport error: {:?}", error);
            None
        }
    }
}

fn handle_linn_command(
    network: &mut FirmwareNetwork,
    endpoint: Endpoint,
    session: &mut LpecSession,
    command: HifiCommand,
) -> Option<HifiStatus> {
    println!("linn: command {:?}", command);
    let result = {
        let mut stream = match network.connect(endpoint) {
            Ok(stream) => stream,
            Err(error) => {
                println!("linn: command tcp connect failed: {:?}", error);
                return None;
            }
        };
        session.handle_command(&mut stream, command)
    };

    match result {
        Ok(Some(status)) => {
            println!("linn: command sent");
            Some(status)
        }
        Ok(None) => {
            println!("linn: command sent");
            None
        }
        Err(error) => {
            println!("linn: command failed: {:?}", error);
            session.reset();
            network.reset_lpec();
            None
        }
    }
}

fn hifi_status_event(
    last_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    pending_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    status: HifiStatus,
) -> Event {
    let artwork_uri = status.album_art_uri.clone();
    let should_load_artwork =
        status.playback == PlaybackState::Playing && !artwork_uri.as_str().is_empty();

    if should_load_artwork
        && last_artwork_uri.as_str() != artwork_uri.as_str()
        && pending_artwork_uri.as_str() != artwork_uri.as_str()
    {
        pending_artwork_uri.clear();
        let _ = pending_artwork_uri.push_str(artwork_uri.as_str());
    } else if artwork_uri.as_str().is_empty() {
        last_artwork_uri.clear();
        pending_artwork_uri.clear();
    }

    Event::HifiStatus(status)
}

fn load_pending_hifi_artwork(
    network: &mut FirmwareNetwork,
    last_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    pending_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    http_buffer: &mut [u8],
    decode_buffer: &mut [u8],
    artwork_pool: &mut ArtworkPool,
) -> Option<HifiArtwork> {
    if pending_artwork_uri.is_empty() {
        return None;
    };

    let mut uri = heapless::String::<HIFI_URI_LEN>::new();
    let _ = uri.push_str(pending_artwork_uri.as_str());
    pending_artwork_uri.clear();
    load_hifi_artwork(
        network,
        last_artwork_uri,
        uri.as_str(),
        http_buffer,
        decode_buffer,
        artwork_pool,
    )
}

fn load_hifi_artwork(
    network: &mut FirmwareNetwork,
    last_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    uri: &str,
    http_buffer: &mut [u8],
    decode_buffer: &mut [u8],
    artwork_pool: &mut ArtworkPool,
) -> Option<HifiArtwork> {
    last_artwork_uri.clear();
    let _ = last_artwork_uri.push_str(uri);
    println!("linn: artwork load {}", uri);

    let pixels = artwork_pool.acquire();

    match load_artwork_with_buffers_into(network, uri, http_buffer, decode_buffer, pixels) {
        Ok(artwork) => Some(artwork),
        Err(error) => {
            println!("linn: artwork failed: {:?}", error);
            None
        }
    }
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
