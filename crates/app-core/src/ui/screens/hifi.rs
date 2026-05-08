use core::time::Duration;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Arc, PrimitiveStyle, Rectangle, Triangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use crate::RenderError;

use super::super::{
    components::{DURATION_WIDTH, draw_duration, draw_progress_bar, ui_font},
    geometry::centered_square,
    style::*,
};

#[derive(Clone, Copy)]
struct Track {
    title: &'static str,
    artist: &'static str,
}

const TRACKS: [Track; 5] = [
    Track {
        title: "Blinding Lights",
        artist: "The Weeknd",
    },
    Track {
        title: "Levitating",
        artist: "Dua Lipa",
    },
    Track {
        title: "As It Was",
        artist: "Harry Styles",
    },
    Track {
        title: "Flowers",
        artist: "Miley Cyrus",
    },
    Track {
        title: "Bad Guy",
        artist: "Billie Eilish",
    },
];
const TRACK_ROTATION_SECONDS: u64 = 5;
const ROUND_SAFE_SQUARE_SIZE: u32 = 330;
const SONG_TOP: i32 = 22;
const ARTIST_TOP: i32 = 56;
const PLAY_SIZE: u32 = 104;
const PLAY_CENTER_Y: i32 = 142;
const TIMER_TOP: i32 = 218;
const PROGRESS_TOP: i32 = 274;
const PROGRESS_WIDTH: u32 = 294;
const PROGRESS_HEIGHT: u32 = 18;
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
    pub(super) timer_origin: Point,
    pub(super) progress: Rectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct VolumeLayout {
    pub(super) center: Point,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct State {
    playing: bool,
    volume_percent: u8,
    total_seconds: u64,
    remaining_seconds: u64,
    last_second: u64,
    current_second: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    TogglePlayback,
}

impl State {
    pub(crate) const fn new(uptime_ms: u64) -> Self {
        let current_second = uptime_ms / 1000;

        Self {
            playing: true,
            volume_percent: 60,
            total_seconds: 20 * 60,
            remaining_seconds: 20 * 60,
            last_second: current_second,
            current_second,
        }
    }

    #[cfg(test)]
    pub(crate) const fn playing(&self) -> bool {
        self.playing
    }

    #[cfg(test)]
    pub(crate) const fn total_seconds(&self) -> u64 {
        self.total_seconds
    }

    #[cfg(test)]
    pub(crate) const fn remaining_seconds(&self) -> u64 {
        self.remaining_seconds
    }

    pub(crate) fn on_tick(&mut self, uptime_ms: u64) -> bool {
        if !self.playing || self.remaining_seconds == 0 {
            return false;
        }

        let previous_track_index = self.track_index();
        let current_second = uptime_ms / 1000;
        self.current_second = current_second;
        let track_changed = self.track_index() != previous_track_index;

        let elapsed = current_second.saturating_sub(self.last_second);
        if elapsed == 0 {
            return track_changed;
        }

        self.remaining_seconds = self.remaining_seconds.saturating_sub(elapsed);
        self.last_second = current_second;
        if self.remaining_seconds == 0 {
            self.playing = false;
        }
        true
    }

    pub(crate) fn handle_touch(
        &mut self,
        layout: &Layout,
        point: Point,
        uptime_ms: u64,
    ) -> Option<crate::Screen> {
        let action = hit_test(layout, point)?;
        self.handle(action, uptime_ms);
        None
    }

    fn handle(&mut self, action: Action, uptime_ms: u64) {
        match action {
            Action::TogglePlayback => {
                if self.playing {
                    self.on_tick(uptime_ms);
                    self.playing = false;
                } else {
                    let current_second = uptime_ms / 1000;
                    self.current_second = current_second;
                    self.last_second = current_second;
                    self.playing = true;
                }
            }
        }
    }

    fn track(&self) -> Track {
        TRACKS[self.track_index()]
    }

    fn track_index(&self) -> usize {
        ((self.current_second / TRACK_ROTATION_SECONDS) as usize) % TRACKS.len()
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
    let track = state.track();

    draw_volume(display, ui_layout, state.volume_percent)?;
    draw_play_pause_button(display, ui_layout.play_button, state.playing)?;
    draw_duration(
        display,
        ui_layout.timer_origin,
        Duration::from_secs(state.remaining_seconds),
        body_style.clone(),
    )?;
    draw_progress_bar(
        display,
        ui_layout.progress,
        state.remaining_seconds,
        state.total_seconds,
    )?;

    Text::with_text_style(
        track.title,
        ui_layout.song_origin,
        song_style,
        centered_top_text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;
    Text::with_text_style(
        track.artist,
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
    fn main_controls_stay_inside_safe_square() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert!(
            ui_layout
                .safe_square
                .contains(ui_layout.play_button.center())
        );
        assert!(ui_layout.safe_square.contains(ui_layout.timer_origin));
        assert!(ui_layout.safe_square.contains(ui_layout.progress.top_left));
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
}

fn draw_play_pause_button<D>(
    display: &mut D,
    rect: Rectangle,
    playing: bool,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let center = rect_visual_center(rect);

    if playing {
        for x_offset in [-21, 5] {
            let bar = Rectangle::new(center + Point::new(x_offset, -32), Size::new(16, 64));
            bar.into_styled(PrimitiveStyle::with_fill(TEXT_PRIMARY))
                .draw(display)
                .map_err(RenderError::Draw)?;
        }
    } else {
        Triangle::new(
            center + Point::new(-16, -30),
            center + Point::new(-16, 30),
            center + Point::new(34, 0),
        )
        .into_styled(PrimitiveStyle::with_fill(TEXT_PRIMARY))
        .draw(display)
        .map_err(RenderError::Draw)?;
    }

    Ok(())
}

fn rect_visual_center(rect: Rectangle) -> Point {
    rect.top_left + Point::new((rect.size.width / 2) as i32, (rect.size.height / 2) as i32)
}
