use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::Rectangle,
    text::{Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use crate::RenderError;

use super::super::{
    Navigation,
    components::{ButtonTone, draw_button, ui_font},
    geometry::horizontal_pair,
    style::{OLED_BLACK, TEXT_PRIMARY},
};

const CONTENT_INSET: i32 = 44;
const TITLE_Y: i32 = 64;
const BUTTON_Y: i32 = 154;
const BUTTON_HEIGHT: u32 = 150;
const BUTTON_GAP: i32 = 22;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Layout {
    pub(super) title_origin: Point,
    pub(super) stopwatch_button: Rectangle,
    pub(super) hifi_button: Rectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct State;

impl State {
    pub(crate) const fn new() -> Self {
        Self
    }
}

pub(crate) fn layout(bounds: Rectangle) -> Layout {
    let content_left = bounds.top_left.x + CONTENT_INSET;
    let content_width = bounds.size.width.saturating_sub((CONTENT_INSET * 2) as u32);
    let button_y = bounds.top_left.y + BUTTON_Y;
    let (stopwatch_button, hifi_button) = horizontal_pair(
        content_left,
        button_y,
        content_width,
        BUTTON_HEIGHT,
        BUTTON_GAP,
    );

    Layout {
        title_origin: Point::new(content_left, bounds.top_left.y + TITLE_Y),
        stopwatch_button,
        hifi_button,
    }
}

pub(crate) fn hit_test(layout: &Layout, point: Point) -> Option<Navigation> {
    if layout.stopwatch_button.contains(point) {
        Some(Navigation::Stopwatch)
    } else if layout.hifi_button.contains(point) {
        Some(Navigation::HifiControl)
    } else {
        None
    }
}

pub(crate) fn render<D>(
    _state: &State,
    display: &mut D,
    ui_layout: &Layout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let title_font = ui_font!(BOLD);
    let title_style = BitmapFontStyleBuilder::new()
        .text_color(TEXT_PRIMARY)
        .background_color(OLED_BLACK)
        .font(&title_font)
        .build();
    let top_text_style = TextStyleBuilder::new().baseline(Baseline::Top).build();

    Text::with_text_style(
        "Launcher",
        ui_layout.title_origin,
        title_style,
        top_text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;

    draw_button(
        display,
        ui_layout.stopwatch_button,
        "STOP WATCH",
        true,
        ButtonTone::Start,
    )?;
    draw_button(
        display,
        ui_layout.hifi_button,
        "HIFI",
        true,
        ButtonTone::Stop,
    )
}

#[cfg(test)]
pub(crate) fn button_centers(
    bounds: Rectangle,
) -> (
    embedded_graphics::geometry::Point,
    embedded_graphics::geometry::Point,
) {
    let ui_layout = layout(bounds);

    (
        ui_layout.stopwatch_button.center(),
        ui_layout.hifi_button.center(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::SCREEN_BOUNDS;

    #[test]
    fn hit_tests_app_buttons() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_eq!(
            hit_test(&ui_layout, ui_layout.stopwatch_button.center()),
            Some(Navigation::Stopwatch)
        );
        assert_eq!(
            hit_test(&ui_layout, ui_layout.hifi_button.center()),
            Some(Navigation::HifiControl)
        );
    }
}
