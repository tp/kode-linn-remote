//! Per-target render state.
//!
//! A smart-skipping renderer has to remember what it last drew, or it cannot
//! know what to skip. The question is *where* that memory lives.
//!
//! It used to live inside screen state, alongside the app's own data. But it
//! does not describe the app — it describes the relationship between the app
//! and **one** display. Keeping it in the app meant three things went wrong:
//!
//! - Rendering the same `App` to a second target let the first target's memory
//!   suppress drawing on the second, which came out blank.
//! - A frame that failed part-way had already updated some of the memory, so
//!   the next frame skipped widgets that were never actually drawn.
//! - Anything that disturbed the panel from outside — a driver reset, a
//!   brightness transition, the focus ring moving — had to reach into screen
//!   state to correct it.
//!
//! [`RenderSession`] holds that memory next to the target it describes. One
//! display, one session. Two targets, two sessions, and neither can lie about
//! the other.

use embedded_graphics::primitives::Rectangle;

use super::{SCREEN_BOUNDS, ScreenLayouts, screens};
use crate::Screen;

/// What one render target currently shows.
///
/// Create one per display or framebuffer and pass it to [`crate::App::render`]
/// alongside that target, for the target's whole lifetime.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RenderSession {
    /// Layouts are screen-fixed, so the session keeps its own copy rather than
    /// making every caller thread them through the invalidation API.
    layouts: ScreenLayouts,
    last_screen: Option<Screen>,
    /// Bounds of the focus ring as last drawn. The ring is an overlay, so when
    /// it moves the old outline has to be erased — which the per-screen caches
    /// know nothing about, since nothing they track has changed.
    last_focus: Option<Rectangle>,
    launcher: screens::launcher::RenderCache,
    hifi: screens::hifi::RenderCache,
}

impl RenderSession {
    #[must_use]
    pub fn new() -> Self {
        let layouts = ScreenLayouts::new(SCREEN_BOUNDS);
        Self {
            hifi: screens::hifi::RenderCache::new(layouts.hifi()),
            layouts,
            last_screen: None,
            last_focus: None,
            launcher: screens::launcher::RenderCache::default(),
        }
    }

    /// Forgets everything about this target. The next frame repaints in full.
    ///
    /// Use after anything that changes the panel behind the renderer's back:
    /// a driver reset, a power or brightness transition, or a framebuffer
    /// being swapped or reallocated.
    pub fn invalidate_all(&mut self) {
        self.last_screen = None;
        self.last_focus = None;
        self.launcher = screens::launcher::RenderCache::default();
        self.hifi.reset(self.layouts.hifi());
    }

    /// Forgets one screen, leaving the others intact.
    pub fn invalidate_screen(&mut self, screen: Screen) {
        match screen {
            Screen::Launcher => self.launcher = screens::launcher::RenderCache::default(),
            Screen::Stopwatch => {}
            Screen::HifiControl => self.hifi.reset(self.layouts.hifi()),
        }
        if self.last_screen == Some(screen) {
            self.last_screen = None;
            self.last_focus = None;
        }
    }

    /// Records that something cleared the panel outside `App::render`.
    ///
    /// Equivalent to [`Self::invalidate_all`], named for the situation so call
    /// sites read as what happened rather than what to do about it.
    pub fn note_external_clear(&mut self) {
        self.invalidate_all();
    }

    /// Whether the target needs clearing before this frame, and the bookkeeping
    /// that goes with it.
    ///
    /// A clear is needed when the screen changed, or when the focus ring moved
    /// and would otherwise leave its previous outline behind.
    pub(crate) fn begin_frame(&mut self, screen: Screen, focus: Option<Rectangle>) -> bool {
        let screen_changed = self.last_screen != Some(screen);
        let focus_moved = self.last_focus != focus;

        if screen_changed || focus_moved {
            self.invalidate_screen(screen);
        }

        self.last_screen = Some(screen);
        self.last_focus = focus;
        screen_changed || focus_moved
    }

    /// Throws away what this frame believed it drew.
    ///
    /// Called when a frame fails part-way. The conservative choice: rather than
    /// unpicking which draws landed before the error, forget the screen
    /// entirely so the retry paints it fresh. A redundant repaint is cheap; a
    /// cache that claims pixels exist when they do not is not recoverable.
    pub(crate) fn abandon_frame(&mut self, screen: Screen) {
        self.invalidate_screen(screen);
        self.last_screen = None;
        self.last_focus = None;
    }

    pub(crate) fn launcher(&mut self) -> &mut screens::launcher::RenderCache {
        &mut self.launcher
    }

    pub(crate) fn hifi(&mut self) -> &mut screens::hifi::RenderCache {
        &mut self.hifi
    }
}

impl Default for RenderSession {
    fn default() -> Self {
        Self::new()
    }
}
