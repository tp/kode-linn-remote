use embedded_graphics::{pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

use super::style::OLED_BLACK;

/// Draws into a `DrawTarget<Color = Rgb565>`.
///
/// Widgets always draw at *absolute* coordinates. The painter handles any
/// translation needed to land pixels in a scratch buffer.
///
/// Widgets that take `previous_*` fields are responsible for short-circuiting
/// inside `draw` when nothing has changed (smart-skip).
///
/// The `A` type parameter is the per-screen action enum, currently unused in
/// the trait surface (each screen does hit-testing via a standalone function
/// against `Layout` rectangles). Keeping it generic preserves the option to
/// add `fn hit(&self, point: Point) -> Option<A>` later without ripping the
/// trait apart.
pub(super) trait Widget<A> {
    fn bounds(&self) -> Rectangle;

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>;

    /// Opt into the painter's text scratch buffer. Default: false.
    fn use_scratch(&self) -> bool {
        false
    }

    /// Whether the painter should call `draw` at all this frame.
    ///
    /// Default `true`. Smart-skipping widgets — anything that would early-
    /// return inside `draw()` because state matches the previous frame —
    /// **must** override this to `false` in that case. Otherwise the
    /// painter's scratch path will dutifully clear the buffer and blit it
    /// (a black band) over the existing pixels, wiping the previously-
    /// rendered content.
    fn should_draw(&self) -> bool {
        true
    }
}

/// A fixed rectangle that owns mutually-exclusive widget content over time.
///
/// When `previous_kind != current_kind`, [`Slot::clear_if_kind_changed`] paints
/// the bounds black so the new widget never inherits ghost pixels from the old
/// one. Used for places like the play/pause area, which switches between a
/// spinner and a play button depending on state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Slot {
    pub bounds: Rectangle,
    pub previous_kind: Option<u8>,
}

impl Slot {
    pub(super) const fn new(bounds: Rectangle) -> Self {
        Self {
            bounds,
            previous_kind: None,
        }
    }

    /// Paints the slot's bounds black if the slot's content kind has changed
    /// since the last frame. Returns the new kind for the caller to record.
    pub(super) fn clear_if_kind_changed<D>(
        &self,
        display: &mut D,
        new_kind: u8,
    ) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        if self.previous_kind == Some(new_kind) {
            return Ok(());
        }
        display.fill_solid(&self.bounds, OLED_BLACK)
    }
}
