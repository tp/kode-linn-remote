# Kode Linn Remote

Rust workspace for a handheld Linn hi-fi remote targeting the
[Kode Dot](https://kode.diy/product/kode-dot), with a shared application core
and a Mac simulator for fast UI iteration.

The app core and simulator target the Kode Dot's 410 x 502 portrait AMOLED and
its directional pad.

## Target Hardware

The **ESP32-P4 revision** (November 2026 batch):

- ESP32-P4 application processor with an ESP32-C5 wireless co-processor
- 32 MB PSRAM, 32 MB flash
- Dual-band Wi-Fi 2.4 / 5 GHz, Bluetooth LE 5, Thread, Zigbee
- 2.13" AMOLED touchscreen, 410 x 502, mounted portrait
- Directional pad plus two control buttons
- 9-axis IMU (LSM6DSV + LIS2MDL), NFC, RFID, IR, speaker, microphone,
  haptics, RGB LED, microSD

> Kode publishes the panel as "a crisp 502x410 touch panel" — that is its
> native scan resolution, long side first. It is mounted portrait, screen above
> the pad, so the framebuffer is 410 wide by 502 tall. Writing it the other way
> round transposes every layout.
>
> The vendor documentation at <https://docs.kode.diy> still describes the
> earlier **ESP32-S3** revision. Its pin maps and drivers do not transfer, so
> board facts carried over from it are marked `Confidence::Provisional` in
> `crates/board-kode-dot` — those are the ones to re-check against real
> hardware.

## What Is Here

- `crates/app-core`: hardware-independent app state, events, and
  `embedded-graphics` rendering. `no_std`.
- `crates/board-kode-dot`: Kode Dot board facts — display geometry, input
  model, peripheral part numbers, each tagged with how far it can be trusted.
- `apps/sim`: native macOS simulator using AppKit through `objc2`.
- `apps/firmware`: **legacy** ESP32-C6 firmware for the retired Waveshare
  board, kept for its CO5300 QSPI display driver. There is no Kode Dot
  firmware yet.
- `crates/board-waveshare-c6`: board facts for that retired board.

## Run The Simulator

```sh
cargo run -p sim
```

Controls:

- Click inside the display area to send a tap to the shared app core.
- Drive the on-screen directional pad, or use the **arrow keys**. The pad moves
  a focus ring between the controls on the current screen.
- **Select** (or Return) activates the focused control; **Back** (or Escape)
  goes up one level.
- Use the network dropdown to choose the mocked network status.
- `Advance +1s` adds a manual tick; `Zoom 2x` enlarges the framebuffer without
  changing what the embedded renderer produces.

## Look At The Screens Without A Window

Renders every screen to PNG at the panel's real resolution and exits:

```sh
cargo run -p sim -- --snapshot target/snapshots
```

Useful for reviewing a layout change, and it needs no display or hardware.

## Check The Core

```sh
cargo test -p app-core
cargo check
```

## Reference Material

- Kode Dot product page: <https://kode.diy/product/kode-dot>
- Kode Dot docs (ESP32-S3 revision): <https://docs.kode.diy>
- Kode DIY GitHub: <https://github.com/kodediy>
- Espressif Rust book: <https://docs.espressif.com/projects/rust/book/>
- Awesome ESP Rust: <https://github.com/esp-rs/awesome-esp-rust>
