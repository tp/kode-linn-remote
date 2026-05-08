use embedded_graphics::prelude::*;

use super::layout::UiLayout;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UiAction {
    StartStopwatch,
    StopStopwatch,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct InteractionState {
    pub(crate) stopwatch_running: bool,
}

pub(crate) fn hit_test(
    layout: &UiLayout,
    point: Point,
    state: InteractionState,
) -> Option<UiAction> {
    if layout.buttons.start.contains(point) && !state.stopwatch_running {
        Some(UiAction::StartStopwatch)
    } else if layout.buttons.stop.contains(point) && state.stopwatch_running {
        Some(UiAction::StopStopwatch)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::layout::{SCREEN_BOUNDS, layout};

    #[test]
    fn default_layout_hit_tests_existing_stopwatch_points() {
        let ui_layout = layout(SCREEN_BOUNDS);
        let (start, stop) = crate::ui::button_centers();
        let stopped = InteractionState {
            stopwatch_running: false,
        };
        let running = InteractionState {
            stopwatch_running: true,
        };

        assert_eq!(
            hit_test(&ui_layout, start, stopped),
            Some(UiAction::StartStopwatch)
        );
        assert_eq!(hit_test(&ui_layout, stop, stopped), None);
        assert_eq!(hit_test(&ui_layout, start, running), None);
        assert_eq!(
            hit_test(&ui_layout, stop, running),
            Some(UiAction::StopStopwatch)
        );
    }
}
