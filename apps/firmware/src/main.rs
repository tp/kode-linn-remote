#![no_std]
#![no_main]

mod display;
mod net;
mod touch;

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use app_config::{AppConfig, WifiConfig};
use app_core::{
    App, Command, Event, HIFI_URI_LEN, HifiStatus, NetworkStatus, PlaybackState,
    RECOMMENDED_SCRATCH_PIXELS, Screen,
};
use app_runtime::lpec::{Error as LpecError, LpecSession, load_artwork};
use app_runtime::net::Endpoint;
use board_waveshare_c6::{BOARD_NAME, DISPLAY_SIZE, peripherals};
use display::AmoledDisplay;
use embassy_futures::block_on;
use embedded_graphics::{draw_target::DrawTarget, pixelcolor::Rgb565, prelude::RgbColor};
use esp_backtrace as _;
use esp_hal::{
    i2c::master::{Config as I2cConfig, I2c},
    interrupt::software::SoftwareInterruptControl,
    peripherals::WIFI,
    ram,
    time::{Duration, Instant, Rate},
    timer::timg::TimerGroup,
};
use esp_println::println;
use esp_radio::wifi::{
    Config as WifiRadioConfig, ControllerConfig, Interfaces, WifiController, WifiError,
    scan::ScanConfig, sta::StationConfig,
};
use net::{FirmwareNetError, FirmwareNetwork};
use touch::{FT6146_ADDRESS, TouchController};

esp_bootloader_esp_idf::esp_app_desc!();

const TOUCH_POLL_MS: u64 = 50;
const HEARTBEAT_MS: u64 = 1_000;
const LPEC_EVENT_POLL_MS: u64 = 100;
const WIFI_CONNECT_ATTEMPTS: u8 = 5;
const WIFI_SCAN_RETRIES: u8 = 3;
const LOCAL_CONFIG: &str = include_str!("../../../config/local.env");

#[esp_hal::main]
fn main() -> ! {
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

    let mut uptime_ms = 0;
    let mut next_heartbeat_ms = 0;
    let mut next_lpec_event_poll_ms = 0;
    let mut consecutive_touch_errors = 0_u32;
    let mut rendered_screen = app.screen();
    let mut lpec_session = LpecSession::new();
    let mut last_hifi_artwork_uri = heapless::String::<HIFI_URI_LEN>::new();
    let mut pending_hifi_artwork_uri = heapless::String::<HIFI_URI_LEN>::new();

    println!("app-core: initialized");

    if app_config.wifi.ssid.is_some() && app_config.wifi.password.is_some() {
        let render_requested = app
            .update(Event::NetworkStatus(NetworkStatus::Connecting))
            .render_requested;
        let _ = render_app(
            &mut app,
            &mut display,
            &mut scratch,
            &mut rendered_screen,
            render_requested,
        );
    }

    let (mut network, _wifi_controller) = match connect_wifi(peripherals.WIFI, &app_config.wifi) {
        WifiConnection::Connected {
            controller,
            interfaces,
        } => {
            let render_requested = app
                .update(Event::NetworkStatus(NetworkStatus::Online))
                .render_requested;
            let _ = render_app(
                &mut app,
                &mut display,
                &mut scratch,
                &mut rendered_screen,
                render_requested,
            );

            let mut network = FirmwareNetwork::new(interfaces.station);
            println!(
                "net: starting DHCP for Linn endpoint {}.{}.{}.{}:{}",
                app_config.linn_lpec_endpoint.address[0],
                app_config.linn_lpec_endpoint.address[1],
                app_config.linn_lpec_endpoint.address[2],
                app_config.linn_lpec_endpoint.address[3],
                app_config.linn_lpec_endpoint.port
            );
            match network.wait_config_up() {
                Ok(()) => {
                    if let Some(config) = network.config_v4() {
                        println!("net: dhcp address {}", config.address);
                    }
                }
                Err(error) => println!("net: dhcp failed: {:?}", error),
            }

            (Some(network), Some(controller))
        }
        WifiConnection::MissingCredentials | WifiConnection::Failed => {
            let render_requested = app
                .update(Event::NetworkStatus(NetworkStatus::Offline))
                .render_requested;
            let _ = render_app(
                &mut app,
                &mut display,
                &mut scratch,
                &mut rendered_screen,
                render_requested,
            );
            (None, None)
        }
    };

    loop {
        let mut render_requested = false;
        let mut frame_rendered = false;
        let mut pending_command = None;

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

        frame_rendered |= render_app(
            &mut app,
            &mut display,
            &mut scratch,
            &mut rendered_screen,
            render_requested,
        );
        render_requested = false;

        if let Some(command) = pending_command {
            render_requested |= handle_linn_command(
                &mut network,
                app_config.linn_lpec_endpoint,
                &mut app,
                &mut lpec_session,
                command,
            );
            next_lpec_event_poll_ms = 0;
        }

        if app.screen() == Screen::HifiControl && uptime_ms >= next_lpec_event_poll_ms {
            render_requested |= poll_linn_events(
                &mut network,
                app_config.linn_lpec_endpoint,
                &mut app,
                &mut lpec_session,
                &mut last_hifi_artwork_uri,
                &mut pending_hifi_artwork_uri,
            );
            next_lpec_event_poll_ms = uptime_ms.saturating_add(LPEC_EVENT_POLL_MS);
        }

        frame_rendered |= render_app(
            &mut app,
            &mut display,
            &mut scratch,
            &mut rendered_screen,
            render_requested,
        );

        let artwork_render_requested = load_pending_hifi_artwork(
            &mut network,
            &mut app,
            &mut last_hifi_artwork_uri,
            &mut pending_hifi_artwork_uri,
        );
        frame_rendered |= render_app(
            &mut app,
            &mut display,
            &mut scratch,
            &mut rendered_screen,
            artwork_render_requested,
        );

        if uptime_ms >= next_heartbeat_ms {
            println!(
                "heartbeat: uptime={}ms interactions={} redraw={}",
                app.uptime_ms(),
                app.interaction_count(),
                frame_rendered
            );
            next_heartbeat_ms = next_heartbeat_ms.saturating_add(HEARTBEAT_MS);
        }

        wait_millis(TOUCH_POLL_MS);
        uptime_ms += TOUCH_POLL_MS;
    }
}

fn connect_wifi<'d>(wifi: WIFI<'d>, config: &WifiConfig) -> WifiConnection<'d> {
    let (Some(ssid), Some(password)) = (&config.ssid, &config.password) else {
        println!("wifi: missing WIFI_SSID/WIFI_PASSWORD");
        return WifiConnection::MissingCredentials;
    };

    println!("wifi: connecting to configured SSID");
    let target_ssid = ssid.as_str();
    let station_config = WifiRadioConfig::Station(
        StationConfig::default()
            .with_ssid(target_ssid)
            .with_password(password.as_str().into()),
    );

    let (mut controller, interfaces) = match esp_radio::wifi::new(
        wifi,
        ControllerConfig::default().with_initial_config(station_config.clone()),
    ) {
        Ok(wifi) => wifi,
        Err(error) => {
            println!("wifi: init failed: {:?}", error);
            return WifiConnection::Failed;
        }
    };

    let target_seen = scan_configured_network(&mut controller, target_ssid);
    if !target_seen {
        println!("wifi: configured SSID not visible before connect; trying station connect anyway");
    }

    for attempt in 1..=WIFI_CONNECT_ATTEMPTS {
        match block_on(controller.connect_async()) {
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
                let _ = block_on(controller.disconnect_async());
                if let Err(error) = controller.set_config(&station_config) {
                    println!("wifi: station reconfigure failed: {:?}", error);
                }
                if attempt < WIFI_CONNECT_ATTEMPTS && attempt % 2 == 0 {
                    let _ = scan_configured_network(&mut controller, target_ssid);
                }
                wait_millis(1_000);
            }
        }
    }

    let _ = scan_configured_network(&mut controller, target_ssid);
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

fn render_app<D>(
    app: &mut App,
    display: &mut D,
    scratch: &mut [Rgb565],
    rendered_screen: &mut Screen,
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
        Ok(()) => {
            *rendered_screen = app.screen();
            println!("display: frame rendered");
            true
        }
        Err(_) => {
            println!("display: render failed");
            false
        }
    }
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
    network: &mut Option<FirmwareNetwork>,
    endpoint: Endpoint,
    app: &mut App,
    session: &mut LpecSession,
    last_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    pending_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
) -> bool {
    let started_at = Instant::now();
    let Some(network) = network.as_mut() else {
        return false;
    };

    let result = {
        let mut stream = match network.connect_events(endpoint) {
            Ok(stream) => stream,
            Err(error) => {
                println!(
                    "linn: event tcp connect failed after {}ms: {:?}",
                    started_at.elapsed().as_millis(),
                    error
                );
                return false;
            }
        };
        session.poll(&mut stream)
    };

    match result {
        Ok(Some(status)) => apply_hifi_status(app, last_artwork_uri, pending_artwork_uri, status),
        Ok(None) | Err(LpecError::Connect(FirmwareNetError::ReadTimeout)) => false,
        Err(error) => {
            println!("linn: session poll failed: {:?}", error);
            session.reset();
            network.reset_lpec();
            false
        }
    }
}

fn handle_linn_command(
    network: &mut Option<FirmwareNetwork>,
    endpoint: Endpoint,
    app: &mut App,
    session: &mut LpecSession,
    command: Command,
) -> bool {
    let Some(network) = network.as_mut() else {
        println!("linn: command ignored while network is offline");
        return false;
    };

    let Command::Hifi(command) = command;
    println!("linn: command {:?}", command);
    let result = {
        let mut stream = match network.connect(endpoint) {
            Ok(stream) => stream,
            Err(error) => {
                println!("linn: command tcp connect failed: {:?}", error);
                return false;
            }
        };
        session.handle_command(&mut stream, command)
    };

    match result {
        Ok(Some(status)) => {
            println!("linn: command sent");
            app.update(Event::HifiStatus(status)).render_requested
        }
        Ok(None) => {
            println!("linn: command sent");
            false
        }
        Err(error) => {
            println!("linn: command failed: {:?}", error);
            session.reset();
            network.reset_lpec();
            false
        }
    }
}

fn apply_hifi_status(
    app: &mut App,
    last_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    pending_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    status: HifiStatus,
) -> bool {
    let artwork_uri = status.album_art_uri.clone();
    let should_load_artwork =
        status.playback == PlaybackState::Playing && !artwork_uri.as_str().is_empty();
    let render_requested = app.update(Event::HifiStatus(status)).render_requested;

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

    render_requested
}

fn load_pending_hifi_artwork(
    network: &mut Option<FirmwareNetwork>,
    app: &mut App,
    last_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    pending_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
) -> bool {
    if pending_artwork_uri.is_empty() {
        return false;
    }

    let Some(network) = network.as_mut() else {
        return false;
    };

    let mut uri = heapless::String::<HIFI_URI_LEN>::new();
    let _ = uri.push_str(pending_artwork_uri.as_str());
    pending_artwork_uri.clear();
    load_hifi_artwork(network, app, last_artwork_uri, uri.as_str())
}

fn load_hifi_artwork(
    network: &mut FirmwareNetwork,
    app: &mut App,
    last_artwork_uri: &mut heapless::String<HIFI_URI_LEN>,
    uri: &str,
) -> bool {
    last_artwork_uri.clear();
    let _ = last_artwork_uri.push_str(uri);
    println!("linn: artwork load");

    match load_artwork(network, uri) {
        Ok(artwork) => app.update(Event::HifiArtwork(artwork)).render_requested,
        Err(error) => {
            println!("linn: artwork failed: {:?}", error);
            false
        }
    }
}

fn scan_configured_network(controller: &mut WifiController<'_>, target_ssid: &str) -> bool {
    println!("wifi: scanning visible SSIDs");
    let scan_config = ScanConfig::default().with_show_hidden(true).with_max(32);

    for attempt in 1..=WIFI_SCAN_RETRIES {
        match block_on(controller.scan_async(&scan_config)) {
            Ok(access_points) => {
                let target = access_points
                    .iter()
                    .find(|access_point| access_point.ssid.as_str() == target_ssid);
                println!(
                    "wifi: scan saw {} network(s), configured SSID exact={}",
                    access_points.len(),
                    target.is_some()
                );
                if let Some(access_point) = target {
                    println!(
                        "wifi: configured SSID channel={} rssi={} auth={:?}",
                        access_point.channel,
                        access_point.signal_strength,
                        access_point.auth_method
                    );
                    return true;
                }
                if !access_points.is_empty() || attempt == WIFI_SCAN_RETRIES {
                    return false;
                }
            }
            Err(error) => {
                println!(
                    "wifi: scan failed attempt {attempt}/{WIFI_SCAN_RETRIES}: {:?}",
                    error
                );
                if attempt == WIFI_SCAN_RETRIES {
                    return false;
                }
            }
        }
        wait_millis(500);
    }

    false
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

fn wait_millis(ms: u64) {
    let start = Instant::now();
    let duration = Duration::from_millis(ms);

    while start.elapsed() < duration {
        core::hint::spin_loop();
    }
}
