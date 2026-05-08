# User Guide

This project currently provides a desktop demo of the ESP32-C6 home tools interface. The demo is meant for day-to-day UI and app logic work before flashing the real board.

## Start The Demo

```sh
cargo run -p sim
```

The simulator no longer uses SDL2. It uses a native macOS AppKit window through Rust `objc2` bindings.

## Use The Demo

- The window represents the 466 x 466 AMOLED display on the Waveshare board.
- Use the touch buttons to simulate fixed touch points.
- Use the Boot and User buttons to mimic hardware button inputs.
- Use the network buttons to cycle the mocked network status.
- Use `Tick +1s` to advance mocked uptime.
- Close the window to exit.

## What The Demo Shows

- Device uptime.
- Mocked network status.
- Interaction count.
- Current touch state.

The simulator and firmware use the same `app-core` crate, so application behavior should stay consistent between the Mac and the ESP32-C6.

## Flashing The Device

Once the board arrives, connect it over USB and run:

```sh
cargo run -p firmware --target riscv32imac-unknown-none-elf --release
```

The first firmware milestone only proves that the Rust firmware starts and can run the shared app core. Display and touch output will be added after validating the board wiring.
