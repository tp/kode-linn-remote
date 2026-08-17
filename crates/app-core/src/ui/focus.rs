//! D-pad focus movement, shared by every screen.
//!
//! The Kode Dot has a four-way pad and two control buttons, so each screen
//! needs a notion of "the control the pad is currently on". Rather than give
//! every screen its own hand-written navigation table, each screen publishes
//! the rectangles of its focusable controls in reading order and this module
//! does purely geometric movement over them.
//!
//! Geometric movement means one implementation covers a row of two buttons,
//! a 3x2 grid of pins, and anything laid out later, with no per-screen tables
//! to keep in sync with the layout constants.

use embedded_graphics::{prelude::*, primitives::Rectangle};
use heapless::Vec;

use crate::Direction;

/// Upper bound on focusable controls per screen. The busiest screen today is
/// the HiFi pins page with six.
pub(crate) const MAX_FOCUS_TARGETS: usize = 8;

pub(crate) type FocusTargets = Vec<Rectangle, MAX_FOCUS_TARGETS>;

fn center(rect: &Rectangle) -> Point {
    Point::new(
        rect.top_left.x + (rect.size.width / 2) as i32,
        rect.top_left.y + (rect.size.height / 2) as i32,
    )
}

/// Cost of moving from `from` to `to` in `direction`, or `None` when `to` does
/// not lie in that direction at all.
///
/// The along-axis distance dominates and the cross-axis offset is penalised, so
/// the pad prefers the neighbour straight ahead over one that is nearer as the
/// crow flies but off to the side. Ties break toward the smaller cross offset.
fn cost(from: Point, to: Point, direction: Direction) -> Option<i64> {
    let dx = (to.x - from.x) as i64;
    let dy = (to.y - from.y) as i64;

    let (along, across) = match direction {
        Direction::Up => (-dy, dx.abs()),
        Direction::Down => (dy, dx.abs()),
        Direction::Left => (-dx, dy.abs()),
        Direction::Right => (dx, dy.abs()),
    };

    if along <= 0 {
        return None;
    }

    Some(along + across * 3)
}

/// The index to focus after pressing `direction`.
///
/// Returns `None` when there is nothing in that direction, which callers treat
/// as "stay put". There is one exception: horizontal movement off the end of a
/// row continues to the neighbour in reading order, which on a grid is the
/// start of the next line or the end of the previous one. That is what every
/// grid does, and it means a left/right-only user can still reach every tile.
///
/// It deliberately stops at the ends of the list rather than wrapping around
/// to the far corner — line wrapping follows the reading order a user already
/// has in their head, while corner wrapping makes it easy to lose the ring on
/// a small panel.
pub(crate) fn step(
    targets: &FocusTargets,
    current: Option<usize>,
    direction: Direction,
) -> Option<usize> {
    if targets.is_empty() {
        return None;
    }

    let Some(current) = current.filter(|index| *index < targets.len()) else {
        // No focus yet: the pad lands on the first control rather than moving.
        return Some(0);
    };

    let origin = center(&targets[current]);

    let nearest = targets
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != current)
        .filter_map(|(index, rect)| cost(origin, center(rect), direction).map(|cost| (cost, index)))
        .min_by_key(|(cost, index)| (*cost, *index))
        .map(|(_, index)| index);

    if nearest.is_some() {
        return nearest;
    }

    // Off the end of a row: continue in reading order onto the adjacent line.
    match direction {
        Direction::Right => (current + 1 < targets.len()).then_some(current + 1),
        Direction::Left => current.checked_sub(1),
        Direction::Up | Direction::Down => None,
    }
}

/// The control containing `point`, used to keep the ring in step with touches
/// so switching between finger and pad mid-interaction is not disorienting.
pub(crate) fn hit(targets: &FocusTargets, point: Point) -> Option<usize> {
    targets.iter().position(|rect| rect.contains(point))
}

#[cfg(test)]
mod tests {
    use super::*;
    use embedded_graphics::geometry::Size;

    fn rect(x: i32, y: i32) -> Rectangle {
        Rectangle::new(Point::new(x, y), Size::new(80, 60))
    }

    fn targets(rects: &[Rectangle]) -> FocusTargets {
        FocusTargets::from_slice(rects).expect("fits")
    }

    #[test]
    fn first_press_focuses_first_target() {
        let targets = targets(&[rect(0, 0), rect(100, 0)]);
        assert_eq!(step(&targets, None, Direction::Right), Some(0));
        assert_eq!(step(&targets, None, Direction::Up), Some(0));
    }

    #[test]
    fn moves_along_a_row() {
        let targets = targets(&[rect(0, 0), rect(100, 0)]);
        assert_eq!(step(&targets, Some(0), Direction::Right), Some(1));
        assert_eq!(step(&targets, Some(1), Direction::Left), Some(0));
    }

    #[test]
    fn does_not_wrap_around_the_ends_of_the_list() {
        let targets = targets(&[rect(0, 0), rect(100, 0)]);
        assert_eq!(step(&targets, Some(1), Direction::Right), None);
        assert_eq!(step(&targets, Some(0), Direction::Left), None);
    }

    #[test]
    fn horizontal_movement_wraps_onto_the_adjacent_line() {
        // 2x2 grid in reading order, as the music picker publishes it.
        let targets = targets(&[rect(0, 0), rect(100, 0), rect(0, 100), rect(100, 100)]);

        // Off the right of the top row, on to the start of the bottom one.
        assert_eq!(step(&targets, Some(1), Direction::Right), Some(2));
        // And back again.
        assert_eq!(step(&targets, Some(2), Direction::Left), Some(1));

        // The ends of the whole grid still stop rather than wrapping round.
        assert_eq!(step(&targets, Some(3), Direction::Right), None);
        assert_eq!(step(&targets, Some(0), Direction::Left), None);

        // Vertical movement is unaffected: no line to wrap on to.
        assert_eq!(step(&targets, Some(1), Direction::Up), None);
        assert_eq!(step(&targets, Some(2), Direction::Down), None);
    }

    #[test]
    fn prefers_the_neighbour_straight_ahead_in_a_grid() {
        // 3x2 grid, reading order.
        let targets = targets(&[
            rect(0, 0),
            rect(100, 0),
            rect(200, 0),
            rect(0, 100),
            rect(100, 100),
            rect(200, 100),
        ]);

        // Down from the middle of the top row lands directly below, not on a
        // diagonal neighbour that is closer in raw distance.
        assert_eq!(step(&targets, Some(1), Direction::Down), Some(4));
        assert_eq!(step(&targets, Some(4), Direction::Up), Some(1));
        assert_eq!(step(&targets, Some(3), Direction::Right), Some(4));
    }

    #[test]
    fn hit_matches_the_containing_rect() {
        let targets = targets(&[rect(0, 0), rect(100, 0)]);
        assert_eq!(hit(&targets, Point::new(110, 10)), Some(1));
        assert_eq!(hit(&targets, Point::new(90, 10)), None);
    }
}
