#![no_std]

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};
use u8g2_fonts::{
    FontRenderer, fonts,
    types::{FontColor, HorizontalAlignment, VerticalPosition},
};

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
pub enum RenderError<E> {
    Draw(E),
    Font(u8g2_fonts::Error<E>),
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

        Rectangle::new(Point::zero(), DISPLAY_SIZE)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(2, 5, 8)))
            .draw(display)
            .map_err(RenderError::Draw)?;

        Rectangle::new(Point::new(24, 24), Size::new(418, 92))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::new(6, 18, 14)))
            .draw(display)
            .map_err(RenderError::Draw)?;

        title_font()
            .render(
                "ESP32-C6 Home Tools",
                Point::new(44, 48),
                VerticalPosition::Top,
                FontColor::Transparent(Rgb565::WHITE),
                display,
            )
            .map_err(RenderError::Font)?;

        draw_button(display, START_BUTTON, "START", !self.stopwatch_running)?;
        draw_button(display, STOP_BUTTON, "STOP", self.stopwatch_running)?;

        body_font()
            .render(
                format_args!("stopwatch: {}s", self.stopwatch_seconds),
                Point::new(44, 244),
                VerticalPosition::Top,
                FontColor::Transparent(Rgb565::new(23, 55, 47)),
                display,
            )
            .map_err(RenderError::Font)?;

        let network = match self.network_status {
            NetworkStatus::Offline => "network: offline",
            NetworkStatus::Connecting => "network: connecting",
            NetworkStatus::Online => "network: online",
        };
        body_font()
            .render(
                network,
                Point::new(44, 312),
                VerticalPosition::Top,
                FontColor::Transparent(Rgb565::new(23, 55, 47)),
                display,
            )
            .map_err(RenderError::Font)?;

        body_font()
            .render(
                format_args!("interactions: {}", self.interaction_count),
                Point::new(44, 352),
                VerticalPosition::Top,
                FontColor::Transparent(Rgb565::new(23, 55, 47)),
                display,
            )
            .map_err(RenderError::Font)?;

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

fn draw_button<D>(
    display: &mut D,
    rect: Rectangle,
    label: &str,
    active: bool,
) -> Result<(), RenderError<D::Error>>
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
        .draw(display)
        .map_err(RenderError::Draw)?;

    button_font()
        .render_aligned(
            label,
            rect.center() + Point::new(0, 1),
            VerticalPosition::Center,
            HorizontalAlignment::Center,
            FontColor::Transparent(text),
            display,
        )
        .map_err(RenderError::Font)?;

    Ok(())
}

const fn body_font() -> FontRenderer {
    FontRenderer::new::<fonts::u8g2_font_helvR12_tr>()
}

const fn title_font() -> FontRenderer {
    FontRenderer::new::<fonts::u8g2_font_helvB12_tr>()
}

const fn button_font() -> FontRenderer {
    FontRenderer::new::<fonts::u8g2_font_helvB12_tr>()
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
