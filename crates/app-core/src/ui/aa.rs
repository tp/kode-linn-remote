//! Anti-aliased primitives.
//!
//! `embedded-graphics` rasterises with binary coverage — a pixel is either the
//! shape's colour or it isn't — which is why its curves and rounded corners
//! come out visibly stepped next to the pre-anti-aliased bitmap fonts. This
//! module fills that gap for the handful of shapes the UI actually curves:
//! rounded rectangles, circles and ring segments.
//!
//! # How
//!
//! Coverage is measured by supersampling: each pixel is probed on a 4x4 grid
//! of sample points, and the number of samples landing inside the shape gives
//! 17 coverage levels. That is plenty at this panel's pixel density, and it
//! keeps every operation in integer arithmetic — no `libm`, no float, nothing
//! that would be awkward in `no_std` firmware.
//!
//! # Why a backdrop colour is required
//!
//! `DrawTarget` is write-only: there is no way to read back what a pixel
//! currently holds and blend against it. Callers therefore pass the colour
//! they know is behind the shape. That is never a guess in this UI — controls
//! sit on the OLED black background, and labels sit on their control's fill.
//!
//! All geometry below is carried in *sample units*, where one pixel spans
//! [`UNIT`] units, so sample centres land on whole numbers and every inside
//! test stays exact.

use embedded_graphics::{Pixel, pixelcolor::Rgb565, prelude::*, primitives::Rectangle};

/// Sample points per axis. 4x4 gives 17 coverage levels.
const SAMPLES: i32 = 4;
/// Sample units per pixel. Sample centres sit at odd multiples of one unit.
const UNIT: i32 = SAMPLES * 2;
/// Total samples per pixel, and therefore full coverage.
const FULL: u32 = (SAMPLES * SAMPLES) as u32;

/// Mixes `fg` over `bg`, where `coverage` runs from 0 to [`FULL`].
///
/// Channels are mixed in their native Rgb565 widths. That is not gamma
/// correct, but it matches how the panel treats the values and avoids dragging
/// a colour-space conversion into the render path.
fn blend(fg: Rgb565, bg: Rgb565, coverage: u32) -> Rgb565 {
    if coverage == 0 {
        return bg;
    }
    if coverage >= FULL {
        return fg;
    }

    let mix = |a: u8, b: u8| -> u8 {
        ((a as u32 * coverage + b as u32 * (FULL - coverage)) / FULL) as u8
    };

    Rgb565::new(
        mix(fg.r(), bg.r()),
        mix(fg.g(), bg.g()),
        mix(fg.b(), bg.b()),
    )
}

/// A shape that can answer "is this sample point inside me?".
trait Shape {
    fn contains(&self, x: i32, y: i32) -> bool;
}

/// Rounded rectangle in sample units. A circle is the case where the radius
/// equals half of both sides.
struct RoundedRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    radius: i32,
}

impl RoundedRect {
    fn new(rect: Rectangle, radius: u32) -> Self {
        let left = rect.top_left.x * UNIT;
        let top = rect.top_left.y * UNIT;
        let max_radius = (rect.size.width.min(rect.size.height) / 2) as i32;
        Self {
            left,
            top,
            right: left + rect.size.width as i32 * UNIT,
            bottom: top + rect.size.height as i32 * UNIT,
            radius: (radius as i32).min(max_radius) * UNIT,
        }
    }

    /// Shrinks by `amount` pixels on every side, keeping the corners concentric.
    fn inset(&self, amount: i32) -> Self {
        let amount = amount * UNIT;
        Self {
            left: self.left + amount,
            top: self.top + amount,
            right: self.right - amount,
            bottom: self.bottom - amount,
            radius: (self.radius - amount).max(0),
        }
    }
}

impl Shape for RoundedRect {
    fn contains(&self, x: i32, y: i32) -> bool {
        if x < self.left || x >= self.right || y < self.top || y >= self.bottom {
            return false;
        }

        // Distance past the inner box that the corner arcs are struck from.
        // Zero on the straight edges, so only true corners get the radius test.
        let dx = (self.left + self.radius - x)
            .max(x - (self.right - self.radius))
            .max(0);
        let dy = (self.top + self.radius - y)
            .max(y - (self.bottom - self.radius))
            .max(0);

        dx * dx + dy * dy <= self.radius * self.radius
    }
}

/// Sine of `degrees`, scaled by 1024, from a quarter-turn table.
///
/// Only used to turn an arc's start and end angles into direction vectors, so
/// a whole-degree table is ample and costs under 200 bytes.
fn sin_scaled(degrees: i32) -> i32 {
    /// sin(0..=90 degrees) * 1024.
    const QUARTER: [i16; 91] = [
        0, 18, 36, 54, 71, 89, 107, 125, 143, 160, 178, 195, 213, 230, 248, 265, 282, 299, 316,
        333, 350, 367, 384, 400, 416, 433, 449, 465, 481, 496, 512, 527, 543, 558, 573, 587, 602,
        616, 630, 644, 658, 672, 685, 698, 711, 724, 737, 749, 761, 773, 784, 796, 807, 818, 828,
        839, 849, 858, 868, 877, 886, 895, 903, 911, 919, 926, 933, 940, 946, 952, 958, 963, 968,
        973, 977, 981, 984, 987, 990, 993, 995, 996, 998, 999, 1000, 1000, 1000, 1000, 1000, 1000,
        1000,
    ];

    let degrees = degrees.rem_euclid(360);
    match degrees {
        0..=90 => QUARTER[degrees as usize] as i32,
        91..=180 => QUARTER[(180 - degrees) as usize] as i32,
        181..=270 => -(QUARTER[(degrees - 180) as usize] as i32),
        _ => -(QUARTER[(360 - degrees) as usize] as i32),
    }
}

fn cos_scaled(degrees: i32) -> i32 {
    sin_scaled(degrees + 90)
}

/// A stroked arc: the part of an annulus lying within an angular sweep.
///
/// Angles match `embedded-graphics`: zero degrees points at three o'clock and
/// the sweep advances directly in screen space, where y grows downward. A
/// positive sweep therefore travels *visually clockwise*, so an arc from 215
/// to 325 degrees is the fan across the top of the circle. Verified against
/// `embedded_graphics::primitives::Arc` rather than derived, because the
/// mirrored alternative also looks plausible on paper and renders upside down.
struct RingSegment {
    center_x: i32,
    center_y: i32,
    outer_squared: i32,
    inner_squared: i32,
    start: (i32, i32),
    end: (i32, i32),
    reflex: bool,
}

impl RingSegment {
    fn new(
        center: Point,
        diameter: u32,
        stroke_width: u32,
        start_deg: i32,
        sweep_deg: i32,
    ) -> Self {
        let outer = (diameter as i32 * UNIT) / 2;
        let inner = (outer - stroke_width as i32 * UNIT).max(0);
        let end_deg = start_deg + sweep_deg;

        Self {
            center_x: center.x * UNIT + UNIT / 2,
            center_y: center.y * UNIT + UNIT / 2,
            outer_squared: outer * outer,
            inner_squared: inner * inner,
            start: (cos_scaled(start_deg), sin_scaled(start_deg)),
            end: (cos_scaled(end_deg), sin_scaled(end_deg)),
            reflex: sweep_deg.abs() > 180,
        }
    }
}

impl Shape for RingSegment {
    fn contains(&self, x: i32, y: i32) -> bool {
        let dx = x - self.center_x;
        let dy = y - self.center_y;
        let distance_squared = dx * dx + dy * dy;

        if distance_squared > self.outer_squared || distance_squared < self.inner_squared {
            return false;
        }

        // Which side of the start and end rays the point falls on, via the 2D
        // cross product. Rays and deltas share the same screen-space basis, so
        // this needs no sign correction: positive means the point is further
        // along the sweep than the ray. A reflex sweep is the union of the two
        // half-planes rather than their intersection.
        let after_start = self.start.0 * dy - self.start.1 * dx >= 0;
        let before_end = self.end.0 * dy - self.end.1 * dx <= 0;

        if self.reflex {
            after_start || before_end
        } else {
            after_start && before_end
        }
    }
}

/// Triangle in sample units, tested by the sign of the cross product against
/// each edge.
struct Triangle {
    points: [(i32, i32); 3],
    clockwise: bool,
}

impl Triangle {
    fn new(a: Point, b: Point, c: Point) -> Self {
        let to_units = |p: Point| (p.x * UNIT, p.y * UNIT);
        let points = [to_units(a), to_units(b), to_units(c)];
        // Winding decides which sign means "inside", so accept either.
        let area = (points[1].0 - points[0].0) * (points[2].1 - points[0].1)
            - (points[1].1 - points[0].1) * (points[2].0 - points[0].0);
        Self {
            points,
            clockwise: area < 0,
        }
    }
}

impl Shape for Triangle {
    fn contains(&self, x: i32, y: i32) -> bool {
        for index in 0..3 {
            let (ax, ay) = self.points[index];
            let (bx, by) = self.points[(index + 1) % 3];
            let side = (bx - ax) * (y - ay) - (by - ay) * (x - ax);
            let outside = if self.clockwise { side > 0 } else { side < 0 };
            if outside {
                return false;
            }
        }
        true
    }
}

/// Filled triangle, anti-aliased against `backdrop`.
pub(super) fn triangle<D>(
    display: &mut D,
    a: Point,
    b: Point,
    c: Point,
    fill: Rgb565,
    backdrop: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let left = a.x.min(b.x).min(c.x);
    let top = a.y.min(b.y).min(c.y);
    let right = a.x.max(b.x).max(c.x) + 1;
    let bottom = a.y.max(b.y).max(c.y) + 1;
    let bounds = Rectangle::new(
        Point::new(left, top),
        Size::new((right - left) as u32, (bottom - top) as u32),
    );

    draw_shape(display, bounds, &Triangle::new(a, b, c), fill, backdrop)
}

/// Coverage of one pixel, from 0 to [`FULL`].
///
/// Probes the pixel's corners first: when they agree, the pixel is wholly in
/// or wholly out and the 4x4 grid would only confirm it. Every curve here is
/// thicker than a pixel, so no feature can hide between the corners.
fn coverage(shape: &impl Shape, x: i32, y: i32) -> u32 {
    let left = x * UNIT;
    let top = y * UNIT;
    let right = left + UNIT - 1;
    let bottom = top + UNIT - 1;

    let first = shape.contains(left, top);
    if first == shape.contains(right, top)
        && first == shape.contains(left, bottom)
        && first == shape.contains(right, bottom)
    {
        return if first { FULL } else { 0 };
    }

    let mut inside = 0;
    for row in 0..SAMPLES {
        for column in 0..SAMPLES {
            if shape.contains(left + column * 2 + 1, top + row * 2 + 1) {
                inside += 1;
            }
        }
    }
    inside
}

/// Draws `shape` within `bounds`, touching only the pixels it actually covers.
///
/// Uncovered pixels are left alone rather than painted with `backdrop`. That
/// matters whenever shapes overlap: the Wi-Fi icon is three concentric arcs,
/// and filling each one's bounding box would let the smaller arcs punch
/// rectangular bites out of the larger ones behind them. Partially covered
/// pixels are still blended against `backdrop`, so this assumes the caller's
/// declared backdrop is what lies underneath the shape's own edges.
///
/// Note this never uses `fill_solid`: the panel snaps solid fills to an
/// even-aligned window, which would smear the very edge pixels being computed.
fn draw_shape<D>(
    display: &mut D,
    bounds: Rectangle,
    shape: &impl Shape,
    foreground: Rgb565,
    backdrop: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let pixels = bounds
        .points()
        .filter_map(|point| match coverage(shape, point.x, point.y) {
            0 => None,
            covered => Some(Pixel(point, blend(foreground, backdrop, covered))),
        });

    display.draw_iter(pixels)
}

/// Filled rounded rectangle with an optional border, anti-aliased against
/// `backdrop`.
///
/// Border and fill are resolved in a single pass so the border blends over the
/// fill and the fill over the backdrop, rather than compounding two separate
/// anti-aliased passes into a muddy edge.
pub(super) fn rounded_rect<D>(
    display: &mut D,
    rect: Rectangle,
    radius: u32,
    fill: Rgb565,
    border: Rgb565,
    border_width: u32,
    backdrop: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let outer = RoundedRect::new(rect, radius);
    let inner = outer.inset(border_width as i32);
    let width = rect.size.width as usize;
    if width == 0 {
        return Ok(());
    }

    for row in 0..rect.size.height as i32 {
        let y = rect.top_left.y + row;
        let colors = (0..rect.size.width as i32).map(|column| {
            let x = rect.top_left.x + column;
            let edge = blend(border, backdrop, coverage(&outer, x, y));
            blend(fill, edge, coverage(&inner, x, y))
        });
        display.fill_contiguous(
            &Rectangle::new(Point::new(rect.top_left.x, y), Size::new(width as u32, 1)),
            colors,
        )?;
    }

    Ok(())
}

/// Outline of a rounded rectangle, anti-aliased against `backdrop`.
pub(super) fn rounded_rect_outline<D>(
    display: &mut D,
    rect: Rectangle,
    radius: u32,
    stroke: Rgb565,
    stroke_width: u32,
    backdrop: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    rounded_rect(
        display,
        rect,
        radius,
        backdrop,
        stroke,
        stroke_width,
        backdrop,
    )
}

/// Filled circle, anti-aliased against `backdrop`.
pub(super) fn circle<D>(
    display: &mut D,
    center: Point,
    diameter: u32,
    fill: Rgb565,
    backdrop: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let bounds = circle_bounds(center, diameter);
    rounded_rect(display, bounds, diameter / 2, fill, fill, 0, backdrop)
}

/// Circle outline, anti-aliased against `backdrop`.
pub(super) fn circle_outline<D>(
    display: &mut D,
    center: Point,
    diameter: u32,
    stroke: Rgb565,
    stroke_width: u32,
    backdrop: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let bounds = circle_bounds(center, diameter);
    rounded_rect(
        display,
        bounds,
        diameter / 2,
        backdrop,
        stroke,
        stroke_width,
        backdrop,
    )
}

/// Geometry of a stroked arc, kept together so [`arc`] stays readable at the
/// call site instead of taking a run of six bare integers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ArcSpec {
    pub(super) center: Point,
    pub(super) diameter: u32,
    pub(super) stroke_width: u32,
    /// Zero degrees points at three o'clock; a positive sweep advances
    /// visually clockwise. See [`RingSegment`].
    pub(super) start_deg: i32,
    pub(super) sweep_deg: i32,
}

/// Stroked arc, anti-aliased against `backdrop`.
pub(super) fn arc<D>(
    display: &mut D,
    spec: ArcSpec,
    stroke: Rgb565,
    backdrop: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let ArcSpec {
        center,
        diameter,
        stroke_width,
        start_deg,
        sweep_deg,
    } = spec;
    let segment = RingSegment::new(center, diameter, stroke_width, start_deg, sweep_deg);
    draw_shape(
        display,
        circle_bounds(center, diameter),
        &segment,
        stroke,
        backdrop,
    )
}

/// Bounding box of a circle drawn about `center`, matching how
/// `embedded_graphics::primitives::Circle::with_center` places one.
fn circle_bounds(center: Point, diameter: u32) -> Rectangle {
    let radius = (diameter / 2) as i32;
    Rectangle::new(
        Point::new(center.x - radius, center.y - radius),
        Size::new(diameter, diameter),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec::Vec;
    use embedded_graphics::pixelcolor::RgbColor;

    #[test]
    fn blend_returns_the_endpoints_exactly() {
        let fg = Rgb565::WHITE;
        let bg = Rgb565::BLACK;
        assert_eq!(blend(fg, bg, 0), bg);
        assert_eq!(blend(fg, bg, FULL), fg);
        assert_eq!(blend(fg, bg, FULL + 5), fg);
    }

    #[test]
    fn blend_midpoint_lands_between() {
        let mixed = blend(Rgb565::WHITE, Rgb565::BLACK, FULL / 2);
        assert!(mixed.r() > 0 && mixed.r() < Rgb565::MAX_R);
        assert!(mixed.g() > 0 && mixed.g() < Rgb565::MAX_G);
    }

    #[test]
    fn square_corner_radius_is_a_plain_rectangle() {
        let rect = Rectangle::new(Point::new(0, 0), Size::new(10, 10));
        let shape = RoundedRect::new(rect, 0);
        assert_eq!(coverage(&shape, 0, 0), FULL);
        assert_eq!(coverage(&shape, 9, 9), FULL);
        assert_eq!(coverage(&shape, 10, 0), 0);
    }

    #[test]
    fn rounded_corners_are_partially_covered() {
        let rect = Rectangle::new(Point::new(0, 0), Size::new(40, 40));
        let shape = RoundedRect::new(rect, 12);

        // Dead centre is solid, the extreme corner is empty, and the corner
        // arc itself lands somewhere in between — which is the whole point.
        assert_eq!(coverage(&shape, 20, 20), FULL);
        assert_eq!(coverage(&shape, 0, 0), 0);

        // Walk out along the corner's true diagonal, where the arc actually
        // crosses pixels, rather than along its bounding box.
        let diagonal: Vec<u32> = (0..12).map(|i| coverage(&shape, i, i)).collect();
        assert!(
            diagonal.iter().any(|c| *c > 0 && *c < FULL),
            "expected fractional coverage across the corner arc, got {diagonal:?}"
        );
        assert_eq!(diagonal[0], 0, "outermost corner pixel is outside the arc");
        assert_eq!(diagonal[11], FULL, "innermost is fully covered");
    }

    #[test]
    fn circle_is_symmetric() {
        let bounds = circle_bounds(Point::new(20, 20), 20);
        let shape = RoundedRect::new(bounds, 10);
        let left = coverage(&shape, 11, 20);
        let right = coverage(&shape, 28, 20);
        let top = coverage(&shape, 20, 11);
        assert_eq!(left, right);
        assert_eq!(left, top);
    }

    #[test]
    fn triangle_covers_its_interior_either_winding() {
        let a = Point::new(0, 0);
        let b = Point::new(0, 20);
        let c = Point::new(20, 10);

        for shape in [Triangle::new(a, b, c), Triangle::new(a, c, b)] {
            assert_eq!(coverage(&shape, 3, 10), FULL, "well inside");
            assert_eq!(coverage(&shape, 18, 2), 0, "outside the sloped edge");
        }
    }

    #[test]
    fn triangle_edges_are_partially_covered() {
        let shape = Triangle::new(Point::new(0, 0), Point::new(0, 20), Point::new(20, 10));
        let along_slope: Vec<u32> = (2..18).map(|y| coverage(&shape, 12, y)).collect();
        assert!(
            along_slope.iter().any(|c| *c > 0 && *c < FULL),
            "expected fractional coverage on the sloped edge, got {along_slope:?}"
        );
    }

    #[test]
    fn sine_table_matches_known_angles() {
        assert_eq!(sin_scaled(0), 0);
        assert_eq!(sin_scaled(90), 1000);
        assert_eq!(sin_scaled(180), 0);
        assert_eq!(sin_scaled(270), -1000);
        assert_eq!(cos_scaled(0), 1000);
        assert_eq!(cos_scaled(180), -1000);
        // Wraps rather than panicking on out-of-range input.
        assert_eq!(sin_scaled(450), sin_scaled(90));
        assert_eq!(sin_scaled(-90), sin_scaled(270));
    }

    #[test]
    fn ring_segment_respects_its_sweep() {
        // A quarter turn from three o'clock advances downward on screen.
        let segment = RingSegment::new(Point::new(50, 50), 40, 4, 0, 90);
        let center = 50 * UNIT + UNIT / 2;
        let radius = 19 * UNIT;

        assert!(segment.contains(center + radius, center), "start of sweep");
        assert!(segment.contains(center, center + radius), "end of sweep");
        assert!(
            !segment.contains(center - radius, center),
            "opposite side must be outside a quarter sweep"
        );
        assert!(!segment.contains(center, center), "hole is not filled");
    }

    #[test]
    fn wifi_fan_sits_above_its_centre() {
        // The exact sweep the network icon uses. Guards the convention that
        // renders the fan the right way up.
        let segment = RingSegment::new(Point::new(50, 50), 72, 4, 215, 110);
        let center_x = 50 * UNIT + UNIT / 2;
        let center_y = 50 * UNIT + UNIT / 2;
        let radius = 34 * UNIT;

        assert!(
            segment.contains(center_x, center_y - radius),
            "fan must cover the top of the circle"
        );
        assert!(
            !segment.contains(center_x, center_y + radius),
            "fan must not reach the bottom"
        );
    }

    #[test]
    fn reflex_sweep_covers_the_long_way_round() {
        let segment = RingSegment::new(Point::new(50, 50), 40, 4, 0, 270);
        let center = 50 * UNIT + UNIT / 2;
        let radius = 19 * UNIT;

        assert!(segment.contains(center, center - radius));
        assert!(segment.contains(center - radius, center));
        assert!(segment.contains(center, center + radius));
    }
}
