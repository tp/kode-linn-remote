use core::time::Duration;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Line, PrimitiveStyle, Rectangle, RoundedRectangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use super::{aa, style::*};
use crate::RenderError;

const BUTTON_LABEL_OFFSET: Point = Point::new(0, 1);
const TIME_DIGIT_CELL_WIDTH: i32 = 18;
const TIME_COLON_GAP: i32 = 4;
const TIME_COLON_DOT_SIZE: u32 = 3;
const TIME_COLON_TOP_OFFSET: i32 = 15;
const TIME_COLON_BOTTOM_OFFSET: i32 = 27;
const DURATION_HEIGHT: u32 = 40;
const SPINNER_CLEAR_SIZE: u32 = 96;
const SPINNER_DOT_COUNT: u32 = 8;
const WIFI_ICON_STROKE: u32 = 4;
const NETWORK_BLOCKED_ICON_DIAMETER: u32 = 24;
pub(super) const DURATION_WIDTH: i32 =
    3 * 2 * TIME_DIGIT_CELL_WIDTH + 2 * (2 * TIME_COLON_GAP + TIME_COLON_DOT_SIZE as i32);

macro_rules! ui_font {
    ($weight:tt) => {
        mplus!(
            2,
            $weight,
            line_height(40),
            false,
            4,
            8,
            kern(' '..='~', ["ff", "ffi", "ffl"]),
            ["Ä", "Ö", "Ü", "ä", "ö", "ü", "ß", "ẞ", "‘", "’", "‚", "“", "”", "„", "´"]
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

/// Outlines the control the D-pad is on.
///
/// Drawn as an overlay after the screen paints, so it needs no cooperation
/// from each screen's dirty-region cache. `App` forces a full repaint when the
/// ring moves, which is what clears the previous outline.
pub(super) fn draw_focus_ring<D>(
    display: &mut D,
    rect: Rectangle,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let inset = FOCUS_RING_INSET;
    let top_left = Point::new(rect.top_left.x - inset, rect.top_left.y - inset);
    let size = Size::new(
        rect.size.width.saturating_add((inset * 2) as u32),
        rect.size.height.saturating_add((inset * 2) as u32),
    );

    aa::rounded_rect_outline(
        display,
        Rectangle::new(top_left, size),
        FOCUS_RING_RADIUS,
        FOCUS_RING,
        FOCUS_RING_STROKE,
        OLED_BLACK,
    )
    .map_err(RenderError::Draw)
}

pub(super) fn clear_rect<D>(display: &mut D, rect: Rectangle) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    rect.into_styled(PrimitiveStyle::with_fill(OLED_BLACK))
        .draw(display)
        .map_err(RenderError::Draw)
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
    // Covers every pixel of `rect`, blending the corner arcs down to the OLED
    // background, so no separate clear is needed first. Only correct where the
    // panel really does sit on the background -- see [`draw_panel_over`].
    aa::rounded_rect(display, rect, radius, fill, stroke, 1, OLED_BLACK).map_err(RenderError::Draw)
}

/// A panel over something other than the OLED background.
///
/// [`draw_panel`] writes every pixel of its bounding box, so the corners it
/// rounds away are painted with the backdrop it was told about. Told "black"
/// while sitting on artwork, it paints black wedges into the corners and undoes
/// the rounding. `backdrop_at` supplies what is really underneath.
pub(super) fn draw_panel_over<D, F>(
    display: &mut D,
    rect: Rectangle,
    radius: u32,
    fill: Rgb565,
    stroke: Rgb565,
    backdrop_at: F,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
    F: Fn(Point) -> Rgb565,
{
    aa::rounded_rect_over(display, rect, radius, fill, stroke, 1, backdrop_at)
        .map_err(RenderError::Draw)
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
    clear_rect(
        display,
        Rectangle::new(origin, Size::new(DURATION_WIDTH as u32, DURATION_HEIGHT)),
    )?;

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
    clear_spinner(display, center)?;
    draw_spinner_dots(display, center, phase).map_err(RenderError::Draw)
}

/// Paints just the eight dots — no clear. Each dot covers the same pixels as
/// the previous frame's dot at the same position, so for a continuously
/// animated spinner the clear is unnecessary work and visible flicker.
pub(super) fn draw_spinner_dots<D>(
    display: &mut D,
    center: Point,
    phase: u8,
) -> Result<(), D::Error>
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
        aa::circle(display, center + OFFSETS[index], 18, color, OLED_BLACK)?;
    }

    Ok(())
}

pub(super) fn clear_spinner<D>(display: &mut D, center: Point) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    clear_rect(
        display,
        Rectangle::new(
            center
                - Point::new(
                    (SPINNER_CLEAR_SIZE / 2) as i32,
                    (SPINNER_CLEAR_SIZE / 2) as i32,
                ),
            Size::new(SPINNER_CLEAR_SIZE, SPINNER_CLEAR_SIZE),
        ),
    )
}

pub(super) fn draw_wifi_icon<D>(display: &mut D, center: Point) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let origin = center + Point::new(0, 19);
    for (diameter, start_deg, sweep_deg) in [(72, 215, 110), (48, 220, 100), (24, 228, 84)] {
        aa::arc(
            display,
            aa::ArcSpec {
                center: origin,
                diameter,
                stroke_width: WIFI_ICON_STROKE,
                start_deg,
                sweep_deg,
            },
            TEXT_PRIMARY,
            OLED_BLACK,
        )
        .map_err(RenderError::Draw)?;
    }

    aa::circle(
        display,
        center + Point::new(0, 31),
        8,
        TEXT_PRIMARY,
        OLED_BLACK,
    )
    .map_err(RenderError::Draw)
}

pub(super) fn draw_network_blocked_icon<D>(
    display: &mut D,
    center: Point,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    aa::circle_outline(
        display,
        center,
        NETWORK_BLOCKED_ICON_DIAMETER,
        TEXT_SECONDARY,
        2,
        OLED_BLACK,
    )
    .map_err(RenderError::Draw)?;

    Line::new(center + Point::new(-8, 8), center + Point::new(8, -8))
        .into_styled(PrimitiveStyle::with_stroke(TEXT_SECONDARY, 2))
        .draw(display)
        .map_err(RenderError::Draw)
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
