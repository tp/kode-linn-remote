# ESP32-C6 Home Tools

Rust workspace for home tools targeting the Waveshare ESP32-C6 Touch AMOLED 1.43 board, with a shared application core and a Mac simulator for fast UI iteration.

## What Is Here

- `crates/app-core`: hardware-independent app state, events, and `embedded-graphics` rendering.
- `crates/board-waveshare-c6`: board facts and future board support code for the Waveshare ESP32-C6 Touch AMOLED 1.43.
- `apps/sim`: native macOS simulator using AppKit through `objc2`.
- `apps/firmware`: ESP32-C6 firmware entrypoint using `esp-hal`.

## Run The Simulator

Run the demo:

```sh
cargo run -p sim
```

Controls:

- Use the simulator buttons to mimic touch points, button presses, ticks, and network status.
- Close the window to quit.

## Check The Core

```sh
cargo test -p app-core
cargo check
```

## Firmware Bring-Up

This workspace uses current Espressif Rust crates, which require Rust 1.88 or newer.

Install the ESP flashing tool:

```sh
cargo install espflash
```

Check the firmware for the ESP32-C6 target:

```sh
cargo check -p firmware --target riscv32imac-unknown-none-elf --release
```

When the board is connected over USB, flash and monitor:

```sh
cargo run -p firmware --target riscv32imac-unknown-none-elf --release
```

The current firmware initializes the HAL and shared app core. Display and touch bring-up are intentionally left as the next hardware step after confirming the exact board wiring from the Waveshare schematic and examples.

## Reference Material

- Espressif Rust book: https://docs.espressif.com/projects/rust/book/
- Awesome ESP Rust: https://github.com/esp-rs/awesome-esp-rust
- Awesome Embedded Rust: https://github.com/rust-embedded/awesome-embedded-rust
- Waveshare board docs: https://docs.waveshare.com/ESP32-C6-Touch-AMOLED-1.43
