use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};

use super::{geometry::SCREEN_BOUNDS, screens, style::OLED_BLACK};
use crate::{ActiveScreen, App, RenderError};

impl App {
    pub fn render<D>(&self, display: &mut D) -> Result<(), RenderError<D::Error>>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        Rectangle::new(SCREEN_BOUNDS.top_left, SCREEN_BOUNDS.size)
            .into_styled(PrimitiveStyle::with_fill(OLED_BLACK))
            .draw(display)
            .map_err(RenderError::Draw)?;

        let context = self.ui_context();

        match &self.active_screen {
            ActiveScreen::Launcher(state) => {
                screens::launcher::render(state, display, self.ui_layouts.launcher())
            }
            ActiveScreen::Stopwatch(state) => {
                screens::stopwatch::render(state, context, display, self.ui_layouts.stopwatch())
            }
            ActiveScreen::HifiControl(state) => {
                screens::hifi::render(state, display, self.ui_layouts.hifi())
            }
        }
    }
}
