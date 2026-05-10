use embedded_graphics::{
    draw_target::DrawTargetExt, pixelcolor::Rgb565, prelude::*, primitives::Rectangle,
};
use embedded_graphics_framebuf::{FrameBuf, backends::FrameBufferBackend};

use super::{style::OLED_BLACK, widget::Widget};

/// Recommended scratch capacity for screens that use text widgets. Sized for
/// the widest text band in any screen (~378 px) with a 40 px line height,
/// rounded up for slack: 384 * 40 = 15,360 px = 30,720 bytes.
pub const RECOMMENDED_SCRATCH_PIXELS: usize = 384 * 40;

/// Wraps a display + a shared scratch buffer. Dispatches widgets, enforces the
/// 2-px panel-alignment convention, and routes text widgets through the
/// scratch buffer for flicker-free composited blits.
pub(super) struct Painter<'a, D> {
    display: &'a mut D,
    scratch: &'a mut [Rgb565],
}

impl<'a, D> Painter<'a, D>
where
    D: DrawTarget<Color = Rgb565>,
{
    pub(super) fn new(display: &'a mut D, scratch: &'a mut [Rgb565]) -> Self {
        Self { display, scratch }
    }

    pub(super) fn display(&mut self) -> &mut D {
        self.display
    }

    pub(super) fn draw<A, W: Widget<A>>(&mut self, widget: &W) -> Result<(), D::Error> {
        if !widget.should_draw() {
            return Ok(());
        }
        let bounds = widget.bounds();

        if !widget.use_scratch() {
            // Non-scratch widgets compose via embedded-graphics primitives,
            // which go through the display's per-primitive alignment path.
            // No bounds-alignment requirement.
            return widget.draw(self.display);
        }

        // Scratch widgets are blitted via a single fill_contiguous call sized
        // to bounds — the panel needs even X/Y/W/H, otherwise the CO5300's
        // 2-row write window expansion shifts edge pixels.
        debug_assert!(
            is_two_aligned(bounds),
            "scratch widget bounds must be 2-px aligned: {bounds:?}"
        );

        let pixels = bounds.size.width as usize * bounds.size.height as usize;
        debug_assert!(
            pixels <= self.scratch.len(),
            "widget bounds {bounds:?} exceed scratch capacity ({} > {})",
            pixels,
            self.scratch.len(),
        );
        if pixels == 0 || pixels > self.scratch.len() {
            return widget.draw(self.display);
        }

        let backend = SliceBackend(&mut self.scratch[..pixels]);
        let mut framebuf =
            FrameBuf::new(backend, bounds.size.width as usize, bounds.size.height as usize);
        // FrameBuf is Infallible, so the inner draw calls cannot fail.
        let _ = framebuf.clear(OLED_BLACK);
        {
            let mut translated = framebuf.translated(-bounds.top_left);
            let _ = widget.draw(&mut translated);
        }
        self.display
            .fill_contiguous(&bounds, (&framebuf).into_iter().map(|pixel| pixel.1))
    }
}

pub(super) fn is_two_aligned(bounds: Rectangle) -> bool {
    bounds.top_left.x & 1 == 0
        && bounds.top_left.y & 1 == 0
        && bounds.size.width & 1 == 0
        && bounds.size.height & 1 == 0
}

/// Adapts a borrowed slice into a [`FrameBufferBackend`]. The crate ships
/// implementations for fixed-size arrays only, but we want to share one
/// runtime-sized scratch buffer across widgets of varying sizes.
struct SliceBackend<'a>(&'a mut [Rgb565]);

impl FrameBufferBackend for SliceBackend<'_> {
    type Color = Rgb565;

    fn set(&mut self, index: usize, color: Rgb565) {
        self.0[index] = color;
    }

    fn get(&self, index: usize) -> Rgb565 {
        self.0[index]
    }

    fn nr_elements(&self) -> usize {
        self.0.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::{geometry::Point, primitives::Rectangle};

    #[test]
    fn flags_odd_origin_x() {
        assert!(!is_two_aligned(Rectangle::new(
            Point::new(1, 0),
            Size::new(10, 10),
        )));
    }

    #[test]
    fn flags_odd_size_height() {
        assert!(!is_two_aligned(Rectangle::new(
            Point::new(0, 0),
            Size::new(10, 11),
        )));
    }

    #[test]
    fn accepts_two_aligned_bounds() {
        assert!(is_two_aligned(Rectangle::new(
            Point::new(2, 4),
            Size::new(10, 12),
        )));
    }
}
