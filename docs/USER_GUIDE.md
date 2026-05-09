# User Guide

This project currently provides a desktop demo of the ESP32-C6 home tools interface. The demo is meant for day-to-day UI and app logic work before flashing the real board.

## Start The Demo

```sh
cargo run -p sim
```

The simulator no longer uses SDL2. It uses a native macOS AppKit window through Rust `objc2` bindings.

## Use The Demo

- The window represents the 466 x 466 AMOLED display on the Waveshare board.
- The simulator opens at native physical-pixel scale for the current Mac display.
- Use `Zoom 2x` to enlarge the device framebuffer without changing what the embedded renderer produces. The window reserves space for 2x mode, so toggling zoom does not resize the window.
- Click anywhere inside the display area to simulate a tap.
- Use the on-screen Start and Stop controls to run or pause the stopwatch.
- The simulator-only tap highlight stays bright for one second and then fades.
- Use the Boot and User buttons to mimic hardware button inputs.
- Use the network dropdown to choose the mocked network status.
- Use the display shape dropdown to switch between the round hardware mask and the full rectangular framebuffer.
- Time advances automatically; use `Advance +1s` when you want an extra manual tick.
- Use the debug panel to compare core render requests with simulator redraws.
- Close the window to exit.

## What The Demo Shows

- Start and stop controls.
- Stopwatch seconds, which only tick while running.
- Mocked network status.
- Interaction count.
- Recent tap location.

The simulator and firmware use the same `app-core` crate, so application behavior should stay consistent between the Mac and the ESP32-C6.

## Flashing The Device

Connect the board over USB and run:

```sh
cargo run -p firmware --target riscv32imac-unknown-none-elf --release
```

The first firmware milestone proves that the ESP-IDF bootloader accepts the Rust
image, the firmware starts, and the shared app core can receive ticks. A
successful boot prints a serial banner followed by one-second heartbeat lines:

```text
boot: Waveshare ESP32-C6 Touch AMOLED 1.43
display: CO5300 466x466
touch: FT6146, imu: QMI8658, rtc: PCF85063, gpio expander: TCA9554
app-core: initialized
heartbeat: uptime=0ms interactions=0 redraw=false
```

Display and touch output will be added after validating the board wiring.
