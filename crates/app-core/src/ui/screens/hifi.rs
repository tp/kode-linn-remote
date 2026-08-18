//! The hi-fi remote: two screens, six buttons.
//!
//! # Why this screen does not use the focus ring
//!
//! Every other screen publishes focusable rectangles and lets
//! [`super::super::focus`] move a ring between them, with `Select` replaying
//! the focused control as a tap. That is the right model for a screen you
//! *browse*.
//!
//! It is the wrong model for a screen you *operate*. The most common thing
//! anyone does with a remote is nudge the volume, and a focus ring turns that
//! into two presses: one to move the ring onto the volume control, one to
//! press it. So [`HifiPage::NowPlaying`] binds the pad directly — up and down
//! are volume, left and right are the track, `Select` is play/pause — and
//! publishes no focus targets at all.
//!
//! [`HifiPage::Choices`] is a grid of things to play, which *is* a browse
//! screen, so it keeps the ring and the geometric movement that comes with it.
//!
//! The invariant across both: **the pad manipulates what is on screen,
//! `Select` acts, `Back` leaves.**

use embedded_graphics::{
    Pixel,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use heapless::String;
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use crate::{
    Button, HIFI_ARTWORK_SIZE, HIFI_TEXT_LEN, HIFI_URI_LEN, HIFI_VOLUME_MAX, HifiArtwork, HifiPins,
    HifiStatus, PlaybackState, RenderError,
};

use super::super::{
    aa,
    components::{clear_rect, draw_panel, draw_panel_over, draw_spinner_dots, ui_font},
    focus::FocusTargets,
    optimistic::{OPTIMISTIC_TTL_MS, Optimistic},
    painter::Painter,
    style::*,
    widget::{Slot, Widget},
};

// ---------------------------------------------------------------------------
// Now Playing layout
//
// Absolute framebuffer coordinates on the 410 x 502 panel rather than offsets
// into an inset body: the artwork is the hero and wants more width than a
// cosmetic margin would leave it.
//
// Vertical rhythm, top to bottom:
//   28 margin / 330 artwork / 14 / 40 title / 2 / 40 artist / 16 / 6 progress
//   / 26 margin  =  502 exactly.
//
// Every origin and extent here is even, and the centred widgets have *odd*
// half-widths, which is what `Painter::is_two_aligned` actually requires: the
// panel is 410 wide so its centre is x=205, and an odd centre minus an even
// half-width lands on an odd edge. In short, centred widths must satisfy
// `width % 4 == 2`. Both 330 and 190 below do.
// ---------------------------------------------------------------------------

/// Horizontal margin for the text bands. The artwork is wider than this.
const TEXT_INSET: i32 = 24;
const TEXT_WIDTH: u32 = 362;

const ARTWORK_TOP: i32 = 28;
/// The artwork slot is exactly the decoded artwork, so one constant governs
/// both the buffer and the destination rectangle and they cannot drift.
const ARTWORK_SIZE: u32 = HIFI_ARTWORK_SIZE;

const TITLE_TOP: i32 = 372;
const TITLE_HEIGHT: u32 = 40;
const ARTIST_TOP: i32 = 414;
/// One full line of the UI font, which is `line_height(40)`. Anything less
/// clips the text rather than shrinking it — there is only one font size in
/// this UI, so bands are sized to it rather than the other way round.
const ARTIST_HEIGHT: u32 = 40;

const PROGRESS_TOP: i32 = 470;
const PROGRESS_WIDTH: u32 = 330;
const PROGRESS_HEIGHT: u32 = 6;

// Volume readout. There is no permanent volume bar: on Now Playing one would
// sit directly under the progress bar and read as a second progress bar, and
// on Choices the pad moves the ring so volume cannot be changed there at all.
// It appears while it is being changed, then goes away.
const OVERLAY_LEFT: i32 = 84;
const OVERLAY_TOP: i32 = 140;
const OVERLAY_WIDTH: u32 = 242;
const OVERLAY_HEIGHT: u32 = 106;
const OVERLAY_RADIUS: u32 = 22;
const OVERLAY_VALUE_TOP: i32 = 158;
const OVERLAY_VALUE_HEIGHT: u32 = 40;
const OVERLAY_TRACK_LEFT: i32 = 110;
const OVERLAY_TRACK_TOP: i32 = 214;
const OVERLAY_TRACK_WIDTH: u32 = 190;
const OVERLAY_TRACK_HEIGHT: u32 = 10;

// Paused badge. Drawn over the artwork rather than spelled out in the artist
// line: the state belongs to the picture, and a word beside the artist reads
// as part of the artist's name. It is centred on the same line as the volume
// readout, which therefore covers it completely while volume is being changed.
/// How long the previous track's cover may stand in for one still being
/// fetched.
///
/// Long enough to cover a fetch and decode, short enough that a track with no
/// artwork of its own does not keep wearing someone else's.
const STALE_ARTWORK_GRACE_MS: u64 = 900;
const PAUSE_BADGE_SIZE: u32 = 106;
const PAUSE_BADGE_RADIUS: u32 = 22;

/// How long the volume readout stays up after the last press.
const VOLUME_OVERLAY_MS: u64 = 1_000;
/// Volume change per press. Hold-to-ramp comes from platform-side auto-repeat
/// of the same event, so this stays a single step.
const VOLUME_STEP: i16 = 2;

/// Past this point into a track, `Left` restarts it instead of going back.
///
/// This is deliberately *one* comparison rather than a double-press timer:
/// restarting sets elapsed to zero, so a second press immediately afterwards
/// falls through to the previous track on its own. The familiar
/// press-twice-to-go-back behaviour is an emergent consequence, needing no
/// extra state and no timing window.
const RESTART_THRESHOLD_SECONDS: u32 = 3;
/// How far ahead of our own clock an incoming position may be, while a restart
/// or a track change is still settling, before it is treated as describing the
/// position we just left.
///
/// Generous enough to absorb ordinary poll jitter, far tighter than the gap
/// between "a few seconds in" and wherever the track had actually reached.
const PROGRESS_GUARD_TOLERANCE_SECONDS: u32 = 3;

const LOADING_TIMEOUT_MS: u64 = 5_000;

// Marquee bounce: hold at start, scroll to end, brief hold, scroll back.
const MARQUEE_HOLD_START_MS: u64 = 1_000;
const MARQUEE_HOLD_END_MS: u64 = 500;
const MARQUEE_SCROLL_PX_PER_SEC: u64 = 30;

// Slot kinds for the artwork area, which is shared by the spinner, the
// artwork itself, and the play/pause glyph shown when there is no artwork.
const ART_SLOT_SPINNER: u8 = 1;
const ART_SLOT_PLAY_ICON: u8 = 2;
const ART_SLOT_PAUSE_BARS: u8 = 3;
const ART_SLOT_BUFFERING: u8 = 4;
const ART_SLOT_ARTWORK: u8 = 5;

// ---------------------------------------------------------------------------
// Choices layout
//
// 2 x 2, because that is what the panel affords. At ~304 PPI a 3 x 2 grid
// gives 9.5 mm covers and 2 x 3 gives 12.5 mm; three rows wide enough to fill
// two columns do not fit in 502 px at all. 170 px is ~14.2 mm, about the
// smallest a picture can be and still be recognised at a glance.
//
// Four *visible* is not four *total*. When the list outgrows the viewport this
// should scroll rather than page, so that a tile's position relative to its
// neighbours never changes. Two things here exist to keep that cheap: the row
// pitch is even (210 + 12 = 222), so a row-snapped scroll offset keeps every
// tile on the 2-px write grid; and `focus_targets` publishes only the tiles
// currently on screen, so `focus::step` stays bounded however long the list
// gets.
// ---------------------------------------------------------------------------

const CHOICES_HEADER_TOP: i32 = 8;
const CHOICES_HEADER_HEIGHT: u32 = 40;
const CHOICES_GRID_TOP: i32 = 56;
const CHOICES_GRID_LEFT: i32 = 20;
const TILE_WIDTH: u32 = 170;
/// Square, because cover art is square and cropping it to fit loses the top
/// and bottom of every picture.
const TILE_ART_HEIGHT: u32 = TILE_WIDTH;
/// One line of the UI font, which is `line_height(40)`. There is only one font
/// size in this UI, so the band is sized to it rather than the other way
/// round — a shorter band clips the text instead of shrinking it.
///
/// The caption sits below the art rather than over it, which is what costs the
/// tiles 16 px of width: 186 px art plus a 40 px caption makes a 226 px tile,
/// and two rows of those do not fit in 502 px. Labelling over the artwork
/// would have kept the extra width, but the label's backing strip squares off
/// the card's rounded lower corners, and 14.2 mm of cover reads about as well
/// as 15.5 mm.
const TILE_CAPTION_HEIGHT: u32 = 40;
const TILE_HEIGHT: u32 = TILE_ART_HEIGHT + TILE_CAPTION_HEIGHT;
const TILE_COL_GAP: i32 = 30;
const TILE_ROW_GAP: i32 = 12;
const TILE_RADIUS: u32 = 18;
const CHOICES_COLS: usize = 2;

/// Tiles on screen at once. The pin list may be longer; this is the viewport.
pub(crate) const CHOICES_VISIBLE: usize = 4;

/// Placeholder tints, used until pins carry artwork of their own.
///
/// Linn's `Ds/Pins` service gives this app an id and a title and nothing else,
/// so there is no cover art to draw yet. A flat tinted card per slot at least
/// gives each choice a stable colour as well as a stable position, which is
/// what makes it findable without reading. The colours are the Dot's own.
const TILE_TINTS: [(Rgb565, Rgb565); CHOICES_VISIBLE] = [
    (dot::BLUE_DEEP, dot::BLUE),
    (dot::RED_DEEP, dot::RED),
    (dot::SLATE_DEEP, dot::SLATE),
    (dot::SLATE_DIM, dot::SHELL),
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HifiPage {
    #[default]
    NowPlaying,
    Choices,
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Layout {
    /// The whole panel. A page change clears this rather than the outgoing
    /// page's body, so the two pages are free to occupy different areas
    /// without leaving each other's pixels behind.
    pub(super) panel: Rectangle,
    pub(super) now_playing: NowPlayingLayout,
    pub(super) choices: ChoicesLayout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NowPlayingLayout {
    pub(super) artwork: Rectangle,
    pub(super) title_band: Rectangle,
    pub(super) title_origin: Point,
    pub(super) artist_band: Rectangle,
    pub(super) artist_origin: Point,
    pub(super) progress: Rectangle,
    pub(super) overlay_panel: Rectangle,
    pub(super) overlay_value: Rectangle,
    pub(super) overlay_track: Rectangle,
    pub(super) pause_badge: Rectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ChoicesLayout {
    pub(super) header_band: Rectangle,
    pub(super) header_origin: Point,
    /// Full tile bounds — art plus caption. These are the focus targets and
    /// the touch targets.
    pub(super) tiles: [Rectangle; CHOICES_VISIBLE],
    pub(super) tile_art: [Rectangle; CHOICES_VISIBLE],
    pub(super) tile_caption: [Rectangle; CHOICES_VISIBLE],
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct State {
    page: HifiPage,
    status: HifiStatus,
    artwork: Option<HifiArtwork>,
    pins: HifiPins,
    created_at_ms: u64,
    loading: bool,
    last_second: u64,
    current_ms: u64,
    current_second: u64,
    marquee: MarqueeState,
    /// Set when the pin set changes, cleared once Choices repaints.
    pins_dirty: bool,
    /// Uptime at which the volume readout stops being shown. `None` means it
    /// is not up. Cleared by `on_tick` when it lapses, so the frame that hides
    /// it gets requested rather than waiting for unrelated activity.
    volume_overlay_until_ms: Option<u64>,
    /// Playback state we set on a press, held until the device agrees.
    optimistic_playback: Option<Optimistic<PlaybackState>>,
    /// Volume we set on a press, held until the device agrees.
    ///
    /// A held turn of the dial is why several presses in a row no longer jump
    /// about: the device reports each intermediate level it passes through,
    /// and without this the bar would chase them backwards.
    optimistic_volume: Option<Optimistic<u8>>,
    /// Uptime after which the cover on screen stops standing in for the one
    /// still being fetched. `None` means the cover belongs to this track.
    stale_artwork_until_ms: Option<u64>,
    /// Cover per pin tile, filled in as each is fetched.
    pin_artwork: [Option<HifiArtwork>; CHOICES_VISIBLE],
    /// What a pin is showing while the streamer works out what to play.
    ///
    /// `None` once the device has named a track, or once the window lapses.
    pin_intro: Option<PinIntro>,
    /// Uptime until which a position ahead of our own is taken to describe the
    /// place we just left rather than where we are.
    ///
    /// Unlike playback or volume, a position cannot be held until the device
    /// reports the same value -- it changes every second, so there is no
    /// equality to wait for. What can be said is that immediately after a
    /// restart the position only goes down, so anything far ahead is stale.
    progress_guard_until_ms: Option<u64>,
}

/// A pin standing in for the track it is about to start.
///
/// A pin cannot predict a track -- it starts an album or playlist whose first
/// song nobody has told us -- but it can say what was pressed, which is true
/// however the device resolves it. `previous_title` is what tells a status that
/// has not caught up yet from one that has.
#[derive(Clone, Debug, Eq, PartialEq)]
struct PinIntro {
    expires_at_ms: u64,
    previous_title: String<HIFI_TEXT_LEN>,
}

/// What a screen did with a raw pad press.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ButtonOutcome {
    pub(crate) redraw: bool,
    pub(crate) command: Option<Command>,
    /// The page changed, so any focus index the app is holding now points at
    /// an unrelated control and must be dropped.
    pub(crate) page_changed: bool,
}

/// Everything this screen knows about what is currently on one render target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct RenderCache {
    last_rendered_page: Option<HifiPage>,
    now_playing: NowPlayingCache,
    choices: ChoicesCache,
    art_slot: Slot,
}

impl RenderCache {
    pub(crate) fn new(layout: &Layout) -> Self {
        Self {
            last_rendered_page: None,
            now_playing: NowPlayingCache::default(),
            choices: ChoicesCache::default(),
            art_slot: Slot::new(layout.now_playing.artwork),
        }
    }

    pub(crate) fn reset(&mut self, layout: &Layout) {
        *self = Self::new(layout);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
struct NowPlayingCache {
    has_rendered: bool,
    spinner_phase: Option<u8>,
    progress_filled_px: Option<u32>,
    duration_seconds: Option<u32>,
    title: String<HIFI_TEXT_LEN>,
    artist: String<HIFI_TEXT_LEN>,
    artwork_uri: String<HIFI_URI_LEN>,
    loading_visible: bool,
    title_marquee_offset_px: Option<i32>,
    artist_marquee_offset_px: Option<i32>,
    /// Whether the volume readout is currently on the panel. Drives the
    /// repaint of the artwork underneath when it goes away.
    overlay_visible: bool,
    overlay_value: Option<u8>,
    overlay_percent: Option<u8>,
    /// Whether the paused badge is on the panel. Like the volume readout, it
    /// covers artwork, so taking it away means putting those pixels back.
    pause_badge_visible: bool,
}

/// Marquee bookkeeping for the two scrolling text bands.
///
/// Deliberately *not* part of [`RenderCache`]. The overflow widths are
/// measured from the font and describe the text, not the panel, and
/// [`State::on_tick`] reads them to decide whether a frame is needed at all —
/// which happens on the update path, where no render session is in scope.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct MarqueeState {
    title_overflow_px: u32,
    title_anim_base_ms: u64,
    artist_overflow_px: u32,
    artist_anim_base_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
struct ChoicesCache {
    header_drawn: bool,
    drawn_titles: [String<HIFI_TEXT_LEN>; CHOICES_VISIBLE],
    drawn_active: [bool; CHOICES_VISIBLE],
    /// Source URI of the cover already on each tile, so one landing repaints
    /// just that tile.
    drawn_art: [String<HIFI_URI_LEN>; CHOICES_VISIBLE],
    has_rendered: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    /// Restart this track, or step back to the previous one — see
    /// [`RESTART_THRESHOLD_SECONDS`].
    PreviousOrRestart,
    TogglePlayback,
    NextTrack,
    InvokePinSlot(usize),
    VolumeDelta(i16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    InvokePinId {
        id: u32,
    },
    PreviousTrack,
    /// Seek to the start of the current track without changing it.
    Restart,
    TogglePlayback,
    NextTrack,
    SetVolume {
        volume: u8,
    },
}

impl State {
    pub(crate) fn new(uptime_ms: u64) -> Self {
        let current_second = uptime_ms / 1000;

        Self {
            page: HifiPage::NowPlaying,
            status: HifiStatus::waiting(),
            artwork: None,
            pins: HifiPins::new(),
            created_at_ms: uptime_ms,
            loading: true,
            last_second: current_second,
            current_ms: uptime_ms,
            current_second,
            marquee: MarqueeState::default(),
            pins_dirty: false,
            volume_overlay_until_ms: None,
            optimistic_playback: None,
            optimistic_volume: None,
            stale_artwork_until_ms: None,
            pin_artwork: [const { None }; CHOICES_VISIBLE],
            pin_intro: None,
            progress_guard_until_ms: None,
        }
    }

    #[cfg(test)]
    pub(crate) const fn page(&self) -> HifiPage {
        self.page
    }

    /// Raw pad handling, ahead of the app's focus-ring path.
    ///
    /// Returns `None` when the press is not this screen's to consume, which
    /// the app reads as "fall through to geometric focus movement". Now
    /// Playing consumes everything; Choices consumes only `Back`, leaving the
    /// directions to move the ring and `Select` to be replayed as a tap on the
    /// focused tile.
    pub(crate) fn intercept_button(
        &mut self,
        button: Button,
        uptime_ms: u64,
    ) -> Option<ButtonOutcome> {
        match self.page {
            HifiPage::NowPlaying => {
                let action = match button {
                    Button::Up => Action::VolumeDelta(VOLUME_STEP),
                    Button::Down => Action::VolumeDelta(-VOLUME_STEP),
                    Button::Left => Action::PreviousOrRestart,
                    Button::Right => Action::NextTrack,
                    Button::Select => Action::TogglePlayback,
                    Button::Back => {
                        self.page = HifiPage::Choices;
                        return Some(ButtonOutcome {
                            redraw: true,
                            command: None,
                            page_changed: true,
                        });
                    }
                };
                let before = self.page;
                let command = self.handle(action, uptime_ms);
                Some(ButtonOutcome {
                    redraw: true,
                    command,
                    page_changed: self.page != before,
                })
            }
            HifiPage::Choices => match button {
                Button::Back => {
                    self.page = HifiPage::NowPlaying;
                    Some(ButtonOutcome {
                        redraw: true,
                        command: None,
                        page_changed: true,
                    })
                }
                _ => None,
            },
        }
    }

    /// Focusable controls for the page currently shown, in reading order.
    ///
    /// Now Playing has none by design: it binds the pad to actions rather than
    /// to movement, so a ring there would have nothing to move between.
    pub(crate) fn focus_targets(&self, layout: &Layout) -> FocusTargets {
        let mut targets = FocusTargets::new();
        if matches!(self.page, HifiPage::Choices) {
            for tile in layout.choices.tiles {
                let _ = targets.push(tile);
            }
        }
        targets
    }

    pub(crate) fn on_tick(&mut self, uptime_ms: u64) -> bool {
        // Lapse the volume readout first, and unconditionally: the playback
        // tick below has several early returns and the readout has to come
        // down regardless of which one wins.
        let overlay_lapsed = match self.volume_overlay_until_ms {
            Some(until) if uptime_ms >= until => {
                self.volume_overlay_until_ms = None;
                true
            }
            _ => false,
        };

        // A cover cannot stand in for its replacement indefinitely. When the
        // window closes without one arriving, drop it here rather than in the
        // renderer, so the frame that removes it gets requested.
        let stale_lapsed = match self.stale_artwork_until_ms {
            Some(until) if uptime_ms >= until => {
                self.stale_artwork_until_ms = None;
                if self
                    .artwork
                    .as_ref()
                    .is_some_and(|artwork| artwork.source_uri != self.status.album_art_uri)
                {
                    self.artwork = None;
                }
                true
            }
            _ => false,
        };

        if self
            .progress_guard_until_ms
            .is_some_and(|until| uptime_ms >= until)
        {
            self.progress_guard_until_ms = None;
        }

        if self
            .pin_intro
            .as_ref()
            .is_some_and(|intro| uptime_ms >= intro.expires_at_ms)
        {
            self.pin_intro = None;
        }

        let playback_changed = self.tick_playback(uptime_ms);
        playback_changed || overlay_lapsed || stale_lapsed
    }

    fn tick_playback(&mut self, uptime_ms: u64) -> bool {
        self.current_ms = uptime_ms;
        if self.loading {
            let timed_out = uptime_ms.saturating_sub(self.created_at_ms) >= LOADING_TIMEOUT_MS;
            if timed_out {
                self.loading = false;
                return true;
            }
            self.current_second = uptime_ms / 1000;
            return true;
        }

        let marquee_active = matches!(self.page, HifiPage::NowPlaying)
            && (self.marquee.title_overflow_px > 0 || self.marquee.artist_overflow_px > 0);

        if self.status.playback == PlaybackState::Buffering {
            return true;
        }

        if self.status.playback != PlaybackState::Playing || self.status.duration_seconds == 0 {
            return marquee_active;
        }

        let current_second = uptime_ms / 1000;
        self.current_second = current_second;

        let elapsed = current_second.saturating_sub(self.last_second);
        if elapsed == 0 {
            return marquee_active;
        }

        self.status.elapsed_seconds = self
            .status
            .elapsed_seconds
            .saturating_add(elapsed as u32)
            .min(self.status.duration_seconds);
        self.last_second = current_second;
        if self.status.elapsed_seconds >= self.status.duration_seconds {
            self.status.playback = PlaybackState::Stopped;
        }
        true
    }

    /// Starts distrusting positions that run ahead of ours.
    ///
    /// The session that answers status polls is not the one that sends
    /// commands, so it does not know a restart happened and keeps reporting
    /// the old position until the device's next time event -- about a second.
    /// Whether a poll lands inside that window is a race, which is why the
    /// symptom comes and goes.
    fn guard_progress(&mut self, uptime_ms: u64) {
        self.progress_guard_until_ms = Some(uptime_ms.saturating_add(OPTIMISTIC_TTL_MS));
    }

    /// Decides whether an incoming position describes now or a moment ago.
    fn accept_progress(&mut self, incoming: u32, uptime_ms: u64) -> u32 {
        let Some(until) = self.progress_guard_until_ms else {
            return incoming;
        };
        if uptime_ms >= until {
            self.progress_guard_until_ms = None;
            return incoming;
        }
        if incoming
            > self
                .status
                .elapsed_seconds
                .saturating_add(PROGRESS_GUARD_TOLERANCE_SECONDS)
        {
            // Still describing where we were.
            return self.status.elapsed_seconds;
        }
        // The device has come back to somewhere plausible, so it has caught up
        // and there is nothing left to guard against.
        self.progress_guard_until_ms = None;
        incoming
    }

    pub(crate) fn apply_status(&mut self, mut status: HifiStatus, uptime_ms: u64) -> bool {
        let was_loading = self.loading;

        // A pin is on screen and the device is still describing what it
        // replaced. Keep the pin's own words; everything else -- volume,
        // transport -- is about the player and still applies.
        if self.pin_intro_holds(&status, uptime_ms) {
            status.title = self.status.title.clone();
            status.artist = self.status.artist.clone();
            status.album = self.status.album.clone();
            status.album_art_uri = self.status.album_art_uri.clone();
            status.duration_seconds = self.status.duration_seconds;
        }

        status.elapsed_seconds = self.accept_progress(status.elapsed_seconds, uptime_ms);

        // What the user just asked for outranks a reply composed before they
        // asked, until the device agrees or the hold runs out.
        status.playback =
            Optimistic::reconcile(&mut self.optimistic_playback, status.playback, uptime_ms);
        status.volume_percent = Optimistic::reconcile(
            &mut self.optimistic_volume,
            status.volume_percent,
            uptime_ms,
        );

        let has_live_content = has_live_content(&status);

        // The cover for the new track has not been fetched yet, and an empty
        // slot is a bigger visual event than a cover that is briefly one track
        // behind. Keep showing it, and give it a deadline so a track with no
        // artwork at all cannot inherit the previous one's.
        if self
            .artwork
            .as_ref()
            .is_some_and(|artwork| artwork.source_uri != status.album_art_uri)
        {
            self.stale_artwork_until_ms = Some(uptime_ms.saturating_add(STALE_ARTWORK_GRACE_MS));
        }

        if self.status == status && (!was_loading || !has_live_content) {
            return false;
        }

        self.status = status;
        if has_live_content {
            self.loading = false;
        }
        self.current_ms = uptime_ms;
        let current_second = uptime_ms / 1000;
        self.current_second = current_second;
        self.last_second = current_second;
        true
    }

    pub(crate) fn apply_artwork(&mut self, artwork: HifiArtwork) -> bool {
        if artwork.source_uri.is_empty()
            || artwork.source_uri != self.status.album_art_uri
            || !artwork.is_complete()
        {
            return false;
        }

        // The replacement arrived, so the cover on screen is current again.
        self.stale_artwork_until_ms = None;

        // Compare by URI rather than by pixels. The old full-buffer equality
        // check was 9,216 comparisons; at this artwork size it would be
        // 108,900 of them per artwork event, on a core with no FPU and better
        // things to do.
        if self
            .artwork
            .as_ref()
            .is_some_and(|current| current.source_uri == artwork.source_uri)
        {
            return false;
        }

        self.artwork = Some(artwork);
        true
    }

    pub(crate) fn apply_pins(&mut self, pins: HifiPins) -> bool {
        if self.pins == pins {
            return false;
        }
        // A pin whose id changed is a different thing to play, so its cover is
        // no longer its cover.
        for slot in 0..CHOICES_VISIBLE {
            let same = self
                .pins
                .get(slot)
                .zip(pins.get(slot))
                .is_some_and(|(before, after)| before.id == after.id);
            if !same {
                self.pin_artwork[slot] = None;
            }
        }
        self.pins = pins;
        self.pins_dirty = true;
        true
    }

    pub(crate) fn apply_pin_artwork(&mut self, slot: usize, artwork: HifiArtwork) -> bool {
        if slot >= CHOICES_VISIBLE || !artwork.is_complete() {
            return false;
        }
        // The pin has to still want this cover: the list can change while an
        // image is in flight.
        if self
            .pins
            .get(slot)
            .is_none_or(|pin| pin.artwork_uri != artwork.source_uri)
        {
            return false;
        }
        if self.pin_artwork[slot]
            .as_ref()
            .is_some_and(|current| current.source_uri == artwork.source_uri)
        {
            return false;
        }
        self.pin_artwork[slot] = Some(artwork);
        self.pins_dirty = true;
        true
    }

    pub(crate) fn handle_touch(
        &mut self,
        layout: &Layout,
        point: Point,
        uptime_ms: u64,
    ) -> Option<Command> {
        let action = match self.page {
            // Now Playing is deliberately tap-inert. The remote can be picked
            // up, carried and held against a chest without changing what is
            // playing — and the touch controller has nothing to report on the
            // screen the device spends nearly all its time on.
            HifiPage::NowPlaying => None,
            HifiPage::Choices => hit_test_choices(&layout.choices, point),
        }?;
        self.handle(action, uptime_ms)
    }

    fn handle(&mut self, action: Action, uptime_ms: u64) -> Option<Command> {
        match action {
            Action::PreviousOrRestart => {
                if self.status.elapsed_seconds >= RESTART_THRESHOLD_SECONDS {
                    // Same track, rewound: keep the metadata and the artwork,
                    // move only the clock.
                    self.status.elapsed_seconds = 0;
                    let current_second = uptime_ms / 1000;
                    self.current_second = current_second;
                    self.last_second = current_second;
                    self.guard_progress(uptime_ms);
                    Some(Command::Restart)
                } else {
                    self.begin_track_change(uptime_ms);
                    Some(Command::PreviousTrack)
                }
            }
            Action::TogglePlayback => {
                if playback_can_pause(self.status.playback) {
                    self.tick_playback(uptime_ms);
                    self.status.playback = PlaybackState::Paused;
                } else {
                    let current_second = uptime_ms / 1000;
                    self.current_second = current_second;
                    self.last_second = current_second;
                    self.status.playback = PlaybackState::Playing;
                }
                // Defend the press. Without this the device's next report --
                // composed before the command landed -- puts the old state
                // back, and the icon appears, vanishes, and returns.
                self.optimistic_playback = Some(Optimistic::hold(self.status.playback, uptime_ms));
                Some(Command::TogglePlayback)
            }
            Action::NextTrack => {
                self.begin_track_change(uptime_ms);
                Some(Command::NextTrack)
            }
            Action::InvokePinSlot(slot) => {
                let pin = self.pins.get(slot)?;
                let id = pin.id;
                let title = pin.title.clone();
                let artwork_uri = pin.artwork_uri.clone();

                // Playing something is the point of this screen, so go
                // straight back to it rather than leaving the user on a grid
                // wondering whether the press landed.
                self.page = HifiPage::NowPlaying;
                self.show_pin_while_it_starts(title, artwork_uri, slot, uptime_ms);
                Some(Command::InvokePinId { id })
            }
            Action::VolumeDelta(delta) => {
                // Raise the readout even when the level is already at the
                // rail: the press was real, and showing nothing would read as
                // a dead button.
                self.volume_overlay_until_ms = Some(uptime_ms.saturating_add(VOLUME_OVERLAY_MS));
                let current = self.status.volume_percent as i16;
                let next = (current + delta).clamp(0, HIFI_VOLUME_MAX as i16) as u8;
                if next == self.status.volume_percent {
                    return None;
                }
                self.status.volume_percent = next;
                self.optimistic_volume = Some(Optimistic::hold(next, uptime_ms));
                Some(Command::SetVolume { volume: next })
            }
        }
    }

    /// Puts the pin on screen while the streamer works out what to play.
    ///
    /// Returning to Now Playing after tapping a pin used to show the previous
    /// track -- the one thing certainly not about to play. The pin's own name
    /// and cover are a smaller claim and a true one: it is what was pressed,
    /// whatever the device picks out of it.
    ///
    /// Artist and album are cleared rather than kept. Leaving the old artist
    /// under a new title would read as a track that does not exist.
    fn show_pin_while_it_starts(
        &mut self,
        title: String<{ crate::HIFI_PIN_TITLE_LEN }>,
        artwork_uri: String<{ crate::HIFI_URI_LEN }>,
        slot: usize,
        uptime_ms: u64,
    ) {
        let previous_title = self.status.title.clone();

        self.status.title.clear();
        let _ = self.status.title.push_str(title.as_str());
        self.status.artist.clear();
        self.status.album.clear();
        self.status.elapsed_seconds = 0;
        self.status.duration_seconds = 0;

        // The grid the user just came from has already fetched this cover, so
        // it costs a clone rather than a round trip.
        self.status.album_art_uri.clear();
        let _ = self.status.album_art_uri.push_str(artwork_uri.as_str());
        self.artwork = self
            .pin_artwork
            .get(slot)
            .and_then(|artwork| artwork.clone())
            .filter(|artwork| artwork.source_uri == self.status.album_art_uri);
        self.stale_artwork_until_ms = None;

        // A pin is a request to play, so say so rather than leaving a pause
        // badge over something that is starting.
        self.status.playback = PlaybackState::Playing;
        self.optimistic_playback = Some(Optimistic::hold(PlaybackState::Playing, uptime_ms));

        self.pin_intro = Some(PinIntro {
            expires_at_ms: uptime_ms.saturating_add(OPTIMISTIC_TTL_MS),
            previous_title,
        });
        self.guard_progress(uptime_ms);
    }

    /// Whether a status still describes the track the pin replaced.
    fn pin_intro_holds(&mut self, incoming: &HifiStatus, uptime_ms: u64) -> bool {
        let Some(intro) = self.pin_intro.as_ref() else {
            return false;
        };
        if uptime_ms >= intro.expires_at_ms {
            self.pin_intro = None;
            return false;
        }
        // The device naming anything other than what we left means it has
        // moved on, and it knows better than we do what it started.
        if incoming.title != intro.previous_title {
            self.pin_intro = None;
            return false;
        }
        true
    }

    /// Marks the current track as on its way out without emptying the screen.
    ///
    /// This used to blank the title, drop the cover and force `Buffering`,
    /// which manufactured three visible states out of one press: an empty
    /// slot, then a spinner, then a play icon once a status arrived without
    /// artwork — before the real cover finally landed. None of that came from
    /// the device; the runtime knows what comes next and says so within a
    /// frame.
    ///
    /// Only the position is reset, because that is the one field certain to be
    /// wrong the moment a skip is requested.
    fn begin_track_change(&mut self, uptime_ms: u64) {
        self.status.elapsed_seconds = 0;
        self.guard_progress(uptime_ms);
        if self.artwork.is_some() {
            self.stale_artwork_until_ms = Some(uptime_ms.saturating_add(STALE_ARTWORK_GRACE_MS));
        }
    }
}

// ---------------------------------------------------------------------------
// Layout construction
// ---------------------------------------------------------------------------

pub(crate) fn layout(bounds: Rectangle) -> Layout {
    let center_x = bounds.top_left.x + (bounds.size.width / 2) as i32;
    let top = bounds.top_left.y;
    let left = bounds.top_left.x;

    Layout {
        panel: bounds,
        now_playing: now_playing_layout(left, top, center_x),
        choices: choices_layout(left, top, center_x),
    }
}

fn now_playing_layout(left: i32, top: i32, center_x: i32) -> NowPlayingLayout {
    let artwork = Rectangle::new(
        Point::new(center_x - (ARTWORK_SIZE / 2) as i32, top + ARTWORK_TOP),
        Size::new(ARTWORK_SIZE, ARTWORK_SIZE),
    );
    let title_band = Rectangle::new(
        Point::new(left + TEXT_INSET, top + TITLE_TOP),
        Size::new(TEXT_WIDTH, TITLE_HEIGHT),
    );
    let artist_band = Rectangle::new(
        Point::new(left + TEXT_INSET, top + ARTIST_TOP),
        Size::new(TEXT_WIDTH, ARTIST_HEIGHT),
    );
    let progress = Rectangle::new(
        Point::new(center_x - (PROGRESS_WIDTH / 2) as i32, top + PROGRESS_TOP),
        Size::new(PROGRESS_WIDTH, PROGRESS_HEIGHT),
    );

    NowPlayingLayout {
        artwork,
        title_band,
        title_origin: Point::new(center_x, title_band.top_left.y),
        artist_band,
        artist_origin: Point::new(center_x, artist_band.top_left.y),
        progress,
        overlay_panel: Rectangle::new(
            Point::new(left + OVERLAY_LEFT, top + OVERLAY_TOP),
            Size::new(OVERLAY_WIDTH, OVERLAY_HEIGHT),
        ),
        overlay_value: Rectangle::new(
            Point::new(left + OVERLAY_LEFT, top + OVERLAY_VALUE_TOP),
            Size::new(OVERLAY_WIDTH, OVERLAY_VALUE_HEIGHT),
        ),
        overlay_track: Rectangle::new(
            Point::new(left + OVERLAY_TRACK_LEFT, top + OVERLAY_TRACK_TOP),
            Size::new(OVERLAY_TRACK_WIDTH, OVERLAY_TRACK_HEIGHT),
        ),
        pause_badge: Rectangle::new(
            Point::new(
                center_x - (PAUSE_BADGE_SIZE / 2) as i32,
                artwork.top_left.y + (ARTWORK_SIZE / 2) as i32 - (PAUSE_BADGE_SIZE / 2) as i32,
            ),
            Size::new(PAUSE_BADGE_SIZE, PAUSE_BADGE_SIZE),
        ),
    }
}

fn choices_layout(left: i32, top: i32, center_x: i32) -> ChoicesLayout {
    let header_band = Rectangle::new(
        Point::new(left + TEXT_INSET, top + CHOICES_HEADER_TOP),
        Size::new(TEXT_WIDTH, CHOICES_HEADER_HEIGHT),
    );

    let mut tiles = [Rectangle::new(Point::zero(), Size::zero()); CHOICES_VISIBLE];
    let mut tile_art = tiles;
    let mut tile_caption = tiles;
    for slot in 0..CHOICES_VISIBLE {
        let col = (slot % CHOICES_COLS) as i32;
        let row = (slot / CHOICES_COLS) as i32;
        let x = left + CHOICES_GRID_LEFT + col * (TILE_WIDTH as i32 + TILE_COL_GAP);
        let y = top + CHOICES_GRID_TOP + row * (TILE_HEIGHT as i32 + TILE_ROW_GAP);
        tiles[slot] = Rectangle::new(Point::new(x, y), Size::new(TILE_WIDTH, TILE_HEIGHT));
        tile_art[slot] = Rectangle::new(Point::new(x, y), Size::new(TILE_WIDTH, TILE_ART_HEIGHT));
        tile_caption[slot] = Rectangle::new(
            Point::new(x, y + TILE_ART_HEIGHT as i32),
            Size::new(TILE_WIDTH, TILE_CAPTION_HEIGHT),
        );
    }

    ChoicesLayout {
        header_band,
        header_origin: Point::new(center_x, header_band.top_left.y),
        tiles,
        tile_art,
        tile_caption,
    }
}

fn hit_test_choices(layout: &ChoicesLayout, point: Point) -> Option<Action> {
    layout
        .tiles
        .iter()
        .position(|rect| rect.contains(point))
        .map(Action::InvokePinSlot)
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

pub(crate) fn render<D>(
    state: &mut State,
    cache: &mut RenderCache,
    display: &mut D,
    scratch: &mut [Rgb565],
    ui_layout: &Layout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    cache.art_slot.bounds = ui_layout.now_playing.artwork;

    if state.pins_dirty {
        cache.choices = ChoicesCache::default();
        state.pins_dirty = false;
    }

    if cache.last_rendered_page != Some(state.page) {
        // Clear the whole panel rather than the outgoing page's body: the two
        // pages use different insets, so a body-sized clear would leave the
        // other page's edges behind.
        clear_rect(display, ui_layout.panel)?;
        cache.now_playing = NowPlayingCache::default();
        cache.choices = ChoicesCache::default();
        cache.art_slot.previous_kind = None;
        cache.last_rendered_page = Some(state.page);
    }

    match state.page {
        HifiPage::NowPlaying => {
            render_now_playing(state, cache, display, scratch, &ui_layout.now_playing)
        }
        HifiPage::Choices => render_choices(state, cache, display, scratch, &ui_layout.choices),
    }
}

fn render_now_playing<D>(
    state: &mut State,
    cache: &mut RenderCache,
    display: &mut D,
    scratch: &mut [Rgb565],
    ui_layout: &NowPlayingLayout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let mut painter = Painter::new(display, scratch);

    let art_kind = compute_art_kind(state);
    cache
        .art_slot
        .clear_if_kind_changed(painter.display(), art_kind)
        .map_err(RenderError::Draw)?;

    // The readout goes away on a tick, not on a press, so its disappearance
    // has to be noticed here. Handled *before* the artwork so that the artwork
    // pass repaints what the panel covered rather than leaving a hole.
    let overlay_visible = state.volume_overlay_until_ms.is_some();
    let overlay_just_hidden = !overlay_visible && cache.now_playing.overlay_visible;
    if overlay_just_hidden {
        cache.now_playing.overlay_visible = false;
        cache.now_playing.overlay_value = None;
        cache.now_playing.overlay_percent = None;
    }

    if state.loading {
        if overlay_just_hidden {
            clear_rect(painter.display(), ui_layout.overlay_panel)?;
        }
        let spinner = Spinner {
            center: rect_visual_center(ui_layout.artwork),
            phase: spinner_phase(state.current_ms),
            previous_phase: if cache.art_slot.previous_kind == Some(ART_SLOT_SPINNER)
                && !overlay_just_hidden
            {
                cache.now_playing.spinner_phase
            } else {
                None
            },
        };
        painter.draw(&spinner).map_err(RenderError::Draw)?;
        cache.now_playing.spinner_phase = Some(spinner.phase);
        cache.art_slot.previous_kind = Some(art_kind);
        cache.now_playing.loading_visible = true;
        cache.now_playing.has_rendered = true;
        return Ok(());
    }

    if cache.now_playing.loading_visible {
        // Leaving the spinner-only phase: everything skipped while it was up
        // needs a fresh first frame.
        cache.now_playing.title.clear();
        cache.now_playing.artist.clear();
        cache.now_playing.title_marquee_offset_px = None;
        cache.now_playing.artist_marquee_offset_px = None;
        cache.now_playing.progress_filled_px = None;
        cache.now_playing.duration_seconds = None;
        cache.now_playing.loading_visible = false;
    }

    // Anything but artwork is a glyph on black, so the panel's footprint has
    // to be cleared before it is redrawn. Artwork restores itself from its own
    // buffer and needs no clear.
    if overlay_just_hidden && art_kind != ART_SLOT_ARTWORK {
        clear_rect(painter.display(), ui_layout.overlay_panel)?;
    }

    match art_kind {
        ART_SLOT_BUFFERING => {
            let spinner = Spinner {
                center: rect_visual_center(ui_layout.artwork),
                phase: spinner_phase(state.current_ms),
                previous_phase: if cache.art_slot.previous_kind == Some(ART_SLOT_BUFFERING)
                    && !overlay_just_hidden
                {
                    cache.now_playing.spinner_phase
                } else {
                    None
                },
            };
            painter.draw(&spinner).map_err(RenderError::Draw)?;
            cache.now_playing.spinner_phase = Some(spinner.phase);
        }
        ART_SLOT_ARTWORK => {
            let artwork = state.artwork.as_ref().expect("artwork present");
            let painted = cache.now_playing.artwork_uri.as_str() == artwork.source_uri.as_str()
                && cache.art_slot.previous_kind == Some(ART_SLOT_ARTWORK);
            // Already on the panel and only the readout's footprint is stale:
            // re-blit that rectangle instead of all 330 x 330 of it.
            let region = if painted && overlay_just_hidden {
                Some(ui_layout.overlay_panel)
            } else {
                None
            };
            let widget = ArtworkWidget {
                rect: ui_layout.artwork,
                artwork,
                region,
                skip: painted && !overlay_just_hidden,
            };
            painter.draw(&widget).map_err(RenderError::Draw)?;
            cache.now_playing.artwork_uri.clear();
            let _ = cache
                .now_playing
                .artwork_uri
                .push_str(artwork.source_uri.as_str());
        }
        ART_SLOT_PAUSE_BARS => {
            let widget = PauseBars {
                rect: ui_layout.artwork,
                already_drawn: cache.art_slot.previous_kind == Some(ART_SLOT_PAUSE_BARS)
                    && !overlay_just_hidden,
            };
            painter.draw(&widget).map_err(RenderError::Draw)?;
        }
        ART_SLOT_PLAY_ICON => {
            let widget = PlayTriangle {
                rect: ui_layout.artwork,
                already_drawn: cache.art_slot.previous_kind == Some(ART_SLOT_PLAY_ICON)
                    && !overlay_just_hidden,
            };
            painter.draw(&widget).map_err(RenderError::Draw)?;
        }
        _ => {}
    }
    cache.art_slot.previous_kind = Some(art_kind);

    // Paused, and there is a picture to mark. Without artwork the slot above
    // already shows pause bars as its state glyph, so badging that too would
    // draw the same thing twice.
    // Only when pause is what the streamer was *asked* for. `Stopped` shows up
    // in passing while a track loads -- between tracks, and for a moment after
    // a pin starts something new -- and badging that made the overlay blink on
    // and off through every transition, saying "paused" about a player that was
    // busy starting.
    let paused = matches!(state.status.playback, PlaybackState::Paused);
    let badge_wanted = paused && art_kind == ART_SLOT_ARTWORK;
    if badge_wanted {
        // Redrawn after the volume readout lapses, because that panel is
        // larger than the badge and covered it completely.
        if !cache.now_playing.pause_badge_visible || overlay_just_hidden {
            draw_panel(
                painter.display(),
                ui_layout.pause_badge,
                PAUSE_BADGE_RADIUS,
                OLED_BLACK,
                SURFACE_BORDER,
            )?;
            let bars = PauseBars {
                rect: ui_layout.pause_badge,
                already_drawn: false,
            };
            painter.draw(&bars).map_err(RenderError::Draw)?;
            cache.now_playing.pause_badge_visible = true;
        }
    } else if cache.now_playing.pause_badge_visible {
        // Playing again: put back the artwork the badge was sitting on. The
        // pixels are already in `State::artwork`, so this is a blit from a
        // buffer we hold rather than a refetch.
        cache.now_playing.pause_badge_visible = false;
        if art_kind == ART_SLOT_ARTWORK {
            let artwork = state.artwork.as_ref().expect("artwork present");
            let widget = ArtworkWidget {
                rect: ui_layout.artwork,
                artwork,
                region: Some(ui_layout.pause_badge),
                skip: false,
            };
            painter.draw(&widget).map_err(RenderError::Draw)?;
        } else {
            clear_rect(painter.display(), ui_layout.pause_badge)?;
        }
    }

    let duration_changed =
        cache.now_playing.duration_seconds != Some(state.status.duration_seconds);
    let new_filled_px = progress_filled_px(
        state.status.elapsed_seconds,
        state.status.duration_seconds,
        ui_layout.progress.size.width,
    );
    let progress = LevelBar {
        bar: ui_layout.progress,
        track_color: PROGRESS_TRACK,
        active_color: PROGRESS_FILL,
        filled_px: new_filled_px,
        previous_filled_px: if duration_changed {
            None
        } else {
            cache.now_playing.progress_filled_px
        },
        // Left square. It spans the panel edge to edge, where round ends would
        // read as a gap rather than a shape.
        radius: 0,
        backdrop: OLED_BLACK,
    };
    painter.draw(&progress).map_err(RenderError::Draw)?;
    cache.now_playing.progress_filled_px = Some(new_filled_px);
    cache.now_playing.duration_seconds = Some(state.status.duration_seconds);

    let has_rendered = cache.now_playing.has_rendered;
    let title_text = non_empty_or(&state.status.title, "No track");
    draw_marquee_band(
        &mut painter,
        MarqueeInput {
            band: ui_layout.title_band,
            origin: ui_layout.title_origin,
            text: title_text,
            primary: true,
        },
        &mut state.marquee.title_overflow_px,
        &mut state.marquee.title_anim_base_ms,
        &mut cache.now_playing.title,
        &mut cache.now_playing.title_marquee_offset_px,
        state.current_ms,
        has_rendered,
    )?;

    // "Not playing" explains an empty screen at rest. Printing it under a title
    // we have just said *is* playing contradicts the line above it -- which is
    // what a pin does while the streamer works out what it started, and what a
    // track with no credited artist does always. Silence is the honest answer
    // there; the hint is only for when nothing is going on.
    let artist_text = if matches!(
        state.status.playback,
        PlaybackState::Playing | PlaybackState::Buffering
    ) {
        state.status.artist.as_str()
    } else {
        non_empty_or(&state.status.artist, "Not playing")
    };
    draw_marquee_band(
        &mut painter,
        MarqueeInput {
            band: ui_layout.artist_band,
            origin: ui_layout.artist_origin,
            text: artist_text,
            primary: false,
        },
        &mut state.marquee.artist_overflow_px,
        &mut state.marquee.artist_anim_base_ms,
        &mut cache.now_playing.artist,
        &mut cache.now_playing.artist_marquee_offset_px,
        state.current_ms,
        has_rendered,
    )?;

    // Last, so it sits over the artwork rather than under it.
    if overlay_visible {
        if !cache.now_playing.overlay_visible {
            // The overlay sits on the artwork, so its rounded corners have to
            // be blended against the picture. Told "black" they would be
            // painted black, which squares off the very corners they round.
            let artwork = state.artwork.as_ref();
            let art_rect = ui_layout.artwork;
            draw_panel_over(
                painter.display(),
                ui_layout.overlay_panel,
                OVERLAY_RADIUS,
                OLED_BLACK,
                SURFACE_BORDER,
                |point| artwork_pixel_at(artwork, art_rect, point),
            )?;
            cache.now_playing.overlay_value = None;
            cache.now_playing.overlay_percent = None;
        }

        let value = state.status.volume_percent.min(HIFI_VOLUME_MAX);
        let readout = VolumeValue {
            band: ui_layout.overlay_value,
            value,
            unchanged: cache.now_playing.overlay_value == Some(value),
        };
        painter.draw(&readout).map_err(RenderError::Draw)?;
        cache.now_playing.overlay_value = Some(value);

        let width = ui_layout.overlay_track.size.width;
        let track = LevelBar {
            bar: ui_layout.overlay_track,
            track_color: VOLUME_TRACK,
            active_color: VOLUME_ACTIVE,
            filled_px: filled_width_for_volume(value, width),
            previous_filled_px: cache
                .now_playing
                .overlay_percent
                .map(|percent| filled_width_for_volume(percent, width)),
            // A pill: half the height, so both ends are fully round. Square
            // ends read as unfinished inside a panel with a 22 px radius.
            radius: OVERLAY_TRACK_HEIGHT / 2,
            backdrop: OLED_BLACK,
        };
        painter.draw(&track).map_err(RenderError::Draw)?;
        cache.now_playing.overlay_percent = Some(value);
        cache.now_playing.overlay_visible = true;
    }

    cache.now_playing.has_rendered = true;
    Ok(())
}

fn render_choices<D>(
    state: &mut State,
    cache: &mut RenderCache,
    display: &mut D,
    scratch: &mut [Rgb565],
    ui_layout: &ChoicesLayout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let mut painter = Painter::new(display, scratch);

    if !cache.choices.header_drawn {
        let header = CenteredBand {
            band: ui_layout.header_band,
            origin: ui_layout.header_origin,
            text: "PICK SOME MUSIC",
        };
        painter.draw(&header).map_err(RenderError::Draw)?;
        cache.choices.header_drawn = true;
    }

    for (slot, tint) in TILE_TINTS.iter().enumerate() {
        let pin = state.pins.get(slot);
        let active = pin.is_some();
        let label = tile_label(slot, pin);

        let label_changed = cache.choices.drawn_titles[slot].as_str() != label.as_str();
        let active_changed = cache.choices.drawn_active[slot] != active;
        let art_changed = cache.choices.drawn_art[slot]
            != state.pin_artwork[slot]
                .as_ref()
                .map(|artwork| artwork.source_uri.clone())
                .unwrap_or_default();
        if cache.choices.has_rendered && !label_changed && !active_changed && !art_changed {
            continue;
        }

        // A cover if the device gave us one, the tinted card otherwise. The
        // tint is still what makes an artless pin findable without reading,
        // so it stays rather than becoming an empty rectangle.
        match state.pin_artwork[slot].as_ref() {
            Some(artwork) if active => {
                draw_scaled_artwork(painter.display(), ui_layout.tile_art[slot], artwork)?;
            }
            _ => {
                let (fill, stroke) = if active {
                    *tint
                } else {
                    (ACTION_INACTIVE, ACTION_INACTIVE_BORDER)
                };
                draw_panel(
                    painter.display(),
                    ui_layout.tile_art[slot],
                    TILE_RADIUS,
                    fill,
                    stroke,
                )?;
            }
        }

        let caption_band = ui_layout.tile_caption[slot];
        let caption = CenteredBand {
            band: caption_band,
            origin: Point::new(caption_band.center().x, caption_band.top_left.y),
            text: label.as_str(),
        };
        painter.draw(&caption).map_err(RenderError::Draw)?;

        cache.choices.drawn_titles[slot].clear();
        let _ = cache.choices.drawn_titles[slot].push_str(label.as_str());
        cache.choices.drawn_active[slot] = active;
        cache.choices.drawn_art[slot] = state.pin_artwork[slot]
            .as_ref()
            .map(|artwork| artwork.source_uri.clone())
            .unwrap_or_default();
    }
    cache.choices.has_rendered = true;
    Ok(())
}

/// Blits a cover into a rectangle smaller than itself.
///
/// Pin covers arrive at the same 330 px as the now-playing hero, because they
/// come down the same fetch-and-decode path; reusing it costs a few hundred KB
/// of PSRAM and saves a second image pipeline. Sampling is nearest-neighbour,
/// which is what the decoder already does going from source to slot, and tiles
/// repaint rarely enough that per-pixel sampling is not worth avoiding.
fn draw_scaled_artwork<D>(
    display: &mut D,
    target: Rectangle,
    artwork: &HifiArtwork,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let pixels = artwork.pixels();
    let source_size = crate::HIFI_ARTWORK_SIZE as usize;
    if pixels.len() < source_size * source_size {
        return Ok(());
    }

    let width = target.size.width as usize;
    let height = target.size.height as usize;
    if width == 0 || height == 0 {
        return Ok(());
    }

    display
        .fill_contiguous(
            &target,
            (0..height).flat_map(|y| {
                let source_y = y * source_size / height;
                (0..width).map(move |x| {
                    let source_x = x * source_size / width;
                    pixels[source_y * source_size + source_x]
                })
            }),
        )
        .map_err(RenderError::Draw)
}

/// The artwork pixel showing at a point, or the OLED background where there is
/// none.
///
/// The overlay is drawn over the picture, so anything of it that rounds away at
/// the corners has to blend into what is actually there.
fn artwork_pixel_at(artwork: Option<&HifiArtwork>, art_rect: Rectangle, point: Point) -> Rgb565 {
    let Some(artwork) = artwork else {
        return OLED_BLACK;
    };
    let x = point.x - art_rect.top_left.x;
    let y = point.y - art_rect.top_left.y;
    let size = HIFI_ARTWORK_SIZE as i32;
    if x < 0 || y < 0 || x >= size || y >= size {
        return OLED_BLACK;
    }
    let pixels = artwork.pixels();
    let index = (y * size + x) as usize;
    pixels.get(index).copied().unwrap_or(OLED_BLACK)
}

/// What to write under a tile.
///
/// Three cases, and conflating them hides a real fault. A pin that exists but
/// has no title is *not* an empty slot: the device answered `GetIdArray` and
/// then failed or declined to answer `ReadList` for that id. Labelling that
/// "Empty" makes a broken title fetch look like an unconfigured pin, so an
/// untitled pin shows its id instead — which also says, on the panel itself,
/// exactly which pin the device would not describe.
fn tile_label(slot: usize, pin: Option<&crate::HifiPin>) -> String<HIFI_TEXT_LEN> {
    let mut label = String::<HIFI_TEXT_LEN>::new();
    let Some(pin) = pin else {
        let _ = label.push_str("Empty ");
        let digit = (slot as u8 + 1).min(9);
        let _ = label.push((b'0' + digit) as char);
        return label;
    };

    if !pin.title.is_empty() && label.push_str(pin.title.as_str()).is_ok() {
        return label;
    }

    label.clear();
    if core::fmt::write(&mut label, format_args!("Pin {}", pin.id)).is_err() {
        label.clear();
        let _ = label.push_str("Pin");
    }
    label
}

struct MarqueeInput<'a> {
    band: Rectangle,
    origin: Point,
    text: &'a str,
    primary: bool,
}

/// Draws one scrolling text band and updates every piece of bookkeeping that
/// goes with it. Both bands do exactly the same thing, so they share this
/// rather than repeating thirty lines each.
#[allow(clippy::too_many_arguments)]
fn draw_marquee_band<D>(
    painter: &mut Painter<'_, D>,
    input: MarqueeInput<'_>,
    overflow_px: &mut u32,
    anim_base_ms: &mut u64,
    drawn_text: &mut String<HIFI_TEXT_LEN>,
    drawn_offset_px: &mut Option<i32>,
    current_ms: u64,
    has_rendered: bool,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let text_changed = drawn_text.as_str() != input.text;
    if text_changed {
        *anim_base_ms = current_ms;
        *drawn_offset_px = None;
    }
    let text_width = measure_band_text_width(input.text);
    let overflow = text_width.saturating_sub(input.band.size.width);
    let offset = compute_marquee_offset(current_ms.saturating_sub(*anim_base_ms), overflow);
    let unchanged = has_rendered && !text_changed && *drawn_offset_px == Some(offset);

    let band = MarqueeBand {
        band: input.band,
        centered_origin: input.origin,
        text: input.text,
        unchanged,
        primary: input.primary,
        overflow_px: overflow,
        offset_px: offset,
    };
    painter.draw(&band).map_err(RenderError::Draw)?;

    drawn_text.clear();
    let _ = drawn_text.push_str(input.text);
    *overflow_px = overflow;
    *drawn_offset_px = Some(offset);
    Ok(())
}

fn compute_art_kind(state: &State) -> u8 {
    if state.loading {
        return ART_SLOT_SPINNER;
    }
    match state.status.playback {
        PlaybackState::Buffering => ART_SLOT_BUFFERING,
        playback => {
            // A cover one track behind still reads as "this is playing";
            // dropping to an icon and back reads as a fault. The stale window
            // is what stops that leniency becoming a lie.
            if state.artwork.as_ref().is_some_and(|artwork| {
                artwork.source_uri == state.status.album_art_uri
                    || state.stale_artwork_until_ms.is_some()
            }) {
                // Artwork is the hero whether or not it is playing; the
                // transport state reads off the artist line instead.
                ART_SLOT_ARTWORK
            } else if playback == PlaybackState::Playing {
                // Reports the state, rather than the action a press would
                // take. This slot used to *be* the play/pause button, where
                // pause bars while playing meant "press to pause"; it is a
                // status area now, and keeping that inversion would have it
                // showing bars — universally read as "paused" — during
                // playback.
                ART_SLOT_PLAY_ICON
            } else {
                ART_SLOT_PAUSE_BARS
            }
        }
    }
}

fn progress_filled_px(elapsed: u32, duration: u32, width: u32) -> u32 {
    if duration == 0 {
        0
    } else {
        ((width as u64 * elapsed.min(duration) as u64) / duration as u64) as u32
    }
}

fn filled_width_for_volume(percent: u8, width: u32) -> u32 {
    let clamped = percent.min(HIFI_VOLUME_MAX) as u32;
    (width * clamped) / HIFI_VOLUME_MAX as u32
}

#[cfg(test)]
pub(crate) fn artwork_center(bounds: Rectangle) -> embedded_graphics::geometry::Point {
    layout(bounds).now_playing.artwork.center()
}

#[cfg(test)]
pub(crate) fn tile_center(bounds: Rectangle, slot: usize) -> embedded_graphics::geometry::Point {
    layout(bounds).choices.tiles[slot].center()
}

fn non_empty_or<'a>(value: &'a str, fallback: &'static str) -> &'a str {
    if value.is_empty() { fallback } else { value }
}

fn has_live_content(status: &HifiStatus) -> bool {
    !status.title.is_empty()
        || !status.artist.is_empty()
        || status.duration_seconds > 0
        || status.elapsed_seconds > 0
        || status.volume_percent > 0
        || status.playback != PlaybackState::Unknown
}

fn playback_can_pause(playback: PlaybackState) -> bool {
    matches!(playback, PlaybackState::Playing | PlaybackState::Buffering)
}

fn spinner_phase(uptime_ms: u64) -> u8 {
    ((uptime_ms / 120) % 8) as u8
}

// =====================================================================
// Widgets
// =====================================================================

/// A horizontal fill gauge that repaints only the span between the old and new
/// levels rather than the whole bar.
///
/// Used for both the progress bar and the volume readout's track — they are
/// the same shape, and keeping one implementation means a change to the
/// partial-repaint logic cannot fix one and miss the other.
struct LevelBar {
    bar: Rectangle,
    track_color: Rgb565,
    active_color: Rgb565,
    filled_px: u32,
    previous_filled_px: Option<u32>,
    /// Corner radius. Zero keeps the square ends and the cheap incremental
    /// repaint; anything else rounds both ends and repaints the whole bar.
    radius: u32,
    /// What a rounded end is blended against. Ignored when `radius` is zero.
    backdrop: Rgb565,
}

impl LevelBar {
    /// Horizontal span `[from, to)` of the bar.
    fn segment(&self, from_px: u32, to_px: u32) -> Rectangle {
        Rectangle::new(
            Point::new(self.bar.top_left.x + from_px as i32, self.bar.top_left.y),
            Size::new(to_px.saturating_sub(from_px), self.bar.size.height),
        )
    }
}

impl Widget<Action> for LevelBar {
    fn bounds(&self) -> Rectangle {
        self.bar
    }

    fn should_draw(&self) -> bool {
        self.previous_filled_px != Some(self.filled_px)
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        if self.radius == 0 {
            return self.draw_square(target);
        }

        // Both ends are rounded, so the whole bar is repainted rather than the
        // span that changed: growing the fill has to redraw the cap it used to
        // end with, and shrinking it has to restore the track's. At 190 x 10
        // that is under two thousand pixels, which is cheaper than the
        // bookkeeping needed to avoid it.
        aa::rounded_rect(
            target,
            self.bar,
            self.radius,
            self.track_color,
            self.track_color,
            0,
            self.backdrop,
        )?;

        if self.filled_px == 0 {
            return Ok(());
        }
        // A fill shorter than a full cap cannot be a pill; let the radius
        // shrink with it so the end stays round instead of turning into a
        // wedge.
        let filled = self.segment(0, self.filled_px);
        let fill_radius = self.radius.min(self.filled_px / 2);
        aa::rounded_rect(
            target,
            filled,
            fill_radius,
            self.active_color,
            self.active_color,
            0,
            self.track_color,
        )
    }
}

impl LevelBar {
    /// Square ends, repainting only the span that changed.
    fn draw_square<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        match self.previous_filled_px {
            // First frame — paint the full track, then the filled span.
            None => {
                target.fill_solid(&self.bar, self.track_color)?;
                if self.filled_px > 0 {
                    target.fill_solid(&self.segment(0, self.filled_px), self.active_color)?;
                }
            }
            Some(previous) if previous == self.filled_px => {}
            Some(previous) => {
                let (from_px, to_px, color) = if self.filled_px > previous {
                    (previous, self.filled_px, self.active_color)
                } else {
                    (self.filled_px, previous, self.track_color)
                };
                if to_px > from_px {
                    target.fill_solid(&self.segment(from_px, to_px), color)?;
                }
            }
        }
        Ok(())
    }
}

/// The volume number inside the readout panel.
struct VolumeValue {
    band: Rectangle,
    value: u8,
    unchanged: bool,
}

impl Widget<Action> for VolumeValue {
    fn bounds(&self) -> Rectangle {
        self.band
    }

    fn use_scratch(&self) -> bool {
        true
    }

    fn should_draw(&self) -> bool {
        !self.unchanged
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let font = ui_font!(BOLD);
        let style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_PRIMARY)
            .background_color(OLED_BLACK)
            .font(&font)
            .build();
        let text_style = TextStyleBuilder::new()
            .alignment(Alignment::Center)
            .baseline(Baseline::Top)
            .build();

        let mut buffer = String::<4>::new();
        let value = self.value;
        if value >= 100 {
            let _ = buffer.push((b'0' + (value / 100)) as char);
        }
        if value >= 10 {
            let _ = buffer.push((b'0' + ((value / 10) % 10)) as char);
        }
        let _ = buffer.push((b'0' + (value % 10)) as char);

        Text::with_text_style(
            buffer.as_str(),
            Point::new(self.band.center().x, self.band.top_left.y),
            style,
            text_style,
        )
        .draw(target)?;
        Ok(())
    }
}

/// Static centred text — the Choices header and each tile's caption.
struct CenteredBand<'a> {
    band: Rectangle,
    origin: Point,
    text: &'a str,
}

impl Widget<Action> for CenteredBand<'_> {
    fn bounds(&self) -> Rectangle {
        self.band
    }

    fn use_scratch(&self) -> bool {
        true
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let font = ui_font!(500);
        let style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_PRIMARY)
            .background_color(OLED_BLACK)
            .font(&font)
            .build();
        let text_style = TextStyleBuilder::new()
            .alignment(Alignment::Center)
            .baseline(Baseline::Top)
            .build();
        Text::with_text_style(self.text, self.origin, style, text_style).draw(target)?;
        Ok(())
    }
}

struct Spinner {
    center: Point,
    phase: u8,
    previous_phase: Option<u8>,
}

impl Widget<Action> for Spinner {
    fn bounds(&self) -> Rectangle {
        Rectangle::with_center(self.center, Size::new(96, 96))
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        if self.previous_phase == Some(self.phase) {
            return Ok(());
        }
        // Each frame's circles cover the same 8 positions, so no clear is
        // needed — the dots overwrite their previous selves exactly.
        draw_spinner_dots(target, self.center, self.phase)
    }
}

struct PlayTriangle {
    rect: Rectangle,
    already_drawn: bool,
}

impl Widget<Action> for PlayTriangle {
    fn bounds(&self) -> Rectangle {
        self.rect
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        if self.already_drawn {
            return Ok(());
        }
        // Sized to sit *inside* the volume readout's panel. The readout is
        // drawn over this slot, and a glyph taller than the panel leaves its
        // tips poking out around the edges, which reads as a rendering bug.
        let center = rect_visual_center(self.rect);
        aa::triangle(
            target,
            center + Point::new(-22, -40),
            center + Point::new(-22, 40),
            center + Point::new(46, 0),
            TEXT_PRIMARY,
            OLED_BLACK,
        )
    }
}

struct PauseBars {
    rect: Rectangle,
    already_drawn: bool,
}

impl Widget<Action> for PauseBars {
    fn bounds(&self) -> Rectangle {
        self.rect
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        if self.already_drawn {
            return Ok(());
        }
        // Same constraint as the play triangle: shorter than the volume
        // readout that covers it.
        let center = rect_visual_center(self.rect);
        for x_offset in [-30, 8] {
            let bar = Rectangle::new(center + Point::new(x_offset, -40), Size::new(22, 80));
            bar.into_styled(PrimitiveStyle::with_fill(TEXT_PRIMARY))
                .draw(target)?;
        }
        Ok(())
    }
}

/// The album artwork, painted 1:1 into its own rectangle.
///
/// `region` restricts painting to a sub-rectangle, which is how the volume
/// readout's footprint is restored when it disappears: the pixels are already
/// in `State::artwork`, so putting them back is a blit from a buffer we hold
/// rather than a refetch or a full-square repaint.
struct ArtworkWidget<'a> {
    rect: Rectangle,
    artwork: &'a HifiArtwork,
    region: Option<Rectangle>,
    skip: bool,
}

impl Widget<Action> for ArtworkWidget<'_> {
    fn bounds(&self) -> Rectangle {
        self.region.unwrap_or(self.rect)
    }

    fn should_draw(&self) -> bool {
        !self.skip
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        if self.skip {
            return Ok(());
        }
        let stride = HIFI_ARTWORK_SIZE as i32;
        let top_left = self.rect.top_left;
        let region = self.region.unwrap_or(self.rect);

        // Clamp the requested region into the artwork's own pixel space, so a
        // region straddling the edge paints what exists and nothing else.
        let x0 = (region.top_left.x - top_left.x).clamp(0, stride);
        let y0 = (region.top_left.y - top_left.y).clamp(0, stride);
        let x1 = (x0 + region.size.width as i32).clamp(0, stride);
        let y1 = (y0 + region.size.height as i32).clamp(0, stride);

        let pixels = self.artwork.pixels();
        target.draw_iter((y0..y1).flat_map(move |y| {
            (x0..x1).filter_map(move |x| {
                let index = (y as usize * stride as usize) + x as usize;
                pixels
                    .get(index)
                    .copied()
                    .map(|color| Pixel(top_left + Point::new(x, y), color))
            })
        }))
    }
}

/// Width in pixels that `text` would occupy when rendered with the band font.
fn measure_band_text_width(text: &str) -> u32 {
    let body_font = ui_font!(500);
    let style = BitmapFontStyleBuilder::new()
        .text_color(TEXT_PRIMARY)
        .background_color(OLED_BLACK)
        .font(&body_font)
        .build();
    Text::new(text, Point::zero(), style)
        .bounding_box()
        .size
        .width
}

/// Bounce-marquee offset, in pixels, for `elapsed_ms` since the cycle started.
/// `overflow_px` is `text_width - band_width`; `0` means the text fits.
fn compute_marquee_offset(elapsed_ms: u64, overflow_px: u32) -> i32 {
    if overflow_px == 0 {
        return 0;
    }
    let scroll_dur_ms = (overflow_px as u64 * 1000) / MARQUEE_SCROLL_PX_PER_SEC;
    let p1 = MARQUEE_HOLD_START_MS;
    let p2 = p1 + scroll_dur_ms;
    let p3 = p2 + MARQUEE_HOLD_END_MS;
    let cycle = p3 + scroll_dur_ms;
    if cycle == 0 {
        return 0;
    }
    let t = elapsed_ms % cycle;
    let overflow = overflow_px as i32;
    if t < p1 {
        0
    } else if t < p2 {
        let scrolled = ((t - p1) * MARQUEE_SCROLL_PX_PER_SEC) / 1000;
        (scrolled as i32).min(overflow)
    } else if t < p3 {
        overflow
    } else {
        let scrolled = ((t - p3) * MARQUEE_SCROLL_PX_PER_SEC) / 1000;
        (overflow - scrolled as i32).max(0)
    }
}

/// Centered text that scrolls horizontally inside `band` when the text would
/// otherwise overflow. Uses the painter's scratch buffer so out-of-band glyph
/// pixels are clipped naturally and the visible region is blitted in one go.
struct MarqueeBand<'a> {
    band: Rectangle,
    centered_origin: Point,
    text: &'a str,
    unchanged: bool,
    primary: bool,
    overflow_px: u32,
    offset_px: i32,
}

impl Widget<Action> for MarqueeBand<'_> {
    fn bounds(&self) -> Rectangle {
        self.band
    }

    fn use_scratch(&self) -> bool {
        true
    }

    fn should_draw(&self) -> bool {
        !self.unchanged
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let body_font = ui_font!(500);
        let color = if self.primary {
            TEXT_PRIMARY
        } else {
            TEXT_SECONDARY
        };
        let style = BitmapFontStyleBuilder::new()
            .text_color(color)
            .background_color(OLED_BLACK)
            .font(&body_font)
            .build();
        if let Err(RenderError::Draw(e)) = clear_rect(target, self.band) {
            return Err(e);
        }
        if self.overflow_px == 0 {
            let text_style = TextStyleBuilder::new()
                .alignment(Alignment::Center)
                .baseline(Baseline::Top)
                .build();
            Text::with_text_style(self.text, self.centered_origin, style, text_style)
                .draw(target)?;
        } else {
            let text_style = TextStyleBuilder::new()
                .alignment(Alignment::Left)
                .baseline(Baseline::Top)
                .build();
            let origin = Point::new(self.band.top_left.x - self.offset_px, self.band.top_left.y);
            Text::with_text_style(self.text, origin, style, text_style).draw(target)?;
        }
        Ok(())
    }
}

fn rect_visual_center(rect: Rectangle) -> Point {
    rect.top_left + Point::new((rect.size.width / 2) as i32, (rect.size.height / 2) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HifiPin, ui::SCREEN_BOUNDS, ui::painter::is_two_aligned};
    use alloc::vec::Vec;

    fn state_with_track(elapsed: u32) -> State {
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        let _ = status.title.push_str("Yellow Submarine");
        let _ = status.artist.push_str("The Beatles");
        status.playback = PlaybackState::Playing;
        status.duration_seconds = 160;
        status.elapsed_seconds = elapsed;
        status.volume_percent = 30;
        state.apply_status(status, 0);
        state
    }

    /// A state with a cover on screen, which is what makes the transition
    /// states visible in the first place.
    fn state_with_artwork(uri: &str) -> State {
        let mut state = state_with_track(20);
        let mut status = state.status.clone();
        status.album_art_uri.clear();
        let _ = status.album_art_uri.push_str(uri);
        state.apply_status(status, 0);

        let mut artwork = HifiArtwork::new(uri).unwrap();
        for _ in 0..crate::HIFI_ARTWORK_PIXELS {
            artwork.push_pixel(Rgb565::CSS_REBECCAPURPLE);
        }
        state.apply_artwork(artwork);
        state
    }

    #[test]
    fn a_skip_does_not_empty_the_screen() {
        let mut state = state_with_track(20);
        state.handle(Action::NextTrack, 1_000);

        // The old behaviour cleared all of this and forced Buffering, which is
        // what produced a blank slot, then a spinner, then a play icon.
        assert_eq!(state.status.title.as_str(), "Yellow Submarine");
        assert_eq!(state.status.playback, PlaybackState::Playing);
        // Only the position is certainly wrong the moment a skip is asked for.
        assert_eq!(state.status.elapsed_seconds, 0);
    }

    #[test]
    fn the_cover_stays_up_while_its_replacement_is_fetched() {
        let mut state = state_with_artwork("http://art/one.jpg");
        state.handle(Action::NextTrack, 1_000);

        // The predicted track arrives with a different cover that has not been
        // decoded yet.
        let mut next = state.status.clone();
        next.album_art_uri.clear();
        let _ = next.album_art_uri.push_str("http://art/two.jpg");
        state.apply_status(next, 1_000);

        assert!(state.artwork.is_some(), "cover was dropped on the press");
        assert_eq!(compute_art_kind(&state), ART_SLOT_ARTWORK);
    }

    #[test]
    fn a_cover_that_never_arrives_is_eventually_given_up() {
        let mut state = state_with_artwork("http://art/one.jpg");
        state.handle(Action::NextTrack, 1_000);
        let mut next = state.status.clone();
        next.album_art_uri.clear();
        let _ = next.album_art_uri.push_str("http://art/two.jpg");
        state.apply_status(next, 1_000);

        state.on_tick(1_000 + STALE_ARTWORK_GRACE_MS);

        assert!(
            state.artwork.is_none(),
            "a stale cover must not outlive its window"
        );
    }

    #[test]
    fn pausing_is_not_undone_by_a_reply_composed_before_the_press() {
        let mut state = state_with_track(20);
        state.handle(Action::TogglePlayback, 1_000);
        assert_eq!(state.status.playback, PlaybackState::Paused);

        // The device answers with the state it had when the command arrived.
        let mut stale = state.status.clone();
        stale.playback = PlaybackState::Playing;
        state.apply_status(stale, 1_100);

        assert_eq!(
            state.status.playback,
            PlaybackState::Paused,
            "the pause icon flickered"
        );
    }

    #[test]
    fn the_device_wins_once_a_pause_hold_expires() {
        let mut state = state_with_track(20);
        state.handle(Action::TogglePlayback, 1_000);

        let mut stale = state.status.clone();
        stale.playback = PlaybackState::Playing;
        state.apply_status(stale, 1_000 + crate::ui::optimistic::OPTIMISTIC_TTL_MS);

        assert_eq!(state.status.playback, PlaybackState::Playing);
    }

    #[test]
    fn volume_does_not_chase_the_levels_it_passed_through() {
        let mut state = state_with_track(20);
        // Three nudges in quick succession.
        state.handle(Action::VolumeDelta(VOLUME_STEP), 1_000);
        state.handle(Action::VolumeDelta(VOLUME_STEP), 1_050);
        state.handle(Action::VolumeDelta(VOLUME_STEP), 1_100);
        let reached = state.status.volume_percent;

        // The device reports an intermediate level it is only now catching up to.
        let mut lagging = state.status.clone();
        lagging.volume_percent = 30 + VOLUME_STEP as u8;
        state.apply_status(lagging, 1_150);

        assert_eq!(
            state.status.volume_percent, reached,
            "the volume bar jumped backwards"
        );
    }

    #[test]
    fn volume_settles_on_the_device_once_it_agrees() {
        let mut state = state_with_track(20);
        state.handle(Action::VolumeDelta(VOLUME_STEP), 1_000);
        let reached = state.status.volume_percent;

        let mut agreed = state.status.clone();
        agreed.volume_percent = reached;
        state.apply_status(agreed, 1_050);

        // Hold resolved: a later, genuinely new level from elsewhere lands.
        let mut elsewhere = state.status.clone();
        elsewhere.volume_percent = 12;
        state.apply_status(elsewhere, 1_100);
        assert_eq!(state.status.volume_percent, 12);
    }

    #[test]
    fn a_pin_cover_is_kept_only_while_the_pin_still_wants_it() {
        let mut state = State::new(0);
        let mut pins = HifiPins::new();
        let mut pin = HifiPin::new(1, "Village");
        let _ = pin.artwork_uri.push_str("http://art/village.jpg");
        pins.set(0, pin);
        state.apply_pins(pins);

        let mut artwork = HifiArtwork::new("http://art/village.jpg").unwrap();
        for _ in 0..crate::HIFI_ARTWORK_PIXELS {
            artwork.push_pixel(Rgb565::CSS_REBECCAPURPLE);
        }
        assert!(state.apply_pin_artwork(0, artwork.clone()));
        assert!(state.pin_artwork[0].is_some());

        // A cover for a URI the pin no longer names is not this pin's cover.
        let mut other = HifiArtwork::new("http://art/stale.jpg").unwrap();
        for _ in 0..crate::HIFI_ARTWORK_PIXELS {
            other.push_pixel(Rgb565::CSS_REBECCAPURPLE);
        }
        assert!(!state.apply_pin_artwork(0, other));
    }

    #[test]
    fn changing_what_a_pin_plays_drops_its_cover() {
        let mut state = State::new(0);
        let mut pins = HifiPins::new();
        let mut pin = HifiPin::new(1, "Village");
        let _ = pin.artwork_uri.push_str("http://art/village.jpg");
        pins.set(0, pin);
        state.apply_pins(pins);

        let mut artwork = HifiArtwork::new("http://art/village.jpg").unwrap();
        for _ in 0..crate::HIFI_ARTWORK_PIXELS {
            artwork.push_pixel(Rgb565::CSS_REBECCAPURPLE);
        }
        state.apply_pin_artwork(0, artwork);

        let mut replaced = HifiPins::new();
        replaced.set(0, HifiPin::new(9, "Something else"));
        state.apply_pins(replaced);

        assert!(
            state.pin_artwork[0].is_none(),
            "slot 0 kept a cover for a pin it no longer holds"
        );
    }

    #[test]
    fn three_nudges_apply_three_steps() {
        // The reported symptom: pressing down three times only moved one or
        // two, because the level each press read had been dragged back to what
        // the device last reported.
        let mut state = state_with_track(20);
        let start = state.status.volume_percent as i16;

        let mut sent = Vec::new();
        for press in 0..3 {
            let at = 1_000 + press * 40;
            if let Some(Command::SetVolume { volume }) =
                state.handle(Action::VolumeDelta(-VOLUME_STEP), at)
            {
                sent.push(volume);
            }
            // Between presses the device answers with the level from before.
            let mut lagging = state.status.clone();
            lagging.volume_percent = start as u8;
            state.apply_status(lagging, at + 10);
        }

        assert_eq!(sent.len(), 3, "a press produced no command");
        let expected = (start - 3 * VOLUME_STEP).max(0) as u8;
        assert_eq!(
            state.status.volume_percent, expected,
            "three presses did not move three steps"
        );
        assert_eq!(sent.last().copied(), Some(expected));
    }

    #[test]
    fn restarting_ignores_the_position_it_was_rewound_from() {
        let mut state = state_with_track(45);
        let outcome = state.handle(Action::PreviousOrRestart, 1_000);
        assert_eq!(outcome, Some(Command::Restart));
        assert_eq!(state.status.elapsed_seconds, 0);

        // The session answering the poll is not the one that sent the command,
        // so it keeps reporting the old position until the device's next time
        // event.
        let mut stale = state.status.clone();
        stale.elapsed_seconds = 45;
        state.apply_status(stale, 1_100);

        assert_eq!(
            state.status.elapsed_seconds, 0,
            "the progress bar jumped back to where the track was"
        );
    }

    #[test]
    fn a_plausible_position_ends_the_guard_immediately() {
        let mut state = state_with_track(45);
        state.handle(Action::PreviousOrRestart, 1_000);

        let mut caught_up = state.status.clone();
        caught_up.elapsed_seconds = 1;
        state.apply_status(caught_up, 1_100);

        assert_eq!(state.status.elapsed_seconds, 1);
        assert!(
            state.progress_guard_until_ms.is_none(),
            "the guard outlived the disagreement it existed for"
        );
    }

    #[test]
    fn the_guard_does_not_freeze_progress_forever() {
        let mut state = state_with_track(45);
        state.handle(Action::PreviousOrRestart, 1_000);

        // A device that insists is eventually believed, however odd it sounds.
        let mut insisting = state.status.clone();
        insisting.elapsed_seconds = 45;
        state.apply_status(insisting, 1_000 + crate::ui::optimistic::OPTIMISTIC_TTL_MS);

        assert_eq!(state.status.elapsed_seconds, 45);
    }

    #[test]
    fn loading_a_track_does_not_claim_to_be_paused() {
        let mut state = state_with_artwork("http://art/one.jpg");
        for transient in [PlaybackState::Stopped, PlaybackState::Buffering] {
            let mut status = state.status.clone();
            status.playback = transient;
            state.apply_status(status, 1_000);
            assert!(
                !matches!(state.status.playback, PlaybackState::Paused),
                "{transient:?} was treated as a pause"
            );
        }
    }

    /// The caption under the title, as the renderer would choose it.
    fn artist_caption(state: &State) -> &str {
        if matches!(
            state.status.playback,
            PlaybackState::Playing | PlaybackState::Buffering
        ) {
            state.status.artist.as_str()
        } else {
            non_empty_or(&state.status.artist, "Not playing")
        }
    }

    #[test]
    fn a_starting_pin_is_not_captioned_not_playing() {
        let mut state = state_with_artwork("http://art/old.jpg");
        let (pins, cover) = pins_with_cover();
        state.apply_pins(pins);
        state.apply_pin_artwork(0, cover);
        state.handle(Action::InvokePinSlot(0), 1_000);

        assert_eq!(state.status.title.as_str(), "Village");
        assert_eq!(
            artist_caption(&state),
            "",
            "the caption contradicted the title above it"
        );
    }

    #[test]
    fn an_idle_screen_still_says_so() {
        let mut state = State::new(0);
        let mut idle = HifiStatus::empty();
        idle.playback = PlaybackState::Stopped;
        state.apply_status(idle, 0);

        assert_eq!(artist_caption(&state), "Not playing");
    }

    fn pins_with_cover() -> (HifiPins, HifiArtwork) {
        let mut pins = HifiPins::new();
        let mut pin = HifiPin::new(1, "Village");
        let _ = pin.artwork_uri.push_str("http://art/village.jpg");
        pins.set(0, pin);

        let mut artwork = HifiArtwork::new("http://art/village.jpg").unwrap();
        for _ in 0..crate::HIFI_ARTWORK_PIXELS {
            artwork.push_pixel(Rgb565::CSS_REBECCAPURPLE);
        }
        (pins, artwork)
    }

    #[test]
    fn tapping_a_pin_shows_the_pin_not_the_old_track() {
        let mut state = state_with_artwork("http://art/old.jpg");
        let (pins, cover) = pins_with_cover();
        state.apply_pins(pins);
        state.apply_pin_artwork(0, cover);

        state.handle(Action::InvokePinSlot(0), 1_000);

        assert_eq!(state.status.title.as_str(), "Village");
        // The old artist under a new title would describe a track that does
        // not exist.
        assert!(state.status.artist.is_empty());
        assert_eq!(
            state.artwork.as_ref().map(|a| a.source_uri.as_str()),
            Some("http://art/village.jpg"),
            "the pin's own cover was already fetched for the grid"
        );
        assert_eq!(compute_art_kind(&state), ART_SLOT_ARTWORK);
    }

    #[test]
    fn the_old_track_does_not_come_back_while_the_pin_starts() {
        let mut state = state_with_artwork("http://art/old.jpg");
        let old_title = state.status.title.clone();
        let (pins, cover) = pins_with_cover();
        state.apply_pins(pins);
        state.apply_pin_artwork(0, cover);
        state.handle(Action::InvokePinSlot(0), 1_000);

        // The streamer has not started anything yet and still reports what it
        // was playing.
        let mut lagging = HifiStatus::empty();
        lagging.title = old_title;
        lagging.playback = PlaybackState::Playing;
        lagging.volume_percent = 42;
        state.apply_status(lagging, 1_100);

        assert_eq!(state.status.title.as_str(), "Village");
        // Volume is about the player, not the track, so it still lands.
        assert_eq!(state.status.volume_percent, 42);
    }

    #[test]
    fn the_track_the_pin_started_replaces_it() {
        let mut state = state_with_artwork("http://art/old.jpg");
        let (pins, cover) = pins_with_cover();
        state.apply_pins(pins);
        state.apply_pin_artwork(0, cover);
        state.handle(Action::InvokePinSlot(0), 1_000);

        let mut started = HifiStatus::empty();
        let _ = started.title.push_str("Track One");
        started.playback = PlaybackState::Playing;
        state.apply_status(started, 1_100);

        assert_eq!(state.status.title.as_str(), "Track One");
        assert!(state.pin_intro.is_none());
    }

    #[test]
    fn a_pin_that_never_starts_does_not_hold_the_screen() {
        let mut state = state_with_artwork("http://art/old.jpg");
        let old_title = state.status.title.clone();
        let (pins, cover) = pins_with_cover();
        state.apply_pins(pins);
        state.apply_pin_artwork(0, cover);
        state.handle(Action::InvokePinSlot(0), 1_000);

        let mut lagging = HifiStatus::empty();
        lagging.title = old_title.clone();
        lagging.playback = PlaybackState::Playing;
        state.apply_status(lagging, 1_000 + crate::ui::optimistic::OPTIMISTIC_TTL_MS);

        assert_eq!(state.status.title.as_str(), old_title.as_str());
    }

    fn filled_pins() -> HifiPins {
        let mut pins = HifiPins::new();
        for slot in 0..CHOICES_VISIBLE {
            let mut title = String::new();
            let _ = title.push_str("Choice");
            pins.set(
                slot,
                HifiPin {
                    id: slot as u32 + 1,
                    title,
                    artwork_uri: String::new(),
                },
            );
        }
        pins
    }

    fn on_choices() -> State {
        let mut state = State::new(0);
        state.intercept_button(Button::Back, 0);
        state
    }

    // ---- layout ----

    #[test]
    fn every_layout_rect_is_two_aligned() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let np = ui_layout.now_playing;
        for rect in [
            np.artwork,
            np.title_band,
            np.artist_band,
            np.progress,
            np.overlay_panel,
            np.overlay_value,
            np.overlay_track,
            ui_layout.choices.header_band,
        ] {
            assert!(is_two_aligned(rect), "not 2-px aligned: {rect:?}");
        }
        for slot in 0..CHOICES_VISIBLE {
            for rect in [
                ui_layout.choices.tiles[slot],
                ui_layout.choices.tile_art[slot],
                ui_layout.choices.tile_caption[slot],
            ] {
                assert!(is_two_aligned(rect), "not 2-px aligned: {rect:?}");
            }
        }
    }

    #[test]
    fn every_layout_rect_fits_the_panel() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let np = ui_layout.now_playing;
        let mut rects: Vec<Rectangle> = Vec::new();
        rects.extend_from_slice(&[
            np.artwork,
            np.title_band,
            np.artist_band,
            np.progress,
            np.overlay_panel,
            np.overlay_track,
            ui_layout.choices.header_band,
        ]);
        rects.extend_from_slice(&ui_layout.choices.tiles);
        for rect in rects {
            let right = rect.top_left.x + rect.size.width as i32;
            let bottom = rect.top_left.y + rect.size.height as i32;
            assert!(
                rect.top_left.x >= 0
                    && rect.top_left.y >= 0
                    && right <= SCREEN_BOUNDS.size.width as i32
                    && bottom <= SCREEN_BOUNDS.size.height as i32,
                "escapes the panel: {rect:?}"
            );
        }
    }

    #[test]
    fn tiles_do_not_overlap() {
        let tiles = layout(SCREEN_BOUNDS).choices.tiles;
        for a in 0..CHOICES_VISIBLE {
            for b in (a + 1)..CHOICES_VISIBLE {
                let (first, second) = (tiles[a], tiles[b]);
                let overlaps = first.top_left.x < second.top_left.x + second.size.width as i32
                    && second.top_left.x < first.top_left.x + first.size.width as i32
                    && first.top_left.y < second.top_left.y + second.size.height as i32
                    && second.top_left.y < first.top_left.y + first.size.height as i32;
                assert!(!overlaps, "tiles {a} and {b} overlap");
            }
        }
    }

    #[test]
    fn the_tile_row_pitch_stays_on_the_write_grid() {
        // Scrolling will offset the grid by whole rows. An odd pitch would put
        // every other row on an odd y and trip the painter's alignment rule.
        let tiles = layout(SCREEN_BOUNDS).choices.tiles;
        let pitch = tiles[CHOICES_COLS].top_left.y - tiles[0].top_left.y;
        assert_eq!(pitch, 222);
        assert_eq!(pitch % 2, 0);
    }

    #[test]
    fn artwork_slot_matches_the_decoded_artwork_size() {
        // The widget blits 1:1 from the artwork buffer, so a mismatch would
        // silently crop the image or leave a border.
        let artwork = layout(SCREEN_BOUNDS).now_playing.artwork;
        assert_eq!(artwork.size.width, HIFI_ARTWORK_SIZE);
        assert_eq!(artwork.size.height, HIFI_ARTWORK_SIZE);
    }

    #[test]
    fn the_volume_readout_sits_inside_the_artwork() {
        let np = layout(SCREEN_BOUNDS).now_playing;
        for rect in [np.overlay_panel, np.overlay_value, np.overlay_track] {
            assert!(
                rect.top_left.x >= np.artwork.top_left.x
                    && rect.top_left.y >= np.artwork.top_left.y
                    && rect.top_left.x + rect.size.width as i32
                        <= np.artwork.top_left.x + np.artwork.size.width as i32
                    && rect.top_left.y + rect.size.height as i32
                        <= np.artwork.top_left.y + np.artwork.size.height as i32,
                "readout escapes the artwork it restores from: {rect:?}"
            );
        }
    }

    // ---- pad binding ----

    #[test]
    fn now_playing_publishes_no_focus_targets() {
        let state = State::new(0);
        assert_eq!(state.page(), HifiPage::NowPlaying);
        assert!(state.focus_targets(&layout(SCREEN_BOUNDS)).is_empty());
    }

    #[test]
    fn choices_publishes_only_the_visible_tiles() {
        let state = on_choices();
        assert_eq!(
            state.focus_targets(&layout(SCREEN_BOUNDS)).len(),
            CHOICES_VISIBLE
        );
    }

    #[test]
    fn up_and_down_change_volume_in_one_press() {
        let mut state = state_with_track(0);
        let outcome = state.intercept_button(Button::Up, 0).expect("consumed");
        assert_eq!(outcome.command, Some(Command::SetVolume { volume: 32 }));
        let outcome = state.intercept_button(Button::Down, 0).expect("consumed");
        assert_eq!(outcome.command, Some(Command::SetVolume { volume: 30 }));
    }

    #[test]
    fn volume_clamps_at_the_receiver_maximum() {
        let mut state = state_with_track(0);
        for _ in 0..80 {
            state.intercept_button(Button::Up, 0);
        }
        assert_eq!(state.status.volume_percent, HIFI_VOLUME_MAX);
        for _ in 0..80 {
            state.intercept_button(Button::Down, 0);
        }
        assert_eq!(state.status.volume_percent, 0);
    }

    #[test]
    fn right_is_always_the_next_track() {
        let mut state = state_with_track(90);
        let outcome = state.intercept_button(Button::Right, 0).expect("consumed");
        assert_eq!(outcome.command, Some(Command::NextTrack));
    }

    #[test]
    fn left_restarts_once_past_the_threshold_then_goes_back() {
        let mut state = state_with_track(RESTART_THRESHOLD_SECONDS);

        // Far enough in: rewind rather than skip.
        let outcome = state.intercept_button(Button::Left, 0).expect("consumed");
        assert_eq!(outcome.command, Some(Command::Restart));
        assert_eq!(state.status.elapsed_seconds, 0);
        // The track did not change, so its metadata survives.
        assert_eq!(state.status.title.as_str(), "Yellow Submarine");

        // Restarting zeroed the clock, so the very next press falls through to
        // the previous track — no timer, no second mechanism.
        let outcome = state.intercept_button(Button::Left, 0).expect("consumed");
        assert_eq!(outcome.command, Some(Command::PreviousTrack));
    }

    #[test]
    fn left_goes_back_immediately_near_the_start_of_a_track() {
        let mut state = state_with_track(RESTART_THRESHOLD_SECONDS - 1);
        let outcome = state.intercept_button(Button::Left, 0).expect("consumed");
        assert_eq!(outcome.command, Some(Command::PreviousTrack));
    }

    #[test]
    fn select_toggles_playback() {
        let mut state = state_with_track(10);
        let outcome = state.intercept_button(Button::Select, 0).expect("consumed");
        assert_eq!(outcome.command, Some(Command::TogglePlayback));
        assert_eq!(state.status.playback, PlaybackState::Paused);

        let outcome = state.intercept_button(Button::Select, 0).expect("consumed");
        assert_eq!(outcome.command, Some(Command::TogglePlayback));
        assert_eq!(state.status.playback, PlaybackState::Playing);
    }

    #[test]
    fn back_toggles_between_the_two_screens_and_never_leaves() {
        let mut state = State::new(0);
        let outcome = state.intercept_button(Button::Back, 0).expect("consumed");
        assert!(outcome.page_changed);
        assert_eq!(state.page(), HifiPage::Choices);

        let outcome = state.intercept_button(Button::Back, 0).expect("consumed");
        assert!(outcome.page_changed);
        assert_eq!(state.page(), HifiPage::NowPlaying);
    }

    #[test]
    fn choices_leaves_the_directions_and_select_to_the_focus_ring() {
        let mut state = on_choices();
        for button in [
            Button::Up,
            Button::Down,
            Button::Left,
            Button::Right,
            // Select is replayed as a tap on the focused tile, so it also
            // falls through rather than being intercepted.
            Button::Select,
        ] {
            assert!(
                state.intercept_button(button, 0).is_none(),
                "{button:?} should fall through to the focus path"
            );
        }
    }

    // ---- volume readout ----

    #[test]
    fn volume_readout_appears_on_change_and_lapses_on_its_own() {
        let mut state = state_with_track(0);
        state.intercept_button(Button::Up, 1_000);
        assert!(state.volume_overlay_until_ms.is_some());

        // Still up just before the timeout.
        state.on_tick(1_000 + VOLUME_OVERLAY_MS - 1);
        assert!(state.volume_overlay_until_ms.is_some());

        // The tick that lapses it must request a frame, or the readout would
        // stay on the panel until something unrelated forced a repaint.
        let redraw = state.on_tick(1_000 + VOLUME_OVERLAY_MS);
        assert!(redraw);
        assert!(state.volume_overlay_until_ms.is_none());
    }

    #[test]
    fn volume_readout_appears_even_when_the_level_cannot_move() {
        let mut state = state_with_track(0);
        state.status.volume_percent = HIFI_VOLUME_MAX;
        let outcome = state.intercept_button(Button::Up, 0).expect("consumed");
        // Nothing to send — but the press was real, so it still shows.
        assert_eq!(outcome.command, None);
        assert!(state.volume_overlay_until_ms.is_some());
    }

    // ---- choices ----

    #[test]
    fn tapping_a_tile_plays_it_and_returns_to_now_playing() {
        let mut state = on_choices();
        state.apply_pins(filled_pins());

        let ui_layout = layout(SCREEN_BOUNDS);
        let command = state.handle_touch(&ui_layout, ui_layout.choices.tiles[2].center(), 0);
        assert_eq!(command, Some(Command::InvokePinId { id: 3 }));
        assert_eq!(state.page(), HifiPage::NowPlaying);
    }

    #[test]
    fn an_untitled_pin_is_labelled_by_id_not_called_empty() {
        // The device answered GetIdArray but not ReadList. That is a fault to
        // surface, not an unconfigured slot to hide.
        let mut pins = HifiPins::new();
        pins.set(
            0,
            HifiPin {
                id: 4711,
                title: String::new(),
                artwork_uri: String::new(),
            },
        );
        assert_eq!(tile_label(0, pins.get(0)).as_str(), "Pin 4711");
        assert_eq!(tile_label(1, pins.get(1)).as_str(), "Empty 2");
    }

    #[test]
    fn a_titled_pin_is_labelled_by_its_title() {
        let pins = filled_pins();
        assert_eq!(tile_label(0, pins.get(0)).as_str(), "Choice");
    }

    #[test]
    fn tapping_an_empty_tile_does_nothing() {
        let mut state = on_choices();
        let ui_layout = layout(SCREEN_BOUNDS);
        assert_eq!(
            state.handle_touch(&ui_layout, ui_layout.choices.tiles[0].center(), 0),
            None
        );
        assert_eq!(state.page(), HifiPage::Choices);
    }

    #[test]
    fn now_playing_ignores_touch_entirely() {
        let mut state = state_with_track(10);
        let ui_layout = layout(SCREEN_BOUNDS);
        for point in [
            ui_layout.now_playing.artwork.center(),
            ui_layout.now_playing.title_band.center(),
            ui_layout.now_playing.progress.center(),
            Point::new(0, 0),
        ] {
            assert_eq!(state.handle_touch(&ui_layout, point, 0), None);
        }
        assert_eq!(state.status.playback, PlaybackState::Playing);
    }

    // ---- helpers ----

    #[test]
    fn progress_fills_proportionally() {
        assert_eq!(progress_filled_px(0, 100, 330), 0);
        assert_eq!(progress_filled_px(50, 100, 330), 165);
        assert_eq!(progress_filled_px(100, 100, 330), 330);
        // Nothing to divide by is not a panic.
        assert_eq!(progress_filled_px(10, 0, 330), 0);
    }

    #[test]
    fn the_paused_badge_sits_inside_the_artwork_it_restores_from() {
        // It is drawn over the artwork and erased by re-blitting that region,
        // so a badge hanging outside the artwork would leave a hole.
        let np = layout(SCREEN_BOUNDS).now_playing;
        assert!(is_two_aligned(np.pause_badge));
        assert!(
            np.pause_badge.top_left.x >= np.artwork.top_left.x
                && np.pause_badge.top_left.y >= np.artwork.top_left.y
                && np.pause_badge.top_left.x + np.pause_badge.size.width as i32
                    <= np.artwork.top_left.x + np.artwork.size.width as i32
                && np.pause_badge.top_left.y + np.pause_badge.size.height as i32
                    <= np.artwork.top_left.y + np.artwork.size.height as i32
        );
    }

    #[test]
    fn the_volume_readout_covers_the_paused_badge_completely() {
        // Both are centred on the artwork. If the readout did not cover the
        // badge, lapsing it would restore artwork over half a pause glyph.
        let np = layout(SCREEN_BOUNDS).now_playing;
        assert!(
            np.overlay_panel.top_left.x <= np.pause_badge.top_left.x
                && np.overlay_panel.top_left.y <= np.pause_badge.top_left.y
                && np.overlay_panel.top_left.x + np.overlay_panel.size.width as i32
                    >= np.pause_badge.top_left.x + np.pause_badge.size.width as i32
                && np.overlay_panel.top_left.y + np.overlay_panel.size.height as i32
                    >= np.pause_badge.top_left.y + np.pause_badge.size.height as i32
        );
    }
}
