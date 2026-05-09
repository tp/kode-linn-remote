use core::time::Duration;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use super::style::*;
use crate::RenderError;

const BUTTON_LABEL_OFFSET: Point = Point::new(0, 1);
const TIME_DIGIT_CELL_WIDTH: i32 = 18;
const TIME_COLON_GAP: i32 = 4;
const TIME_COLON_DOT_SIZE: u32 = 3;
const TIME_COLON_TOP_OFFSET: i32 = 15;
const TIME_COLON_BOTTOM_OFFSET: i32 = 27;
const SPINNER_DOT_COUNT: u32 = 8;
pub(super) const DURATION_WIDTH: i32 =
    3 * 2 * TIME_DIGIT_CELL_WIDTH + 2 * (2 * TIME_COLON_GAP + TIME_COLON_DOT_SIZE as i32);

macro_rules! ui_font {
    ($weight:tt) => {
        mplus!(
            2,
            $weight,
            line_height(40),
            true,
            4,
            4,
            kern(' '..='~', ["ff", "ffi", "ffl"])
        )
    };
}
pub(super) use ui_font;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ButtonTone {
    Start,
    Stop,
}

pub(super) fn draw_button<D>(
    display: &mut D,
    rect: Rectangle,
    label: &str,
    active: bool,
    tone: ButtonTone,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let (fill, border) = match (active, tone) {
        (true, ButtonTone::Start) => (ACTION_START, ACTION_START_BORDER),
        (true, ButtonTone::Stop) => (ACTION_STOP, ACTION_STOP_BORDER),
        (false, _) => (ACTION_INACTIVE, ACTION_INACTIVE_BORDER),
    };
    let text = if active { TEXT_PRIMARY } else { TEXT_DISABLED };

    draw_panel(display, rect, BUTTON_RADIUS, fill, border)?;

    let button_font = ui_font!(BOLD);
    let character_style = BitmapFontStyleBuilder::new()
        .text_color(text)
        .background_color(fill)
        .font(&button_font)
        .build();
    let text_style = TextStyleBuilder::new()
        .alignment(Alignment::Center)
        .baseline(Baseline::Middle)
        .build();

    Text::with_text_style(
        label,
        rect.center() + BUTTON_LABEL_OFFSET,
        character_style,
        text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;

    Ok(())
}

pub(super) fn draw_panel<D>(
    display: &mut D,
    rect: Rectangle,
    radius: u32,
    fill: Rgb565,
    stroke: Rgb565,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    RoundedRectangle::with_equal_corners(rect, Size::new(radius, radius))
        .into_styled(
            PrimitiveStyleBuilder::new()
                .fill_color(fill)
                .stroke_color(stroke)
                .stroke_width(1)
                .build(),
        )
        .draw(display)
        .map_err(RenderError::Draw)
}

pub(super) fn draw_progress_bar<D>(
    display: &mut D,
    track: Rectangle,
    remaining_seconds: u64,
    total_seconds: u64,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    draw_panel(display, track, 8, ACTION_INACTIVE, ACTION_INACTIVE_BORDER)?;

    let filled_width = if total_seconds == 0 {
        0
    } else {
        ((track.size.width as u64 * remaining_seconds) / total_seconds) as u32
    };
    if filled_width == 0 {
        return Ok(());
    }

    let fill = Rectangle::new(track.top_left, Size::new(filled_width, track.size.height));
    draw_panel(display, fill, 8, ACTION_START, ACTION_START_BORDER)
}

pub(super) fn draw_duration<D>(
    display: &mut D,
    origin: Point,
    duration: Duration,
    character_style: impl embedded_graphics::text::renderer::TextRenderer<Color = Rgb565> + Clone,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let (hours, minutes, seconds) = duration_parts(duration);
    let separator_width = 2 * TIME_COLON_GAP + TIME_COLON_DOT_SIZE as i32;
    let group_width = 2 * TIME_DIGIT_CELL_WIDTH;

    draw_two_digit_group(display, origin, hours, character_style.clone())?;

    let first_colon_x = origin.x + group_width + TIME_COLON_GAP;
    draw_time_colon(display, Point::new(first_colon_x, origin.y), TEXT_SECONDARY)?;

    let minutes_x = origin.x + group_width + separator_width;
    draw_two_digit_group(
        display,
        Point::new(minutes_x, origin.y),
        minutes,
        character_style.clone(),
    )?;

    let second_colon_x = minutes_x + group_width + TIME_COLON_GAP;
    draw_time_colon(
        display,
        Point::new(second_colon_x, origin.y),
        TEXT_SECONDARY,
    )?;

    let seconds_x = minutes_x + group_width + separator_width;
    draw_two_digit_group(
        display,
        Point::new(seconds_x, origin.y),
        seconds,
        character_style,
    )
}

pub(super) fn draw_spinner<D>(
    display: &mut D,
    center: Point,
    phase: u8,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    const OFFSETS: [Point; SPINNER_DOT_COUNT as usize] = [
        Point::new(0, -32),
        Point::new(23, -23),
        Point::new(32, 0),
        Point::new(23, 23),
        Point::new(0, 32),
        Point::new(-23, 23),
        Point::new(-32, 0),
        Point::new(-23, -23),
    ];
    const COLORS: [Rgb565; SPINNER_DOT_COUNT as usize] = [
        Rgb565::new(20, 63, 31),
        Rgb565::new(12, 55, 31),
        Rgb565::new(7, 44, 30),
        Rgb565::new(5, 32, 25),
        Rgb565::new(4, 22, 20),
        Rgb565::new(5, 32, 25),
        Rgb565::new(7, 44, 30),
        Rgb565::new(12, 55, 31),
    ];

    let phase = phase as usize % OFFSETS.len();
    for index in 0..OFFSETS.len() {
        let color = COLORS[(index + phase) % COLORS.len()];
        Circle::with_center(center + OFFSETS[index], 18)
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(display)
            .map_err(RenderError::Draw)?;
    }

    Ok(())
}

fn duration_parts(duration: Duration) -> (u8, u8, u8) {
    let total_seconds = duration.as_secs();
    let seconds = (total_seconds % 60) as u8;
    let total_minutes = total_seconds / 60;
    let minutes = (total_minutes % 60) as u8;
    let hours = ((total_minutes / 60) % 100) as u8;

    (hours, minutes, seconds)
}

fn draw_two_digit_group<D>(
    display: &mut D,
    origin: Point,
    value: u8,
    character_style: impl embedded_graphics::text::renderer::TextRenderer<Color = Rgb565> + Clone,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let top_text_style = TextStyleBuilder::new().baseline(Baseline::Top).build();
    let tens = digit_text(value / 10);
    let ones = digit_text(value % 10);

    for (digit, x_offset) in [(tens, 0), (ones, TIME_DIGIT_CELL_WIDTH)] {
        Text::with_text_style(
            digit,
            origin + Point::new(x_offset, 0),
            character_style.clone(),
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;
    }

    Ok(())
}

fn digit_text(digit: u8) -> &'static str {
    match digit {
        0 => "0",
        1 => "1",
        2 => "2",
        3 => "3",
        4 => "4",
        5 => "5",
        6 => "6",
        7 => "7",
        8 => "8",
        9 => "9",
        _ => unreachable!("digit_text expects a single decimal digit"),
    }
}

fn draw_time_colon<D>(
    display: &mut D,
    origin: Point,
    color: Rgb565,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let style = PrimitiveStyle::with_fill(color);
    for y_offset in [TIME_COLON_TOP_OFFSET, TIME_COLON_BOTTOM_OFFSET] {
        RoundedRectangle::with_equal_corners(
            Rectangle::new(
                origin + Point::new(0, y_offset),
                Size::new(TIME_COLON_DOT_SIZE, TIME_COLON_DOT_SIZE),
            ),
            Size::new(2, 2),
        )
        .into_styled(style)
        .draw(display)
        .map_err(RenderError::Draw)?;
    }

    Ok(())
}
