use embedded_graphics::{pixelcolor::Rgb565, prelude::RgbColor};

/// Corner radius of every card-like surface: buttons, stopwatch cards, pin
/// tiles. Deliberately one value rather than one per widget, because the focus
/// ring's radius is derived from it — a surface that picked its own radius
/// would end up inside a ring whose curve no longer matched it.
pub(super) const CARD_RADIUS: u32 = 18;
pub(super) const BUTTON_RADIUS: u32 = CARD_RADIUS;

/// Colours taken from the Kode Dot itself.
///
/// The device is a warm off-white shell with a black panel, one blue and one
/// red button, and slate-grey UI chrome. Matching the hardware means the
/// on-screen accents read as part of the same object rather than as a
/// generic dark theme, and it gives the two control buttons an obvious
/// on-screen counterpart.
///
/// Values are the sampled sRGB hex, quantised to Rgb565's 5/6/5 channels.
/// Keep the hex in the comment: the quantised triples are unreadable, and
/// re-deriving them by eye is how a palette drifts.
pub(super) mod dot {
    use embedded_graphics::pixelcolor::Rgb565;

    /// `#2F7FC8` — the blue control button.
    pub(in crate::ui) const BLUE: Rgb565 = Rgb565::new(6, 31, 24);
    /// `#1E5A8F` — blue darkened for use as a large fill.
    pub(in crate::ui) const BLUE_DEEP: Rgb565 = Rgb565::new(4, 22, 17);
    /// `#E0523C` — the red control button.
    pub(in crate::ui) const RED: Rgb565 = Rgb565::new(27, 20, 7);
    /// `#8F2E1F` — red darkened for use as a large fill.
    pub(in crate::ui) const RED_DEEP: Rgb565 = Rgb565::new(17, 11, 4);
    /// `#F0EDE6` — the shell's warm off-white.
    pub(in crate::ui) const SHELL: Rgb565 = Rgb565::new(29, 59, 28);
    /// `#54697A` — the slate of the on-device UI chrome.
    pub(in crate::ui) const SLATE: Rgb565 = Rgb565::new(10, 26, 15);
    /// `#9DAAB6` — slate lifted until it reads as body text on black. Dim
    /// enough to sit below the title, bright enough not to look disabled.
    pub(in crate::ui) const SLATE_LIGHT: Rgb565 = Rgb565::new(19, 42, 22);
    /// `#35434E` — slate dimmed, for borders.
    pub(in crate::ui) const SLATE_DIM: Rgb565 = Rgb565::new(6, 17, 9);
    /// `#141A1F` — slate at panel depth, barely lifted off black.
    pub(in crate::ui) const SLATE_DEEP: Rgb565 = Rgb565::new(2, 6, 3);
}

/// True black, not a dark grey: unlit AMOLED pixels draw no power, so the
/// background being genuinely off is a battery decision as much as a visual
/// one. It also happens to match the panel's own bezel.
pub(super) const OLED_BLACK: Rgb565 = Rgb565::BLACK;

pub(super) const SURFACE: Rgb565 = dot::SLATE_DEEP;
pub(super) const SURFACE_BORDER: Rgb565 = dot::SLATE_DIM;

/// Warm off-white rather than pure white, matching the shell. Pure white on
/// true black is harsh at this pixel density.
pub(super) const TEXT_PRIMARY: Rgb565 = dot::SHELL;
/// Secondary text is dimmer than primary, which is what separates a track
/// title from its artist without a second font or a second size.
///
/// This costs nothing: `BitmapFontStyleBuilder` takes the colour at draw time
/// and blends against the generated glyph coverage, so both tones share one
/// rasterised font. Only the blend changes.
pub(super) const TEXT_SECONDARY: Rgb565 = dot::SLATE_LIGHT;
pub(super) const TEXT_DISABLED: Rgb565 = dot::SLATE;

/// Primary action, carrying the blue button's colour.
pub(super) const ACTION_START: Rgb565 = dot::BLUE_DEEP;
pub(super) const ACTION_START_BORDER: Rgb565 = dot::BLUE;
/// Destructive or stopping action, carrying the red button's colour.
pub(super) const ACTION_STOP: Rgb565 = dot::RED_DEEP;
pub(super) const ACTION_STOP_BORDER: Rgb565 = dot::RED;
pub(super) const ACTION_INACTIVE: Rgb565 = dot::SLATE_DEEP;
pub(super) const ACTION_INACTIVE_BORDER: Rgb565 = dot::SLATE_DIM;

/// Volume bar. The track is the unfilled remainder and the active span is the
/// current level, so the track must be the dimmer of the two — it was the
/// brighter one while this was a ring on the round panel, which read as a
/// full bar at zero volume.
pub(super) const VOLUME_TRACK: Rgb565 = dot::SLATE_DIM;
pub(super) const VOLUME_ACTIVE: Rgb565 = dot::BLUE;

/// Track progress. Deliberately *not* the same pair as the volume readout:
/// the two used to sit twelve pixels apart looking identical, one tracking the
/// track and one tracking the level, and read as a single confusing pair. The
/// volume bar is gone now, but keeping the colours distinct means a future
/// reader cannot reintroduce the confusion by accident.
pub(super) const PROGRESS_TRACK: Rgb565 = dot::SLATE_DEEP;
pub(super) const PROGRESS_FILL: Rgb565 = dot::SHELL;

/// Ring drawn around the control the D-pad is currently on.
///
/// Off-white rather than the blue accent, because the ring now sits directly
/// against what it outlines and both control buttons are themselves blue — a
/// blue ring flush against a blue button is not a highlight.
pub(super) const FOCUS_RING: Rgb565 = dot::SHELL;
/// Width of the ring's stroke band.
///
/// One pixel wider than the inset, so the band reaches a pixel *inside* the
/// control's bounds. That pixel is not decoration. A rounded control has its
/// own anti-aliased edge, blended against the background before the ring is
/// drawn, and a ring that merely butts up against it leaves that half-dark
/// thread showing between the two — the ring can be told what lies under it,
/// but not undo what is already there. Painting over it is what makes the two
/// read as touching.
pub(super) const FOCUS_RING_STROKE: u32 = 4;
/// How far the ring's outer edge sits beyond the control's bounds.
///
/// Small enough that the ring lies against the control rather than floating
/// off it: any wider and a black channel opens up, which around a bright cover
/// reads as a notch at the corners rather than as deliberate spacing.
pub(super) const FOCUS_RING_INSET: i32 = 3;
/// Concentric with the surface it surrounds.
///
/// An outer curve sitting `g` pixels outside an inner one has to use
/// `r_inner + g`; then both arcs share a centre and follow one another instead
/// of diverging at the corners. Get it wrong and the spacing breathes: at 14,
/// six pixels out, this ring cleared a tile's cover by 3 px along the edges
/// and 7 px across the diagonal.
pub(super) const FOCUS_RING_RADIUS: u32 = CARD_RADIUS + FOCUS_RING_INSET as u32;
