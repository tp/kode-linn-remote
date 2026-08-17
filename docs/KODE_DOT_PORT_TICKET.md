# Kode Dot Port

## Status

The shared app core and the simulator now target the Kode Dot. Firmware does
not, and cannot yet.

## What Moved

- `crates/board-kode-dot` is the single source of truth for display geometry
  and the input model. `app-core` re-exports `DISPLAY_SIZE` from it, so
  correcting the resolution there reflows every screen.
- Screens were re-laid out for a 410 x 502 portrait rectangle. The round
  board's safe-area machinery is gone: `centered_square` and the HiFi
  `ROUND_SAFE_SQUARE_SIZE` body are removed, and pages use the full
  framebuffer minus a cosmetic inset.
- The HiFi volume readout was a 270-degree ring tracing the round panel edge.
  A rectangle has no edge to trace, so it is now a slim bar pinned along the
  bottom, below every page body so page-change clears do not erase it.
- `Button` went from `{Boot, User}` to `{Up, Down, Left, Right, Select, Back}`.
  The pad moves a focus ring; `Select` replays the focused control as a tap at
  its centre so pad and touch share one dispatch path.

## Blocked: Rust Firmware For The ESP32-P4

`esp-hal` 1.1.2 does not support the ESP32-P4. Its chip features are `esp32`,
`esp32c2`, `esp32c3`, `esp32c5`, `esp32c6`, `esp32c61`, `esp32h2`, `esp32s2`
and `esp32s3` — there is no `esp32p4`.

Consequences:

- `apps/firmware` still targets the retired Waveshare ESP32-C6 board. It
  compiles, but `app-core` now renders 410 x 502 while
  `board-waveshare-c6::DISPLAY_SIZE` is 466 x 466, so flashing it would
  produce a misaligned screen. It is kept for the CO5300 QSPI driver, not to
  be run.
- Wi-Fi on the P4 comes from the ESP32-C5 co-processor rather than an on-die
  radio, so `esp-radio` does not apply either; that path needs an
  `esp-hosted`-style link once a HAL exists.

Re-check `esp-hal` for `esp32p4` before starting firmware work. Until then the
simulator is the only place this project runs.

## Open Questions For First Bring-Up

Everything marked `Confidence::Provisional` in `crates/board-kode-dot`:

1. **Display controller and interface.** The S3 revision used a CO5300 over
   QuadSPI. The P4 has a MIPI-DSI host and may drive the panel differently,
   which would also invalidate the 2-px write-window alignment the simulator's
   `Framebuffer::fill_solid` currently mirrors.
2. **Touch controller.** CST820 over I2C at 0x15 on the S3 revision.
3. **Button wiring.** On the S3 revision the pad sat behind a TCA95xx I/O
   expander at 0x20 with one key wired straight to GPIO0. The P4 revision
   advertises "two control buttons", which `board-kode-dot` models as
   `Select` and `Back`; confirm which physical key is which.

Settled, and no longer open:

- **Panel geometry.** 410 x 502, portrait. Kode publishes it as "502x410" —
  the native scan resolution, long side first — but the panel is mounted with
  the screen above the pad, so the framebuffer is 410 wide. Do not transpose
  it.
- **Memory.** 32 MB PSRAM and 32 MB flash.
