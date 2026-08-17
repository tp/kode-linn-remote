#![no_std]

//! Board facts for the [Kode Dot](https://kode.diy/product/kode-dot).
//!
//! # Which revision this describes
//!
//! This crate targets the **ESP32-P4 revision** (November 2026 batch), whose
//! published specification is:
//!
//! - ESP32-P4 application processor with an **ESP32-C5 wireless co-processor**
//! - Dual-band Wi-Fi 2.4 / 5 GHz, Bluetooth LE 5, Thread, Zigbee
//! - 2.13" AMOLED touchscreen
//! - Directional pad plus two control buttons
//! - 9-axis IMU, NFC 13.56 MHz (read and emulate), RFID 125 kHz, IR TX/RX
//! - Speaker, microphone, haptic motor, RGB LED, microSD
//! - USB-C with OTG, 20-pin expansion header, rear magnetic connector
//!
//! The earlier **ESP32-S3 revision** is the one currently documented at
//! <https://docs.kode.diy>. Its pin maps and driver notes do *not* transfer to
//! the P4 revision, so anything below sourced from those docs is marked
//! [`Confidence::Provisional`] until it can be checked against real hardware.
//!
//! Values marked provisional are the ones to re-verify on first bring-up.
//! Nothing in this crate touches hardware; it is facts and input types only.

use embedded_graphics::prelude::Size;

/// How far a fact in this crate can be trusted before hardware arrives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Confidence {
    /// Published for the ESP32-P4 revision on the Kode Dot product page.
    Published,
    /// Carried over from the ESP32-S3 revision docs; unconfirmed for the P4
    /// revision and worth re-checking against hardware or a schematic.
    Provisional,
}

pub const BOARD_NAME: &str = "Kode Dot (ESP32-P4 + ESP32-C5)";

/// Panel resolution in pixels, portrait.
///
/// The 2.13" AMOLED on the ESP32-S3 revision is 410 x 502. The product page
/// for the P4 revision lists the panel size but not its resolution, so this is
/// [`Confidence::Provisional`]: same physical panel is the likely case, but it
/// is not confirmed. This constant is the single source of truth for display
/// geometry across the workspace — correcting it here reflows every screen.
pub const DISPLAY_SIZE: Size = Size::new(410, 502);

/// Confidence in [`DISPLAY_SIZE`].
pub const DISPLAY_SIZE_CONFIDENCE: Confidence = Confidence::Provisional;

/// The panel is a plain rectangle: unlike the round 466 x 466 board this
/// project started on, there are no corners lost to a circular mask, so
/// layouts can use the full framebuffer.
pub const DISPLAY_IS_ROUND: bool = false;

/// Physical panel diagonal, in hundredths of an inch (2.13").
pub const DISPLAY_DIAGONAL_CENTI_INCH: u32 = 213;

pub mod peripherals {
    use super::Confidence;

    /// Application processor. Published for this revision.
    pub const SOC: &str = "ESP32-P4";
    /// Wireless co-processor. Published for this revision.
    pub const WIRELESS_COPROCESSOR: &str = "ESP32-C5";

    /// Display controller on the ESP32-S3 revision.
    ///
    /// Unconfirmed for the P4 revision, which has a MIPI-DSI capable host and
    /// may well drive the panel differently.
    pub const DISPLAY_CONTROLLER: &str = "CO5300";
    pub const DISPLAY_CONTROLLER_CONFIDENCE: Confidence = Confidence::Provisional;

    /// Touch controller on the ESP32-S3 revision (I2C address 0x15).
    pub const TOUCH_CONTROLLER: &str = "CST820";
    pub const TOUCH_CONTROLLER_CONFIDENCE: Confidence = Confidence::Provisional;

    /// I/O expander that carries the D-pad on the ESP32-S3 revision
    /// (I2C address 0x20).
    pub const GPIO_EXPANDER: &str = "TCA95xx (16-bit)";
    pub const GPIO_EXPANDER_CONFIDENCE: Confidence = Confidence::Provisional;

    pub const RTC: &str = "MAX31328";
    pub const RTC_CONFIDENCE: Confidence = Confidence::Provisional;

    pub const FUEL_GAUGE: &str = "BQ27220";
    pub const FUEL_GAUGE_CONFIDENCE: Confidence = Confidence::Provisional;

    pub const PMIC: &str = "BQ25896";
    pub const PMIC_CONFIDENCE: Confidence = Confidence::Provisional;
}

/// A direction on the four-way pad.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    Up,
    Down,
    Left,
    Right,
}

/// The two control buttons flanking the directional pad.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ControlButton {
    /// Primary / confirm.
    Select,
    /// Secondary / dismiss, and "go up one level" from a subscreen.
    Back,
}

/// Everything the board can hand to the application as input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BoardInput {
    Touch { x: i32, y: i32, pressed: bool },
    Direction(Direction),
    Control(ControlButton),
}
