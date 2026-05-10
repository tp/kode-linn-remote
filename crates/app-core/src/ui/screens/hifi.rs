use core::time::Duration;

use embedded_graphics::{
    Pixel,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Arc, PrimitiveStyle, Rectangle, Triangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use crate::{
    HIFI_ARTWORK_PIXELS, HIFI_ARTWORK_SIZE, HifiArtwork, HifiStatus, PlaybackState, RenderError,
};

use super::super::{
    components::{
        ButtonTone, DURATION_WIDTH, clear_rect, draw_button, draw_duration, draw_progress_bar,
        draw_spinner, ui_font,
    },
    geometry::centered_square,
    style::*,
};

const ROUND_SAFE_SQUARE_SIZE: u32 = 330;
const SONG_TOP: i32 = 22;
const ARTIST_TOP: i32 = 56;
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Layout {
    pub(super) safe_square: Rectangle,
    pub(super) song_origin: Point,
    pub(super) artist_origin: Point,
    pub(super) volume: VolumeLayout,
    pub(super) play_button: Rectangle,
    pub(super) pin_buttons: [Rectangle; 2],
    pub(super) timer_origin: Point,
    pub(super) progress: Rectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct VolumeLayout {
    pub(super) center: Point,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct State {
    data: ScreenData,
    loading: bool,
    last_second: u64,
    current_ms: u64,
    current_second: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct ScreenData {
    status: HifiStatus,
    artwork: Option<HifiArtwork>,
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
            data: ScreenData::empty(),
            loading: true,
            last_second: current_second,
            current_ms: uptime_ms,
            current_second,
        }
    }

    pub(crate) fn on_tick(&mut self, uptime_ms: u64) -> bool {
        self.current_ms = uptime_ms;
        if self.loading {
            self.current_second = uptime_ms / 1000;
            return true;
        }

        if self.data.status.playback == PlaybackState::Buffering {
            return true;
        }

        if self.data.status.playback != PlaybackState::Playing
            || self.data.status.duration_seconds == 0
        {
            return false;
        }

        let current_second = uptime_ms / 1000;
        self.current_second = current_second;

        let elapsed = current_second.saturating_sub(self.last_second);
        if elapsed == 0 {
            return false;
        }

        self.data.status.elapsed_seconds = self
            .data
            .status
            .elapsed_seconds
            .saturating_add(elapsed as u32)
            .min(self.data.status.duration_seconds);
        self.last_second = current_second;
        if self.data.status.elapsed_seconds >= self.data.status.duration_seconds {
            self.data.status.playback = PlaybackState::Stopped;
        }
        true
    }

    pub(crate) fn apply_status(&mut self, status: HifiStatus, uptime_ms: u64) -> bool {
        let was_loading = self.loading;
        let changed = self.data.apply_status(status);
        self.loading = !self.data.is_ready();
        self.current_ms = uptime_ms;
        let current_second = uptime_ms / 1000;
        self.current_second = current_second;
        self.last_second = current_second;
        changed || was_loading != self.loading
    }

    pub(crate) fn apply_artwork(&mut self, artwork: HifiArtwork) -> bool {
        self.data.apply_artwork(artwork) && !self.loading
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
                if playback_can_pause(self.data.status.playback) {
                    self.on_tick(uptime_ms);
                    self.data.status.playback = PlaybackState::Paused;
                } else {
                    let current_second = uptime_ms / 1000;
                    self.current_second = current_second;
                    self.last_second = current_second;
                    self.data.status.playback = PlaybackState::Playing;
                }
                Some(Command::TogglePlayback)
            }
            Action::InvokePin(pin) => Some(Command::ActivatePreset { preset: pin }),
        }
    }
}

impl ScreenData {
    fn empty() -> Self {
        Self {
            status: HifiStatus::empty(),
            artwork: None,
        }
    }

    fn is_ready(&self) -> bool {
        !self.status.title.is_empty() && self.status.playback != PlaybackState::Unknown
    }

    fn apply_status(&mut self, status: HifiStatus) -> bool {
        let artwork_changed = if self
            .artwork
            .as_ref()
            .is_some_and(|artwork| artwork.source_uri != status.album_art_uri)
        {
            self.artwork = None;
            true
        } else {
            false
        };
        let status_changed = self.status != status;
        self.status = status;
        status_changed || artwork_changed
    }

    fn apply_artwork(&mut self, artwork: HifiArtwork) -> bool {
        if artwork.source_uri.is_empty()
            || artwork.source_uri != self.status.album_art_uri
            || artwork.pixels.len() != HIFI_ARTWORK_PIXELS
        {
            return false;
        }

        if self.artwork.as_ref() == Some(&artwork) {
            return false;
        }

        self.artwork = Some(artwork);
        true
    }
}

pub(crate) fn layout(bounds: Rectangle) -> Layout {
    let center_x = bounds.top_left.x + (bounds.size.width / 2) as i32;
    let center_y = bounds.top_left.y + (bounds.size.height / 2) as i32;
    let safe_square = centered_square(bounds, ROUND_SAFE_SQUARE_SIZE);
    let play_center = Point::new(center_x, safe_square.top_left.y + PLAY_CENTER_Y);

    Layout {
        safe_square,
        song_origin: Point::new(center_x, safe_square.top_left.y + SONG_TOP),
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
        timer_origin: Point::new(
            center_x - DURATION_WIDTH / 2,
            safe_square.top_left.y + TIMER_TOP,
        ),
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
    state: &State,
    display: &mut D,
    ui_layout: &Layout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    if state.loading {
        draw_volume(display, ui_layout, state.data.status.volume_percent)?;
        draw_spinner(
            display,
            ui_layout.volume.center,
            spinner_phase(state.current_ms),
        )?;
        return Ok(());
    }

    let body_font = ui_font!(500);
    let centered_top_text_style = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Top)
        .build();
    let song_style = BitmapFontStyleBuilder::new()
        .text_color(TEXT_PRIMARY)
        .background_color(OLED_BLACK)
        .font(&body_font)
        .build();
    let body_style = BitmapFontStyleBuilder::new()
        .text_color(TEXT_SECONDARY)
        .background_color(OLED_BLACK)
        .font(&body_font)
        .build();

    let data = &state.data;
    let status = &data.status;

    draw_volume(display, ui_layout, status.volume_percent)?;
    draw_play_pause_button(
        display,
        ui_layout.play_button,
        status.playback,
        spinner_phase(state.current_ms),
        data.artwork.as_ref().filter(|artwork| {
            status.playback == PlaybackState::Playing && artwork.source_uri == status.album_art_uri
        }),
    )?;
    for (index, rect) in ui_layout.pin_buttons.iter().enumerate() {
        draw_button(
            display,
            *rect,
            if index == 0 { "1" } else { "2" },
            true,
            ButtonTone::Start,
        )?;
    }
    draw_duration(
        display,
        ui_layout.timer_origin,
        Duration::from_secs(status.elapsed_seconds as u64),
        body_style.clone(),
    )?;
    draw_progress_bar(
        display,
        ui_layout.progress,
        status.elapsed_seconds as u64,
        status.duration_seconds as u64,
    )?;

    clear_rect(
        display,
        Rectangle::new(
            Point::new(ui_layout.safe_square.top_left.x, ui_layout.song_origin.y),
            Size::new(ui_layout.safe_square.size.width, 40),
        ),
    )?;
    Text::with_text_style(
        status.title.as_str(),
        ui_layout.song_origin,
        song_style,
        centered_top_text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;

    clear_rect(
        display,
        Rectangle::new(
            Point::new(ui_layout.safe_square.top_left.x, ui_layout.artist_origin.y),
            Size::new(ui_layout.safe_square.size.width, 40),
        ),
    )?;
    Text::with_text_style(
        status.artist.as_str(),
        ui_layout.artist_origin,
        body_style,
        centered_top_text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;

    Ok(())
}

fn draw_volume<D>(
    display: &mut D,
    ui_layout: &Layout,
    volume_percent: u8,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let track = PrimitiveStyle::with_stroke(VOLUME_TRACK, VOLUME_STROKE_WIDTH);
    let active = PrimitiveStyle::with_stroke(VOLUME_ACTIVE, VOLUME_STROKE_WIDTH);
    let active_sweep = VOLUME_SWEEP_DEGREES * volume_percent.min(100) as f32 / 100.0;

    Arc::with_center(
        ui_layout.volume.center,
        VOLUME_DIAMETER,
        VOLUME_START_DEGREES.deg(),
        VOLUME_SWEEP_DEGREES.deg(),
    )
    .into_styled(track)
    .draw(display)
    .map_err(RenderError::Draw)?;

    Arc::with_center(
        ui_layout.volume.center,
        VOLUME_DIAMETER,
        VOLUME_START_DEGREES.deg(),
        active_sweep.deg(),
    )
    .into_styled(active)
    .draw(display)
    .map_err(RenderError::Draw)
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

fn playback_can_pause(playback: PlaybackState) -> bool {
    matches!(playback, PlaybackState::Playing | PlaybackState::Buffering)
}

fn spinner_phase(uptime_ms: u64) -> u8 {
    ((uptime_ms / 120) % 8) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::SCREEN_BOUNDS;

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

    #[test]
    fn state_advances_live_elapsed_time_while_playing() {
        let mut state = State::new(0);
        let mut status = ready_status(PlaybackState::Playing);
        status.duration_seconds = 120;
        status.elapsed_seconds = 10;
        state.apply_status(status, 0);

        assert!(state.on_tick(1_000));
        assert_eq!(state.data.status.elapsed_seconds, 11);
    }

    #[test]
    fn paused_state_does_not_advance_elapsed_time() {
        let mut state = State::new(0);
        let mut status = ready_status(PlaybackState::Paused);
        status.duration_seconds = 120;
        status.elapsed_seconds = 10;
        state.apply_status(status, 0);

        assert!(!state.on_tick(5_000));
        assert_eq!(state.data.status.elapsed_seconds, 10);
    }

    #[test]
    fn buffering_state_animates_without_advancing_elapsed_time() {
        let mut state = State::new(0);
        let mut status = ready_status(PlaybackState::Buffering);
        status.duration_seconds = 120;
        status.elapsed_seconds = 10;
        state.apply_status(status, 0);

        assert!(state.on_tick(120));
        assert_eq!(state.data.status.elapsed_seconds, 10);
    }

    #[test]
    fn loading_spinner_requests_frames_until_status_arrives() {
        let mut state = State::new(0);

        assert!(state.on_tick(100));
        assert!(state.loading);

        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        assert!(state.apply_status(status.clone(), 200));
        assert!(state.loading);

        status.playback = PlaybackState::Playing;
        assert!(state.apply_status(status, 300));
        assert!(!state.loading);
    }

    #[test]
    fn loading_spinner_does_not_time_out_without_major_content() {
        let mut state = State::new(0);
        let mut status = HifiStatus::empty();
        status.playback = PlaybackState::Playing;
        status.duration_seconds = 120;
        status.elapsed_seconds = 10;

        assert!(state.apply_status(status, 200));
        assert!(state.loading);
        assert!(state.on_tick(60_000));
        assert!(state.loading);
    }

    #[test]
    fn incomplete_status_returns_to_loading() {
        let mut state = State::new(0);
        state.apply_status(ready_status(PlaybackState::Playing), 100);
        assert!(!state.loading);

        let mut status = HifiStatus::empty();
        status.playback = PlaybackState::Playing;
        assert!(state.apply_status(status, 200));
        assert!(state.loading);
    }

    fn ready_status(playback: PlaybackState) -> HifiStatus {
        let mut status = HifiStatus::empty();
        status.title.push_str("Caroline").unwrap();
        status.playback = playback;
        status
    }
}

fn draw_play_pause_button<D>(
    display: &mut D,
    rect: Rectangle,
    playback: PlaybackState,
    spinner_phase: u8,
    artwork: Option<&HifiArtwork>,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    clear_rect(display, rect)?;

    let center = rect_visual_center(rect);

    match playback {
        PlaybackState::Playing => {
            if let Some(artwork) = artwork {
                draw_artwork(display, center, artwork)?;
            } else {
                for x_offset in [-21, 5] {
                    let bar = Rectangle::new(center + Point::new(x_offset, -32), Size::new(16, 64));
                    bar.into_styled(PrimitiveStyle::with_fill(TEXT_PRIMARY))
                        .draw(display)
                        .map_err(RenderError::Draw)?;
                }
            }
        }
        PlaybackState::Buffering => {
            draw_spinner(display, center, spinner_phase)?;
        }
        PlaybackState::Paused | PlaybackState::Stopped | PlaybackState::Unknown => {
            Triangle::new(
                center + Point::new(-16, -30),
                center + Point::new(-16, 30),
                center + Point::new(34, 0),
            )
            .into_styled(PrimitiveStyle::with_fill(TEXT_PRIMARY))
            .draw(display)
            .map_err(RenderError::Draw)?;
        }
    }

    Ok(())
}

fn draw_artwork<D>(
    display: &mut D,
    center: Point,
    artwork: &HifiArtwork,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let top_left = center
        - Point::new(
            (HIFI_ARTWORK_SIZE / 2) as i32,
            (HIFI_ARTWORK_SIZE / 2) as i32,
        );
    let size = HIFI_ARTWORK_SIZE as i32;

    display
        .draw_iter((0..size).flat_map(|y| {
            (0..size).filter_map(move |x| {
                let index = (y as usize * HIFI_ARTWORK_SIZE as usize) + x as usize;
                artwork
                    .pixels
                    .get(index)
                    .copied()
                    .map(|color| Pixel(top_left + Point::new(x, y), color))
            })
        }))
        .map_err(RenderError::Draw)
}

fn rect_visual_center(rect: Rectangle) -> Point {
    rect.top_left + Point::new((rect.size.width / 2) as i32, (rect.size.height / 2) as i32)
}
