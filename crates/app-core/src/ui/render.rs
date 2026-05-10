use embedded_graphics::{pixelcolor::Rgb565, prelude::*};

use super::screens;
use crate::{ActiveScreen, App, RenderError};

impl App {
    pub fn render<D>(
        &mut self,
        display: &mut D,
        scratch: &mut [Rgb565],
    ) -> Result<(), RenderError<D::Error>>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        // Clear when transitioning to a different screen so the new screen
        // doesn't inherit pixels from the old one. This is just a uniform
        // fill (~22 ms on the panel) — far cheaper than a full re-render —
        // and only happens on transitions.
        let current_screen = self.active_screen.screen();
        if self.last_rendered_screen != Some(current_screen) {
            display.clear(Rgb565::BLACK).map_err(RenderError::Draw)?;
            self.last_rendered_screen = Some(current_screen);
        }

        let context = self.ui_context();

        match &mut self.active_screen {
            ActiveScreen::Launcher(state) => screens::launcher::render(
                state,
                context,
                display,
                scratch,
                self.ui_layouts.launcher(),
            ),
            ActiveScreen::Stopwatch(state) => screens::stopwatch::render(
                state,
                context,
                display,
                scratch,
                self.ui_layouts.stopwatch(),
            ),
            ActiveScreen::HifiControl(state) => {
                screens::hifi::render(state, display, scratch, self.ui_layouts.hifi())
            }
        }
    }
}
