## Display rendering performance follow-up

Status: deferred until hardware integration is complete

Improve firmware display rendering performance once the Kode Dot display path exists. Full-screen redraws will become a bottleneck as soon as touch, Wi-Fi, sensors, and runtime state updates are active.

### Context

The Kode Dot panel is a 2.13" AMOLED, 410x502 portrait. One full RGB565 frame is about 402 KiB.

**This ticket cannot start yet.** `esp-hal` has no ESP32-P4 support, so there is no firmware display path to optimize — see `KODE_DOT_PORT_TICKET.md`. Several of its assumptions also need re-checking against the ESP32-P4 revision before any of the design below is valid:

- **Transport.** The ESP32-S3 revision drove the panel through a CO5300 over QuadSPI. The ESP32-P4 has a MIPI-DSI host and may drive it differently, which would make the SPI/DMA work below moot.
- **Memory budget.** Full double buffering was not realistic on the older internal-RAM budget. The Kode Dot has 32 MB of PSRAM, so two 402 KiB framebuffers are a rounding error against it. Much of the tile-based complexity below may simply be unnecessary — check that before porting any of it.
- **Alignment — now a driver requirement, not just a caveat.** The CO5300 required even-aligned write windows of at least two scanlines, and the old driver had no framebuffer, so it satisfied that by widening each fill and painting the extra rows. That is lossy: a fill starting on an odd row grows upward over the row above it. The simulator reproduced this faithfully and it was silently eating the bottom row of glyphs whose ink ended on an even row, making bars and stems render about a pixel thin.

  The Kode Dot's driver **must** therefore compose a full frame in PSRAM and blit it, aligning the blit window without disturbing its contents, rather than widening individual fills. With 32 MB available this costs nothing and removes the whole class of bug. The simulator now models that framebuffer behaviour; `app-core`'s painter still asserts 2-px alignment for scratch blits, which is a separate concern about where scratch buffers land.

### Proposed Design

- Add lightweight render timing logs around `app.render()` in firmware to establish baseline frame times before changing the pipeline.
- If the panel is still QSPI-driven, convert the firmware display transport from blocking non-DMA SPI to blocking DMA SPI using `esp-hal`'s `SpiDmaBus::half_duplex_write`.
- Use static DMA buffers/descriptors sized for meaningfully larger display chunks while staying within RAM constraints.
- Keep whatever command/address/data sequence and window-alignment rule the shipping controller requires.
- Add dirty-region rendering in `app-core` so small state changes do not redraw the full screen.
- Represent render requests as full-frame redraws or bounded dirty rectangles, while keeping screen transitions as full redraws.
- Start dirty-region coverage with high-value cases:
  - stopwatch elapsed-time ticks,
  - hi-fi elapsed/progress updates,
  - loading spinner animation,
  - small status text changes.
- Let the simulator consume the same dirty-region data so shared rendering behavior remains testable.

### Acceptance Criteria

- Firmware logs render duration for initial and update frames in a way that can be compared before and after optimization.
- Display writes use DMA-backed half-duplex QSPI transfers for color payloads.
- The driver still renders text without split glyphs or alignment artifacts.
- Startup still renders the first frame before enabling full brightness, avoiding the initial green flash.
- `app-core` exposes render outcomes that can distinguish full redraws from dirty rectangles without depending on firmware-only APIs.
- Dirty rendering updates the stopwatch and hi-fi screens without full-screen clears for routine tick/progress updates.
- Screen transitions and layout-changing updates still force full redraws.
- Verification passes with `cargo fmt --check`, `cargo check`, `cargo test -p app-core`, and `cargo check -p firmware --target riscv32imac-unknown-none-elf --release`.

### Notes

- Do not begin this before a Kode Dot firmware exists at all, and then not before touch input, Wi-Fi/runtime integration, and remaining board peripherals are usable enough to exercise real UI updates.
- Prefer blocking DMA first; async rendering can be considered later only if blocking DMA plus dirty regions are insufficient.
- PSRAM is confirmed at 32 MB, but still justify a full framebuffer by measured performance rather than assuming it — PSRAM bandwidth, not capacity, is the likely constraint.
- Keep hardware-specific controller and DMA details in `apps/firmware` or board-support code; keep `app-core` deterministic and `no_std`.
