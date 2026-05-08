#![no_std]

use embedded_graphics::prelude::*;

mod ui;

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
                // The interaction counter is visible UI state, so every tap changes the frame.
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

    pub const fn network_status(&self) -> NetworkStatus {
        self.network_status
    }

    fn handle_touch(&mut self, point: TouchPoint) {
        let point = Point::new(point.x, point.y);
        let layout = ui::layout(ui::SCREEN_BOUNDS);
        let interaction_state = ui::InteractionState {
            stopwatch_running: self.stopwatch_running,
        };

        match ui::hit_test(&layout, point, interaction_state) {
            Some(ui::UiAction::StartStopwatch) => {
                self.stopwatch_running = true;
                self.last_stopwatch_second = self.uptime_ms / 1000;
            }
            Some(ui::UiAction::StopStopwatch) => {
                self.update_stopwatch();
                self.stopwatch_running = false;
            }
            None => {}
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

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::geometry::Point;
    use embedded_graphics::mock_display::MockDisplay;
    use embedded_graphics::pixelcolor::Rgb565;

    fn touch_point(point: Point) -> TouchPoint {
        TouchPoint {
            x: point.x,
            y: point.y,
        }
    }

    fn start_button_touch() -> TouchPoint {
        let (start, _) = ui::button_centers();

        touch_point(start)
    }

    fn stop_button_touch() -> TouchPoint {
        let (_, stop) = ui::button_centers();

        touch_point(stop)
    }

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

        app.update(Event::TouchDown(start_button_touch()));

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

        app.update(Event::TouchDown(start_button_touch()));
        app.update(Event::Tick { uptime_ms: 1_000 });
        app.update(Event::Tick { uptime_ms: 2_000 });

        assert!(app.running());
        assert_eq!(app.stopwatch_seconds(), 2);

        app.update(Event::TouchDown(stop_button_touch()));
        app.update(Event::Tick { uptime_ms: 5_000 });

        assert!(!app.running());
        assert_eq!(app.stopwatch_seconds(), 2);
    }

    #[test]
    fn stopped_stopwatch_does_not_advance_with_uptime() {
        let mut app = App::new();

        app.update(Event::TouchDown(start_button_touch()));
        app.update(Event::Tick { uptime_ms: 3_000 });
        app.update(Event::TouchDown(stop_button_touch()));
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

    #[test]
    fn default_network_status_is_offline() {
        let app = App::new();

        assert_eq!(app.network_status(), NetworkStatus::Offline);
    }

    #[test]
    fn network_status_change_requests_render() {
        let mut app = App::new();

        let outcome = app.update(Event::NetworkStatus(NetworkStatus::Online));

        assert_eq!(app.network_status(), NetworkStatus::Online);
        assert!(outcome.render_requested);
    }

    #[test]
    fn repeated_network_status_does_not_request_render() {
        let mut app = App::new();

        let outcome = app.update(Event::NetworkStatus(NetworkStatus::Offline));

        assert_eq!(app.network_status(), NetworkStatus::Offline);
        assert!(!outcome.render_requested);
    }

    #[test]
    fn render_draws_all_network_statuses() {
        for status in [
            NetworkStatus::Offline,
            NetworkStatus::Connecting,
            NetworkStatus::Online,
        ] {
            let mut app = App::new();
            app.update(Event::NetworkStatus(status));

            let mut display = MockDisplay::<Rgb565>::new();
            display.set_allow_overdraw(true);
            display.set_allow_out_of_bounds_drawing(true);

            app.render(&mut display).unwrap();
        }
    }
}
