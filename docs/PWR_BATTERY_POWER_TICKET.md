# PWR battery power-off follow-up

Status: deferred until the current branch is cleaned up

Implement the Waveshare PWR button battery power-control path so the handheld
can turn itself off instead of running until the battery is empty.

## Context

The Waveshare ESP32-C6 Touch AMOLED 1.43 has two side buttons: BOOT and PWR.
The current firmware only handles BOOT on GPIO9 as an app button. When a battery
is connected, the firmware enables the TCA9554 GPIO expander pins used by the
board power path, but it never handles the PWR button as a shutdown request or
releases the battery power latch.

Waveshare documents the PWR button as program-controlled battery power control.
Their `07_BATT_PWR_Test` example powers on by holding PWR on battery, then powers
off by holding PWR again after firmware releases the control path. USB power will
keep the board alive, so the off behavior must be tested with USB disconnected.

Relevant references:

- https://docs.waveshare.com/ESP32-C6-Touch-AMOLED-1.43
- https://docs.waveshare.com/ESP32-C6-Touch-AMOLED-1.43/Development-Environment-Setup-ESP-IDF
- https://files.waveshare.com/wiki/ESP32-C6-Touch-AMOLED-1.43/ESP32-C6-Touch-AMOLED-1.43_Rev1.3.pdf

## Proposed Design

- Record the board facts in `crates/board-waveshare-c6`: PWR button signal,
  battery power latch signal, TCA9554 pin mapping, active levels, and any hold
  timing requirement confirmed from the schematic or Waveshare demo source.
- Replace the current magic TCA9554 writes in firmware with named board-support
  helpers for enabling and releasing battery power.
- Add firmware handling for the PWR button as a long-press shutdown request.
  Keep BOOT as the app/navigation button.
- On PWR long press, shut down user-visible peripherals first where practical:
  stop rendering, turn off AMOLED brightness/panel power, quiet runtime/network
  work, then release the battery power latch.
- Do not route PWR through `app-core` as normal app input unless the app needs a
  visible confirmation state. The actual power latch behavior is board/firmware
  responsibility.

## Acceptance Criteria

- With USB disconnected and battery connected, holding PWR after boot turns the
  board fully off instead of leaving it running.
- BOOT/GPIO9 behavior remains unchanged for app navigation.
- Power latch setup uses named constants/helpers rather than raw TCA9554 register
  writes in `main.rs`.
- Short accidental PWR presses do not immediately power off the device.
- Firmware logs make power-control state transitions diagnosable while USB is
  connected, even though USB prevents true power-off.
- Verification passes with `cargo fmt --check`, `cargo check`,
  `cargo test -p app-core`, and
  `cargo check -p firmware --target riscv32imac-unknown-none-elf --release`.

## Notes

- Test true off behavior on battery only; USB VBUS bypasses the battery-off user
  experience.
- Be conservative about I2C ownership: the TCA9554 is shared with display/touch
  support, so the final implementation should avoid competing bus access.
- Keep hardware facts out of `app-core`; use board support and firmware code for
  power-control details.
