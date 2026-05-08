#![no_std]

use core::fmt::Write;

use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use heapless::String;

pub const DISPLAY_SIZE: Size = Size::new(466, 466);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Tick { uptime_ms: u64 },
    TouchDown(TouchPoint),
    TouchUp,
    ButtonPressed(Button),
    NetworkStatus(NetworkStatus),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TouchPoint {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Button {
    Boot,
    User,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkStatus {
    Offline,
    Connecting,
    Online,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct UpdateOutcome {
    pub render_requested: bool,
}

#[derive(Debug)]
pub struct App {
    uptime_ms: u64,
    touch: Option<TouchPoint>,
    network_status: NetworkStatus,
    interaction_count: u32,
}

impl App {
    pub const fn new() -> Self {
        Self {
            uptime_ms: 0,
            touch: None,
            network_status: NetworkStatus::Offline,
            interaction_count: 0,
        }
    }

    pub fn update(&mut self, event: Event) -> UpdateOutcome {
        match event {
            Event::Tick { uptime_ms } => {
                self.uptime_ms = uptime_ms;
            }
            Event::TouchDown(point) => {
                self.touch = Some(point);
                self.interaction_count = self.interaction_count.saturating_add(1);
            }
            Event::TouchUp => {
                self.touch = None;
            }
            Event::ButtonPressed(_) => {
                self.interaction_count = self.interaction_count.saturating_add(1);
            }
            Event::NetworkStatus(status) => {
                self.network_status = status;
            }
        }

        UpdateOutcome {
            render_requested: true,
        }
    }

    pub fn render<D>(&self, display: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        display.clear(Rgb565::BLACK)?;

        Rectangle::new(Point::zero(), DISPLAY_SIZE)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(2, 5, 8)))
            .draw(display)?;

        Rectangle::new(Point::new(24, 24), Size::new(418, 92))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(6, 18, 14)))
            .draw(display)?;

        let title_style = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
        Text::with_baseline(
            "ESP32-C6 Home Tools",
            Point::new(44, 48),
            title_style,
            Baseline::Top,
        )
        .draw(display)?;

        let body_style = MonoTextStyle::new(&FONT_10X20, Rgb565::new(23, 55, 47));

        let mut uptime: String<32> = String::new();
        let _ = write!(uptime, "uptime: {}s", self.uptime_ms / 1000);
        Text::with_baseline(&uptime, Point::new(44, 148), body_style, Baseline::Top)
            .draw(display)?;

        let network = match self.network_status {
            NetworkStatus::Offline => "network: offline",
            NetworkStatus::Connecting => "network: connecting",
            NetworkStatus::Online => "network: online",
        };
        Text::with_baseline(network, Point::new(44, 188), body_style, Baseline::Top)
            .draw(display)?;

        let mut interactions: String<32> = String::new();
        let _ = write!(interactions, "interactions: {}", self.interaction_count);
        Text::with_baseline(
            &interactions,
            Point::new(44, 228),
            body_style,
            Baseline::Top,
        )
        .draw(display)?;

        let touch_label = if self.touch.is_some() {
            "touch: active"
        } else {
            "touch: idle"
        };
        Text::with_baseline(touch_label, Point::new(44, 268), body_style, Baseline::Top)
            .draw(display)?;

        if let Some(point) = self.touch {
            Circle::with_center(Point::new(point.x, point.y), 18)
                .into_styled(PrimitiveStyle::with_fill(Rgb565::new(31, 42, 10)))
                .draw(display)?;
        }

        Ok(())
    }

    pub const fn uptime_ms(&self) -> u64 {
        self.uptime_ms
    }

    pub const fn interaction_count(&self) -> u32 {
        self.interaction_count
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::mock_display::MockDisplay;

    #[test]
    fn tick_updates_uptime() {
        let mut app = App::new();

        let outcome = app.update(Event::Tick { uptime_ms: 12_000 });

        assert_eq!(app.uptime_ms(), 12_000);
        assert!(outcome.render_requested);
    }

    #[test]
    fn touch_counts_as_interaction() {
        let mut app = App::new();

        app.update(Event::TouchDown(TouchPoint { x: 100, y: 120 }));
        app.update(Event::TouchUp);

        assert_eq!(app.interaction_count(), 1);
    }

    #[test]
    fn render_draws_to_rgb565_display() {
        let app = App::new();
        let mut display = MockDisplay::<Rgb565>::new();
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);

        app.render(&mut display).unwrap();
    }
}
