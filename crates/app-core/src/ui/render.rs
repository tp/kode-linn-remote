use core::fmt::Write as _;
use core::time::Duration;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{
        Circle, Line, PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle,
    },
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use super::layout::{SCREEN_BOUNDS, layout};
use super::style::*;
use crate::{App, NetworkStatus, RenderError};

const BUTTON_LABEL_OFFSET: Point = Point::new(0, 1);
const NETWORK_ICON_DIAMETER: u32 = 22;
const TIME_DIGIT_CELL_WIDTH: i32 = 18;
const TIME_COLON_GAP: i32 = 4;
const TIME_COLON_DOT_SIZE: u32 = 3;
const TIME_COLON_TOP_OFFSET: i32 = 15;
const TIME_COLON_BOTTOM_OFFSET: i32 = 27;

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

impl App {
    pub fn render<D>(&self, display: &mut D) -> Result<(), RenderError<D::Error>>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let ui_layout = layout(SCREEN_BOUNDS);
        let title_font = ui_font!(BOLD);
        let body_font = ui_font!(500);
        let top_text_style = TextStyleBuilder::new().baseline(Baseline::Top).build();

        SCREEN_BOUNDS
            .into_styled(PrimitiveStyle::with_fill(OLED_BLACK))
            .draw(display)
            .map_err(RenderError::Draw)?;

        draw_panel(
            display,
            ui_layout.header.panel,
            CARD_RADIUS,
            SURFACE,
            SURFACE_BORDER,
        )?;

        let title_style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_PRIMARY)
            .background_color(SURFACE)
            .font(&title_font)
            .build();
        Text::with_text_style(
            "ESP32-C6 Home Tools",
            ui_layout.header.title_origin,
            title_style,
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        draw_button(
            display,
            ui_layout.buttons.start,
            "START",
            !self.stopwatch_running,
            ButtonTone::Start,
        )?;
        draw_button(
            display,
            ui_layout.buttons.stop,
            "STOP",
            self.stopwatch_running,
            ButtonTone::Stop,
        )?;

        let body_style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_SECONDARY)
            .background_color(OLED_BLACK)
            .font(&body_font)
            .build();

        Text::with_text_style(
            "ideal",
            ui_layout.info.ideal.label.top_left,
            body_style.clone(),
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;
        draw_ideal_duration(
            display,
            ui_layout.info.ideal.value.top_left,
            Duration::from_secs(self.stopwatch_seconds),
            body_style.clone(),
        )?;

        let mut stopwatch = heapless::String::<32>::new();
        write!(stopwatch, "stopwatch: {}s", self.stopwatch_seconds)
            .map_err(|_| RenderError::TextFormat)?;
        Text::with_text_style(
            &stopwatch,
            ui_layout.info.stopwatch.top_left,
            body_style.clone(),
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        let network_text_origin = if self.network_status == NetworkStatus::Online {
            ui_layout.info.network.text_without_icon_origin
        } else {
            draw_network_unavailable_icon(display, ui_layout.info.network.icon_center)?;
            ui_layout.info.network.text_with_icon_origin
        };
        Text::with_text_style(
            network_text(self.network_status),
            network_text_origin,
            body_style.clone(),
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        let mut interactions = heapless::String::<32>::new();
        write!(interactions, "interactions: {}", self.interaction_count)
            .map_err(|_| RenderError::TextFormat)?;
        Text::with_text_style(
            &interactions,
            ui_layout.info.interactions.top_left,
            body_style,
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        Ok(())
    }
}

fn draw_ideal_duration<D>(
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ButtonTone {
    Start,
    Stop,
}

fn network_text(status: NetworkStatus) -> &'static str {
    match status {
        NetworkStatus::Offline => "network: offline",
        NetworkStatus::Connecting => "network: connecting",
        NetworkStatus::Online => "network: online",
    }
}

fn draw_network_unavailable_icon<D>(
    display: &mut D,
    center: Point,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    Circle::with_center(center, NETWORK_ICON_DIAMETER)
        .into_styled(PrimitiveStyle::with_stroke(TEXT_SECONDARY, 2))
        .draw(display)
        .map_err(RenderError::Draw)?;

    Line::new(center + Point::new(-8, 8), center + Point::new(8, -8))
        .into_styled(PrimitiveStyle::with_stroke(TEXT_SECONDARY, 2))
        .draw(display)
        .map_err(RenderError::Draw)
}

fn draw_button<D>(
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

fn draw_panel<D>(
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
