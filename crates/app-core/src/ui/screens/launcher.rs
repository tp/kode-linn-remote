use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::Rectangle,
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use crate::{NetworkStatus, RenderError, Screen};

use super::super::{
    AppContext,
    components::{
        ButtonTone, clear_rect, draw_button, draw_network_blocked_icon, draw_spinner,
        draw_spinner_dots, draw_wifi_icon, ui_font,
    },
    focus::FocusTargets,
    geometry::vertical_pair,
    painter::Painter,
    style::{OLED_BLACK, TEXT_PRIMARY, TEXT_SECONDARY},
    widget::Widget,
};

// Tuned for the Kode Dot's 410 x 502 portrait rectangle. The panel has no
// rounded mask eating its corners, so the inset is cosmetic rather than a
// safe area, and the extra height lets the app buttons stack full-width
// instead of splitting the narrow width between them.
const CONTENT_INSET: i32 = 24;
const TITLE_Y: i32 = 40;
const BUTTON_Y: i32 = 108;
const BUTTON_HEIGHT: u32 = 140;
const BUTTON_GAP: i32 = 20;
const NETWORK_STATUS_Y: i32 = 452;
const NETWORK_STATUS_WIDTH: u32 = 178;
const NETWORK_STATUS_HEIGHT: u32 = 72;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Layout {
    pub(super) title_origin: Point,
    pub(super) stopwatch_button: Rectangle,
    pub(super) hifi_button: Rectangle,
    pub(super) network_status_center: Point,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct State {
    render_cache: RenderCache,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RenderCache {
    static_drawn: bool,
    network_status_drawn: Option<NetworkStatus>,
    spinner_phase_drawn: Option<u8>,
}

impl State {
    pub(crate) const fn new() -> Self {
        Self {
            render_cache: RenderCache::new(),
        }
    }

    /// Drops every cached "already drawn" flag so the next render repaints
    /// from scratch. Used when the focus ring moves and the old outline has to
    /// disappear along with it.
    pub(crate) fn invalidate(&mut self) {
        self.render_cache = RenderCache::new();
    }

    pub(crate) fn on_tick(&self, context: AppContext) -> bool {
        matches!(context.network_status, NetworkStatus::Connecting)
            && self.render_cache.spinner_phase_drawn != Some(spinner_phase(context.uptime_ms))
    }
}

impl RenderCache {
    const fn new() -> Self {
        Self {
            static_drawn: false,
            network_status_drawn: None,
            spinner_phase_drawn: None,
        }
    }
}

pub(crate) fn layout(bounds: Rectangle) -> Layout {
    let content_left = bounds.top_left.x + CONTENT_INSET;
    let content_width = bounds.size.width.saturating_sub((CONTENT_INSET * 2) as u32);
    let button_y = bounds.top_left.y + BUTTON_Y;
    let (stopwatch_button, hifi_button) = vertical_pair(
        content_left,
        button_y,
        content_width,
        BUTTON_HEIGHT,
        BUTTON_GAP,
    );

    Layout {
        title_origin: Point::new(content_left, bounds.top_left.y + TITLE_Y),
        stopwatch_button,
        hifi_button,
        network_status_center: Point::new(
            bounds.top_left.x + (bounds.size.width / 2) as i32,
            bounds.top_left.y + NETWORK_STATUS_Y,
        ),
    }
}

/// Focusable controls, in reading order.
pub(crate) fn focus_targets(layout: &Layout) -> FocusTargets {
    let mut targets = FocusTargets::new();
    let _ = targets.push(layout.stopwatch_button);
    let _ = targets.push(layout.hifi_button);
    targets
}

pub(crate) fn handle_touch(layout: &Layout, point: Point) -> Option<Screen> {
    hit_test(layout, point)
}

fn hit_test(layout: &Layout, point: Point) -> Option<Screen> {
    if layout.stopwatch_button.contains(point) {
        Some(Screen::Stopwatch)
    } else if layout.hifi_button.contains(point) {
        Some(Screen::HifiControl)
    } else {
        None
    }
}

pub(crate) fn render<D>(
    state: &mut State,
    context: AppContext,
    display: &mut D,
    scratch: &mut [Rgb565],
    ui_layout: &Layout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let mut painter = Painter::new(display, scratch);
    let static_chrome = StaticChrome {
        layout: *ui_layout,
        already_drawn: state.render_cache.static_drawn,
    };
    painter.draw(&static_chrome).map_err(RenderError::Draw)?;
    state.render_cache.static_drawn = true;

    let status_center = ui_layout.network_status_center;
    let spinner_phase = spinner_phase(context.uptime_ms);
    let network_status = NetworkStatusWidget {
        center: status_center,
        status: context.network_status,
        spinner_phase,
        previous_status: state.render_cache.network_status_drawn,
        previous_spinner_phase: state.render_cache.spinner_phase_drawn,
    };
    painter.draw(&network_status).map_err(RenderError::Draw)?;
    state.render_cache.network_status_drawn = Some(context.network_status);
    state.render_cache.spinner_phase_drawn = match context.network_status {
        NetworkStatus::Connecting => Some(spinner_phase),
        NetworkStatus::Online | NetworkStatus::Offline => None,
    };

    Ok(())
}

fn spinner_phase(uptime_ms: u64) -> u8 {
    ((uptime_ms / 125) % 8) as u8
}

struct StaticChrome {
    layout: Layout,
    already_drawn: bool,
}

impl Widget<()> for StaticChrome {
    fn bounds(&self) -> Rectangle {
        Rectangle::new(Point::zero(), Size::zero())
    }

    fn should_draw(&self) -> bool {
        !self.already_drawn
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        let title_font = ui_font!(BOLD);
        let title_style = BitmapFontStyleBuilder::new()
            .text_color(TEXT_PRIMARY)
            .background_color(OLED_BLACK)
            .font(&title_font)
            .build();
        let top_text_style = TextStyleBuilder::new().baseline(Baseline::Top).build();

        Text::with_text_style(
            "Launcher",
            self.layout.title_origin,
            title_style,
            top_text_style,
        )
        .draw(target)?;

        draw_button(
            target,
            self.layout.stopwatch_button,
            "STOP WATCH",
            true,
            ButtonTone::Start,
        )
        .map_err(render_error_into_draw)?;
        draw_button(
            target,
            self.layout.hifi_button,
            "HIFI",
            true,
            ButtonTone::Stop,
        )
        .map_err(render_error_into_draw)?;

        Ok(())
    }
}

struct NetworkStatusWidget {
    center: Point,
    status: NetworkStatus,
    spinner_phase: u8,
    previous_status: Option<NetworkStatus>,
    previous_spinner_phase: Option<u8>,
}

impl Widget<()> for NetworkStatusWidget {
    fn bounds(&self) -> Rectangle {
        Rectangle::new(
            self.center
                - Point::new(
                    (NETWORK_STATUS_WIDTH / 2) as i32,
                    (NETWORK_STATUS_HEIGHT / 2) as i32,
                ),
            Size::new(NETWORK_STATUS_WIDTH, NETWORK_STATUS_HEIGHT),
        )
    }

    fn should_draw(&self) -> bool {
        self.previous_status != Some(self.status)
            || (matches!(self.status, NetworkStatus::Connecting)
                && self.previous_spinner_phase != Some(self.spinner_phase))
    }

    fn draw<D>(&self, target: &mut D) -> Result<(), D::Error>
    where
        D: DrawTarget<Color = Rgb565>,
    {
        // Phase-only update while still Connecting: skip the clear and just
        // repaint the dots — they cover their previous selves exactly. The
        // 180x96 clear was the source of the visible flicker.
        let phase_only_update = matches!(self.status, NetworkStatus::Connecting)
            && self.previous_status == Some(NetworkStatus::Connecting);
        if phase_only_update {
            return draw_spinner_dots(target, self.center, self.spinner_phase);
        }

        clear_rect(target, self.bounds()).map_err(render_error_into_draw)?;

        match self.status {
            NetworkStatus::Connecting => {
                draw_spinner(target, self.center, self.spinner_phase)
                    .map_err(render_error_into_draw)?;
            }
            NetworkStatus::Online => {
                draw_wifi_icon(target, self.center + Point::new(0, -19))
                    .map_err(render_error_into_draw)?;
            }
            NetworkStatus::Offline => {
                draw_network_blocked_icon(target, self.center + Point::new(-39, 0))
                    .map_err(render_error_into_draw)?;
                let offline_font = ui_font!(BOLD);
                let offline_style = BitmapFontStyleBuilder::new()
                    .text_color(TEXT_SECONDARY)
                    .background_color(OLED_BLACK)
                    .font(&offline_font)
                    .build();
                let offline_text_style = TextStyleBuilder::new()
                    .alignment(Alignment::Left)
                    .baseline(Baseline::Middle)
                    .build();

                Text::with_text_style(
                    "offline",
                    self.center + Point::new(-9, 1),
                    offline_style,
                    offline_text_style,
                )
                .draw(target)?;
            }
        }

        Ok(())
    }
}

fn render_error_into_draw<E>(error: RenderError<E>) -> E {
    match error {
        RenderError::Draw(error) => error,
        RenderError::TextFormat => unreachable!("launcher text uses fixed literals"),
    }
}

#[cfg(test)]
pub(crate) fn button_centers(
    bounds: Rectangle,
) -> (
    embedded_graphics::geometry::Point,
    embedded_graphics::geometry::Point,
) {
    let ui_layout = layout(bounds);

    (
        ui_layout.stopwatch_button.center(),
        ui_layout.hifi_button.center(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::SCREEN_BOUNDS;

    #[test]
    fn hit_tests_app_buttons() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_eq!(
            hit_test(&ui_layout, ui_layout.stopwatch_button.center()),
            Some(Screen::Stopwatch)
        );
        assert_eq!(
            hit_test(&ui_layout, ui_layout.hifi_button.center()),
            Some(Screen::HifiControl)
        );
    }
}
