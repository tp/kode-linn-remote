use core::time::Duration;

use embedded_graphics::{
    Pixel,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Arc, PrimitiveStyle, Rectangle, Triangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use heapless::String;
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use crate::{
    HIFI_ARTWORK_PIXELS, HIFI_ARTWORK_SIZE, HIFI_PIN_COUNT, HIFI_TEXT_LEN, HIFI_URI_LEN,
    HIFI_VOLUME_MAX, HifiArtwork, HifiPins, HifiStatus, PlaybackState, RenderError,
};

use super::super::{
    components::{
        ButtonTone, DURATION_WIDTH, clear_rect, draw_button, draw_duration, draw_progress_bar,
        draw_spinner_dots, ui_font,
    },
    geometry::centered_square,
    painter::Painter,
    style::*,
    widget::{Slot, Widget},
};

const ROUND_SAFE_SQUARE_SIZE: u32 = 330;
const SONG_TOP: i32 = 22;
const ARTIST_TOP: i32 = 56;
const TEXT_BAND_HEIGHT: u32 = 40;
const PLAY_SIZE: u32 = 104;
const PLAY_CENTER_Y: i32 = 142;
const TRACK_BUTTON_SIZE: u32 = 64;
const TRACK_BUTTON_GAP: i32 = 24;
const TIMER_TOP: i32 = 218;
const PROGRESS_TOP: i32 = 274;
const PROGRESS_WIDTH: u32 = 294;
const PROGRESS_HEIGHT: u32 = 18;
const VOLUME_DIAMETER: u32 = 442;
const VOLUME_START_DEGREES: f32 = 135.0;
const VOLUME_SWEEP_DEGREES: f32 = 270.0;
const VOLUME_STROKE_WIDTH: u32 = 8;
const LOADING_TIMEOUT_MS: u64 = 5_000;

// Marquee bounce: hold at start, scroll to end, brief hold, scroll back.
const MARQUEE_HOLD_START_MS: u64 = 1_000;
const MARQUEE_HOLD_END_MS: u64 = 500;
const MARQUEE_SCROLL_PX_PER_SEC: u64 = 30;

// Slot kinds for the play / spinner / artwork center area.
const PLAY_SLOT_SPINNER: u8 = 1;
const PLAY_SLOT_PLAY_ICON: u8 = 2;
const PLAY_SLOT_PAUSE_BARS: u8 = 3;
const PLAY_SLOT_BUFFERING: u8 = 4;
const PLAY_SLOT_ARTWORK: u8 = 5;

// Pins page layout: 3 columns x 2 rows.
const PINS_GRID_TOP: i32 = 110;
const PINS_BUTTON_SIZE: Size = Size::new(96, 80);
const PINS_GRID_GAP: i32 = 14;

// Volume page layout.
const VOLUME_TITLE_SLOT_HEIGHT: u32 = 40;
const VOLUME_TITLE_TOP: i32 = 96;
const VOLUME_VALUE_SLOT_HEIGHT: u32 = 56;
const VOLUME_VALUE_TOP: i32 = 152;
const VOLUME_BUTTON_SIZE: u32 = 88;
const VOLUME_BUTTON_Y: i32 = 268;
const VOLUME_BUTTON_GAP: i32 = 56;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum HifiPage {
    #[default]
    Status,
    Pins,
    Volume,
}

impl HifiPage {
    fn next(self) -> Self {
        match self {
            Self::Status => Self::Pins,
            Self::Pins => Self::Volume,
            Self::Volume => Self::Status,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Layout {
    pub(super) page_body: Rectangle,
    pub(super) volume: VolumeLayout,
    pub(super) status: StatusLayout,
    pub(super) pins: PinsLayout,
    pub(super) volume_page: VolumePageLayout,
}

impl Layout {
    const fn body_for_page(&self, page: HifiPage) -> Rectangle {
        match page {
            HifiPage::Status => self.status.body,
            HifiPage::Pins => self.pins.body,
            HifiPage::Volume => self.volume_page.body,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct VolumeLayout {
    pub(super) center: Point,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct StatusLayout {
    pub(super) body: Rectangle,
    pub(super) song_band: Rectangle,
    pub(super) song_origin: Point,
    pub(super) artist_band: Rectangle,
    pub(super) artist_origin: Point,
    pub(super) previous_button: Rectangle,
    pub(super) play_button: Rectangle,
    pub(super) next_button: Rectangle,
    pub(super) timer_origin: Point,
    pub(super) timer_bounds: Rectangle,
    pub(super) progress: Rectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PinsLayout {
    pub(super) body: Rectangle,
    pub(super) buttons: [Rectangle; HIFI_PIN_COUNT],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct VolumePageLayout {
    pub(super) body: Rectangle,
    pub(super) title_slot: Rectangle,
    pub(super) value_slot: Rectangle,
    pub(super) controls_slot: Rectangle,
    pub(super) digit_origin: Point,
    pub(super) decrement_button: Rectangle,
    pub(super) increment_button: Rectangle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct State {
    page: HifiPage,
    last_rendered_page: Option<HifiPage>,
    status: HifiStatus,
    artwork: Option<HifiArtwork>,
    pins: HifiPins,
    created_at_ms: u64,
    loading: bool,
    last_second: u64,
    current_ms: u64,
    current_second: u64,
    status_cache: StatusCache,
    pins_cache: PinsCache,
    volume_cache: VolumeCache,
    play_slot: Slot,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
struct StatusCache {
    has_rendered: bool,
    volume_percent: Option<u8>,
    spinner_phase: Option<u8>,
    elapsed_seconds: Option<u32>,
    duration_seconds: Option<u32>,
    progress_filled_px: Option<u32>,
    title: String<HIFI_TEXT_LEN>,
    artist: String<HIFI_TEXT_LEN>,
    artwork_uri: String<HIFI_URI_LEN>,
    loading_visible: bool,
    title_overflow_px: u32,
    title_anim_base_ms: u64,
    title_marquee_offset_px: Option<i32>,
    artist_overflow_px: u32,
    artist_anim_base_ms: u64,
    artist_marquee_offset_px: Option<i32>,
    transport_controls_drawn: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
struct PinsCache {
    drawn_titles: [String<HIFI_TEXT_LEN>; HIFI_PIN_COUNT],
    drawn_active: [bool; HIFI_PIN_COUNT],
    has_rendered: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
struct VolumeCache {
    static_drawn: bool,
    volume_percent: Option<u8>,
    digit_value: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    PreviousTrack,
    TogglePlayback,
    NextTrack,
    InvokePinSlot(usize),
    VolumeDelta(i16),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    InvokePinId { id: u32 },
    PreviousTrack,
    TogglePlayback,
    NextTrack,
    SetVolume { volume: u8 },
}

impl State {
    pub(crate) fn new(uptime_ms: u64) -> Self {
        let current_second = uptime_ms / 1000;

        Self {
            page: HifiPage::Status,
            last_rendered_page: None,
            status: HifiStatus::waiting(),
            artwork: None,
            pins: HifiPins::new(),
            created_at_ms: uptime_ms,
            loading: true,
            last_second: current_second,
            current_ms: uptime_ms,
            current_second,
            status_cache: StatusCache::default(),
            pins_cache: PinsCache::default(),
            volume_cache: VolumeCache::default(),
            // Bounds is filled in lazily — Layout is screen-fixed but State is
            // created before we know it.
            play_slot: Slot::new(Rectangle::new(Point::zero(), Size::zero())),
        }
    }

    #[cfg(test)]
    pub(crate) fn page(&self) -> HifiPage {
        self.page
    }

    pub(crate) fn cycle_page(&mut self) {
        self.page = self.page.next();
    }

    pub(crate) fn on_tick(&mut self, uptime_ms: u64) -> bool {
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

        let marquee_active = matches!(self.page, HifiPage::Status)
            && (self.status_cache.title_overflow_px > 0
                || self.status_cache.artist_overflow_px > 0);

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

    pub(crate) fn apply_status(&mut self, status: HifiStatus, uptime_ms: u64) -> bool {
        let was_loading = self.loading;
        let has_live_content = has_live_content(&status);
        if self
            .artwork
            .as_ref()
            .is_some_and(|artwork| artwork.source_uri != status.album_art_uri)
        {
            self.artwork = None;
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
            || artwork.pixels().len() != HIFI_ARTWORK_PIXELS
        {
            return false;
        }

        if self.artwork.as_ref() == Some(&artwork) {
            return false;
        }

        self.artwork = Some(artwork);
        true
    }

    pub(crate) fn apply_pins(&mut self, pins: HifiPins) -> bool {
        if self.pins == pins {
            return false;
        }
        self.pins = pins;
        // Force a Pins-page redraw next time we're on it.
        self.pins_cache = PinsCache::default();
        true
    }

    pub(crate) fn handle_touch(
        &mut self,
        layout: &Layout,
        point: Point,
        uptime_ms: u64,
    ) -> Option<Command> {
        let action = match self.page {
            HifiPage::Status => hit_test_status(&layout.status, point),
            HifiPage::Pins => hit_test_pins(&layout.pins, point),
            HifiPage::Volume => hit_test_volume(&layout.volume_page, point),
        }?;
        self.handle(action, uptime_ms)
    }

    fn handle(&mut self, action: Action, uptime_ms: u64) -> Option<Command> {
        match action {
            Action::PreviousTrack => {
                self.clear_current_track();
                Some(Command::PreviousTrack)
            }
            Action::TogglePlayback => {
                if playback_can_pause(self.status.playback) {
                    self.on_tick(uptime_ms);
                    self.status.playback = PlaybackState::Paused;
                } else {
                    let current_second = uptime_ms / 1000;
                    self.current_second = current_second;
                    self.last_second = current_second;
                    self.status.playback = PlaybackState::Playing;
                }
                Some(Command::TogglePlayback)
            }
            Action::NextTrack => {
                self.clear_current_track();
                Some(Command::NextTrack)
            }
            Action::InvokePinSlot(slot) => {
                let pin = self.pins.get(slot)?;
                Some(Command::InvokePinId { id: pin.id })
            }
            Action::VolumeDelta(delta) => {
                let current = self.status.volume_percent as i16;
                let next = (current + delta).clamp(0, HIFI_VOLUME_MAX as i16) as u8;
                if next == self.status.volume_percent {
                    return None;
                }
                self.status.volume_percent = next;
                Some(Command::SetVolume { volume: next })
            }
        }
    }

    fn clear_current_track(&mut self) {
        self.status.title.clear();
        self.status.artist.clear();
        self.status.album.clear();
        self.status.album_art_uri.clear();
        self.status.elapsed_seconds = 0;
        self.status.duration_seconds = 0;
        self.status.playback = PlaybackState::Buffering;
        self.artwork = None;
    }
}

pub(crate) fn layout(bounds: Rectangle) -> Layout {
    let center_x = bounds.top_left.x + (bounds.size.width / 2) as i32;
    let center_y = bounds.top_left.y + (bounds.size.height / 2) as i32;
    let page_body = centered_square(bounds, ROUND_SAFE_SQUARE_SIZE);

    let status = status_layout(&page_body, center_x);
    let pins = pins_layout(&page_body, center_x);
    let volume_page = volume_page_layout(&page_body, center_x);

    Layout {
        page_body,
        volume: VolumeLayout {
            center: Point::new(center_x, center_y),
        },
        status,
        pins,
        volume_page,
    }
}

fn status_layout(body: &Rectangle, center_x: i32) -> StatusLayout {
    let play_center = Point::new(center_x, body.top_left.y + PLAY_CENTER_Y);
    let play_button = Rectangle::new(
        play_center - Point::new((PLAY_SIZE / 2) as i32, (PLAY_SIZE / 2) as i32),
        Size::new(PLAY_SIZE, PLAY_SIZE),
    );
    let track_button_top = play_center.y - (TRACK_BUTTON_SIZE / 2) as i32;
    let previous_button = Rectangle::new(
        Point::new(
            play_button.top_left.x - TRACK_BUTTON_GAP - TRACK_BUTTON_SIZE as i32,
            track_button_top,
        ),
        Size::new(TRACK_BUTTON_SIZE, TRACK_BUTTON_SIZE),
    );
    let next_button = Rectangle::new(
        Point::new(
            play_button.top_left.x + play_button.size.width as i32 + TRACK_BUTTON_GAP,
            track_button_top,
        ),
        Size::new(TRACK_BUTTON_SIZE, TRACK_BUTTON_SIZE),
    );
    let song_band = Rectangle::new(
        Point::new(body.top_left.x, body.top_left.y + SONG_TOP),
        Size::new(body.size.width, TEXT_BAND_HEIGHT),
    );
    let artist_band = Rectangle::new(
        Point::new(body.top_left.x, body.top_left.y + ARTIST_TOP),
        Size::new(body.size.width, TEXT_BAND_HEIGHT),
    );
    let timer_origin = Point::new(center_x - DURATION_WIDTH / 2, body.top_left.y + TIMER_TOP);
    let timer_bounds = Rectangle::new(
        timer_origin,
        Size::new(DURATION_WIDTH as u32, TEXT_BAND_HEIGHT),
    );
    let progress = Rectangle::new(
        Point::new(
            center_x - (PROGRESS_WIDTH / 2) as i32,
            body.top_left.y + PROGRESS_TOP,
        ),
        Size::new(PROGRESS_WIDTH, PROGRESS_HEIGHT),
    );
    let status_body = vertical_page_body(body, song_band.top_left.y, rect_bottom(progress));

    StatusLayout {
        body: status_body,
        song_band,
        song_origin: Point::new(center_x, body.top_left.y + SONG_TOP),
        artist_band,
        artist_origin: Point::new(center_x, body.top_left.y + ARTIST_TOP),
        previous_button,
        play_button,
        next_button,
        timer_origin,
        timer_bounds,
        progress,
    }
}

fn pins_layout(body: &Rectangle, center_x: i32) -> PinsLayout {
    let cols = 3_i32;
    let rows = HIFI_PIN_COUNT.div_ceil(cols as usize) as i32;
    let total_width = cols * PINS_BUTTON_SIZE.width as i32 + (cols - 1) * PINS_GRID_GAP;
    let total_height = rows * PINS_BUTTON_SIZE.height as i32 + (rows - 1) * PINS_GRID_GAP;
    let start_x = center_x - total_width / 2;
    let start_y = body.top_left.y + PINS_GRID_TOP;
    let mut buttons = [Rectangle::new(Point::zero(), PINS_BUTTON_SIZE); HIFI_PIN_COUNT];
    for slot in 0..HIFI_PIN_COUNT {
        let row = (slot / cols as usize) as i32;
        let col = (slot % cols as usize) as i32;
        let x = start_x + col * (PINS_BUTTON_SIZE.width as i32 + PINS_GRID_GAP);
        let y = start_y + row * (PINS_BUTTON_SIZE.height as i32 + PINS_GRID_GAP);
        buttons[slot] = Rectangle::new(Point::new(x, y), PINS_BUTTON_SIZE);
    }
    PinsLayout {
        body: vertical_page_body(body, start_y, start_y + total_height),
        buttons,
    }
}

fn volume_page_layout(body: &Rectangle, center_x: i32) -> VolumePageLayout {
    let title_slot = Rectangle::new(
        Point::new(body.top_left.x, body.top_left.y + VOLUME_TITLE_TOP),
        Size::new(body.size.width, VOLUME_TITLE_SLOT_HEIGHT),
    );
    let value_slot = Rectangle::new(
        Point::new(body.top_left.x, body.top_left.y + VOLUME_VALUE_TOP),
        Size::new(body.size.width, VOLUME_VALUE_SLOT_HEIGHT),
    );
    let digit_origin = Point::new(center_x, body.top_left.y + VOLUME_VALUE_TOP);
    let half_size = (VOLUME_BUTTON_SIZE / 2) as i32;
    let half_gap = VOLUME_BUTTON_GAP / 2;
    let button_top = body.top_left.y + VOLUME_BUTTON_Y - half_size;
    let decrement_button = Rectangle::new(
        Point::new(center_x - half_gap - VOLUME_BUTTON_SIZE as i32, button_top),
        Size::new(VOLUME_BUTTON_SIZE, VOLUME_BUTTON_SIZE),
    );
    let increment_button = Rectangle::new(
        Point::new(center_x + half_gap, button_top),
        Size::new(VOLUME_BUTTON_SIZE, VOLUME_BUTTON_SIZE),
    );
    let controls_slot = Rectangle::new(
        Point::new(body.top_left.x, button_top),
        Size::new(body.size.width, VOLUME_BUTTON_SIZE),
    );
    VolumePageLayout {
        body: vertical_page_body(body, title_slot.top_left.y, rect_bottom(controls_slot)),
        title_slot,
        value_slot,
        controls_slot,
        digit_origin,
        decrement_button,
        increment_button,
    }
}

fn vertical_page_body(inner: &Rectangle, top: i32, bottom: i32) -> Rectangle {
    Rectangle::new(
        Point::new(inner.top_left.x, top),
        Size::new(inner.size.width, bottom.saturating_sub(top) as u32),
    )
}

fn rect_bottom(rect: Rectangle) -> i32 {
    rect.top_left.y + rect.size.height as i32
}

fn hit_test_status(layout: &StatusLayout, point: Point) -> Option<Action> {
    if layout.previous_button.contains(point) {
        Some(Action::PreviousTrack)
    } else if layout.play_button.contains(point) {
        Some(Action::TogglePlayback)
    } else if layout.next_button.contains(point) {
        Some(Action::NextTrack)
    } else {
        None
    }
}

fn hit_test_pins(layout: &PinsLayout, point: Point) -> Option<Action> {
    layout
        .buttons
        .iter()
        .position(|rect| rect.contains(point))
        .map(Action::InvokePinSlot)
}

fn hit_test_volume(layout: &VolumePageLayout, point: Point) -> Option<Action> {
    if layout.decrement_button.contains(point) {
        Some(Action::VolumeDelta(-1))
    } else if layout.increment_button.contains(point) {
        Some(Action::VolumeDelta(1))
    } else {
        None
    }
}

pub(crate) fn render<D>(
    state: &mut State,
    display: &mut D,
    scratch: &mut [Rgb565],
    ui_layout: &Layout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    state.play_slot.bounds = ui_layout.status.play_button;

    if state.last_rendered_page != Some(state.page) {
        let page_to_clear = state.last_rendered_page.unwrap_or(state.page);
        clear_rect(display, ui_layout.body_for_page(page_to_clear))?;
        invalidate_caches_on_page_change(state);
        state.last_rendered_page = Some(state.page);
    }

    let mut painter = Painter::new(display, scratch);

    // Volume arc is rendered on every page; reflects volume_percent against
    // the receiver max (0..=HIFI_VOLUME_MAX).
    let volume = VolumeArc {
        center: ui_layout.volume.center,
        diameter: VOLUME_DIAMETER,
        stroke_width: VOLUME_STROKE_WIDTH,
        start_deg: VOLUME_START_DEGREES,
        sweep_deg: VOLUME_SWEEP_DEGREES,
        track_color: VOLUME_TRACK,
        active_color: VOLUME_ACTIVE,
        percent: state.status.volume_percent,
        previous_percent: state.status_cache.volume_percent,
    };
    painter.draw(&volume).map_err(RenderError::Draw)?;
    state.status_cache.volume_percent = Some(state.status.volume_percent);

    drop(painter);

    match state.page {
        HifiPage::Status => render_status(state, display, scratch, &ui_layout.status),
        HifiPage::Pins => render_pins(state, display, scratch, &ui_layout.pins),
        HifiPage::Volume => render_volume_page(state, display, scratch, &ui_layout.volume_page),
    }
}

fn invalidate_caches_on_page_change(state: &mut State) {
    state.status_cache = StatusCache::default();
    state.pins_cache = PinsCache::default();
    state.volume_cache = VolumeCache::default();
    state.play_slot.previous_kind = None;
}

fn render_status<D>(
    state: &mut State,
    display: &mut D,
    scratch: &mut [Rgb565],
    ui_layout: &StatusLayout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let mut painter = Painter::new(display, scratch);

    // The play-button area is a slot: spinner / play icon / pause bars / buffering / artwork
    // are mutually exclusive and the slot clears on kind transitions.
    let play_kind = compute_play_kind(state);
    state
        .play_slot
        .clear_if_kind_changed(painter.display(), play_kind)
        .map_err(RenderError::Draw)?;

    if state.loading {
        // Center on the play-button slot so the slot's clear (used when
        // loading ends and the play icon takes over) fully covers the
        // spinner's footprint.
        let spinner = Spinner {
            center: rect_visual_center(ui_layout.play_button),
            phase: spinner_phase(state.current_ms),
            previous_phase: if state.play_slot.previous_kind == Some(PLAY_SLOT_SPINNER) {
                state.status_cache.spinner_phase
            } else {
                None
            },
        };
        painter.draw(&spinner).map_err(RenderError::Draw)?;
        state.status_cache.spinner_phase = Some(spinner.phase);
        state.play_slot.previous_kind = Some(play_kind);
        state.status_cache.loading_visible = true;
        state.status_cache.has_rendered = true;
        return Ok(());
    }

    if state.status_cache.loading_visible {
        // Transitioning out of loading: invalidate caches so all widgets we
        // skipped during the spinner-only phase get a fresh first frame.
        state.status_cache.title.clear();
        state.status_cache.artist.clear();
        state.status_cache.title_marquee_offset_px = None;
        state.status_cache.artist_marquee_offset_px = None;
        state.status_cache.elapsed_seconds = None;
        state.status_cache.duration_seconds = None;
        state.status_cache.progress_filled_px = None;
        state.status_cache.loading_visible = false;
    }

    if !state.status_cache.transport_controls_drawn {
        draw_transport_button(
            painter.display(),
            ui_layout.previous_button,
            "<<",
            ButtonTone::Stop,
        )?;
        draw_transport_button(
            painter.display(),
            ui_layout.next_button,
            ">>",
            ButtonTone::Start,
        )?;
        state.status_cache.transport_controls_drawn = true;
    }

    match play_kind {
        PLAY_SLOT_BUFFERING => {
            let spinner = Spinner {
                center: rect_visual_center(ui_layout.play_button),
                phase: spinner_phase(state.current_ms),
                previous_phase: if state.play_slot.previous_kind == Some(PLAY_SLOT_BUFFERING) {
                    state.status_cache.spinner_phase
                } else {
                    None
                },
            };
            painter.draw(&spinner).map_err(RenderError::Draw)?;
            state.status_cache.spinner_phase = Some(spinner.phase);
        }
        PLAY_SLOT_ARTWORK => {
            let artwork = state.artwork.as_ref().expect("artwork present");
            let already_drawn = state.status_cache.artwork_uri.as_str()
                == artwork.source_uri.as_str()
                && state.play_slot.previous_kind == Some(PLAY_SLOT_ARTWORK);
            let widget = ArtworkWidget {
                rect: ui_layout.play_button,
                artwork,
                already_drawn,
            };
            painter.draw(&widget).map_err(RenderError::Draw)?;
            state.status_cache.artwork_uri.clear();
            let _ = state
                .status_cache
                .artwork_uri
                .push_str(artwork.source_uri.as_str());
        }
        PLAY_SLOT_PAUSE_BARS => {
            let widget = PauseBars {
                rect: ui_layout.play_button,
                already_drawn: state.play_slot.previous_kind == Some(PLAY_SLOT_PAUSE_BARS),
            };
            painter.draw(&widget).map_err(RenderError::Draw)?;
        }
        PLAY_SLOT_PLAY_ICON => {
            let widget = PlayTriangle {
                rect: ui_layout.play_button,
                already_drawn: state.play_slot.previous_kind == Some(PLAY_SLOT_PLAY_ICON),
            };
            painter.draw(&widget).map_err(RenderError::Draw)?;
        }
        _ => {}
    }
    state.play_slot.previous_kind = Some(play_kind);

    let timer = TimerDisplay {
        origin: ui_layout.timer_origin,
        bounds: ui_layout.timer_bounds,
        elapsed: state.status.elapsed_seconds,
        previous_elapsed: state.status_cache.elapsed_seconds,
    };
    painter.draw(&timer).map_err(RenderError::Draw)?;
    state.status_cache.elapsed_seconds = Some(state.status.elapsed_seconds);

    let new_filled_px =
        progress_filled_px(state.status.elapsed_seconds, state.status.duration_seconds);
    let progress = ProgressBarWidget {
        rect: ui_layout.progress,
        filled_px: new_filled_px,
        previous_filled_px: state.status_cache.progress_filled_px,
        previous_duration: state.status_cache.duration_seconds,
        duration: state.status.duration_seconds,
    };
    painter.draw(&progress).map_err(RenderError::Draw)?;
    state.status_cache.progress_filled_px = Some(new_filled_px);
    state.status_cache.duration_seconds = Some(state.status.duration_seconds);

    let song_text = non_empty_or(&state.status.title, "No track");
    let song_text_changed = state.status_cache.title.as_str() != song_text;
    if song_text_changed {
        state.status_cache.title_anim_base_ms = state.current_ms;
        state.status_cache.title_marquee_offset_px = None;
    }
    let song_text_width = measure_band_text_width(song_text);
    let song_overflow_px = song_text_width.saturating_sub(ui_layout.song_band.size.width);
    let song_offset_px = compute_marquee_offset(
        state
            .current_ms
            .saturating_sub(state.status_cache.title_anim_base_ms),
        song_overflow_px,
    );
    let song_unchanged = state.status_cache.has_rendered
        && !song_text_changed
        && state.status_cache.title_marquee_offset_px == Some(song_offset_px);
    let song = MarqueeBand {
        band: ui_layout.song_band,
        centered_origin: ui_layout.song_origin,
        text: song_text,
        unchanged: song_unchanged,
        primary: true,
        overflow_px: song_overflow_px,
        offset_px: song_offset_px,
    };
    painter.draw(&song).map_err(RenderError::Draw)?;
    state.status_cache.title.clear();
    let _ = state.status_cache.title.push_str(song_text);
    state.status_cache.title_overflow_px = song_overflow_px;
    state.status_cache.title_marquee_offset_px = Some(song_offset_px);

    let artist_text = non_empty_or(&state.status.artist, "Not playing");
    let artist_text_changed = state.status_cache.artist.as_str() != artist_text;
    if artist_text_changed {
        state.status_cache.artist_anim_base_ms = state.current_ms;
        state.status_cache.artist_marquee_offset_px = None;
    }
    let artist_text_width = measure_band_text_width(artist_text);
    let artist_overflow_px = artist_text_width.saturating_sub(ui_layout.artist_band.size.width);
    let artist_offset_px = compute_marquee_offset(
        state
            .current_ms
            .saturating_sub(state.status_cache.artist_anim_base_ms),
        artist_overflow_px,
    );
    let artist_unchanged = state.status_cache.has_rendered
        && !artist_text_changed
        && state.status_cache.artist_marquee_offset_px == Some(artist_offset_px);
    let artist = MarqueeBand {
        band: ui_layout.artist_band,
        centered_origin: ui_layout.artist_origin,
        text: artist_text,
        unchanged: artist_unchanged,
        primary: false,
        overflow_px: artist_overflow_px,
        offset_px: artist_offset_px,
    };
    painter.draw(&artist).map_err(RenderError::Draw)?;
    state.status_cache.artist.clear();
    let _ = state.status_cache.artist.push_str(artist_text);
    state.status_cache.artist_overflow_px = artist_overflow_px;
    state.status_cache.artist_marquee_offset_px = Some(artist_offset_px);

    state.status_cache.has_rendered = true;
    Ok(())
}

fn draw_transport_button<D>(
    display: &mut D,
    rect: Rectangle,
    label: &str,
    tone: ButtonTone,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    match draw_button(display, rect, label, true, tone) {
        Ok(()) => Ok(()),
        Err(RenderError::Draw(error)) => Err(RenderError::Draw(error)),
        Err(RenderError::TextFormat) => Ok(()),
    }
}

fn render_pins<D>(
    state: &mut State,
    display: &mut D,
    _scratch: &mut [Rgb565],
    ui_layout: &PinsLayout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    for slot in 0..HIFI_PIN_COUNT {
        let pin = state.pins.get(slot);
        let active = pin.is_some();
        let label = pin_button_label(slot, pin);

        let label_changed = state.pins_cache.drawn_titles[slot].as_str() != label.as_str();
        let active_changed = state.pins_cache.drawn_active[slot] != active;
        if state.pins_cache.has_rendered && !label_changed && !active_changed {
            continue;
        }

        let rect = ui_layout.buttons[slot];
        let tone = if active {
            ButtonTone::Start
        } else {
            ButtonTone::Stop
        };
        if let Err(error) = draw_button(display, rect, label.as_str(), active, tone) {
            return match error {
                RenderError::Draw(error) => Err(RenderError::Draw(error)),
                RenderError::TextFormat => Ok(()),
            };
        }
        state.pins_cache.drawn_titles[slot].clear();
        let _ = state.pins_cache.drawn_titles[slot].push_str(label.as_str());
        state.pins_cache.drawn_active[slot] = active;
    }
    state.pins_cache.has_rendered = true;
    Ok(())
}

fn pin_button_label(slot: usize, pin: Option<&crate::HifiPin>) -> String<HIFI_TEXT_LEN> {
    let mut label = String::<HIFI_TEXT_LEN>::new();
    if let Some(pin) = pin
        && !pin.title.is_empty()
        && label.push_str(pin.title.as_str()).is_ok()
    {
        return label;
    }
    label.clear();
    let _ = label.push_str("Pin ");
    let digit = (slot as u8 + 1).min(9);
    let _ = label.push((b'0' + digit) as char);
    label
}

fn render_volume_page<D>(
    state: &mut State,
    display: &mut D,
    _scratch: &mut [Rgb565],
    ui_layout: &VolumePageLayout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    if !state.volume_cache.static_drawn {
        let title_font = ui_font!(BOLD);
        let title_style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_PRIMARY)
            .background_color(OLED_BLACK)
            .font(&title_font)
            .build();
        let centered = TextStyleBuilder::new()
            .alignment(Alignment::Center)
            .baseline(Baseline::Top)
            .build();
        let title_origin = Point::new(
            ui_layout.title_slot.center().x,
            ui_layout.title_slot.top_left.y,
        );
        Text::with_text_style("VOLUME", title_origin, title_style, centered)
            .draw(display)
            .map_err(RenderError::Draw)?;
        if let Err(error) = draw_button(
            display,
            ui_layout.decrement_button,
            "-",
            true,
            ButtonTone::Stop,
        ) {
            return match error {
                RenderError::Draw(error) => Err(RenderError::Draw(error)),
                RenderError::TextFormat => Ok(()),
            };
        }
        if let Err(error) = draw_button(
            display,
            ui_layout.increment_button,
            "+",
            true,
            ButtonTone::Start,
        ) {
            return match error {
                RenderError::Draw(error) => Err(RenderError::Draw(error)),
                RenderError::TextFormat => Ok(()),
            };
        }
        state.volume_cache.static_drawn = true;
    }

    let value = state.status.volume_percent.min(HIFI_VOLUME_MAX);
    if state.volume_cache.digit_value == Some(value) {
        return Ok(());
    }
    clear_rect(display, ui_layout.value_slot)?;

    let digit_font = ui_font!(BOLD);
    let digit_style = BitmapFontStyleBuilder::new()
        .text_color(TEXT_PRIMARY)
        .background_color(OLED_BLACK)
        .font(&digit_font)
        .build();
    let centered = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Top)
        .build();

    let mut buffer = String::<4>::new();
    if value >= 100 {
        let _ = buffer.push((b'0' + (value / 100)) as char);
    }
    if value >= 10 {
        let _ = buffer.push((b'0' + ((value / 10) % 10)) as char);
    }
    let _ = buffer.push((b'0' + (value % 10)) as char);

    Text::with_text_style(
        buffer.as_str(),
        ui_layout.digit_origin,
        digit_style,
        centered,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;
    state.volume_cache.digit_value = Some(value);
    Ok(())
}

fn compute_play_kind(state: &State) -> u8 {
    if state.loading {
        return PLAY_SLOT_SPINNER;
    }
    match state.status.playback {
        PlaybackState::Buffering => PLAY_SLOT_BUFFERING,
        PlaybackState::Playing => {
            if state
                .artwork
                .as_ref()
                .is_some_and(|artwork| artwork.source_uri == state.status.album_art_uri)
            {
                PLAY_SLOT_ARTWORK
            } else {
                PLAY_SLOT_PAUSE_BARS
            }
        }
        PlaybackState::Paused | PlaybackState::Stopped | PlaybackState::Unknown => {
            PLAY_SLOT_PLAY_ICON
        }
    }
}

fn progress_filled_px(elapsed: u32, duration: u32) -> u32 {
    if duration == 0 {
        0
    } else {
        ((PROGRESS_WIDTH as u64 * elapsed.min(duration) as u64) / duration as u64) as u32
    }
}

#[cfg(test)]
pub(crate) fn play_button_center(bounds: Rectangle) -> embedded_graphics::geometry::Point {
    layout(bounds).status.play_button.center()
}

#[cfg(test)]
pub(crate) fn previous_button_center(bounds: Rectangle) -> embedded_graphics::geometry::Point {
    layout(bounds).status.previous_button.center()
}

#[cfg(test)]
pub(crate) fn next_button_center(bounds: Rectangle) -> embedded_graphics::geometry::Point {
    layout(bounds).status.next_button.center()
}

#[cfg(test)]
pub(crate) fn pin_slot_button_center(
    bounds: Rectangle,
    slot: usize,
) -> embedded_graphics::geometry::Point {
    layout(bounds).pins.buttons[slot].center()
}

#[cfg(test)]
pub(crate) fn volume_decrement_center(bounds: Rectangle) -> embedded_graphics::geometry::Point {
    layout(bounds).volume_page.decrement_button.center()
}

#[cfg(test)]
pub(crate) fn volume_increment_center(bounds: Rectangle) -> embedded_graphics::geometry::Point {
    layout(bounds).volume_page.increment_button.center()
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

struct VolumeArc {
    center: Point,
    diameter: u32,
    stroke_width: u32,
    start_deg: f32,
    sweep_deg: f32,
    track_color: Rgb565,
    active_color: Rgb565,
    percent: u8,
    previous_percent: Option<u8>,
}

impl Widget<Action> for VolumeArc {
    fn bounds(&self) -> Rectangle {
        Rectangle::with_center(self.center, Size::new(self.diameter, self.diameter))
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let new_active = arc_sweep_for_volume(self.percent, self.sweep_deg);

        match self.previous_percent {
            // First frame — paint the full track plus the active wedge.
            None => {
                let track = PrimitiveStyle::with_stroke(self.track_color, self.stroke_width);
                Arc::with_center(
                    self.center,
                    self.diameter,
                    self.start_deg.deg(),
                    self.sweep_deg.deg(),
                )
                .into_styled(track)
                .draw(target)?;
                if new_active > 0.0 {
                    let active = PrimitiveStyle::with_stroke(self.active_color, self.stroke_width);
                    Arc::with_center(
                        self.center,
                        self.diameter,
                        self.start_deg.deg(),
                        new_active.deg(),
                    )
                    .into_styled(active)
                    .draw(target)?;
                }
            }
            Some(prev) if prev == self.percent => {
                // Smart-skip.
            }
            Some(prev) => {
                let prev_active = arc_sweep_for_volume(prev, self.sweep_deg);
                let (delta_start, delta_sweep, color) = if new_active > prev_active {
                    (
                        self.start_deg + prev_active,
                        new_active - prev_active,
                        self.active_color,
                    )
                } else {
                    (
                        self.start_deg + new_active,
                        prev_active - new_active,
                        self.track_color,
                    )
                };
                if delta_sweep > f32::EPSILON {
                    let style = PrimitiveStyle::with_stroke(color, self.stroke_width);
                    Arc::with_center(
                        self.center,
                        self.diameter,
                        delta_start.deg(),
                        delta_sweep.deg(),
                    )
                    .into_styled(style)
                    .draw(target)?;
                }
            }
        }

        Ok(())
    }
}

fn arc_sweep_for_volume(percent: u8, sweep_deg: f32) -> f32 {
    let clamped = percent.min(HIFI_VOLUME_MAX);
    sweep_deg * (clamped as f32) / (HIFI_VOLUME_MAX as f32)
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
        let center = rect_visual_center(self.rect);
        Triangle::new(
            center + Point::new(-16, -30),
            center + Point::new(-16, 30),
            center + Point::new(34, 0),
        )
        .into_styled(PrimitiveStyle::with_fill(TEXT_PRIMARY))
        .draw(target)
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
        let center = rect_visual_center(self.rect);
        for x_offset in [-21, 5] {
            let bar = Rectangle::new(center + Point::new(x_offset, -32), Size::new(16, 64));
            bar.into_styled(PrimitiveStyle::with_fill(TEXT_PRIMARY))
                .draw(target)?;
        }
        Ok(())
    }
}

struct ArtworkWidget<'a> {
    rect: Rectangle,
    artwork: &'a HifiArtwork,
    already_drawn: bool,
}

impl Widget<Action> for ArtworkWidget<'_> {
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
        let center = rect_visual_center(self.rect);
        let top_left = center
            - Point::new(
                (HIFI_ARTWORK_SIZE / 2) as i32,
                (HIFI_ARTWORK_SIZE / 2) as i32,
            );
        let size = HIFI_ARTWORK_SIZE as i32;
        target.draw_iter((0..size).flat_map(|y| {
            (0..size).filter_map(move |x| {
                let index = (y as usize * HIFI_ARTWORK_SIZE as usize) + x as usize;
                self.artwork
                    .pixels()
                    .get(index)
                    .copied()
                    .map(|color| Pixel(top_left + Point::new(x, y), color))
            })
        }))
    }
}

struct TimerDisplay {
    origin: Point,
    bounds: Rectangle,
    elapsed: u32,
    previous_elapsed: Option<u32>,
}

impl Widget<Action> for TimerDisplay {
    fn bounds(&self) -> Rectangle {
        self.bounds
    }

    fn use_scratch(&self) -> bool {
        true
    }

    fn should_draw(&self) -> bool {
        self.previous_elapsed != Some(self.elapsed)
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let body_font = ui_font!(500);
        let body_style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_SECONDARY)
            .background_color(OLED_BLACK)
            .font(&body_font)
            .build();
        match draw_duration(
            target,
            self.origin,
            Duration::from_secs(self.elapsed as u64),
            body_style,
        ) {
            Ok(()) => Ok(()),
            Err(RenderError::Draw(e)) => Err(e),
            Err(RenderError::TextFormat) => Ok(()),
        }
    }
}

struct ProgressBarWidget {
    rect: Rectangle,
    filled_px: u32,
    previous_filled_px: Option<u32>,
    previous_duration: Option<u32>,
    duration: u32,
}

impl Widget<Action> for ProgressBarWidget {
    fn bounds(&self) -> Rectangle {
        self.rect
    }

    fn use_scratch(&self) -> bool {
        true
    }

    fn should_draw(&self) -> bool {
        self.previous_duration != Some(self.duration)
            || self.previous_filled_px != Some(self.filled_px)
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        match draw_progress_bar(
            target,
            self.rect,
            self.filled_px as u64,
            self.rect.size.width as u64,
        ) {
            Ok(()) => Ok(()),
            Err(RenderError::Draw(e)) => Err(e),
            Err(RenderError::TextFormat) => Ok(()),
        }
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
    use crate::HifiPin;
    use crate::ui::SCREEN_BOUNDS;
    use crate::ui::painter::is_two_aligned;

    fn assert_contains_rect(outer: Rectangle, inner: Rectangle) {
        assert!(
            outer.contains(inner.top_left)
                && outer.contains(Point::new(
                    inner.top_left.x + inner.size.width as i32 - 1,
                    inner.top_left.y + inner.size.height as i32 - 1,
                )),
            "{outer:?} should contain {inner:?}"
        );
    }

    fn assert_no_overlap(a: Rectangle, b: Rectangle) {
        assert!(!rects_overlap(a, b), "{a:?} should not overlap {b:?}");
    }

    fn rects_overlap(a: Rectangle, b: Rectangle) -> bool {
        let a_right = a.top_left.x + a.size.width as i32;
        let a_bottom = a.top_left.y + a.size.height as i32;
        let b_right = b.top_left.x + b.size.width as i32;
        let b_bottom = b.top_left.y + b.size.height as i32;

        a.top_left.x < b_right
            && b.top_left.x < a_right
            && a.top_left.y < b_bottom
            && b.top_left.y < a_bottom
    }

    #[test]
    fn hit_tests_play_button() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_eq!(
            hit_test_status(&ui_layout.status, ui_layout.status.previous_button.center()),
            Some(Action::PreviousTrack)
        );
        assert_eq!(
            hit_test_status(&ui_layout.status, ui_layout.status.play_button.center()),
            Some(Action::TogglePlayback)
        );
        assert_eq!(
            hit_test_status(&ui_layout.status, ui_layout.status.next_button.center()),
            Some(Action::NextTrack)
        );
    }

    #[test]
    fn status_page_track_buttons_emit_commands() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);

        assert_eq!(
            state.handle_touch(&ui_layout, ui_layout.status.previous_button.center(), 100),
            Some(Command::PreviousTrack)
        );
        assert_eq!(
            state.handle_touch(&ui_layout, ui_layout.status.next_button.center(), 100),
            Some(Command::NextTrack)
        );
    }

    #[test]
    fn track_buttons_clear_local_current_track() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.title.push_str("Old title").unwrap();
        status.artist.push_str("Old artist").unwrap();
        status.album.push_str("Old album").unwrap();
        status.album_art_uri.push_str("http://art/old.jpg").unwrap();
        status.elapsed_seconds = 42;
        status.duration_seconds = 180;
        status.volume_percent = 37;
        status.playback = PlaybackState::Playing;
        state.apply_status(status, 0);
        let mut artwork = HifiArtwork::new("http://art/old.jpg").unwrap();
        while artwork.push_pixel(Rgb565::WHITE) {}
        assert!(state.apply_artwork(artwork));

        assert_eq!(
            state.handle_touch(&ui_layout, ui_layout.status.next_button.center(), 100),
            Some(Command::NextTrack)
        );

        assert!(state.status.title.is_empty());
        assert!(state.status.artist.is_empty());
        assert!(state.status.album.is_empty());
        assert!(state.status.album_art_uri.is_empty());
        assert_eq!(state.status.elapsed_seconds, 0);
        assert_eq!(state.status.duration_seconds, 0);
        assert_eq!(state.status.volume_percent, 37);
        assert_eq!(state.status.playback, PlaybackState::Buffering);
        assert!(state.artwork.is_none());
    }

    #[test]
    fn hit_tests_pin_slots() {
        let ui_layout = layout(SCREEN_BOUNDS);

        for slot in 0..HIFI_PIN_COUNT {
            assert_eq!(
                hit_test_pins(&ui_layout.pins, ui_layout.pins.buttons[slot].center()),
                Some(Action::InvokePinSlot(slot))
            );
        }
    }

    #[test]
    fn hit_tests_volume_buttons() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_eq!(
            hit_test_volume(
                &ui_layout.volume_page,
                ui_layout.volume_page.decrement_button.center()
            ),
            Some(Action::VolumeDelta(-1))
        );
        assert_eq!(
            hit_test_volume(
                &ui_layout.volume_page,
                ui_layout.volume_page.increment_button.center()
            ),
            Some(Action::VolumeDelta(1))
        );
    }

    #[test]
    fn cycles_through_pages() {
        let mut state = State::new(0);
        assert_eq!(state.page(), HifiPage::Status);
        state.cycle_page();
        assert_eq!(state.page(), HifiPage::Pins);
        state.cycle_page();
        assert_eq!(state.page(), HifiPage::Volume);
        state.cycle_page();
        assert_eq!(state.page(), HifiPage::Status);
    }

    #[test]
    fn pins_page_taps_emit_invoke_pin_id() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut pins = HifiPins::new();
        let mut title = String::<{ crate::HIFI_PIN_TITLE_LEN }>::new();
        title.push_str("Radio").unwrap();
        pins.set(0, HifiPin { id: 4711, title });
        state.apply_pins(pins);
        state.cycle_page(); // -> Pins

        let command = state
            .handle_touch(&ui_layout, ui_layout.pins.buttons[0].center(), 100)
            .unwrap();
        assert_eq!(command, Command::InvokePinId { id: 4711 });
    }

    #[test]
    fn pins_page_inactive_slot_emits_no_command() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        state.cycle_page(); // -> Pins (no pins loaded)
        assert!(
            state
                .handle_touch(&ui_layout, ui_layout.pins.buttons[0].center(), 100)
                .is_none()
        );
    }

    #[test]
    fn volume_buttons_emit_clamped_set_volume() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.volume_percent = 50;
        state.apply_status(status, 0);
        state.cycle_page();
        state.cycle_page(); // -> Volume

        let increment = ui_layout.volume_page.increment_button.center();
        let decrement = ui_layout.volume_page.decrement_button.center();

        assert_eq!(
            state.handle_touch(&ui_layout, increment, 100),
            Some(Command::SetVolume { volume: 51 })
        );
        assert_eq!(
            state.handle_touch(&ui_layout, decrement, 100),
            Some(Command::SetVolume { volume: 50 })
        );
    }

    #[test]
    fn volume_clamped_at_max() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.volume_percent = HIFI_VOLUME_MAX;
        state.apply_status(status, 0);
        state.cycle_page();
        state.cycle_page(); // -> Volume

        // At max — increment is a no-op.
        let increment = ui_layout.volume_page.increment_button.center();
        assert_eq!(state.handle_touch(&ui_layout, increment, 100), None);
    }

    #[test]
    fn page_controls_stay_inside_their_page_bodies() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_contains_rect(ui_layout.status.body, ui_layout.status.song_band);
        assert_contains_rect(ui_layout.status.body, ui_layout.status.artist_band);
        assert_contains_rect(ui_layout.status.body, ui_layout.status.previous_button);
        assert_contains_rect(ui_layout.status.body, ui_layout.status.play_button);
        assert_contains_rect(ui_layout.status.body, ui_layout.status.next_button);
        assert_contains_rect(ui_layout.status.body, ui_layout.status.timer_bounds);
        assert_contains_rect(ui_layout.status.body, ui_layout.status.progress);
        assert_no_overlap(
            ui_layout.status.previous_button,
            ui_layout.status.play_button,
        );
        assert_no_overlap(ui_layout.status.play_button, ui_layout.status.next_button);

        for button in &ui_layout.pins.buttons {
            assert_contains_rect(ui_layout.pins.body, *button);
        }

        assert_contains_rect(ui_layout.volume_page.body, ui_layout.volume_page.title_slot);
        assert_contains_rect(ui_layout.volume_page.body, ui_layout.volume_page.value_slot);
        assert_contains_rect(
            ui_layout.volume_page.body,
            ui_layout.volume_page.controls_slot,
        );
        assert_contains_rect(
            ui_layout.volume_page.controls_slot,
            ui_layout.volume_page.decrement_button,
        );
        assert_contains_rect(
            ui_layout.volume_page.controls_slot,
            ui_layout.volume_page.increment_button,
        );
    }

    #[test]
    fn page_bodies_stay_inside_the_hifi_inner_screen() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_contains_rect(ui_layout.page_body, ui_layout.status.body);
        assert_contains_rect(ui_layout.page_body, ui_layout.pins.body);
        assert_contains_rect(ui_layout.page_body, ui_layout.volume_page.body);
    }

    #[test]
    fn pins_and_volume_do_not_own_status_text_bands() {
        let ui_layout = layout(SCREEN_BOUNDS);

        for body in [ui_layout.pins.body, ui_layout.volume_page.body] {
            assert_no_overlap(body, ui_layout.status.song_band);
            assert_no_overlap(body, ui_layout.status.artist_band);
        }
    }

    #[test]
    fn volume_page_slots_do_not_overlap() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let volume = ui_layout.volume_page;

        assert_no_overlap(volume.title_slot, volume.value_slot);
        assert_no_overlap(volume.value_slot, volume.controls_slot);
        assert_no_overlap(volume.title_slot, volume.controls_slot);
    }

    #[test]
    fn primary_elements_are_centered() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_eq!(ui_layout.status.song_origin.x, ui_layout.volume.center.x);
        assert_eq!(ui_layout.status.artist_origin.x, ui_layout.volume.center.x);
        assert_eq!(
            rect_visual_center(ui_layout.status.play_button).x,
            ui_layout.volume.center.x
        );
        assert_eq!(
            ui_layout.status.timer_origin.x + DURATION_WIDTH / 2,
            ui_layout.volume.center.x
        );
        assert_eq!(
            ui_layout.status.progress.top_left.x
                + (ui_layout.status.progress.size.width / 2) as i32,
            ui_layout.volume.center.x
        );
    }

    /// The scratch-blit fast-path requires 2-px aligned bounds — for direct
    /// (non-scratch) widgets the display driver handles alignment per-primitive,
    /// so they are exempt. Today only the text bands are scratched.
    #[test]
    fn scratched_widget_bounds_are_two_aligned() {
        let ui_layout = layout(SCREEN_BOUNDS);
        for bounds in [ui_layout.status.song_band, ui_layout.status.artist_band] {
            assert!(
                is_two_aligned(bounds),
                "scratched bounds {bounds:?} are not 2-px aligned"
            );
        }
    }

    #[test]
    fn state_advances_live_elapsed_time_while_playing() {
        let mut state = State::new(0);
        let mut status = HifiStatus::waiting();
        status.playback = PlaybackState::Playing;
        status.duration_seconds = 120;
        status.elapsed_seconds = 10;
        state.apply_status(status, 0);

        assert!(state.on_tick(1_000));
        assert_eq!(state.status.elapsed_seconds, 11);
    }

    #[test]
    fn paused_state_does_not_advance_elapsed_time() {
        let mut state = State::new(0);
        let mut status = HifiStatus::waiting();
        status.playback = PlaybackState::Paused;
        status.duration_seconds = 120;
        status.elapsed_seconds = 10;
        state.apply_status(status, 0);

        assert!(!state.on_tick(5_000));
        assert_eq!(state.status.elapsed_seconds, 10);
    }

    #[test]
    fn buffering_state_animates_without_advancing_elapsed_time() {
        let mut state = State::new(0);
        let mut status = HifiStatus::waiting();
        status.playback = PlaybackState::Buffering;
        status.duration_seconds = 120;
        status.elapsed_seconds = 10;
        state.apply_status(status, 0);

        assert!(state.on_tick(120));
        assert_eq!(state.status.elapsed_seconds, 10);
    }

    #[test]
    fn loading_spinner_requests_frames_until_status_arrives() {
        let mut state = State::new(0);

        assert!(state.on_tick(100));
        assert!(state.loading);

        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        assert!(state.apply_status(status, 200));
        assert!(!state.loading);
    }

    #[test]
    fn volume_only_status_finishes_loading() {
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.volume_percent = 42;

        assert!(state.apply_status(status, 200));
        assert!(!state.loading);
    }

    #[test]
    fn loading_spinner_times_out() {
        let mut state = State::new(0);

        assert!(state.on_tick(LOADING_TIMEOUT_MS));
        assert!(!state.loading);
    }

    #[test]
    fn spinner_smart_skips_when_phase_unchanged() {
        use core::cell::Cell;

        struct Counting {
            calls: Cell<u32>,
        }

        impl OriginDimensions for Counting {
            fn size(&self) -> Size {
                Size::new(466, 466)
            }
        }

        impl DrawTarget for Counting {
            type Color = Rgb565;
            type Error = core::convert::Infallible;

            fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
            where
                I: IntoIterator<Item = Pixel<Self::Color>>,
            {
                self.calls
                    .set(self.calls.get() + pixels.into_iter().count() as u32);
                Ok(())
            }
        }

        let spinner = Spinner {
            center: Point::new(100, 100),
            phase: 3,
            previous_phase: Some(3),
        };
        let mut t = Counting {
            calls: Cell::new(0),
        };
        spinner.draw(&mut t).unwrap();
        assert_eq!(t.calls.get(), 0, "spinner should smart-skip on equal phase");
    }

    #[test]
    fn song_text_lands_non_black_pixels_in_band() {
        let layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        status.playback = PlaybackState::Playing;
        state.apply_status(status.clone(), 100);

        let mut display = TestDisplay::new(466, 466);
        let mut scratch = std::vec![Rgb565::BLACK; crate::ui::RECOMMENDED_SCRATCH_PIXELS];

        render(&mut state, &mut display, &mut scratch, &layout).unwrap();

        let band = layout.status.song_band;
        let mut non_black = 0_u32;
        for y in band.top_left.y..band.top_left.y + band.size.height as i32 {
            for x in band.top_left.x..band.top_left.x + band.size.width as i32 {
                let idx = (y as usize * 466) + x as usize;
                if display.pixels[idx] != Rgb565::BLACK {
                    non_black += 1;
                }
            }
        }
        assert!(
            non_black > 0,
            "song band should contain text pixels (got 0 non-black)"
        );
    }

    #[test]
    fn song_text_persists_across_unchanged_renders() {
        let layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        status.playback = PlaybackState::Playing;
        state.apply_status(status, 100);

        let mut display = TestDisplay::new(466, 466);
        let mut scratch = std::vec![Rgb565::BLACK; crate::ui::RECOMMENDED_SCRATCH_PIXELS];

        render(&mut state, &mut display, &mut scratch, &layout).unwrap();
        let after_first = count_non_black(&display, layout.status.song_band);
        assert!(after_first > 0, "first render should draw the title");

        render(&mut state, &mut display, &mut scratch, &layout).unwrap();
        let after_second = count_non_black(&display, layout.status.song_band);
        assert_eq!(
            after_first, after_second,
            "second render with unchanged title must preserve text pixels"
        );
    }

    #[test]
    fn loading_spinner_pixels_are_cleared_on_transition_out_of_loading() {
        let layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut display = TestDisplay::new(466, 466);
        let mut scratch = std::vec![Rgb565::BLACK; crate::ui::RECOMMENDED_SCRATCH_PIXELS];

        render(&mut state, &mut display, &mut scratch, &layout).unwrap();
        assert!(state.loading);

        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        status.artist.push_str("Jacob Banks").unwrap();
        status.playback = PlaybackState::Paused;
        state.apply_status(status, 200);
        render(&mut state, &mut display, &mut scratch, &layout).unwrap();
        assert!(!state.loading);

        let slot_bottom =
            layout.status.play_button.top_left.y + layout.status.play_button.size.height as i32;
        let progress_top = layout.status.progress.top_left.y;
        let x_lo = layout.status.play_button.top_left.x;
        let x_hi =
            layout.status.play_button.top_left.x + layout.status.play_button.size.width as i32;
        for y in slot_bottom..progress_top.min(slot_bottom + 24) {
            for x in x_lo..x_hi {
                if layout.status.timer_bounds.contains(Point::new(x, y)) {
                    continue;
                }
                let idx = (y as usize * 466) + x as usize;
                assert_eq!(
                    display.pixels[idx],
                    Rgb565::BLACK,
                    "ghost spinner pixel at ({x}, {y}) below play-button slot"
                );
            }
        }
    }

    #[test]
    fn volume_value_redraw_preserves_button_controls() {
        let layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.volume_percent = 23;
        state.apply_status(status.clone(), 100);
        state.cycle_page();
        state.cycle_page(); // -> Volume

        let mut display = TestDisplay::new(466, 466);
        let mut scratch = std::vec![Rgb565::BLACK; crate::ui::RECOMMENDED_SCRATCH_PIXELS];
        render(&mut state, &mut display, &mut scratch, &layout).unwrap();

        let button_probe = Point::new(
            layout.volume_page.decrement_button.center().x,
            layout.volume_page.decrement_button.top_left.y + 2,
        );
        assert_ne!(display.pixel_at(button_probe), Rgb565::BLACK);

        status.volume_percent = 24;
        state.apply_status(status, 200);
        render(&mut state, &mut display, &mut scratch, &layout).unwrap();

        assert_ne!(display.pixel_at(button_probe), Rgb565::BLACK);
    }

    fn count_non_black(display: &TestDisplay, band: Rectangle) -> u32 {
        let mut count = 0;
        for y in band.top_left.y..band.top_left.y + band.size.height as i32 {
            for x in band.top_left.x..band.top_left.x + band.size.width as i32 {
                let idx = (y as usize * 466) + x as usize;
                if display.pixels[idx] != Rgb565::BLACK {
                    count += 1;
                }
            }
        }
        count
    }

    #[test]
    fn artist_text_lands_non_black_pixels_in_band() {
        let layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        status.artist.push_str("Jacob Banks").unwrap();
        status.playback = PlaybackState::Playing;
        state.apply_status(status, 100);

        let mut display = TestDisplay::new(466, 466);
        let mut scratch = std::vec![Rgb565::BLACK; crate::ui::RECOMMENDED_SCRATCH_PIXELS];

        render(&mut state, &mut display, &mut scratch, &layout).unwrap();

        let band = layout.status.artist_band;
        let mut non_black = 0_u32;
        for y in band.top_left.y..band.top_left.y + band.size.height as i32 {
            for x in band.top_left.x..band.top_left.x + band.size.width as i32 {
                let idx = (y as usize * 466) + x as usize;
                if display.pixels[idx] != Rgb565::BLACK {
                    non_black += 1;
                }
            }
        }
        assert!(
            non_black > 0,
            "artist band should contain text pixels (got 0 non-black)"
        );
    }

    /// Vec-backed DrawTarget for tests that need to inspect the rendered
    /// pixel buffer at full display resolution.
    struct TestDisplay {
        width: u32,
        height: u32,
        pixels: std::vec::Vec<Rgb565>,
    }

    impl TestDisplay {
        fn new(width: u32, height: u32) -> Self {
            Self {
                width,
                height,
                pixels: std::vec![Rgb565::BLACK; (width * height) as usize],
            }
        }

        fn pixel_at(&self, point: Point) -> Rgb565 {
            self.pixels[(point.y as u32 * self.width + point.x as u32) as usize]
        }
    }

    impl OriginDimensions for TestDisplay {
        fn size(&self) -> Size {
            Size::new(self.width, self.height)
        }
    }

    impl DrawTarget for TestDisplay {
        type Color = Rgb565;
        type Error = core::convert::Infallible;

        fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
        where
            I: IntoIterator<Item = Pixel<Self::Color>>,
        {
            for Pixel(point, color) in pixels {
                if point.x < 0 || point.y < 0 {
                    continue;
                }
                let x = point.x as u32;
                let y = point.y as u32;
                if x >= self.width || y >= self.height {
                    continue;
                }
                let idx = (y * self.width + x) as usize;
                self.pixels[idx] = color;
            }
            Ok(())
        }
    }

    #[test]
    fn volume_arc_smart_skips_when_percent_unchanged() {
        use core::cell::Cell;

        struct Counting {
            calls: Cell<u32>,
        }

        impl OriginDimensions for Counting {
            fn size(&self) -> Size {
                Size::new(466, 466)
            }
        }

        impl DrawTarget for Counting {
            type Color = Rgb565;
            type Error = core::convert::Infallible;

            fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
            where
                I: IntoIterator<Item = Pixel<Self::Color>>,
            {
                self.calls
                    .set(self.calls.get() + pixels.into_iter().count() as u32);
                Ok(())
            }
        }

        let arc = VolumeArc {
            center: Point::new(232, 232),
            diameter: VOLUME_DIAMETER,
            stroke_width: VOLUME_STROKE_WIDTH,
            start_deg: VOLUME_START_DEGREES,
            sweep_deg: VOLUME_SWEEP_DEGREES,
            track_color: VOLUME_TRACK,
            active_color: VOLUME_ACTIVE,
            percent: 50,
            previous_percent: Some(50),
        };
        let mut t = Counting {
            calls: Cell::new(0),
        };
        arc.draw(&mut t).unwrap();
        assert_eq!(t.calls.get(), 0, "volume arc should smart-skip");
    }

    #[test]
    fn volume_arc_full_at_max_volume() {
        // Volume at HIFI_VOLUME_MAX should fill the arc completely.
        assert!(
            (arc_sweep_for_volume(HIFI_VOLUME_MAX, VOLUME_SWEEP_DEGREES) - VOLUME_SWEEP_DEGREES)
                .abs()
                < 0.01
        );
        // And anything beyond max also caps at the full sweep.
        assert!(
            (arc_sweep_for_volume(HIFI_VOLUME_MAX + 10, VOLUME_SWEEP_DEGREES)
                - VOLUME_SWEEP_DEGREES)
                .abs()
                < 0.01
        );
    }
}
