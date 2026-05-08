#![no_std]

use core::fmt::Write as _;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

pub const DISPLAY_SIZE: Size = Size::new(466, 466);
const START_BUTTON: Rectangle = Rectangle::new(Point::new(44, 140), Size::new(170, 72));
const STOP_BUTTON: Rectangle = Rectangle::new(Point::new(252, 140), Size::new(170, 72));
const HEADER_PANEL: Rectangle = Rectangle::new(Point::new(24, 24), Size::new(418, 92));
const CARD_RADIUS: u32 = 18;
const BUTTON_RADIUS: u32 = 18;

const OLED_BLACK: Rgb565 = Rgb565::BLACK;
const SURFACE: Rgb565 = Rgb565::new(1, 2, 3);
const SURFACE_BORDER: Rgb565 = Rgb565::new(5, 9, 11);
const TEXT_PRIMARY: Rgb565 = Rgb565::WHITE;
const TEXT_SECONDARY: Rgb565 = TEXT_PRIMARY;
const TEXT_DISABLED: Rgb565 = Rgb565::new(10, 18, 20);
const ACTION_START: Rgb565 = Rgb565::new(1, 30, 13);
const ACTION_START_BORDER: Rgb565 = Rgb565::new(7, 42, 20);
const ACTION_STOP: Rgb565 = Rgb565::new(24, 4, 6);
const ACTION_STOP_BORDER: Rgb565 = Rgb565::new(31, 13, 14);
const ACTION_INACTIVE: Rgb565 = Rgb565::new(3, 4, 6);
const ACTION_INACTIVE_BORDER: Rgb565 = Rgb565::new(7, 10, 13);

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
pub enum RenderError<E> {
    Draw(E),
    TextFormat,
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
        let render_requested = match event {
            Event::Tick { uptime_ms } => {
                self.uptime_ms = uptime_ms;
                self.update_stopwatch()
            }
            Event::TouchDown(point) => {
                self.interaction_count = self.interaction_count.saturating_add(1);
                self.handle_touch(point);
                true
            }
            Event::TouchUp => false,
            Event::ButtonPressed(_) => {
                self.interaction_count = self.interaction_count.saturating_add(1);
                true
            }
            Event::NetworkStatus(status) => {
                if self.network_status == status {
                    false
                } else {
                    self.network_status = status;
                    true
                }
            }
        };

        UpdateOutcome { render_requested }
    }

    pub fn render<D>(&self, display: &mut D) -> Result<(), RenderError<D::Error>>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        display.clear(Rgb565::BLACK).map_err(RenderError::Draw)?;

        let title_font = mplus!(
            2,
            BOLD,
            line_height(40),
            true,
            4,
            4,
            kern(' '..='~', ["ff", "ffi", "ffl"])
        );
        let body_font = mplus!(
            2,
            500,
            line_height(40),
            true,
            4,
            4,
            kern(' '..='~', ["ff", "ffi", "ffl"])
        );
        let top_text_style = TextStyleBuilder::new().baseline(Baseline::Top).build();

        Rectangle::new(Point::zero(), DISPLAY_SIZE)
            .into_styled(PrimitiveStyle::with_fill(OLED_BLACK))
            .draw(display)
            .map_err(RenderError::Draw)?;

        draw_panel(display, HEADER_PANEL, CARD_RADIUS, SURFACE, SURFACE_BORDER)?;

        let title_style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_PRIMARY)
            .background_color(SURFACE)
            .font(&title_font)
            .build();
        Text::with_text_style(
            "ESP32-C6 Home Tools",
            Point::new(44, 48),
            title_style,
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        draw_button(
            display,
            START_BUTTON,
            "START",
            !self.stopwatch_running,
            ButtonTone::Start,
        )?;
        draw_button(
            display,
            STOP_BUTTON,
            "STOP",
            self.stopwatch_running,
            ButtonTone::Stop,
        )?;

        let body_style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_SECONDARY)
            .background_color(OLED_BLACK)
            .font(&body_font)
            .build();

        let mut stopwatch = heapless::String::<32>::new();
        write!(stopwatch, "stopwatch: {}s", self.stopwatch_seconds)
            .map_err(|_| RenderError::TextFormat)?;
        Text::with_text_style(
            &stopwatch,
            Point::new(44, 244),
            body_style.clone(),
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        let network = match self.network_status {
            NetworkStatus::Offline => "network: offline",
            NetworkStatus::Connecting => "network: connecting",
            NetworkStatus::Online => "network: online",
        };
        Text::with_text_style(
            network,
            Point::new(44, 312),
            body_style.clone(),
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        let mut interactions = heapless::String::<32>::new();
        write!(interactions, "interactions: {}", self.interaction_count)
            .map_err(|_| RenderError::TextFormat)?;
        Text::with_text_style(
            &interactions,
            Point::new(44, 352),
            body_style,
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

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

    fn handle_touch(&mut self, point: TouchPoint) -> bool {
        let point = Point::new(point.x, point.y);

        if START_BUTTON.contains(point) && !self.stopwatch_running {
            self.stopwatch_running = true;
            self.last_stopwatch_second = self.uptime_ms / 1000;
            true
        } else if STOP_BUTTON.contains(point) && self.stopwatch_running {
            self.update_stopwatch();
            self.stopwatch_running = false;
            true
        } else {
            false
        }
    }

    fn update_stopwatch(&mut self) -> bool {
        if !self.stopwatch_running {
            return false;
        }

        let current_second = self.uptime_ms / 1000;
        let elapsed = current_second.saturating_sub(self.last_stopwatch_second);
        if elapsed > 0 {
            self.stopwatch_seconds = self.stopwatch_seconds.saturating_add(elapsed);
            self.last_stopwatch_second = current_second;
            true
        } else {
            false
        }
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ButtonTone {
    Start,
    Stop,
}

fn draw_button<D>(
    display: &mut D,
    rect: Rectangle,
    label: &str,
    active: bool,
    tone: ButtonTone,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let (fill, border) = match (active, tone) {
        (true, ButtonTone::Start) => (ACTION_START, ACTION_START_BORDER),
        (true, ButtonTone::Stop) => (ACTION_STOP, ACTION_STOP_BORDER),
        (false, _) => (ACTION_INACTIVE, ACTION_INACTIVE_BORDER),
    };
    let text = if active { TEXT_PRIMARY } else { TEXT_DISABLED };

    draw_panel(display, rect, BUTTON_RADIUS, fill, border)?;

    let button_font = mplus!(
        2,
        BOLD,
        line_height(40),
        true,
        4,
        4,
        kern(' '..='~', ["ff", "ffi", "ffl"])
    );
    let character_style = BitmapFontStyleBuilder::new()
        .text_color(text)
        .background_color(fill)
        .font(&button_font)
        .build();
    let text_style = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Middle)
        .build();

    Text::with_text_style(
        label,
        rect.center() + Point::new(0, 1),
        character_style,
        text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;

    Ok(())
}

fn draw_panel<D>(
    display: &mut D,
    rect: Rectangle,
    radius: u32,
    fill: Rgb565,
    stroke: Rgb565,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    RoundedRectangle::with_equal_corners(rect, Size::new(radius, radius))
        .into_styled(
            PrimitiveStyleBuilder::new()
                .fill_color(fill)
                .stroke_color(stroke)
                .stroke_width(1)
                .build(),
        )
        .draw(display)
        .map_err(RenderError::Draw)
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
        assert!(!outcome.render_requested);
    }

    #[test]
    fn running_stopwatch_requests_render_once_per_second() {
        let mut app = App::new();

        app.update(Event::TouchDown(TouchPoint { x: 80, y: 170 }));

        assert!(!app.update(Event::Tick { uptime_ms: 500 }).render_requested);
        assert!(
            app.update(Event::Tick { uptime_ms: 1_000 })
                .render_requested
        );
        assert!(
            !app.update(Event::Tick { uptime_ms: 1_200 })
                .render_requested
        );
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
