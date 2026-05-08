use embedded_graphics::{pixelcolor::Rgb565, prelude::RgbColor};

pub(super) const CARD_RADIUS: u32 = 18;
pub(super) const BUTTON_RADIUS: u32 = 18;

pub(super) const OLED_BLACK: Rgb565 = Rgb565::BLACK;
pub(super) const SURFACE: Rgb565 = Rgb565::new(1, 2, 3);
pub(super) const SURFACE_BORDER: Rgb565 = Rgb565::new(5, 9, 11);
pub(super) const TEXT_PRIMARY: Rgb565 = Rgb565::WHITE;
pub(super) const TEXT_SECONDARY: Rgb565 = TEXT_PRIMARY;
pub(super) const TEXT_DISABLED: Rgb565 = Rgb565::new(10, 18, 20);
pub(super) const ACTION_START: Rgb565 = Rgb565::new(1, 30, 13);
pub(super) const ACTION_START_BORDER: Rgb565 = Rgb565::new(7, 42, 20);
pub(super) const ACTION_STOP: Rgb565 = Rgb565::new(24, 4, 6);
pub(super) const ACTION_STOP_BORDER: Rgb565 = Rgb565::new(31, 13, 14);
pub(super) const ACTION_INACTIVE: Rgb565 = Rgb565::new(3, 4, 6);
pub(super) const ACTION_INACTIVE_BORDER: Rgb565 = Rgb565::new(7, 10, 13);
pub(super) const VOLUME_TRACK: Rgb565 = Rgb565::new(9, 45, 31);
pub(super) const VOLUME_ACTIVE: Rgb565 = Rgb565::new(0, 12, 27);
