use core::fmt::Write as _;
use core::time::Duration;

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle},
    text::{Baseline, Text, TextStyleBuilder},
};
use mplusfonts::{mplus, style::BitmapFontStyleBuilder};

use crate::{NetworkStatus, RenderError};

use super::super::{
    AppContext,
    components::{ButtonTone, clear_rect, draw_button, draw_duration, draw_panel, ui_font},
    focus::FocusTargets,
    geometry::{Column, horizontal_pair},
    style::*,
};

// Tuned for the Kode Dot's 410 x 502 portrait rectangle: no round mask to
// dodge, so the insets shrink, and the reclaimed height goes into taller
// controls that are easier to hit with a finger.
const PANEL_INSET: i32 = 20;
const CONTENT_INSET: i32 = 24;
const HEADER_HEIGHT: u32 = 96;
const HEADER_TITLE_TOP: i32 = 26;
const HEADER_BUTTON_GAP: i32 = 24;
const BUTTON_HEIGHT: u32 = 84;
const BUTTON_GAP: i32 = 22;
const BUTTON_INFO_GAP: i32 = 36;
const INFO_ROW_HEIGHT: u32 = 40;
const INFO_ROW_GAP: i32 = 4;
const IDEAL_LABEL_WIDTH: u32 = 96;
const NETWORK_ICON_OFFSET: Point = Point::new(11, 20);
const NETWORK_TEXT_WITH_ICON_OFFSET_X: i32 = 34;
const NETWORK_ICON_DIAMETER: u32 = 22;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Layout {
    pub(super) header: HeaderLayout,
    pub(super) buttons: ButtonRowLayout,
    pub(super) info: InfoLayout,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct HeaderLayout {
    pub(super) panel: Rectangle,
    pub(super) title_origin: Point,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ButtonRowLayout {
    pub(super) start: Rectangle,
    pub(super) stop: Rectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct InfoLayout {
    pub(super) ideal: IdealRowLayout,
    pub(super) stopwatch: Rectangle,
    pub(super) network: NetworkRowLayout,
    pub(super) interactions: Rectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct IdealRowLayout {
    pub(super) bounds: Rectangle,
    pub(super) label: Rectangle,
    pub(super) value: Rectangle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NetworkRowLayout {
    pub(super) bounds: Rectangle,
    pub(super) icon_center: Point,
    pub(super) text_with_icon_origin: Point,
    pub(super) text_without_icon_origin: Point,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct State {
    running: bool,
    seconds: u64,
    last_second: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Action {
    Start,
    Stop,
}

impl State {
    pub(crate) const fn new() -> Self {
        Self {
            running: false,
            seconds: 0,
            last_second: 0,
        }
    }

    /// This screen repaints in full every frame, so there is no cache to drop.
    pub(crate) fn invalidate(&mut self) {}

    pub(crate) fn on_tick(&mut self, uptime_ms: u64) -> bool {
        if !self.running {
            return false;
        }

        let current_second = uptime_ms / 1000;
        let elapsed = current_second.saturating_sub(self.last_second);
        if elapsed == 0 {
            return false;
        }

        self.seconds = self.seconds.saturating_add(elapsed);
        self.last_second = current_second;
        true
    }

    pub(crate) fn handle_touch(
        &mut self,
        layout: &Layout,
        point: Point,
        uptime_ms: u64,
    ) -> Option<crate::Screen> {
        let action = hit_test(layout, point, self)?;
        self.handle(action, uptime_ms);
        None
    }

    fn handle(&mut self, action: Action, uptime_ms: u64) {
        match action {
            Action::Start => {
                self.running = true;
                self.last_second = uptime_ms / 1000;
            }
            Action::Stop => {
                self.on_tick(uptime_ms);
                self.running = false;
            }
        }
    }
}

/// Focusable controls, in reading order.
pub(crate) fn focus_targets(layout: &Layout) -> FocusTargets {
    let mut targets = FocusTargets::new();
    let _ = targets.push(layout.buttons.start);
    let _ = targets.push(layout.buttons.stop);
    targets
}

pub(crate) fn layout(bounds: Rectangle) -> Layout {
    let panel_left = bounds.top_left.x + PANEL_INSET;
    let content_left = bounds.top_left.x + CONTENT_INSET;
    let panel_width = bounds.size.width.saturating_sub((PANEL_INSET * 2) as u32);
    let content_width = bounds.size.width.saturating_sub((CONTENT_INSET * 2) as u32);

    let mut main_flow = Column::new(panel_left, bounds.top_left.y + PANEL_INSET, panel_width, 0);
    let header_panel = main_flow.take(HEADER_HEIGHT);
    main_flow.skip(HEADER_BUTTON_GAP);
    let button_row = main_flow.take(BUTTON_HEIGHT);
    main_flow.skip(BUTTON_INFO_GAP);

    let mut info_flow = Column::new(content_left, main_flow.y, content_width, INFO_ROW_GAP);
    let ideal = ideal_row(info_flow.take(INFO_ROW_HEIGHT));
    let stopwatch = info_flow.take(INFO_ROW_HEIGHT);
    let network = network_row(info_flow.take(INFO_ROW_HEIGHT));
    let interactions = info_flow.take(INFO_ROW_HEIGHT);

    Layout {
        header: HeaderLayout {
            panel: header_panel,
            title_origin: Point::new(content_left, header_panel.top_left.y + HEADER_TITLE_TOP),
        },
        buttons: buttons(button_row.top_left.y, content_left, content_width),
        info: InfoLayout {
            ideal,
            stopwatch,
            network,
            interactions,
        },
    }
}

fn hit_test(layout: &Layout, point: Point, state: &State) -> Option<Action> {
    if layout.buttons.start.contains(point) && !state.running {
        Some(Action::Start)
    } else if layout.buttons.stop.contains(point) && state.running {
        Some(Action::Stop)
    } else {
        None
    }
}

fn buttons(y: i32, content_left: i32, content_width: u32) -> ButtonRowLayout {
    let (start, stop) = horizontal_pair(content_left, y, content_width, BUTTON_HEIGHT, BUTTON_GAP);

    ButtonRowLayout { start, stop }
}

fn ideal_row(bounds: Rectangle) -> IdealRowLayout {
    let label_width = IDEAL_LABEL_WIDTH.min(bounds.size.width);
    let value_width = bounds.size.width.saturating_sub(label_width);

    IdealRowLayout {
        bounds,
        label: Rectangle::new(bounds.top_left, Size::new(label_width, bounds.size.height)),
        value: Rectangle::new(
            bounds.top_left + Point::new(label_width as i32, 0),
            Size::new(value_width, bounds.size.height),
        ),
    }
}

fn network_row(bounds: Rectangle) -> NetworkRowLayout {
    NetworkRowLayout {
        bounds,
        icon_center: bounds.top_left + NETWORK_ICON_OFFSET,
        text_with_icon_origin: bounds.top_left + Point::new(NETWORK_TEXT_WITH_ICON_OFFSET_X, 0),
        text_without_icon_origin: bounds.top_left,
    }
}

pub(crate) fn render<D>(
    state: &mut State,
    context: AppContext,
    display: &mut D,
    _scratch: &mut [Rgb565],
    ui_layout: &Layout,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    let title_font = ui_font!(BOLD);
    let body_font = ui_font!(500);
    let top_text_style = TextStyleBuilder::new().baseline(Baseline::Top).build();

    draw_panel(
        display,
        ui_layout.header.panel,
        CARD_RADIUS,
        SURFACE,
        SURFACE_BORDER,
    )?;

    let title_style = BitmapFontStyleBuilder::new()
        .text_color(TEXT_PRIMARY)
        .background_color(SURFACE)
        .font(&title_font)
        .build();
    Text::with_text_style(
        "Stop Watch",
        ui_layout.header.title_origin,
        title_style,
        top_text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;

    draw_button(
        display,
        ui_layout.buttons.start,
        "START",
        !state.running,
        ButtonTone::Start,
    )?;
    draw_button(
        display,
        ui_layout.buttons.stop,
        "STOP",
        state.running,
        ButtonTone::Stop,
    )?;

    let body_style = BitmapFontStyleBuilder::new()
        .text_color(TEXT_SECONDARY)
        .background_color(OLED_BLACK)
        .font(&body_font)
        .build();

    clear_rect(display, ui_layout.info.ideal.bounds)?;
    Text::with_text_style(
        "ideal",
        ui_layout.info.ideal.label.top_left,
        body_style.clone(),
        top_text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;
    draw_duration(
        display,
        ui_layout.info.ideal.value.top_left,
        Duration::from_secs(state.seconds),
        body_style.clone(),
    )?;

    let mut stopwatch = heapless::String::<32>::new();
    write!(stopwatch, "stopwatch: {}s", state.seconds).map_err(|_| RenderError::TextFormat)?;
    clear_rect(display, ui_layout.info.stopwatch)?;
    Text::with_text_style(
        &stopwatch,
        ui_layout.info.stopwatch.top_left,
        body_style.clone(),
        top_text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;

    let network_text_origin = if context.network_status == NetworkStatus::Online {
        ui_layout.info.network.text_without_icon_origin
    } else {
        ui_layout.info.network.text_with_icon_origin
    };
    clear_rect(display, ui_layout.info.network.bounds)?;
    if context.network_status != NetworkStatus::Online {
        draw_network_unavailable_icon(display, ui_layout.info.network.icon_center)?;
    }
    Text::with_text_style(
        network_text(context.network_status),
        network_text_origin,
        body_style.clone(),
        top_text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;

    let mut interactions = heapless::String::<32>::new();
    write!(interactions, "interactions: {}", context.interaction_count)
        .map_err(|_| RenderError::TextFormat)?;
    clear_rect(display, ui_layout.info.interactions)?;
    Text::with_text_style(
        &interactions,
        ui_layout.info.interactions.top_left,
        body_style,
        top_text_style,
    )
    .draw(display)
    .map_err(RenderError::Draw)?;

    Ok(())
}

fn network_text(status: NetworkStatus) -> &'static str {
    match status {
        NetworkStatus::Offline => "network: offline",
        NetworkStatus::Connecting => "network: connecting",
        NetworkStatus::Online => "network: online",
    }
}

fn draw_network_unavailable_icon<D>(
    display: &mut D,
    center: Point,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>,
{
    Circle::with_center(center, NETWORK_ICON_DIAMETER)
        .into_styled(PrimitiveStyle::with_stroke(TEXT_SECONDARY, 2))
        .draw(display)
        .map_err(RenderError::Draw)?;

    Line::new(center + Point::new(-8, 8), center + Point::new(8, -8))
        .into_styled(PrimitiveStyle::with_stroke(TEXT_SECONDARY, 2))
        .draw(display)
        .map_err(RenderError::Draw)
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
        ui_layout.buttons.start.center(),
        ui_layout.buttons.stop.center(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::SCREEN_BOUNDS;

    #[test]
    fn hit_tests_existing_stopwatch_points() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let (start, stop) = button_centers(SCREEN_BOUNDS);

        assert_eq!(
            hit_test(&ui_layout, start, &State::new()),
            Some(Action::Start)
        );
        assert_eq!(hit_test(&ui_layout, stop, &State::new()), None);

        let mut running = State::new();
        running.handle(Action::Start, 0);
        assert_eq!(hit_test(&ui_layout, start, &running), None);
        assert_eq!(hit_test(&ui_layout, stop, &running), Some(Action::Stop));
    }

    #[test]
    fn info_rows_are_allocated_as_a_column() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let rows = [
            ui_layout.info.ideal.bounds,
            ui_layout.info.stopwatch,
            ui_layout.info.network.bounds,
            ui_layout.info.interactions,
        ];
        let step = INFO_ROW_HEIGHT as i32 + INFO_ROW_GAP;

        for pair in rows.windows(2) {
            assert_eq!(pair[1].top_left.y - pair[0].top_left.y, step);
            assert_eq!(pair[0].top_left.x, pair[1].top_left.x);
            assert_eq!(pair[0].size.height, INFO_ROW_HEIGHT);
        }
        assert_eq!(rows[3].size.height, INFO_ROW_HEIGHT);
    }

    #[test]
    fn layout_offsets_rectangles_from_supplied_bounds() {
        let base = layout(SCREEN_BOUNDS);
        let shifted = layout(Rectangle::new(Point::new(10, 20), crate::DISPLAY_SIZE));

        assert_eq!(
            shifted.header.title_origin - base.header.title_origin,
            Point::new(10, 20)
        );
        assert_eq!(
            shifted.buttons.start.top_left - base.buttons.start.top_left,
            Point::new(10, 20)
        );
        assert_eq!(
            shifted.buttons.stop.top_left - base.buttons.stop.top_left,
            Point::new(10, 20)
        );
        assert_eq!(
            shifted.info.ideal.bounds.top_left - base.info.ideal.bounds.top_left,
            Point::new(10, 20)
        );
    }

    #[test]
    fn ideal_and_network_subrects_keep_render_offsets_in_layout() {
        let ui_layout = layout(SCREEN_BOUNDS);

        assert_eq!(
            ui_layout.info.ideal.value.top_left.x - ui_layout.info.ideal.label.top_left.x,
            IDEAL_LABEL_WIDTH as i32
        );
        assert_eq!(
            ui_layout.info.network.text_with_icon_origin.x
                - ui_layout.info.network.text_without_icon_origin.x,
            NETWORK_TEXT_WITH_ICON_OFFSET_X
        );
        assert_eq!(
            ui_layout.info.network.icon_center - ui_layout.info.network.bounds.top_left,
            NETWORK_ICON_OFFSET
        );
    }

    #[test]
    fn state_tracks_elapsed_running_seconds() {
        let mut state = State::new();

        state.handle(Action::Start, 0);
        assert!(state.on_tick(1_000));
        assert!(state.on_tick(3_000));
        assert_eq!(state.seconds, 3);
        assert!(state.running);

        state.handle(Action::Stop, 5_000);
        assert_eq!(state.seconds, 5);
        assert!(!state.running);
        assert!(!state.on_tick(20_000));
        assert_eq!(state.seconds, 5);
    }
}
