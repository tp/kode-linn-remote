use core::fmt::Write as _;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use crate::{App, DISPLAY_SIZE, NetworkStatus, RenderError};

pub(crate) const START_BUTTON_HIT_BOUNDS: Rectangle =
    Rectangle::new(Point::new(44, 140), Size::new(170, 72));
pub(crate) const STOP_BUTTON_HIT_BOUNDS: Rectangle =
    Rectangle::new(Point::new(252, 140), Size::new(170, 72));

const HEADER_PANEL: Rectangle = Rectangle::new(Point::new(24, 24), Size::new(418, 92));
const CARD_RADIUS: u32 = 18;
const BUTTON_RADIUS: u32 = 18;

const TITLE_ORIGIN: Point = Point::new(44, 48);
const STOPWATCH_TEXT_ORIGIN: Point = Point::new(44, 244);
const NETWORK_TEXT_ORIGIN: Point = Point::new(44, 312);
const INTERACTIONS_TEXT_ORIGIN: Point = Point::new(44, 352);
const BUTTON_LABEL_OFFSET: Point = Point::new(0, 1);

const OLED_BLACK: Rgb565 = Rgb565::BLACK;
const SURFACE: Rgb565 = Rgb565::new(1, 2, 3);
const SURFACE_BORDER: Rgb565 = Rgb565::new(5, 9, 11);
const TEXT_PRIMARY: Rgb565 = Rgb565::WHITE;
const TEXT_SECONDARY: Rgb565 = TEXT_PRIMARY;
const TEXT_DISABLED: Rgb565 = Rgb565::new(10, 18, 20);
const ACTION_START: Rgb565 = Rgb565::new(1, 30, 13);
const ACTION_START_BORDER: Rgb565 = Rgb565::new(7, 42, 20);
const ACTION_STOP: Rgb565 = Rgb565::new(24, 4, 6);
const ACTION_STOP_BORDER: Rgb565 = Rgb565::new(31, 13, 14);
const ACTION_INACTIVE: Rgb565 = Rgb565::new(3, 4, 6);
const ACTION_INACTIVE_BORDER: Rgb565 = Rgb565::new(7, 10, 13);

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
        let title_font = ui_font!(BOLD);
        let body_font = ui_font!(500);
        let top_text_style = TextStyleBuilder::new().baseline(Baseline::Top).build();

        Rectangle::new(Point::zero(), DISPLAY_SIZE)
            .into_styled(PrimitiveStyle::with_fill(OLED_BLACK))
            .draw(display)
            .map_err(RenderError::Draw)?;

        draw_panel(display, HEADER_PANEL, CARD_RADIUS, SURFACE, SURFACE_BORDER)?;

        let title_style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_PRIMARY)
            .background_color(SURFACE)
            .font(&title_font)
            .build();
        Text::with_text_style(
            "ESP32-C6 Home Tools",
            TITLE_ORIGIN,
            title_style,
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        draw_button(
            display,
            START_BUTTON_HIT_BOUNDS,
            "START",
            !self.stopwatch_running,
            ButtonTone::Start,
        )?;
        draw_button(
            display,
            STOP_BUTTON_HIT_BOUNDS,
            "STOP",
            self.stopwatch_running,
            ButtonTone::Stop,
        )?;

        let body_style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_SECONDARY)
            .background_color(OLED_BLACK)
            .font(&body_font)
            .build();

        let mut stopwatch = heapless::String::<32>::new();
        write!(stopwatch, "stopwatch: {}s", self.stopwatch_seconds)
            .map_err(|_| RenderError::TextFormat)?;
        Text::with_text_style(
            &stopwatch,
            STOPWATCH_TEXT_ORIGIN,
            body_style.clone(),
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        Text::with_text_style(
            network_text(self.network_status),
            NETWORK_TEXT_ORIGIN,
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
            INTERACTIONS_TEXT_ORIGIN,
            body_style,
            top_text_style,
        )
        .draw(display)
        .map_err(RenderError::Draw)?;

        Ok(())
    }
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
