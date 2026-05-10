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
    HIFI_ARTWORK_PIXELS, HIFI_ARTWORK_SIZE, HIFI_TEXT_LEN, HIFI_URI_LEN, HifiArtwork, HifiStatus,
    PlaybackState, RenderError,
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
const TIMER_TOP: i32 = 218;
const PROGRESS_TOP: i32 = 274;
const PROGRESS_WIDTH: u32 = 294;
const PROGRESS_HEIGHT: u32 = 18;
const PIN_BUTTON_SIZE: u32 = 54;
const PIN_BUTTON_SIDE_INSET: i32 = 8;
const PIN_BUTTON_TOP: i32 = 218;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Layout {
    pub(super) safe_square: Rectangle,
    pub(super) song_band: Rectangle,
    pub(super) song_origin: Point,
    pub(super) artist_band: Rectangle,
    pub(super) artist_origin: Point,
    pub(super) volume: VolumeLayout,
    pub(super) play_button: Rectangle,
    pub(super) pin_buttons: [Rectangle; 2],
    pub(super) timer_origin: Point,
    pub(super) timer_bounds: Rectangle,
    pub(super) progress: Rectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct VolumeLayout {
    pub(super) center: Point,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct State {
    status: HifiStatus,
    artwork: Option<HifiArtwork>,
    created_at_ms: u64,
    loading: bool,
    last_second: u64,
    current_ms: u64,
    current_second: u64,
    last_rendered: LastRendered,
    play_slot: Slot,
}

#[derive(Clone, Debug, Eq, PartialEq, Default)]
struct LastRendered {
    has_rendered: bool,
    volume_percent: Option<u8>,
    spinner_phase: Option<u8>,
    elapsed_seconds: Option<u32>,
    duration_seconds: Option<u32>,
    progress_filled_px: Option<u32>,
    title: String<HIFI_TEXT_LEN>,
    artist: String<HIFI_TEXT_LEN>,
    artwork_uri: String<HIFI_URI_LEN>,
    pin_buttons_drawn: bool,
    loading_visible: bool,
    title_overflow_px: u32,
    title_anim_base_ms: u64,
    title_marquee_offset_px: Option<i32>,
    artist_overflow_px: u32,
    artist_anim_base_ms: u64,
    artist_marquee_offset_px: Option<i32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    TogglePlayback,
    InvokePin(u8),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Command {
    ActivatePreset { preset: u8 },
    TogglePlayback,
}

impl State {
    pub(crate) fn new(uptime_ms: u64) -> Self {
        let current_second = uptime_ms / 1000;

        Self {
            status: HifiStatus::waiting(),
            artwork: None,
            created_at_ms: uptime_ms,
            loading: true,
            last_second: current_second,
            current_ms: uptime_ms,
            current_second,
            last_rendered: LastRendered::default(),
            // Bounds is filled in lazily — the Layout is screen-fixed but the
            // State is created before we know it.
            play_slot: Slot::new(Rectangle::new(Point::zero(), Size::zero())),
        }
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

        let marquee_active =
            self.last_rendered.title_overflow_px > 0 || self.last_rendered.artist_overflow_px > 0;

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

    pub(crate) fn handle_touch(
        &mut self,
        layout: &Layout,
        point: Point,
        uptime_ms: u64,
    ) -> Option<Command> {
        let action = hit_test(layout, point)?;
        self.handle(action, uptime_ms)
    }

    fn handle(&mut self, action: Action, uptime_ms: u64) -> Option<Command> {
        match action {
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
            Action::InvokePin(pin) => Some(Command::ActivatePreset { preset: pin }),
        }
    }
}

pub(crate) fn layout(bounds: Rectangle) -> Layout {
    let center_x = bounds.top_left.x + (bounds.size.width / 2) as i32;
    let center_y = bounds.top_left.y + (bounds.size.height / 2) as i32;
    let safe_square = centered_square(bounds, ROUND_SAFE_SQUARE_SIZE);
    let play_center = Point::new(center_x, safe_square.top_left.y + PLAY_CENTER_Y);

    let song_band = Rectangle::new(
        Point::new(safe_square.top_left.x, safe_square.top_left.y + SONG_TOP),
        Size::new(safe_square.size.width, TEXT_BAND_HEIGHT),
    );
    let artist_band = Rectangle::new(
        Point::new(safe_square.top_left.x, safe_square.top_left.y + ARTIST_TOP),
        Size::new(safe_square.size.width, TEXT_BAND_HEIGHT),
    );
    let timer_origin = Point::new(
        center_x - DURATION_WIDTH / 2,
        safe_square.top_left.y + TIMER_TOP,
    );
    let timer_bounds = Rectangle::new(
        timer_origin,
        Size::new(DURATION_WIDTH as u32, TEXT_BAND_HEIGHT),
    );

    Layout {
        safe_square,
        song_band,
        song_origin: Point::new(center_x, safe_square.top_left.y + SONG_TOP),
        artist_band,
        artist_origin: Point::new(center_x, safe_square.top_left.y + ARTIST_TOP),
        volume: VolumeLayout {
            center: Point::new(center_x, center_y),
        },
        play_button: Rectangle::new(
            play_center - Point::new((PLAY_SIZE / 2) as i32, (PLAY_SIZE / 2) as i32),
            Size::new(PLAY_SIZE, PLAY_SIZE),
        ),
        pin_buttons: [
            Rectangle::new(
                Point::new(
                    safe_square.top_left.x + PIN_BUTTON_SIDE_INSET,
                    safe_square.top_left.y + PIN_BUTTON_TOP,
                ),
                Size::new(PIN_BUTTON_SIZE, PIN_BUTTON_SIZE),
            ),
            Rectangle::new(
                Point::new(
                    safe_square.top_left.x + safe_square.size.width as i32
                        - PIN_BUTTON_SIDE_INSET
                        - PIN_BUTTON_SIZE as i32,
                    safe_square.top_left.y + PIN_BUTTON_TOP,
                ),
                Size::new(PIN_BUTTON_SIZE, PIN_BUTTON_SIZE),
            ),
        ],
        timer_origin,
        timer_bounds,
        progress: Rectangle::new(
            Point::new(
                center_x - (PROGRESS_WIDTH / 2) as i32,
                safe_square.top_left.y + PROGRESS_TOP,
            ),
            Size::new(PROGRESS_WIDTH, PROGRESS_HEIGHT),
        ),
    }
}

fn hit_test(layout: &Layout, point: Point) -> Option<Action> {
    if layout.play_button.contains(point) {
        Some(Action::TogglePlayback)
    } else if layout.pin_buttons[0].contains(point) {
        Some(Action::InvokePin(1))
    } else if layout.pin_buttons[1].contains(point) {
        Some(Action::InvokePin(2))
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
    state.play_slot.bounds = ui_layout.play_button;
    let mut painter = Painter::new(display, scratch);

    // Volume arc (smart-diff: only the wedge that changed; full-redraw on first frame).
    let volume = VolumeArc {
        center: ui_layout.volume.center,
        diameter: VOLUME_DIAMETER,
        stroke_width: VOLUME_STROKE_WIDTH,
        start_deg: VOLUME_START_DEGREES,
        sweep_deg: VOLUME_SWEEP_DEGREES,
        track_color: VOLUME_TRACK,
        active_color: VOLUME_ACTIVE,
        percent: state.status.volume_percent,
        previous_percent: state.last_rendered.volume_percent,
    };
    painter.draw(&volume).map_err(RenderError::Draw)?;
    state.last_rendered.volume_percent = Some(state.status.volume_percent);

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
        // spinner's footprint. Centering on volume.center used to leave a
        // ~20 px strip of dots below the slot.
        let spinner = Spinner {
            center: rect_visual_center(ui_layout.play_button),
            phase: spinner_phase(state.current_ms),
            previous_phase: if state.play_slot.previous_kind == Some(PLAY_SLOT_SPINNER) {
                state.last_rendered.spinner_phase
            } else {
                None
            },
        };
        painter.draw(&spinner).map_err(RenderError::Draw)?;
        state.last_rendered.spinner_phase = Some(spinner.phase);
        state.play_slot.previous_kind = Some(play_kind);
        state.last_rendered.loading_visible = true;
        state.last_rendered.has_rendered = true;
        return Ok(());
    }

    if state.last_rendered.loading_visible {
        // Transitioning out of loading: invalidate caches so all widgets we
        // skipped during the spinner-only phase get a fresh first frame.
        state.last_rendered.title.clear();
        state.last_rendered.artist.clear();
        state.last_rendered.title_marquee_offset_px = None;
        state.last_rendered.artist_marquee_offset_px = None;
        state.last_rendered.elapsed_seconds = None;
        state.last_rendered.duration_seconds = None;
        state.last_rendered.progress_filled_px = None;
        state.last_rendered.pin_buttons_drawn = false;
        state.last_rendered.loading_visible = false;
    }

    // Play-button area — slot already cleared on kind change.
    match play_kind {
        PLAY_SLOT_BUFFERING => {
            let spinner = Spinner {
                center: rect_visual_center(ui_layout.play_button),
                phase: spinner_phase(state.current_ms),
                previous_phase: if state.play_slot.previous_kind == Some(PLAY_SLOT_BUFFERING) {
                    state.last_rendered.spinner_phase
                } else {
                    None
                },
            };
            painter.draw(&spinner).map_err(RenderError::Draw)?;
            state.last_rendered.spinner_phase = Some(spinner.phase);
        }
        PLAY_SLOT_ARTWORK => {
            let artwork = state.artwork.as_ref().expect("artwork present");
            let already_drawn = state.last_rendered.artwork_uri.as_str()
                == artwork.source_uri.as_str()
                && state.play_slot.previous_kind == Some(PLAY_SLOT_ARTWORK);
            let widget = ArtworkWidget {
                rect: ui_layout.play_button,
                artwork,
                already_drawn,
            };
            painter.draw(&widget).map_err(RenderError::Draw)?;
            state.last_rendered.artwork_uri.clear();
            let _ = state
                .last_rendered
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

    if !state.last_rendered.pin_buttons_drawn {
        for (index, rect) in ui_layout.pin_buttons.iter().enumerate() {
            let widget = PinButton {
                rect: *rect,
                label: if index == 0 { "1" } else { "2" },
            };
            painter.draw(&widget).map_err(RenderError::Draw)?;
        }
        state.last_rendered.pin_buttons_drawn = true;
    }

    let timer = TimerDisplay {
        origin: ui_layout.timer_origin,
        bounds: ui_layout.timer_bounds,
        elapsed: state.status.elapsed_seconds,
        previous_elapsed: state.last_rendered.elapsed_seconds,
    };
    painter.draw(&timer).map_err(RenderError::Draw)?;
    state.last_rendered.elapsed_seconds = Some(state.status.elapsed_seconds);

    let new_filled_px =
        progress_filled_px(state.status.elapsed_seconds, state.status.duration_seconds);
    let progress = ProgressBarWidget {
        rect: ui_layout.progress,
        filled_px: new_filled_px,
        previous_filled_px: state.last_rendered.progress_filled_px,
        previous_duration: state.last_rendered.duration_seconds,
        duration: state.status.duration_seconds,
    };
    painter.draw(&progress).map_err(RenderError::Draw)?;
    state.last_rendered.progress_filled_px = Some(new_filled_px);
    state.last_rendered.duration_seconds = Some(state.status.duration_seconds);

    let song_text = non_empty_or(&state.status.title, "No track");
    let song_text_changed = state.last_rendered.title.as_str() != song_text;
    if song_text_changed {
        state.last_rendered.title_anim_base_ms = state.current_ms;
        state.last_rendered.title_marquee_offset_px = None;
    }
    let song_text_width = measure_band_text_width(song_text);
    let song_overflow_px = song_text_width.saturating_sub(ui_layout.song_band.size.width);
    let song_offset_px = compute_marquee_offset(
        state
            .current_ms
            .saturating_sub(state.last_rendered.title_anim_base_ms),
        song_overflow_px,
    );
    let song_unchanged = state.last_rendered.has_rendered
        && !song_text_changed
        && state.last_rendered.title_marquee_offset_px == Some(song_offset_px);
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
    state.last_rendered.title.clear();
    let _ = state.last_rendered.title.push_str(song_text);
    state.last_rendered.title_overflow_px = song_overflow_px;
    state.last_rendered.title_marquee_offset_px = Some(song_offset_px);

    let artist_text = non_empty_or(&state.status.artist, "Not playing");
    let artist_text_changed = state.last_rendered.artist.as_str() != artist_text;
    if artist_text_changed {
        state.last_rendered.artist_anim_base_ms = state.current_ms;
        state.last_rendered.artist_marquee_offset_px = None;
    }
    let artist_text_width = measure_band_text_width(artist_text);
    let artist_overflow_px = artist_text_width.saturating_sub(ui_layout.artist_band.size.width);
    let artist_offset_px = compute_marquee_offset(
        state
            .current_ms
            .saturating_sub(state.last_rendered.artist_anim_base_ms),
        artist_overflow_px,
    );
    let artist_unchanged = state.last_rendered.has_rendered
        && !artist_text_changed
        && state.last_rendered.artist_marquee_offset_px == Some(artist_offset_px);
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
    state.last_rendered.artist.clear();
    let _ = state.last_rendered.artist.push_str(artist_text);
    state.last_rendered.artist_overflow_px = artist_overflow_px;
    state.last_rendered.artist_marquee_offset_px = Some(artist_offset_px);

    state.last_rendered.has_rendered = true;
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
    layout(bounds).play_button.center()
}

#[cfg(test)]
pub(crate) fn pin_1_button_center(bounds: Rectangle) -> embedded_graphics::geometry::Point {
    layout(bounds).pin_buttons[0].center()
}

#[cfg(test)]
pub(crate) fn pin_2_button_center(bounds: Rectangle) -> embedded_graphics::geometry::Point {
    layout(bounds).pin_buttons[1].center()
}

fn non_empty_or<'a>(value: &'a str, fallback: &'static str) -> &'a str {
    if value.is_empty() { fallback } else { value }
}

fn has_live_content(status: &HifiStatus) -> bool {
    !status.title.is_empty()
        || !status.artist.is_empty()
        || status.duration_seconds > 0
        || status.elapsed_seconds > 0
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
        let new_active = self.sweep_deg * (self.percent.min(100) as f32) / 100.0;

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
                let prev_active = self.sweep_deg * (prev.min(100) as f32) / 100.0;
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

struct PinButton {
    rect: Rectangle,
    label: &'static str,
}

impl Widget<Action> for PinButton {
    fn bounds(&self) -> Rectangle {
        self.rect
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        match draw_button(target, self.rect, self.label, true, ButtonTone::Start) {
            Ok(()) => Ok(()),
            Err(RenderError::Draw(e)) => Err(e),
            Err(RenderError::TextFormat) => Ok(()),
        }
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
        // No-op against an already-black scratch; correctness fallback for the
        // non-scratch test path.
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
            // Left-align and shift by offset; framebuf clipping in the painter
            // discards the glyph pixels that fall outside the band.
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

#[allow(dead_code)]
struct CenteredTextBand<'a> {
    clear_band: Rectangle,
    text_origin: Point,
    text: &'a str,
    unchanged: bool,
    primary: bool,
}

impl Widget<Action> for CenteredTextBand<'_> {
    fn bounds(&self) -> Rectangle {
        self.clear_band
    }

    fn use_scratch(&self) -> bool {
        true
    }

    fn should_draw(&self) -> bool {
        // Critical: scratch widgets that don't override this would have a
        // black band blitted over their existing pixels each unchanged frame.
        !self.unchanged
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let body_font = ui_font!(500);
        let centered_top_text_style = TextStyleBuilder::new()
            .alignment(Alignment::Center)
            .baseline(Baseline::Top)
            .build();
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
        // When painter routes us through scratch, the buffer is pre-cleared
        // black, so this clear_rect is a no-op against an already-black scratch.
        // When scratch isn't used (test paths), it ensures correctness.
        if let Err(RenderError::Draw(e)) = clear_rect(target, self.clear_band) {
            return Err(e);
        }
        Text::with_text_style(self.text, self.text_origin, style, centered_top_text_style)
            .draw(target)?;
        Ok(())
    }
}

fn rect_visual_center(rect: Rectangle) -> Point {
    rect.top_left + Point::new((rect.size.width / 2) as i32, (rect.size.height / 2) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::SCREEN_BOUNDS;
    use crate::ui::painter::is_two_aligned;

    #[test]
    fn hit_tests_play_button() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_eq!(
            hit_test(&ui_layout, ui_layout.play_button.center()),
            Some(Action::TogglePlayback)
        );
    }

    #[test]
    fn hit_tests_pin_buttons() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_eq!(
            hit_test(&ui_layout, ui_layout.pin_buttons[0].center()),
            Some(Action::InvokePin(1))
        );
        assert_eq!(
            hit_test(&ui_layout, ui_layout.pin_buttons[1].center()),
            Some(Action::InvokePin(2))
        );
    }

    #[test]
    fn main_controls_stay_inside_safe_square() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert!(
            ui_layout
                .safe_square
                .contains(ui_layout.play_button.center())
        );
        assert!(ui_layout.safe_square.contains(ui_layout.timer_origin));
        assert!(ui_layout.safe_square.contains(ui_layout.progress.top_left));
        assert!(
            ui_layout
                .safe_square
                .contains(ui_layout.pin_buttons[0].center())
        );
        assert!(
            ui_layout
                .safe_square
                .contains(ui_layout.pin_buttons[1].center())
        );
        assert!(
            ui_layout.pin_buttons[0].top_left.x + (PIN_BUTTON_SIZE as i32)
                < ui_layout.timer_origin.x
        );
        assert!(ui_layout.timer_origin.x + DURATION_WIDTH < ui_layout.pin_buttons[1].top_left.x);
    }

    #[test]
    fn primary_elements_are_centered() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_eq!(ui_layout.song_origin.x, ui_layout.volume.center.x);
        assert_eq!(ui_layout.artist_origin.x, ui_layout.volume.center.x);
        assert_eq!(
            rect_visual_center(ui_layout.play_button).x,
            ui_layout.volume.center.x
        );
        assert_eq!(
            ui_layout.timer_origin.x + DURATION_WIDTH / 2,
            ui_layout.volume.center.x
        );
        assert_eq!(
            ui_layout.progress.top_left.x + (ui_layout.progress.size.width / 2) as i32,
            ui_layout.volume.center.x
        );
    }

    /// The scratch-blit fast-path requires 2-px aligned bounds — for direct
    /// (non-scratch) widgets the display driver handles alignment per-primitive,
    /// so they are exempt. Today only the text bands are scratched.
    #[test]
    fn scratched_widget_bounds_are_two_aligned() {
        let ui_layout = layout(SCREEN_BOUNDS);
        for bounds in [ui_layout.song_band, ui_layout.artist_band] {
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
        // Catches scratch-blit regressions: render a hifi state with a known
        // title and assert the song band has at least one non-black pixel.
        let layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        status.playback = PlaybackState::Playing;
        state.apply_status(status, 100);

        let mut display = TestDisplay::new(466, 466);
        let mut scratch = std::vec![Rgb565::BLACK; crate::ui::RECOMMENDED_SCRATCH_PIXELS];

        render(&mut state, &mut display, &mut scratch, &layout).unwrap();

        let band = layout.song_band;
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
        // Regression: second render with unchanged title must NOT blit a
        // freshly-cleared (black) scratch over the existing text. The widget
        // smart-skips, so the painter must too — otherwise we lose the text.
        let layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        status.playback = PlaybackState::Playing;
        state.apply_status(status, 100);

        let mut display = TestDisplay::new(466, 466);
        let mut scratch = std::vec![Rgb565::BLACK; crate::ui::RECOMMENDED_SCRATCH_PIXELS];

        render(&mut state, &mut display, &mut scratch, &layout).unwrap();
        let after_first = count_non_black(&display, layout.song_band);
        assert!(after_first > 0, "first render should draw the title");

        render(&mut state, &mut display, &mut scratch, &layout).unwrap();
        let after_second = count_non_black(&display, layout.song_band);
        assert_eq!(
            after_first, after_second,
            "second render with unchanged title must preserve text pixels"
        );
    }

    #[test]
    fn loading_spinner_pixels_are_cleared_on_transition_out_of_loading() {
        // Regression: when the loading spinner is centered differently from
        // the play-button slot, the slot's clear leaves a strip of spinner
        // dots untouched. Specifically, with loading spinner @ volume.center
        // (y=233) and play_button slot @ y=210, the bottom ~20 px of the
        // spinner footprint sits below the slot and would survive the
        // transition.
        let layout = layout(SCREEN_BOUNDS);
        let mut state = State::new(0);
        let mut display = TestDisplay::new(466, 466);
        let mut scratch = std::vec![Rgb565::BLACK; crate::ui::RECOMMENDED_SCRATCH_PIXELS];

        // Render while loading — paints the spinner.
        render(&mut state, &mut display, &mut scratch, &layout).unwrap();
        assert!(state.loading);

        // Apply a status so loading ends, then re-render.
        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        status.artist.push_str("Jacob Banks").unwrap();
        status.playback = PlaybackState::Paused;
        state.apply_status(status, 200);
        render(&mut state, &mut display, &mut scratch, &layout).unwrap();
        assert!(!state.loading);

        // Inspect the suspect strip: pixels within the play button's
        // horizontal extent, BUT below the play button's bottom edge,
        // where a spinner centered at volume.center would overhang.
        // That strip is also above the progress bar so no other widget
        // paints there — any non-black pixel is a ghost.
        let slot_bottom = layout.play_button.top_left.y + layout.play_button.size.height as i32;
        let progress_top = layout.progress.top_left.y;
        let x_lo = layout.play_button.top_left.x;
        let x_hi = layout.play_button.top_left.x + layout.play_button.size.width as i32;
        for y in slot_bottom..progress_top.min(slot_bottom + 24) {
            for x in x_lo..x_hi {
                if layout.timer_bounds.contains(Point::new(x, y)) {
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

        let band = layout.artist_band;
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
}
