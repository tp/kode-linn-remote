#![no_std]

use core::fmt::Write;

use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Baseline, Text},
};
use heapless::String;

pub const DISPLAY_SIZE: Size = Size::new(466, 466);
const START_BUTTON: Rectangle = Rectangle::new(Point::new(44, 140), Size::new(170, 72));
const STOP_BUTTON: Rectangle = Rectangle::new(Point::new(252, 140), Size::new(170, 72));

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
    network_status: NetworkStatus,
    interaction_count: u32,
    stopwatch_running: bool,
    stopwatch_seconds: u64,
    last_stopwatch_second: u64,
}

impl App {
    pub const fn new() -> Self {
        Self {
            uptime_ms: 0,
            network_status: NetworkStatus::Offline,
            interaction_count: 0,
            stopwatch_running: false,
            stopwatch_seconds: 0,
            last_stopwatch_second: 0,
        }
    }

    pub fn update(&mut self, event: Event) -> UpdateOutcome {
        match event {
            Event::Tick { uptime_ms } => {
                self.uptime_ms = uptime_ms;
                self.update_stopwatch();
            }
            Event::TouchDown(point) => {
                self.interaction_count = self.interaction_count.saturating_add(1);
                self.handle_touch(point);
            }
            Event::TouchUp => {}
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

        draw_button(display, START_BUTTON, "START", !self.stopwatch_running)?;
        draw_button(display, STOP_BUTTON, "STOP", self.stopwatch_running)?;

        let mut stopwatch: String<32> = String::new();
        let _ = write!(stopwatch, "stopwatch: {}s", self.stopwatch_seconds);
        Text::with_baseline(&stopwatch, Point::new(44, 244), body_style, Baseline::Top)
            .draw(display)?;

        let network = match self.network_status {
            NetworkStatus::Offline => "network: offline",
            NetworkStatus::Connecting => "network: connecting",
            NetworkStatus::Online => "network: online",
        };
        Text::with_baseline(network, Point::new(44, 312), body_style, Baseline::Top)
            .draw(display)?;

        let mut interactions: String<32> = String::new();
        let _ = write!(interactions, "interactions: {}", self.interaction_count);
        Text::with_baseline(
            &interactions,
            Point::new(44, 352),
            body_style,
            Baseline::Top,
        )
        .draw(display)?;

        Ok(())
    }

    pub const fn uptime_ms(&self) -> u64 {
        self.uptime_ms
    }

    pub const fn interaction_count(&self) -> u32 {
        self.interaction_count
    }

    pub const fn running(&self) -> bool {
        self.stopwatch_running
    }

    pub const fn stopwatch_seconds(&self) -> u64 {
        self.stopwatch_seconds
    }

    fn handle_touch(&mut self, point: TouchPoint) {
        let point = Point::new(point.x, point.y);

        if START_BUTTON.contains(point) && !self.stopwatch_running {
            self.stopwatch_running = true;
            self.last_stopwatch_second = self.uptime_ms / 1000;
        } else if STOP_BUTTON.contains(point) && self.stopwatch_running {
            self.update_stopwatch();
            self.stopwatch_running = false;
        }
    }

    fn update_stopwatch(&mut self) {
        if !self.stopwatch_running {
            return;
        }

        let current_second = self.uptime_ms / 1000;
        let elapsed = current_second.saturating_sub(self.last_stopwatch_second);
        if elapsed > 0 {
            self.stopwatch_seconds = self.stopwatch_seconds.saturating_add(elapsed);
            self.last_stopwatch_second = current_second;
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

fn draw_button<D>(
    display: &mut D,
    rect: Rectangle,
    label: &str,
    active: bool,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let fill = if active {
        Rgb565::new(4, 28, 16)
    } else {
        Rgb565::new(3, 8, 10)
    };
    let text = if active {
        Rgb565::WHITE
    } else {
        Rgb565::new(12, 24, 22)
    };

    rect.into_styled(PrimitiveStyle::with_fill(fill))
        .draw(display)?;

    let style = MonoTextStyle::new(&FONT_10X20, text);
    Text::with_baseline(
        label,
        Point::new(rect.top_left.x + 32, rect.top_left.y + 24),
        style,
        Baseline::Top,
    )
    .draw(display)?;

    Ok(())
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
    fn start_and_stop_control_stopwatch() {
        let mut app = App::new();

        app.update(Event::TouchDown(TouchPoint { x: 80, y: 170 }));
        app.update(Event::Tick { uptime_ms: 1_000 });
        app.update(Event::Tick { uptime_ms: 2_000 });

        assert!(app.running());
        assert_eq!(app.stopwatch_seconds(), 2);

        app.update(Event::TouchDown(TouchPoint { x: 300, y: 170 }));
        app.update(Event::Tick { uptime_ms: 5_000 });

        assert!(!app.running());
        assert_eq!(app.stopwatch_seconds(), 2);
    }

    #[test]
    fn stopped_stopwatch_does_not_advance_with_uptime() {
        let mut app = App::new();

        app.update(Event::TouchDown(TouchPoint { x: 80, y: 170 }));
        app.update(Event::Tick { uptime_ms: 3_000 });
        app.update(Event::TouchDown(TouchPoint { x: 300, y: 170 }));
        app.update(Event::Tick { uptime_ms: 20_000 });

        assert_eq!(app.uptime_ms(), 20_000);
        assert_eq!(app.stopwatch_seconds(), 3);
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
