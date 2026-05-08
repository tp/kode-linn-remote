#![no_std]

use embedded_graphics::prelude::Size;

pub const DISPLAY_SIZE: Size = Size::new(466, 466);

pub const BOARD_NAME: &str = "Waveshare ESP32-C6 Touch AMOLED 1.43";

pub mod peripherals {
    pub const DISPLAY_CONTROLLER: &str = "CO5300";
    pub const TOUCH_CONTROLLER: &str = "FT6146";
    pub const IMU: &str = "QMI8658";
    pub const RTC: &str = "PCF85063";
    pub const GPIO_EXPANDER: &str = "TCA9554";
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardInput {
    Touch { x: i32, y: i32, pressed: bool },
    BootButton,
    UserButton,
}
