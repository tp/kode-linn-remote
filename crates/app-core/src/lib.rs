#![no_std]

extern crate alloc;

use alloc::boxed::Box;
pub use board_kode_dot::{ControlButton, Direction};
use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};
use heapless::String;

mod ui;

pub use ui::RECOMMENDED_SCRATCH_PIXELS;
pub use ui::RenderSession;
pub use ui::screens::hifi::Command as HifiCommand;

pub type ArtworkPixel = Rgb565;

/// Panel geometry for the target board. Re-exported so screens and hosts all
/// agree on one number; change it in `board-kode-dot` and every layout reflows.
pub use board_kode_dot::DISPLAY_SIZE;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Tick { uptime_ms: u64 },
    TouchDown(TouchPoint),
    TouchUp,
    ButtonPressed(Button),
    NetworkStatus(NetworkStatus),
    HifiStatus(HifiStatus),
    HifiArtwork(HifiArtwork),
    HifiPins(HifiPins),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TouchPoint {
    pub x: i32,
    pub y: i32,
}

/// Physical inputs on the Kode Dot: a four-way pad plus two control buttons.
///
/// The pad moves a focus ring between the controls on the current screen and
/// [`Button::Select`] activates whatever is focused, so every screen stays
/// operable without touching the panel. [`Button::Back`] goes up one level.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
}

impl Button {
    /// The pad direction this button represents, if it is a pad direction.
    pub const fn direction(self) -> Option<Direction> {
        match self {
            Self::Up => Some(Direction::Up),
            Self::Down => Some(Direction::Down),
            Self::Left => Some(Direction::Left),
            Self::Right => Some(Direction::Right),
            Self::Select | Self::Back => None,
        }
    }
}

impl From<Direction> for Button {
    fn from(direction: Direction) -> Self {
        match direction {
            Direction::Up => Self::Up,
            Direction::Down => Self::Down,
            Direction::Left => Self::Left,
            Direction::Right => Self::Right,
        }
    }
}

impl From<ControlButton> for Button {
    fn from(control: ControlButton) -> Self {
        match control {
            ControlButton::Select => Self::Select,
            ControlButton::Back => Self::Back,
        }
    }
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

pub const HIFI_ARTWORK_PIXELS: usize = HIFI_ARTWORK_SIZE as usize * HIFI_ARTWORK_SIZE as usize;
pub const HIFI_ARTWORK_SIZE: u32 = 96;
pub const HIFI_TEXT_LEN: usize = 64;
pub const HIFI_URI_LEN: usize = 256;
pub const HIFI_PIN_COUNT: usize = 6;
pub const HIFI_PIN_TITLE_LEN: usize = 32;

/// Maximum volume value the receiver accepts; the volume arc fills at this value.
pub const HIFI_VOLUME_MAX: u8 = 70;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HifiPin {
    pub id: u32,
    pub title: String<HIFI_PIN_TITLE_LEN>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct HifiPins {
    pins: [Option<HifiPin>; HIFI_PIN_COUNT],
}

impl HifiPins {
    pub const fn new() -> Self {
        Self {
            pins: [None, None, None, None, None, None],
        }
    }

    pub fn set(&mut self, slot: usize, pin: HifiPin) -> bool {
        if slot >= HIFI_PIN_COUNT {
            return false;
        }
        self.pins[slot] = Some(pin);
        true
    }

    pub fn get(&self, slot: usize) -> Option<&HifiPin> {
        self.pins.get(slot).and_then(|entry| entry.as_ref())
    }

    pub const fn slots(&self) -> &[Option<HifiPin>; HIFI_PIN_COUNT] {
        &self.pins
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HifiStatus {
    pub title: String<HIFI_TEXT_LEN>,
    pub artist: String<HIFI_TEXT_LEN>,
    pub album: String<HIFI_TEXT_LEN>,
    pub album_art_uri: String<HIFI_URI_LEN>,
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
            album_art_uri: String::new(),
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
            album_art_uri: String::new(),
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HifiArtwork {
    pub source_uri: String<HIFI_URI_LEN>,
    pixels: HifiArtworkPixels,
    pixels_len: usize,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum HifiArtworkPixels {
    Owned(Box<[Rgb565; HIFI_ARTWORK_PIXELS]>),
    Static(&'static [Rgb565; HIFI_ARTWORK_PIXELS]),
}

impl HifiArtwork {
    pub fn new(source_uri: &str) -> Option<Self> {
        let mut artwork = Self {
            source_uri: String::new(),
            pixels: HifiArtworkPixels::Owned(Box::new([Rgb565::BLACK; HIFI_ARTWORK_PIXELS])),
            pixels_len: 0,
        };
        artwork.source_uri.push_str(source_uri).ok()?;
        Some(artwork)
    }

    pub fn from_static_pixels(
        source_uri: &str,
        pixels: &'static [Rgb565; HIFI_ARTWORK_PIXELS],
    ) -> Option<Self> {
        let mut source = String::new();
        source.push_str(source_uri).ok()?;
        Some(Self {
            source_uri: source,
            pixels: HifiArtworkPixels::Static(pixels),
            pixels_len: HIFI_ARTWORK_PIXELS,
        })
    }

    pub fn push_pixel(&mut self, color: Rgb565) -> bool {
        if self.pixels_len >= HIFI_ARTWORK_PIXELS {
            return false;
        }
        let HifiArtworkPixels::Owned(pixels) = &mut self.pixels else {
            return false;
        };
        pixels[self.pixels_len] = color;
        self.pixels_len += 1;
        true
    }

    pub fn push_rgb888(&mut self, red: u8, green: u8, blue: u8) -> bool {
        self.push_pixel(Rgb565::new(red >> 3, green >> 2, blue >> 3))
    }

    pub fn is_complete(&self) -> bool {
        self.pixels_len == HIFI_ARTWORK_PIXELS
    }

    pub fn pixels(&self) -> &[Rgb565] {
        match &self.pixels {
            HifiArtworkPixels::Owned(pixels) => &pixels[..self.pixels_len],
            HifiArtworkPixels::Static(pixels) => &pixels[..self.pixels_len],
        }
    }
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
    /// Index into the active screen's focus targets: the control the D-pad is
    /// on. `None` until the pad is first used, so a touch-only session never
    /// shows a ring it did not ask for.
    focus: Option<usize>,
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
            focus: None,
        }
    }

    pub fn update(&mut self, event: Event) -> UpdateOutcome {
        let mut command = None;
        let render_requested = match event {
            Event::Tick { uptime_ms } => {
                self.uptime_ms = uptime_ms;
                let context = self.ui_context();
                match &mut self.active_screen {
                    ActiveScreen::Launcher(state) => state.on_tick(context),
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
            Event::ButtonPressed(button) => {
                self.interaction_count = self.interaction_count.saturating_add(1);
                let (redraw, button_command) = self.handle_button(button);
                command = button_command;
                redraw
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
            Event::HifiArtwork(artwork) => match &mut self.active_screen {
                ActiveScreen::HifiControl(state) => state.apply_artwork(artwork),
                ActiveScreen::Launcher(_) | ActiveScreen::Stopwatch(_) => false,
            },
            Event::HifiPins(pins) => match &mut self.active_screen {
                ActiveScreen::HifiControl(state) => state.apply_pins(pins),
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

    /// Focusable controls on the screen currently shown.
    fn focus_targets(&self) -> ui::focus::FocusTargets {
        match &self.active_screen {
            ActiveScreen::Launcher(_) => {
                ui::screens::launcher::focus_targets(self.ui_layouts.launcher())
            }
            ActiveScreen::Stopwatch(_) => {
                ui::screens::stopwatch::focus_targets(self.ui_layouts.stopwatch())
            }
            ActiveScreen::HifiControl(state) => state.focus_targets(self.ui_layouts.hifi()),
        }
    }

    /// Bounds of the focused control, for the render pass to outline.
    pub(crate) fn focused_rect(&self) -> Option<Rectangle> {
        let targets = self.focus_targets();
        self.focus.and_then(|index| targets.get(index).copied())
    }

    /// Moves the ring. Returns whether it actually moved.
    ///
    /// No cache poking here: [`RenderSession`] compares the ring's bounds
    /// against what it last drew and clears the target itself. Input handling
    /// has no business knowing how a screen repaints.
    fn set_focus(&mut self, focus: Option<usize>) -> bool {
        if self.focus == focus {
            return false;
        }

        self.focus = focus;
        true
    }

    fn handle_button(&mut self, button: Button) -> (bool, Option<Command>) {
        match button {
            Button::Select => self.activate_focused(),
            Button::Back => (self.go_back(), None),
            Button::Up | Button::Down | Button::Left | Button::Right => {
                let direction = button
                    .direction()
                    .expect("pad variants always carry a direction");
                (self.move_focus(direction), None)
            }
        }
    }

    /// Presses the focused control by replaying it as a tap at its centre, so
    /// pad and touch share one dispatch path and cannot drift apart.
    fn activate_focused(&mut self) -> (bool, Option<Command>) {
        let Some(rect) = self.focused_rect() else {
            // Nothing focused yet: the first press just reveals the ring
            // rather than firing a control the user cannot see.
            return (self.set_focus(Some(0)), None);
        };

        let center = TouchPoint {
            x: rect.top_left.x + (rect.size.width / 2) as i32,
            y: rect.top_left.y + (rect.size.height / 2) as i32,
        };
        (true, self.handle_touch(center))
    }

    /// Up one level: out of a HiFi subpage first, then out to the launcher.
    fn go_back(&mut self) -> bool {
        if let ActiveScreen::HifiControl(state) = &mut self.active_screen
            && state.pop_page()
        {
            self.set_focus(None);
            return true;
        }

        if self.active_screen.screen() == Screen::Launcher {
            return false;
        }

        self.navigate(Screen::Launcher);
        true
    }

    fn move_focus(&mut self, direction: Direction) -> bool {
        let targets = self.focus_targets();
        if let Some(next) = ui::focus::step(&targets, self.focus, direction) {
            return self.set_focus(Some(next));
        }

        // Nothing further in that direction. On the HiFi screen the pages form
        // a vertical stack, so running off the top or bottom edge moves
        // between them instead of doing nothing.
        match (&mut self.active_screen, direction) {
            (ActiveScreen::HifiControl(state), Direction::Down) => {
                state.cycle_page();
                self.set_focus(None);
                true
            }
            (ActiveScreen::HifiControl(state), Direction::Up) => {
                state.cycle_page_back();
                self.set_focus(None);
                true
            }
            _ => false,
        }
    }

    fn handle_touch(&mut self, point: TouchPoint) -> Option<Command> {
        let point = Point::new(point.x, point.y);

        // Keep the ring under the finger, but only once the pad has been used;
        // a touch-only session should never sprout a focus ring on its own.
        if self.focus.is_some() {
            let targets = self.focus_targets();
            if let Some(index) = ui::focus::hit(&targets, point) {
                self.set_focus(Some(index));
            }
        }

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
        // Focus indices are per-screen; carrying one across would point at an
        // unrelated control.
        self.focus = None;
    }

    fn ui_context(&self) -> ui::AppContext {
        ui::AppContext {
            network_status: self.network_status,
            interaction_count: self.interaction_count,
            uptime_ms: self.uptime_ms,
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

    fn hifi_previous_touch() -> TouchPoint {
        touch_point(ui::hifi_previous_button_center())
    }

    fn hifi_next_touch() -> TouchPoint {
        touch_point(ui::hifi_next_button_center())
    }

    fn hifi_pin_slot_touch(slot: usize) -> TouchPoint {
        touch_point(ui::hifi_pin_slot_center(slot))
    }

    fn hifi_volume_increment_touch() -> TouchPoint {
        touch_point(ui::hifi_volume_increment_center())
    }

    fn hifi_volume_decrement_touch() -> TouchPoint {
        touch_point(ui::hifi_volume_decrement_center())
    }

    fn loaded_pin(id: u32, title: &str) -> HifiPin {
        let mut pin_title = String::<HIFI_PIN_TITLE_LEN>::new();
        pin_title.push_str(title).unwrap();
        HifiPin {
            id,
            title: pin_title,
        }
    }

    fn hifi_status(playback: PlaybackState) -> HifiStatus {
        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        status.playback = playback;
        status
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
        let mut app = App::new();
        let mut display = MockDisplay::<Rgb565>::new();
        display.set_allow_overdraw(true);
        display.set_allow_out_of_bounds_drawing(true);
        let mut scratch = test_scratch();
        let mut session = RenderSession::new();

        app.render(&mut display, &mut scratch, &mut session)
            .unwrap();
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

        app.update(Event::ButtonPressed(Button::Back));
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
        let mut status = hifi_status(PlaybackState::Playing);
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
    fn hifi_pin_slot_touch_requests_invoke_command_with_loaded_id() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut pins = HifiPins::new();
        pins.set(0, loaded_pin(4711, "Radio"));
        pins.set(1, loaded_pin(8128, "Spotify"));
        app.update(Event::HifiPins(pins));
        hifi_pad_to_pins(&mut app);

        let outcome = app.update(Event::TouchDown(hifi_pin_slot_touch(0)));
        assert!(outcome.render_requested);
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::InvokePinId { id: 4711 }))
        );

        let outcome = app.update(Event::TouchDown(hifi_pin_slot_touch(1)));
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::InvokePinId { id: 8128 }))
        );
    }

    #[test]
    fn hifi_pin_slot_without_loaded_pin_emits_no_command() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        hifi_pad_to_pins(&mut app);

        let outcome = app.update(Event::TouchDown(hifi_pin_slot_touch(0)));
        assert!(outcome.render_requested);
        assert_eq!(outcome.command, None);
    }

    /// Walks the pad from a freshly opened HiFi screen to the Pins page.
    ///
    /// The first press only reveals the focus ring; the second runs off the
    /// bottom of the single transport row, which is what advances the page.
    fn hifi_pad_to_pins(app: &mut App) {
        app.update(Event::ButtonPressed(Button::Down));
        app.update(Event::ButtonPressed(Button::Down));
    }

    /// Continues from the Pins page to the Volume page. The pins form a 3x2
    /// grid, so the ring steps into the second row before the page turns.
    fn hifi_pad_to_volume(app: &mut App) {
        hifi_pad_to_pins(app);
        app.update(Event::ButtonPressed(Button::Down));
        app.update(Event::ButtonPressed(Button::Down));
        app.update(Event::ButtonPressed(Button::Down));
    }

    #[test]
    fn hifi_pad_walks_through_the_pages() {
        let mut app = App::new_on_screen(Screen::HifiControl);

        let mut status = hifi_status(PlaybackState::Paused);
        status.volume_percent = 30;
        app.update(Event::HifiStatus(status));
        hifi_pad_to_volume(&mut app);

        let outcome = app.update(Event::TouchDown(hifi_volume_increment_touch()));
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::SetVolume { volume: 31 }))
        );

        // Back pops out of the subpage to Status, where the volume buttons are
        // no longer on screen.
        app.update(Event::ButtonPressed(Button::Back));
        assert_eq!(app.screen(), Screen::HifiControl);
        let outcome = app.update(Event::TouchDown(hifi_volume_decrement_touch()));
        assert_eq!(outcome.command, None);
    }

    #[test]
    fn back_from_hifi_status_leaves_for_the_launcher() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        app.update(Event::ButtonPressed(Button::Back));
        assert_eq!(app.screen(), Screen::Launcher);
    }

    #[test]
    fn back_from_stopwatch_navigates_to_launcher() {
        let mut app = App::new_on_screen(Screen::Stopwatch);
        app.update(Event::ButtonPressed(Button::Back));
        assert_eq!(app.screen(), Screen::Launcher);
    }

    #[test]
    fn back_on_the_launcher_does_nothing() {
        let mut app = App::new_on_screen(Screen::Launcher);
        let outcome = app.update(Event::ButtonPressed(Button::Back));
        assert_eq!(app.screen(), Screen::Launcher);
        assert!(!outcome.render_requested);
    }

    #[test]
    fn first_pad_press_reveals_the_ring_without_activating() {
        let mut app = App::new_on_screen(Screen::Launcher);

        // Select with nothing focused must not fire a control the user cannot
        // yet see; it only brings the ring up.
        let outcome = app.update(Event::ButtonPressed(Button::Select));
        assert!(outcome.render_requested);
        assert_eq!(app.screen(), Screen::Launcher);
    }

    #[test]
    fn pad_selects_launcher_entries() {
        let mut app = App::new_on_screen(Screen::Launcher);

        // Ring onto the first card, then confirm.
        app.update(Event::ButtonPressed(Button::Down));
        app.update(Event::ButtonPressed(Button::Select));
        assert_eq!(app.screen(), Screen::Stopwatch);

        app.update(Event::ButtonPressed(Button::Back));
        // Second card is below the first on the portrait layout.
        app.update(Event::ButtonPressed(Button::Down));
        app.update(Event::ButtonPressed(Button::Down));
        app.update(Event::ButtonPressed(Button::Select));
        assert_eq!(app.screen(), Screen::HifiControl);
    }

    #[test]
    fn pad_select_runs_the_stopwatch() {
        let mut app = App::new_on_screen(Screen::Stopwatch);

        // Ring onto Start, confirm, and the clock should advance.
        app.update(Event::ButtonPressed(Button::Right));
        app.update(Event::ButtonPressed(Button::Select));
        assert!(
            app.update(Event::Tick { uptime_ms: 2_000 })
                .render_requested
        );

        // Ring right onto Stop and confirm; ticks stop requesting frames.
        app.update(Event::ButtonPressed(Button::Right));
        app.update(Event::ButtonPressed(Button::Select));
        app.update(Event::Tick { uptime_ms: 3_000 });
        assert!(
            !app.update(Event::Tick { uptime_ms: 4_000 })
                .render_requested
        );
    }

    #[test]
    fn pad_select_toggles_hifi_playback() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        app.update(Event::HifiStatus(hifi_status(PlaybackState::Paused)));

        // Ring lands on the first transport control, then steps right onto
        // play/pause in the middle.
        app.update(Event::ButtonPressed(Button::Right));
        app.update(Event::ButtonPressed(Button::Right));
        let outcome = app.update(Event::ButtonPressed(Button::Select));

        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::TogglePlayback))
        );
    }

    #[test]
    fn hifi_play_touch_requests_toggle_command_when_paused() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let status = hifi_status(PlaybackState::Paused);
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
        let status = hifi_status(PlaybackState::Playing);
        app.update(Event::HifiStatus(status));

        let outcome = app.update(Event::TouchDown(hifi_play_touch()));

        assert!(outcome.render_requested);
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::TogglePlayback))
        );
    }

    #[test]
    fn hifi_track_touches_request_track_commands() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        app.update(Event::HifiStatus(hifi_status(PlaybackState::Playing)));

        let previous = app.update(Event::TouchDown(hifi_previous_touch()));
        let next = app.update(Event::TouchDown(hifi_next_touch()));

        assert_eq!(
            previous.command,
            Some(Command::Hifi(HifiCommand::PreviousTrack))
        );
        assert_eq!(next.command, Some(Command::Hifi(HifiCommand::NextTrack)));
    }

    #[test]
    fn hifi_status_updates_screen() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = hifi_status(PlaybackState::Playing);
        status.elapsed_seconds = 30;
        status.duration_seconds = 120;
        status.volume_percent = 42;

        let outcome = app.update(Event::HifiStatus(status));

        assert!(outcome.render_requested);
    }

    #[test]
    fn hifi_stops_when_countdown_reaches_zero() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = hifi_status(PlaybackState::Playing);
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
    fn launcher_connecting_network_status_animates_on_tick() {
        let mut app = App::new();

        app.update(Event::NetworkStatus(NetworkStatus::Connecting));
        let outcome = app.update(Event::Tick { uptime_ms: 120 });

        assert!(outcome.render_requested);
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
            let mut scratch = test_scratch();
            let mut session = RenderSession::new();

            app.render(&mut display, &mut scratch, &mut session)
                .unwrap();
        }
    }

    #[test]
    fn render_draws_all_screens() {
        for screen in [Screen::Launcher, Screen::Stopwatch, Screen::HifiControl] {
            let mut app = App::new_on_screen(screen);
            let mut display = MockDisplay::<Rgb565>::new();
            display.set_allow_overdraw(true);
            display.set_allow_out_of_bounds_drawing(true);
            let mut scratch = test_scratch();
            let mut session = RenderSession::new();

            app.render(&mut display, &mut scratch, &mut session)
                .unwrap();
        }
    }

    /// A display that can be told to fail, and that counts the pixels it took.
    ///
    /// `MockDisplay` cannot fail on demand, and these two tests are entirely
    /// about what happens when a frame does not complete.
    struct FlakyDisplay {
        fail_after: Option<usize>,
        drawn: usize,
    }

    impl FlakyDisplay {
        fn new() -> Self {
            Self {
                fail_after: None,
                drawn: 0,
            }
        }

        fn failing_after(pixels: usize) -> Self {
            Self {
                fail_after: Some(pixels),
                drawn: 0,
            }
        }
    }

    impl DrawTarget for FlakyDisplay {
        type Color = Rgb565;
        type Error = ();

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = embedded_graphics::Pixel<Self::Color>>,
        {
            for _ in pixels {
                if let Some(limit) = self.fail_after
                    && self.drawn >= limit
                {
                    return Err(());
                }
                self.drawn += 1;
            }
            Ok(())
        }
    }

    impl embedded_graphics::geometry::OriginDimensions for FlakyDisplay {
        fn size(&self) -> Size {
            DISPLAY_SIZE
        }
    }

    #[test]
    fn rendering_to_a_second_target_is_not_suppressed_by_the_first() {
        // The bug this guards: with the cache inside App, the first target's
        // "already drawn" flags made the second target come out mostly blank.
        let mut app = App::new_on_screen(Screen::HifiControl);
        app.update(Event::Tick { uptime_ms: 6_000 });
        let mut scratch = test_scratch();

        let mut first_session = RenderSession::new();
        let mut first = FlakyDisplay::new();
        app.render(&mut first, &mut scratch, &mut first_session)
            .unwrap();

        let mut second_session = RenderSession::new();
        let mut second = FlakyDisplay::new();
        app.render(&mut second, &mut scratch, &mut second_session)
            .unwrap();

        assert!(first.drawn > 0, "first target received pixels");
        assert_eq!(
            first.drawn, second.drawn,
            "a fresh session must paint the second target as fully as the first"
        );
    }

    #[test]
    fn reusing_one_session_across_targets_still_smart_skips() {
        // The other half of the contract: a session is per-target *because*
        // reusing one legitimately suppresses redundant drawing.
        let mut app = App::new_on_screen(Screen::HifiControl);
        app.update(Event::Tick { uptime_ms: 6_000 });
        let mut scratch = test_scratch();
        let mut session = RenderSession::new();

        let mut first = FlakyDisplay::new();
        app.render(&mut first, &mut scratch, &mut session).unwrap();

        let mut second = FlakyDisplay::new();
        app.render(&mut second, &mut scratch, &mut session).unwrap();

        assert!(
            second.drawn < first.drawn,
            "unchanged widgets should be skipped on the second frame"
        );
    }

    #[test]
    fn a_failed_frame_does_not_leave_the_cache_claiming_pixels() {
        // The bug this guards: a frame that failed part-way had already
        // updated some cache fields, so the retry skipped widgets that never
        // reached the panel.
        let mut app = App::new_on_screen(Screen::HifiControl);
        app.update(Event::Tick { uptime_ms: 6_000 });
        let mut scratch = test_scratch();

        // Baseline: what a clean frame costs.
        let mut reference_session = RenderSession::new();
        let mut reference = FlakyDisplay::new();
        app.render(&mut reference, &mut scratch, &mut reference_session)
            .unwrap();
        let full_frame = reference.drawn;
        assert!(full_frame > 100, "sanity: a full frame draws a lot");

        // Now fail partway through, then retry on the same session.
        let mut session = RenderSession::new();
        let mut failing = FlakyDisplay::failing_after(full_frame / 2);
        assert!(
            app.render(&mut failing, &mut scratch, &mut session)
                .is_err(),
            "the display was told to fail"
        );

        let mut retry = FlakyDisplay::new();
        app.render(&mut retry, &mut scratch, &mut session).unwrap();

        assert_eq!(
            retry.drawn, full_frame,
            "the retry must repaint in full, not skip what the failed frame never drew"
        );
    }

    #[test]
    fn external_clear_forces_a_full_repaint() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        app.update(Event::Tick { uptime_ms: 6_000 });
        let mut scratch = test_scratch();
        let mut session = RenderSession::new();

        let mut first = FlakyDisplay::new();
        app.render(&mut first, &mut scratch, &mut session).unwrap();

        // Something outside App::render wiped the panel.
        session.note_external_clear();

        let mut after = FlakyDisplay::new();
        app.render(&mut after, &mut scratch, &mut session).unwrap();

        assert_eq!(
            after.drawn, first.drawn,
            "after an external clear the next frame must repaint everything"
        );
    }

    fn test_scratch() -> std::vec::Vec<Rgb565> {
        std::vec![Rgb565::BLACK; RECOMMENDED_SCRATCH_PIXELS]
    }
}
