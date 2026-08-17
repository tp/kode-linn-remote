use embedded_graphics::{pixelcolor::Rgb565, prelude::*};

use super::{RenderSession, components::draw_focus_ring, screens};
use crate::{ActiveScreen, App, RenderError};

impl App {
    /// Draws the current screen onto `display`.
    ///
    /// `session` carries what this particular target already shows, and must
    /// be the same one every time this `App` is drawn to this target — that is
    /// what lets unchanged widgets be skipped. A different target needs a
    /// different session.
    ///
    /// On failure the session forgets the screen, so the next attempt repaints
    /// it in full rather than skipping widgets that never made it to the panel.
    pub fn render<D>(
        &mut self,
        display: &mut D,
        scratch: &mut [Rgb565],
        session: &mut RenderSession,
    ) -> Result<(), RenderError<D::Error>>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let screen = self.active_screen.screen();
        let focused = self.focused_rect();

        // Clear on a screen change so the new screen inherits no pixels, and
        // on a focus move so the ring leaves no outline behind. A uniform fill
        // is far cheaper than a full re-render and only happens on those.
        if session.begin_frame(screen, focused)
            && let Err(error) = display.clear(Rgb565::BLACK)
        {
            session.abandon_frame(screen);
            return Err(RenderError::Draw(error));
        }

        let context = self.ui_context();
        let result = match &mut self.active_screen {
            ActiveScreen::Launcher(_) => screens::launcher::render(
                session.launcher(),
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
            ActiveScreen::HifiControl(state) => screens::hifi::render(
                state,
                session.hifi(),
                display,
                scratch,
                self.ui_layouts.hifi(),
            ),
        };

        if let Err(error) = result {
            session.abandon_frame(screen);
            return Err(error);
        }

        // Drawn last so it sits above the screen's own pixels.
        if let Some(rect) = focused
            && let Err(error) = draw_focus_ring(display, rect)
        {
            session.abandon_frame(screen);
            return Err(error);
        }

        Ok(())
    }
}
