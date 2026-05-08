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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Screen {
    Launcher,
    Stopwatch,
    HifiControl,
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
    ui_layouts: ui::ScreenLayouts,
    active_screen: ActiveScreen,
}

#[derive(Debug)]
enum ActiveScreen {
    Launcher(ui::screens::launcher::State),
    Stopwatch(ui::screens::stopwatch::State),
    HifiControl(ui::screens::hifi::State),
}

impl ActiveScreen {
    const fn new(screen: Screen) -> Self {
        match screen {
            Screen::Launcher => Self::Launcher(ui::screens::launcher::State::new()),
            Screen::Stopwatch => Self::Stopwatch(ui::screens::stopwatch::State::new()),
            Screen::HifiControl => Self::HifiControl(ui::screens::hifi::State::new()),
        }
    }

    const fn screen(&self) -> Screen {
        match self {
            Self::Launcher(_) => Screen::Launcher,
            Self::Stopwatch(_) => Screen::Stopwatch,
            Self::HifiControl(_) => Screen::HifiControl,
        }
    }
}

impl App {
    pub fn new() -> Self {
        Self::new_on_screen(Screen::Launcher)
    }

    pub fn new_on_screen(screen: Screen) -> Self {
        Self {
            uptime_ms: 0,
            network_status: NetworkStatus::Offline,
            interaction_count: 0,
            ui_layouts: ui::ScreenLayouts::new(ui::SCREEN_BOUNDS),
            active_screen: ActiveScreen::new(screen),
        }
    }

    pub fn update(&mut self, event: Event) -> UpdateOutcome {
        let render_requested = match event {
            Event::Tick { uptime_ms } => {
                self.uptime_ms = uptime_ms;
                match &mut self.active_screen {
                    ActiveScreen::Launcher(_) => false,
                    ActiveScreen::Stopwatch(state) => state.on_tick(uptime_ms),
                    ActiveScreen::HifiControl(state) => state.on_tick(uptime_ms),
                }
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
                self.navigate(ui::Navigation::Launcher);
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

    pub const fn network_status(&self) -> NetworkStatus {
        self.network_status
    }

    pub const fn screen(&self) -> Screen {
        self.active_screen.screen()
    }

    fn handle_touch(&mut self, point: TouchPoint) {
        let point = Point::new(point.x, point.y);

        let navigation = match &mut self.active_screen {
            ActiveScreen::Launcher(_) => {
                ui::screens::launcher::hit_test(self.ui_layouts.launcher(), point)
            }
            ActiveScreen::Stopwatch(state) => {
                if let Some(action) =
                    ui::screens::stopwatch::hit_test(self.ui_layouts.stopwatch(), point, state)
                {
                    state.handle(action, self.uptime_ms);
                }
                None
            }
            ActiveScreen::HifiControl(state) => {
                if let Some(action) = ui::screens::hifi::hit_test(self.ui_layouts.hifi(), point) {
                    state.handle(action, self.uptime_ms);
                }
                None
            }
        };

        if let Some(navigation) = navigation {
            self.navigate(navigation);
        }
    }

    fn navigate(&mut self, navigation: ui::Navigation) {
        self.active_screen = ActiveScreen::new(navigation.screen());
        if let ActiveScreen::HifiControl(state) = &mut self.active_screen {
            state.on_enter(self.uptime_ms);
        }
    }

    fn ui_context(&self) -> ui::AppContext {
        ui::AppContext {
            network_status: self.network_status,
            interaction_count: self.interaction_count,
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
        let (start, _) = ui::stopwatch_button_centers();

        touch_point(start)
    }

    fn stop_button_touch() -> TouchPoint {
        let (_, stop) = ui::stopwatch_button_centers();

        touch_point(stop)
    }

    fn launcher_stopwatch_touch() -> TouchPoint {
        let (stopwatch, _) = ui::launcher_button_centers();

        touch_point(stopwatch)
    }

    fn launcher_hifi_touch() -> TouchPoint {
        let (_, hifi) = ui::launcher_button_centers();

        touch_point(hifi)
    }

    fn hifi_play_touch() -> TouchPoint {
        touch_point(ui::hifi_play_button_center())
    }

    fn stopwatch_state(app: &App) -> &ui::screens::stopwatch::State {
        match &app.active_screen {
            ActiveScreen::Stopwatch(state) => state,
            _ => panic!("expected stopwatch screen"),
        }
    }

    fn hifi_state(app: &App) -> &ui::screens::hifi::State {
        match &app.active_screen {
            ActiveScreen::HifiControl(state) => state,
            _ => panic!("expected hifi screen"),
        }
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
        let mut app = App::new_on_screen(Screen::Stopwatch);

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
        let mut app = App::new_on_screen(Screen::Stopwatch);

        app.update(Event::TouchDown(start_button_touch()));
        app.update(Event::Tick { uptime_ms: 1_000 });
        app.update(Event::Tick { uptime_ms: 2_000 });

        assert!(stopwatch_state(&app).running());
        assert_eq!(stopwatch_state(&app).seconds(), 2);

        app.update(Event::TouchDown(stop_button_touch()));
        app.update(Event::Tick { uptime_ms: 5_000 });

        assert!(!stopwatch_state(&app).running());
        assert_eq!(stopwatch_state(&app).seconds(), 2);
    }

    #[test]
    fn stopped_stopwatch_does_not_advance_with_uptime() {
        let mut app = App::new_on_screen(Screen::Stopwatch);

        app.update(Event::TouchDown(start_button_touch()));
        app.update(Event::Tick { uptime_ms: 3_000 });
        app.update(Event::TouchDown(stop_button_touch()));
        app.update(Event::Tick { uptime_ms: 20_000 });

        assert_eq!(app.uptime_ms(), 20_000);
        assert_eq!(stopwatch_state(&app).seconds(), 3);
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
    fn default_screen_is_launcher() {
        let app = App::new();

        assert_eq!(app.screen(), Screen::Launcher);
    }

    #[test]
    fn launcher_selects_app_screens() {
        let mut app = App::new();

        app.update(Event::TouchDown(launcher_stopwatch_touch()));
        assert_eq!(app.screen(), Screen::Stopwatch);

        let mut app = App::new();
        app.update(Event::TouchDown(launcher_hifi_touch()));
        assert_eq!(app.screen(), Screen::HifiControl);
    }

    #[test]
    fn navigation_recreates_screen_state() {
        let mut app = App::new_on_screen(Screen::Stopwatch);

        app.update(Event::TouchDown(start_button_touch()));
        app.update(Event::Tick { uptime_ms: 2_000 });
        assert_eq!(stopwatch_state(&app).seconds(), 2);

        app.update(Event::ButtonPressed(Button::User));
        app.update(Event::TouchDown(launcher_stopwatch_touch()));

        assert_eq!(app.screen(), Screen::Stopwatch);
        assert!(!stopwatch_state(&app).running());
        assert_eq!(stopwatch_state(&app).seconds(), 0);
    }

    #[test]
    fn hifi_counts_down_while_playing() {
        let mut app = App::new_on_screen(Screen::HifiControl);

        assert!(
            app.update(Event::Tick { uptime_ms: 1_000 })
                .render_requested
        );
        assert_eq!(
            hifi_state(&app).remaining_seconds(),
            hifi_state(&app).total_seconds() - 1
        );
        assert!(hifi_state(&app).playing());

        app.update(Event::TouchDown(hifi_play_touch()));
        assert!(!hifi_state(&app).playing());
        assert!(
            !app.update(Event::Tick { uptime_ms: 5_000 })
                .render_requested
        );
        assert_eq!(
            hifi_state(&app).remaining_seconds(),
            hifi_state(&app).total_seconds() - 1
        );
    }

    #[test]
    fn hifi_stops_when_countdown_reaches_zero() {
        let mut app = App::new_on_screen(Screen::HifiControl);

        let total_seconds = hifi_state(&app).total_seconds();
        let outcome = app.update(Event::Tick {
            uptime_ms: total_seconds * 1000,
        });

        assert!(outcome.render_requested);
        assert_eq!(hifi_state(&app).remaining_seconds(), 0);
        assert!(!hifi_state(&app).playing());
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
            let mut app = App::new_on_screen(Screen::Stopwatch);
            app.update(Event::NetworkStatus(status));

            let mut display = MockDisplay::<Rgb565>::new();
            display.set_allow_overdraw(true);
            display.set_allow_out_of_bounds_drawing(true);

            app.render(&mut display).unwrap();
        }
    }

    #[test]
    fn render_draws_all_screens() {
        for screen in [Screen::Launcher, Screen::Stopwatch, Screen::HifiControl] {
            let app = App::new_on_screen(screen);
            let mut display = MockDisplay::<Rgb565>::new();
            display.set_allow_overdraw(true);
            display.set_allow_out_of_bounds_drawing(true);

            app.render(&mut display).unwrap();
        }
    }
}
