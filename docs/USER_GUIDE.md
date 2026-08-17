# User Guide

This project currently provides a desktop demo of the Kode Dot Linn remote interface. The demo is meant for day-to-day UI and app logic work before the hardware arrives — and right now it is the only place the project runs, because `esp-hal` cannot yet target the Kode Dot's ESP32-P4.

## Start The Demo

```sh
cargo run -p sim
```

The simulator no longer uses SDL2. It uses a native macOS AppKit window through Rust `objc2` bindings.

## Use The Demo

- The window represents the 410 x 502 portrait AMOLED display on the Kode Dot.
- The simulator opens at native physical-pixel scale for the current Mac display.
- Use `Zoom 2x` to enlarge the device framebuffer without changing what the embedded renderer produces. The window reserves space for 2x mode, so toggling zoom does not resize the window.
- Click anywhere inside the display area to simulate a tap.
- Use the on-screen Start and Stop controls to run or pause the stopwatch.
- The simulator-only tap highlight stays bright for one second and then fades.
- Use the on-screen directional pad, or the **arrow keys**, to move the focus ring between the controls on the current screen.
- Use **Select** (or Return) to activate the focused control, and **Back** (or Escape) to go up one level.
- Running the pad off the top or bottom of a HiFi page turns to the next page.
- Use the network dropdown to choose the mocked network status.
- Time advances automatically; use `Advance +1s` when you want an extra manual tick.
- Use the debug panel to compare core render requests with simulator redraws.
- Close the window to exit.

## What The Demo Shows

- Start and stop controls.
- Stopwatch seconds, which only tick while running.
- Mocked network status.
- Interaction count.
- Recent tap location.

## Review A Layout Without A Window

```sh
cargo run -p sim -- --snapshot target/snapshots
```

Writes a PNG per screen at the panel's real resolution and exits. No display or
hardware needed.

The simulator and firmware use the same `app-core` crate, so application
behavior should stay consistent between the Mac and the device.

## Flashing The Device

Not yet possible. `esp-hal` 1.1.2 has no `esp32p4` support, so there is no
firmware for the Kode Dot. `apps/firmware` still targets the retired Waveshare
ESP32-C6 board and is kept only for its display driver — its 466 x 466 geometry
no longer matches what `app-core` renders, so flashing it would produce a
misaligned screen.

The board facts still to confirm on first bring-up are the ones tagged
`Confidence::Provisional` in `crates/board-kode-dot`.
