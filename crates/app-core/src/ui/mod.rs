mod aa;
mod components;
pub(crate) mod focus;
mod geometry;
mod painter;
mod render;
pub(crate) mod screens;
mod session;
mod style;
mod widget;

pub use painter::RECOMMENDED_SCRATCH_PIXELS;
pub use session::RenderSession;

use embedded_graphics::primitives::Rectangle;
pub(crate) use geometry::SCREEN_BOUNDS;

use crate::NetworkStatus;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct AppContext {
    pub(crate) network_status: NetworkStatus,
    pub(crate) interaction_count: u32,
    pub(crate) uptime_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScreenLayouts {
    launcher: screens::launcher::Layout,
    stopwatch: screens::stopwatch::Layout,
    hifi: screens::hifi::Layout,
}

impl ScreenLayouts {
    pub(crate) fn new(bounds: Rectangle) -> Self {
        Self {
            launcher: screens::launcher::layout(bounds),
            stopwatch: screens::stopwatch::layout(bounds),
            hifi: screens::hifi::layout(bounds),
        }
    }

    pub(crate) const fn launcher(&self) -> &screens::launcher::Layout {
        &self.launcher
    }

    pub(crate) const fn stopwatch(&self) -> &screens::stopwatch::Layout {
        &self.stopwatch
    }

    pub(crate) const fn hifi(&self) -> &screens::hifi::Layout {
        &self.hifi
    }
}

#[cfg(test)]
pub(crate) fn stopwatch_button_centers() -> (
    embedded_graphics::geometry::Point,
    embedded_graphics::geometry::Point,
) {
    screens::stopwatch::button_centers(SCREEN_BOUNDS)
}

#[cfg(test)]
pub(crate) fn launcher_button_centers() -> (
    embedded_graphics::geometry::Point,
    embedded_graphics::geometry::Point,
) {
    screens::launcher::button_centers(SCREEN_BOUNDS)
}

#[cfg(test)]
pub(crate) fn hifi_artwork_center() -> embedded_graphics::geometry::Point {
    screens::hifi::artwork_center(SCREEN_BOUNDS)
}

#[cfg(test)]
pub(crate) fn hifi_tile_center(slot: usize) -> embedded_graphics::geometry::Point {
    screens::hifi::tile_center(SCREEN_BOUNDS, slot)
}
