mod hit_test;
mod layout;
mod render;
mod style;

pub(crate) use hit_test::{InteractionState, UiAction, hit_test};
pub(crate) use layout::{SCREEN_BOUNDS, layout};

#[cfg(test)]
pub(crate) fn button_centers() -> (
    embedded_graphics::geometry::Point,
    embedded_graphics::geometry::Point,
) {
    let ui_layout = layout(SCREEN_BOUNDS);

    (
        ui_layout.buttons.start.center(),
        ui_layout.buttons.stop.center(),
    )
}
