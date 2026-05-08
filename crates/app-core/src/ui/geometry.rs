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

pub(super) fn centered_square(bounds: Rectangle, size: u32) -> Rectangle {
    let center_x = bounds.top_left.x + (bounds.size.width / 2) as i32;
    let center_y = bounds.top_left.y + (bounds.size.height / 2) as i32;
    let half = (size / 2) as i32;

    Rectangle::new(
        Point::new(center_x - half, center_y - half),
        Size::new(size, size),
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
