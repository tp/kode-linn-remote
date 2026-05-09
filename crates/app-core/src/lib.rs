#![no_std]

use embedded_graphics::prelude::*;
use heapless::String;

mod ui;

pub use ui::screens::hifi::Command as HifiCommand;

pub const DISPLAY_SIZE: Size = Size::new(466, 466);

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Tick { uptime_ms: u64 },
    TouchDown(TouchPoint),
    TouchUp,
    ButtonPressed(Button),
    NetworkStatus(NetworkStatus),
    HifiStatus(HifiStatus),
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
    pub command: Option<Command>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    Hifi(HifiCommand),
}

pub const HIFI_TEXT_LEN: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HifiStatus {
    pub title: String<HIFI_TEXT_LEN>,
    pub artist: String<HIFI_TEXT_LEN>,
    pub album: String<HIFI_TEXT_LEN>,
    pub playback: PlaybackState,
    pub elapsed_seconds: u32,
    pub duration_seconds: u32,
    pub volume_percent: u8,
}

impl HifiStatus {
    pub fn empty() -> Self {
        Self {
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            playback: PlaybackState::Unknown,
            elapsed_seconds: 0,
            duration_seconds: 0,
            volume_percent: 0,
        }
    }

    pub fn waiting() -> Self {
        Self {
            title: string_from("Waiting for Linn"),
            artist: String::new(),
            album: String::new(),
            playback: PlaybackState::Stopped,
            elapsed_seconds: 0,
            duration_seconds: 0,
            volume_percent: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackState {
    Playing,
    Paused,
    Stopped,
    Buffering,
    Unknown,
}

fn string_from<const N: usize>(value: &str) -> String<N> {
    let mut output = String::new();
    let _ = output.push_str(value);
    output
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
    fn new(screen: Screen, uptime_ms: u64) -> Self {
        match screen {
            Screen::Launcher => Self::Launcher(ui::screens::launcher::State::new()),
            Screen::Stopwatch => Self::Stopwatch(ui::screens::stopwatch::State::new()),
            Screen::HifiControl => Self::HifiControl(ui::screens::hifi::State::new(uptime_ms)),
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
            active_screen: ActiveScreen::new(screen, 0),
        }
    }

    pub fn update(&mut self, event: Event) -> UpdateOutcome {
        let mut command = None;
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
                command = self.handle_touch(point);
                true
            }
            Event::TouchUp => false,
            Event::ButtonPressed(_) => {
                self.interaction_count = self.interaction_count.saturating_add(1);
                self.navigate(Screen::Launcher);
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
            Event::HifiStatus(status) => match &mut self.active_screen {
                ActiveScreen::HifiControl(state) => state.apply_status(status, self.uptime_ms),
                ActiveScreen::Launcher(_) | ActiveScreen::Stopwatch(_) => false,
            },
        };

        UpdateOutcome {
            render_requested,
            command,
        }
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

    fn handle_touch(&mut self, point: TouchPoint) -> Option<Command> {
        let point = Point::new(point.x, point.y);

        let (destination, command) = match &mut self.active_screen {
            ActiveScreen::Launcher(_) => (
                ui::screens::launcher::handle_touch(self.ui_layouts.launcher(), point),
                None,
            ),
            ActiveScreen::Stopwatch(state) => (
                state.handle_touch(self.ui_layouts.stopwatch(), point, self.uptime_ms),
                None,
            ),
            ActiveScreen::HifiControl(state) => {
                let command = state
                    .handle_touch(self.ui_layouts.hifi(), point, self.uptime_ms)
                    .map(Command::Hifi);
                (None, command)
            }
        };

        if let Some(destination) = destination {
            self.navigate(destination);
        }

        command
    }

    fn navigate(&mut self, screen: Screen) {
        self.active_screen = ActiveScreen::new(screen, self.uptime_ms);
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

    fn hifi_pin_1_touch() -> TouchPoint {
        touch_point(ui::hifi_pin_1_button_center())
    }

    fn hifi_pin_2_touch() -> TouchPoint {
        touch_point(ui::hifi_pin_2_button_center())
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
        assert!(
            app.update(Event::Tick { uptime_ms: 1_000 })
                .render_requested
        );

        app.update(Event::TouchDown(stop_button_touch()));
        assert!(
            !app.update(Event::Tick { uptime_ms: 5_000 })
                .render_requested
        );
    }

    #[test]
    fn stopped_stopwatch_does_not_advance_with_uptime() {
        let mut app = App::new_on_screen(Screen::Stopwatch);

        app.update(Event::TouchDown(start_button_touch()));
        app.update(Event::Tick { uptime_ms: 3_000 });
        app.update(Event::TouchDown(stop_button_touch()));

        assert!(
            !app.update(Event::Tick { uptime_ms: 20_000 })
                .render_requested
        );
        assert_eq!(app.uptime_ms(), 20_000);
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
        assert!(
            app.update(Event::Tick { uptime_ms: 2_000 })
                .render_requested
        );

        app.update(Event::ButtonPressed(Button::User));
        app.update(Event::TouchDown(launcher_stopwatch_touch()));

        assert_eq!(app.screen(), Screen::Stopwatch);
        assert!(
            !app.update(Event::Tick { uptime_ms: 3_000 })
                .render_requested
        );
    }

    #[test]
    fn hifi_counts_down_while_playing() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = HifiStatus::waiting();
        status.playback = PlaybackState::Playing;
        status.elapsed_seconds = 10;
        status.duration_seconds = 120;
        app.update(Event::HifiStatus(status));

        assert!(
            app.update(Event::Tick { uptime_ms: 1_000 })
                .render_requested
        );

        app.update(Event::TouchDown(hifi_play_touch()));
        assert!(
            !app.update(Event::Tick { uptime_ms: 5_000 })
                .render_requested
        );
    }

    #[test]
    fn hifi_pin_1_touch_requests_invoke_command() {
        let mut app = App::new_on_screen(Screen::HifiControl);

        let outcome = app.update(Event::TouchDown(hifi_pin_1_touch()));

        assert!(outcome.render_requested);
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::ActivatePreset { preset: 1 }))
        );
    }

    #[test]
    fn hifi_pin_2_touch_requests_invoke_command() {
        let mut app = App::new_on_screen(Screen::HifiControl);

        let outcome = app.update(Event::TouchDown(hifi_pin_2_touch()));

        assert!(outcome.render_requested);
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::ActivatePreset { preset: 2 }))
        );
    }

    #[test]
    fn hifi_play_touch_requests_toggle_command_when_paused() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = HifiStatus::waiting();
        status.playback = PlaybackState::Paused;
        app.update(Event::HifiStatus(status));

        let outcome = app.update(Event::TouchDown(hifi_play_touch()));

        assert!(outcome.render_requested);
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::TogglePlayback))
        );
    }

    #[test]
    fn hifi_play_touch_requests_toggle_command_while_playing() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = HifiStatus::waiting();
        status.playback = PlaybackState::Playing;
        app.update(Event::HifiStatus(status));

        let outcome = app.update(Event::TouchDown(hifi_play_touch()));

        assert!(outcome.render_requested);
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::TogglePlayback))
        );
    }

    #[test]
    fn hifi_status_updates_screen() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = HifiStatus::waiting();
        status.playback = PlaybackState::Playing;
        status.elapsed_seconds = 30;
        status.duration_seconds = 120;
        status.volume_percent = 42;

        let outcome = app.update(Event::HifiStatus(status));

        assert!(outcome.render_requested);
    }

    #[test]
    fn hifi_stops_when_countdown_reaches_zero() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = HifiStatus::waiting();
        status.playback = PlaybackState::Playing;
        status.elapsed_seconds = 119;
        status.duration_seconds = 120;
        app.update(Event::HifiStatus(status));

        let outcome = app.update(Event::Tick { uptime_ms: 1_000 });

        assert!(outcome.render_requested);
        assert!(
            !app.update(Event::Tick { uptime_ms: 2_000 })
                .render_requested
        );
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
