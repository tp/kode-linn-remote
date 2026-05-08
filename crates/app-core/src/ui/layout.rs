use embedded_graphics::{prelude::*, primitives::Rectangle};

use crate::DISPLAY_SIZE;

pub(crate) const SCREEN_BOUNDS: Rectangle = Rectangle::new(Point::zero(), DISPLAY_SIZE);

const PANEL_INSET: i32 = 24;
const CONTENT_INSET: i32 = 44;
const HEADER_HEIGHT: u32 = 92;
const HEADER_TITLE_TOP: i32 = 24;
const HEADER_BUTTON_GAP: i32 = 24;
const BUTTON_HEIGHT: u32 = 72;
const BUTTON_GAP: i32 = 38;
const BUTTON_INFO_GAP: i32 = 32;
const INFO_ROW_HEIGHT: u32 = 40;
const INFO_ROW_GAP: i32 = 4;
const IDEAL_LABEL_WIDTH: u32 = 96;
const NETWORK_ICON_OFFSET: Point = Point::new(11, 20);
const NETWORK_TEXT_WITH_ICON_OFFSET_X: i32 = 34;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct UiLayout {
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
struct Column {
    x: i32,
    y: i32,
    width: u32,
    gap: i32,
}

impl Column {
    const fn new(x: i32, y: i32, width: u32, gap: i32) -> Self {
        Self { x, y, width, gap }
    }

    #[must_use]
    fn take(&mut self, height: u32) -> Rectangle {
        let rect = Rectangle::new(Point::new(self.x, self.y), Size::new(self.width, height));
        self.skip(height as i32 + self.gap);
        rect
    }

    fn skip(&mut self, height: i32) {
        self.y = self.y.saturating_add(height);
    }
}

pub(crate) fn layout(bounds: Rectangle) -> UiLayout {
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

    UiLayout {
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

fn buttons(y: i32, content_left: i32, content_width: u32) -> ButtonRowLayout {
    let button_width = content_width.saturating_sub(BUTTON_GAP as u32) / 2;

    ButtonRowLayout {
        start: Rectangle::new(
            Point::new(content_left, y),
            Size::new(button_width, BUTTON_HEIGHT),
        ),
        stop: Rectangle::new(
            Point::new(content_left + button_width as i32 + BUTTON_GAP, y),
            Size::new(button_width, BUTTON_HEIGHT),
        ),
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let shifted = layout(Rectangle::new(Point::new(10, 20), DISPLAY_SIZE));

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
}
