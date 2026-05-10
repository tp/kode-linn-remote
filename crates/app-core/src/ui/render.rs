use embedded_graphics::{pixelcolor::Rgb565, prelude::*};

use super::screens;
use crate::{ActiveScreen, App, RenderError};

impl App {
    pub fn render<D>(&self, display: &mut D) -> Result<(), RenderError<D::Error>>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let context = self.ui_context();

        match &self.active_screen {
            ActiveScreen::Launcher(state) => {
                screens::launcher::render(state, context, display, self.ui_layouts.launcher())
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
