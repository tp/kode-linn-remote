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

// `HifiStatus` and the artwork variants dwarf the input ones, which is what a
// message type carrying both a keypress and a decoded cover looks like. Boxing
// is not on offer on the `no_std` side, and the enum is passed by value on a
// channel rather than stored, so the cost is one move per event.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Event {
    Tick {
        uptime_ms: u64,
    },
    TouchDown(TouchPoint),
    TouchUp,
    ButtonPressed(Button),
    NetworkStatus(NetworkStatus),
    HifiStatus(HifiStatus),
    HifiArtwork(HifiArtwork),
    HifiPins(HifiPins),
    /// Cover for one pin tile. Separate from [`Event::HifiPins`] because the
    /// list arrives long before six images can be fetched, and a tile should
    /// appear as soon as it is named rather than waiting for its picture.
    HifiPinArtwork {
        slot: usize,
        artwork: HifiArtwork,
    },
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
/// Decoded artwork resolution, and therefore the size of the Now Playing
/// artwork slot — the two are the same constant so they cannot drift.
///
/// 330 px is ~27 mm on this panel, big enough to recognise a cover across a
/// room. It is also `330 % 4 == 2`, which is what a centred widget needs to
/// stay on the display controller's 2-px write grid.
pub const HIFI_ARTWORK_SIZE: u32 = 330;
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
    /// Cover for whatever the pin plays. Empty when the DS offers none.
    pub artwork_uri: String<HIFI_URI_LEN>,
}

impl HifiPin {
    /// Builds a pin from a borrowed title, truncating anything that does not
    /// fit. Lets callers construct one without naming `heapless::String`.
    pub fn new(id: u32, title: &str) -> Self {
        Self {
            id,
            title: string_from(title),
            artwork_uri: String::new(),
        }
    }
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

// This enum *is* the app's screen storage: exactly one screen is live at a
// time and its state lives inline here. Being as large as the largest screen is
// the point, and boxing the big variant is not on offer in a `no_std` crate
// without `alloc`.
#[allow(clippy::large_enum_variant)]
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
            Event::HifiPinArtwork { slot, artwork } => match &mut self.active_screen {
                ActiveScreen::HifiControl(state) => state.apply_pin_artwork(slot, artwork),
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
        // Screens that bind the pad to actions rather than to movement get
        // first refusal on every press. Returning `None` means "not mine",
        // and the generic focus path below handles it as usual.
        let uptime_ms = self.uptime_ms;
        if let ActiveScreen::HifiControl(state) = &mut self.active_screen
            && let Some(outcome) = state.intercept_button(button, uptime_ms)
        {
            if outcome.page_changed {
                // Focus indices are per-page; carrying one across would point
                // at an unrelated control.
                self.focus = None;
            }
            return (outcome.redraw, outcome.command.map(Command::Hifi));
        }

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

    /// Up one level, out to the launcher.
    ///
    /// The HiFi screen never reaches here: it intercepts `Back` to move
    /// between Now Playing and Choices, which is the only navigation it has.
    fn go_back(&mut self) -> bool {
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

        // Nothing further in that direction, so the ring stays put rather than
        // wrapping — on a small panel, wrapping makes it easy to lose.
        //
        // This is also the seam a scrolling picker would use: "ran off the
        // edge with nowhere to go" is exactly the signal that should later
        // mean "scroll a row".
        false
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

    fn hifi_artwork_touch() -> TouchPoint {
        touch_point(ui::hifi_artwork_center())
    }

    fn hifi_tile_touch(slot: usize) -> TouchPoint {
        touch_point(ui::hifi_tile_center(slot))
    }

    /// Back is the only way between the two HiFi screens, and it goes both
    /// ways rather than out to the launcher.
    fn hifi_to_choices(app: &mut App) {
        app.update(Event::ButtonPressed(Button::Back));
    }

    #[test]
    fn opening_the_hifi_screen_commands_nothing() {
        // A remote that touches the streamer just by being looked at would be
        // a bad remote. Opening the screen must read and never write.
        let mut app = App::new_on_screen(Screen::HifiControl);

        for event in [
            Event::Tick { uptime_ms: 100 },
            Event::HifiStatus(hifi_status(PlaybackState::Playing)),
            Event::HifiPins(HifiPins::new()),
            Event::Tick { uptime_ms: 1_100 },
            Event::NetworkStatus(NetworkStatus::Online),
        ] {
            assert_eq!(
                app.update(event).command,
                None,
                "opening HiFi produced a command"
            );
        }
    }

    #[test]
    fn navigating_off_the_launcher_commands_nothing() {
        let mut app = App::new();
        app.update(Event::ButtonPressed(Button::Select));

        for _ in 0..4 {
            let outcome = app.update(Event::ButtonPressed(Button::Select));
            assert_eq!(
                outcome.command, None,
                "activating a launcher card sent a command"
            );
            if app.screen() != Screen::Launcher {
                break;
            }
        }
        assert_ne!(app.screen(), Screen::Launcher, "the pad never left home");
    }

    #[test]
    fn the_pad_can_start_a_pin() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut pins = HifiPins::new();
        pins.set(0, loaded_pin(4711, "Radio"));
        pins.set(1, loaded_pin(8128, "Spotify"));
        app.update(Event::HifiPins(pins));
        hifi_to_choices(&mut app);

        // Reveal the ring, then activate what it sits on.
        let revealed = app.update(Event::ButtonPressed(Button::Select));
        assert_eq!(
            revealed.command, None,
            "the first press should only show the ring"
        );

        let activated = app.update(Event::ButtonPressed(Button::Select));
        assert_eq!(
            activated.command,
            Some(Command::Hifi(HifiCommand::InvokePinId { id: 4711 })),
            "the pad could not start a pin the touch screen can"
        );
    }

    #[test]
    fn the_pad_can_start_a_pin_it_moved_to() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut pins = HifiPins::new();
        pins.set(0, loaded_pin(4711, "Radio"));
        pins.set(1, loaded_pin(8128, "Spotify"));
        app.update(Event::HifiPins(pins));
        hifi_to_choices(&mut app);

        app.update(Event::ButtonPressed(Button::Select));
        app.update(Event::ButtonPressed(Button::Right));
        let activated = app.update(Event::ButtonPressed(Button::Select));

        assert_eq!(
            activated.command,
            Some(Command::Hifi(HifiCommand::InvokePinId { id: 8128 }))
        );
    }

    fn loaded_pin(id: u32, title: &str) -> HifiPin {
        let mut pin_title = String::<HIFI_PIN_TITLE_LEN>::new();
        pin_title.push_str(title).unwrap();
        HifiPin {
            id,
            title: pin_title,
            artwork_uri: String::new(),
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

        app.update(Event::ButtonPressed(Button::Select));
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
        hifi_to_choices(&mut app);

        let outcome = app.update(Event::TouchDown(hifi_tile_touch(0)));
        assert!(outcome.render_requested);
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::InvokePinId { id: 4711 }))
        );

        // Playing a choice returns to Now Playing, so getting at the second
        // one means going back to the grid.
        hifi_to_choices(&mut app);
        let outcome = app.update(Event::TouchDown(hifi_tile_touch(1)));
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::InvokePinId { id: 8128 }))
        );
    }

    #[test]
    fn hifi_pin_slot_without_loaded_pin_emits_no_command() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        hifi_to_choices(&mut app);

        let outcome = app.update(Event::TouchDown(hifi_tile_touch(0)));
        assert!(outcome.render_requested);
        assert_eq!(outcome.command, None);
    }

    #[test]
    fn hifi_volume_takes_one_press_on_now_playing() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = hifi_status(PlaybackState::Paused);
        status.volume_percent = 30;
        app.update(Event::HifiStatus(status));

        // No ring to reveal first: the very first press moves the volume.
        let outcome = app.update(Event::ButtonPressed(Button::Up));
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::SetVolume { volume: 32 }))
        );
        let outcome = app.update(Event::ButtonPressed(Button::Down));
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::SetVolume { volume: 30 }))
        );
    }

    #[test]
    fn hifi_pad_moves_between_the_two_screens_and_never_leaves() {
        let mut app = App::new_on_screen(Screen::HifiControl);

        // Back goes to Choices rather than out to the launcher.
        app.update(Event::ButtonPressed(Button::Back));
        assert_eq!(app.screen(), Screen::HifiControl);

        // On Choices the pad moves the ring; volume is not reachable there.
        let outcome = app.update(Event::ButtonPressed(Button::Up));
        assert_eq!(outcome.command, None);

        // And Back comes home again.
        app.update(Event::ButtonPressed(Button::Back));
        assert_eq!(app.screen(), Screen::HifiControl);
        let outcome = app.update(Event::ButtonPressed(Button::Up));
        assert!(matches!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::SetVolume { .. }))
        ));
    }

    #[test]
    fn hifi_choices_select_plays_the_focused_tile() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut pins = HifiPins::new();
        pins.set(0, loaded_pin(4711, "Radio"));
        app.update(Event::HifiPins(pins));
        hifi_to_choices(&mut app);

        // First press reveals the ring on the first tile, second activates it.
        app.update(Event::ButtonPressed(Button::Select));
        let outcome = app.update(Event::ButtonPressed(Button::Select));
        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::InvokePinId { id: 4711 }))
        );
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

        // Straight to it: Now Playing binds Select to play/pause rather than
        // to "activate whatever the ring is on".
        let outcome = app.update(Event::ButtonPressed(Button::Select));

        assert_eq!(
            outcome.command,
            Some(Command::Hifi(HifiCommand::TogglePlayback))
        );
    }

    #[test]
    fn now_playing_ignores_taps_so_the_remote_can_be_carried() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        app.update(Event::HifiStatus(hifi_status(PlaybackState::Playing)));

        let outcome = app.update(Event::TouchDown(hifi_artwork_touch()));

        assert_eq!(outcome.command, None);
    }

    #[test]
    fn hifi_track_buttons_request_track_commands() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = hifi_status(PlaybackState::Playing);
        status.duration_seconds = 200;
        app.update(Event::HifiStatus(status));

        let next = app.update(Event::ButtonPressed(Button::Right));
        assert_eq!(next.command, Some(Command::Hifi(HifiCommand::NextTrack)));

        // Right cleared the track, so elapsed is 0 and Left goes straight back
        // rather than restarting.
        let previous = app.update(Event::ButtonPressed(Button::Left));
        assert_eq!(
            previous.command,
            Some(Command::Hifi(HifiCommand::PreviousTrack))
        );
    }

    #[test]
    fn hifi_left_restarts_a_track_that_is_already_under_way() {
        let mut app = App::new_on_screen(Screen::HifiControl);
        let mut status = hifi_status(PlaybackState::Playing);
        status.elapsed_seconds = 30;
        status.duration_seconds = 200;
        app.update(Event::HifiStatus(status));

        let outcome = app.update(Event::ButtonPressed(Button::Left));
        assert_eq!(outcome.command, Some(Command::Hifi(HifiCommand::Restart)));
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
