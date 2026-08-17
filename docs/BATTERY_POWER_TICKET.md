# Battery power control follow-up

Status: blocked — no Kode Dot firmware exists yet (see `KODE_DOT_PORT_TICKET.md`).

Implement battery power control so the handheld can turn itself off and report
its charge state, instead of running until the battery is empty.

## Context

The Kode Dot carries a rechargeable LiPo pack, charges over USB-C, and — on the
ESP32-S3 revision — used a **BQ25896** PMIC for charging and power path plus a
**BQ27220** fuel gauge for state-of-charge. Both are unconfirmed for the
ESP32-P4 revision this project targets, and the vendor publishes drivers for
them only against the older board:

- <https://github.com/kodediy/kode_bq27220-idf>
- <https://github.com/kodediy/kode_BQ27220>

Unlike the round board this project started on, the Kode Dot has no documented
dedicated power key. It exposes a directional pad and two control buttons, so
whatever triggers power-off has to be a deliberate gesture on those — a long
press, or a confirmed UI action — rather than a separate latch button.

USB power keeps the board alive regardless, so off behavior must be tested with
USB disconnected.

## Proposed Design

- Record the confirmed PMIC and fuel-gauge parts, their I2C addresses, and the
  power-path control signals in `crates/board-kode-dot`, replacing the
  `Confidence::Provisional` entries currently carried over from the ESP32-S3
  revision.
- Add board-support helpers for reading state-of-charge, voltage, current and
  charging status, and for releasing the power path.
- Surface battery state to `app-core` as an event, so the UI can show charge
  level and a low-battery warning. Keep the power-path mechanics in board
  support and firmware.
- Add a deliberate power-off gesture on the pad or control buttons, with a
  visible confirmation state so an accidental press cannot silently kill the
  device mid-use.
- On power-off, shut down user-visible peripherals first where practical: stop
  rendering, turn off AMOLED brightness or panel power, quiet runtime and
  network work, then release the power path.

## Acceptance Criteria

- With USB disconnected and battery connected, the power-off gesture turns the
  board fully off instead of leaving it running.
- Battery state-of-charge is readable and surfaced to the UI.
- Power-path control uses named constants and helpers rather than raw register
  writes in `main.rs`.
- Short accidental presses do not power off the device.
- Firmware logs make power-control state transitions diagnosable while USB is
  connected, even though USB prevents true power-off.
- Verification passes with `cargo fmt --check`, `cargo check`, and
  `cargo test -p app-core`.

## Notes

- Test true off behavior on battery only; USB VBUS bypasses the battery-off
  user experience.
- Be conservative about I2C ownership: the PMIC, fuel gauge, touch controller,
  RTC and any I/O expander may share a bus, so avoid competing access.
- Keep hardware facts out of `app-core`; use board support and firmware code
  for power-control details.
