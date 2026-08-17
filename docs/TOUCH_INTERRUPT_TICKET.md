# Touch interrupt follow-up

Status: deferred until remaining firmware hardware bring-up is complete

Use interrupt-driven touch handling rather than constant polling, for lower idle power use. Polling is fine for validating I2C access, coordinates, and app-core touch events during bring-up, but constant 50 ms polling should not be the final battery-oriented design.

## Context

A polling touch driver reads the controller over the shared I2C bus and emits `app-core` `TouchDown`/`TouchUp` events on state transitions. That keeps repeated button activation under control, but it wakes the CPU and uses the I2C bus continuously even when the user is not touching the screen.

Touch controllers of this class expose an interrupt signal when touch state changes. Use that signal once the rest of the board hardware is integrated enough that GPIO ownership and low-power behavior can be designed cleanly.

The touch controller on the Kode Dot's ESP32-S3 revision was a CST820 on I2C at address 0x15. That is unconfirmed for the ESP32-P4 revision this project targets, and there is no Kode Dot firmware yet — see `KODE_DOT_PORT_TICKET.md`.

## Proposed Design

- Confirm the touch controller part and its interrupt pin for the ESP32-P4 revision from the schematic or vendor source.
- Add the touch interrupt pin to board-support documentation/constants.
- Configure the interrupt pin as an input with the correct pull mode and edge trigger.
- Keep the I2C register reader and touch transition logic.
- Use the interrupt to start an active-touch polling window.
- Poll at a short interval only while touch is active, so movement/release can be detected.
- Return to idle after release.
- Keep a slow fallback poll only if the interrupt line proves unreliable on this board.

## Acceptance Criteria

- Idle firmware does not poll the touch controller every 50 ms.
- Touching the screen wakes the touch path through the interrupt pin.
- The app still receives exactly one `TouchDown` per press and one `TouchUp` per release.
- Launcher navigation and screen controls still work on hardware.
- Sporadic I2C NACKs are handled as dropped samples, not fatal errors.
- Verification passes with `cargo fmt --check`, `cargo check`, `cargo test -p app-core`, and `cargo check -p firmware --target riscv32imac-unknown-none-elf --release`.

## Notes

- Do not start this before the remaining hardware integrations clarify GPIO usage.
- Avoid changing app-core semantics unless needed; this should mostly be firmware and board-support work.
- Keep polling mode available behind a small fallback path or compile-time option until interrupt behavior is proven stable.
- The Kode Dot's directional pad may sit behind an I2C I/O expander, as it did on the ESP32-S3 revision. If so, pad input has the same idle-polling problem and should be solved alongside touch rather than separately.
