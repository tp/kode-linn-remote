#![no_std]
#![no_main]

use app_core::{App, Event};
use esp_backtrace as _;

#[esp_hal::main]
fn main() -> ! {
    let config = esp_hal::Config::default();
    let _peripherals = esp_hal::init(config);

    let mut app = App::new();
    let _ = app.update(Event::Tick { uptime_ms: 0 });

    loop {
        core::hint::spin_loop();
    }
}
