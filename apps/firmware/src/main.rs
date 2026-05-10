#![no_std]
#![no_main]

mod display;

use app_core::{App, Event};
use board_waveshare_c6::{BOARD_NAME, DISPLAY_SIZE, peripherals};
use display::AmoledDisplay;
use esp_backtrace as _;
use esp_hal::{
    i2c::master::{Config as I2cConfig, I2c},
    time::{Duration, Instant, Rate},
};
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

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

    let mut i2c = match I2c::new(
        peripherals.I2C0,
        I2cConfig::default().with_frequency(Rate::from_khz(300)),
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

    println!("app-core: initialized");

    loop {
        let outcome = app.update(Event::Tick { uptime_ms });

        if outcome.render_requested {
            match app.render(&mut display) {
                Ok(()) => println!("display: frame rendered"),
                Err(_) => println!("display: render failed"),
            }
        }

        println!(
            "heartbeat: uptime={}ms interactions={} redraw={}",
            app.uptime_ms(),
            app.interaction_count(),
            outcome.render_requested
        );

        wait_millis(1_000);
        uptime_ms += 1_000;
    }
}

fn wait_millis(ms: u64) {
    let start = Instant::now();
    let duration = Duration::from_millis(ms);

    while start.elapsed() < duration {
        core::hint::spin_loop();
    }
}
