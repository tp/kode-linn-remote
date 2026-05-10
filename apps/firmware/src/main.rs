#![no_std]
#![no_main]

mod display;
mod touch;

use app_core::{App, Event};
use board_waveshare_c6::{BOARD_NAME, DISPLAY_SIZE, peripherals};
use display::AmoledDisplay;
use embedded_graphics::{draw_target::DrawTarget, pixelcolor::Rgb565, prelude::RgbColor};
use esp_backtrace as _;
use esp_hal::{
    i2c::master::{Config as I2cConfig, I2c},
    time::{Duration, Instant, Rate},
};
use esp_println::println;
use touch::{FT6146_ADDRESS, TouchController};

esp_bootloader_esp_idf::esp_app_desc!();

const TOUCH_POLL_MS: u64 = 50;
const HEARTBEAT_MS: u64 = 1_000;

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default();
    let peripherals = esp_hal::init(config);

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

    match app.render(&mut display) {
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
    let mut consecutive_touch_errors = 0_u32;
    let mut rendered_screen = app.screen();

    println!("app-core: initialized");

    loop {
        let mut render_requested = false;

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

        if render_requested {
            let current_screen = app.screen();
            if current_screen != rendered_screen {
                if let Err(error) = display.clear(Rgb565::BLACK) {
                    println!("display: screen clear failed: {}", error);
                }
            }

            match app.render(&mut display) {
                Ok(()) => {
                    rendered_screen = current_screen;
                    println!("display: frame rendered");
                }
                Err(_) => println!("display: render failed"),
            }
        }

        if uptime_ms >= next_heartbeat_ms {
            println!(
                "heartbeat: uptime={}ms interactions={} redraw={}",
                app.uptime_ms(),
                app.interaction_count(),
                render_requested
            );
            next_heartbeat_ms = next_heartbeat_ms.saturating_add(HEARTBEAT_MS);
        }

        wait_millis(TOUCH_POLL_MS);
        uptime_ms += TOUCH_POLL_MS;
    }
}

fn wait_millis(ms: u64) {
    let start = Instant::now();
    let duration = Duration::from_millis(ms);

    while start.elapsed() < duration {
        core::hint::spin_loop();
    }
}
