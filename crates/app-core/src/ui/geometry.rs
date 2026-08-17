use embedded_graphics::{prelude::*, primitives::Rectangle};

use crate::DISPLAY_SIZE;

pub(crate) const SCREEN_BOUNDS: Rectangle = Rectangle::new(Point::zero(), DISPLAY_SIZE);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Column {
    x: i32,
    pub(super) y: i32,
    width: u32,
    gap: i32,
}

impl Column {
    pub(super) const fn new(x: i32, y: i32, width: u32, gap: i32) -> Self {
        Self { x, y, width, gap }
    }

    #[must_use]
    pub(super) fn take(&mut self, height: u32) -> Rectangle {
        let rect = Rectangle::new(Point::new(self.x, self.y), Size::new(self.width, height));
        self.skip(height as i32 + self.gap);
        rect
    }

    pub(super) fn skip(&mut self, height: i32) {
        self.y = self.y.saturating_add(height);
    }
}

/// Two full-width rows stacked with a gap. Portrait panels have height to
/// spare and only ~410 px of width, so stacked cards beat a side-by-side pair.
pub(super) fn vertical_pair(
    left: i32,
    top: i32,
    width: u32,
    height: u32,
    gap: i32,
) -> (Rectangle, Rectangle) {
    (
        Rectangle::new(Point::new(left, top), Size::new(width, height)),
        Rectangle::new(
            Point::new(left, top + height as i32 + gap),
            Size::new(width, height),
        ),
    )
}

/// Shrinks `bounds` by `inset` on every side.
pub(super) fn inset_rect(bounds: Rectangle, inset: i32) -> Rectangle {
    Rectangle::new(
        Point::new(bounds.top_left.x + inset, bounds.top_left.y + inset),
        Size::new(
            bounds.size.width.saturating_sub((inset * 2) as u32),
            bounds.size.height.saturating_sub((inset * 2) as u32),
        ),
    )
}

pub(super) fn horizontal_pair(
    left: i32,
    y: i32,
    width: u32,
    height: u32,
    gap: i32,
) -> (Rectangle, Rectangle) {
    let item_width = width.saturating_sub(gap as u32) / 2;
    (
        Rectangle::new(Point::new(left, y), Size::new(item_width, height)),
        Rectangle::new(
            Point::new(left + item_width as i32 + gap, y),
            Size::new(item_width, height),
        ),
    )
}
