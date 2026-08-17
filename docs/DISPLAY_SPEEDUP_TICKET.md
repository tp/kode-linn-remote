## Display rendering performance follow-up

Status: deferred until hardware integration is complete

Improve firmware display rendering performance after the remaining board hardware is integrated and stable. The current CO5300 display path is correct enough for bring-up, but full-screen redraws and non-DMA SPI writes will become a bottleneck once touch, Wi-Fi, sensors, and runtime state updates are active.

### Context

The Waveshare ESP32-C6 Touch AMOLED 1.43 display is a 466x466 RGB565 panel driven through the CO5300 over QSPI. One full frame is about 424 KiB, so full double buffering is not realistic on the ESP32-C6 internal RAM budget. The driver currently uses small blocking SPI writes because the non-DMA SPI FIFO limits practical transfer chunks.

The panel also has a board-specific alignment constraint discovered during bring-up: color writes must use even-aligned windows with at least two scanlines. Any optimization must preserve that behavior; previous naive tile batching caused glitched text.

### Proposed Design

- Add lightweight render timing logs around `app.render()` in firmware to establish baseline frame times before changing the pipeline.
- Convert the firmware display transport from blocking non-DMA SPI to blocking DMA SPI using `esp-hal`'s `SpiDmaBus::half_duplex_write`.
- Use static DMA buffers/descriptors sized for meaningfully larger display chunks while staying within RAM constraints.
- Keep the existing CO5300 command/address/data sequence and the even-window/two-scanline alignment rule.
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

- Do not begin this before touch input, Wi-Fi/runtime integration, and remaining board peripherals are usable enough to exercise real UI updates.
- Prefer blocking DMA first; async rendering can be considered later only if blocking DMA plus dirty regions are insufficient.
- Do not add a full framebuffer unless external RAM is confirmed, configured, and justified by measured performance.
- Keep hardware-specific CO5300 and DMA details in `apps/firmware` or board-support code; keep `app-core` deterministic and `no_std`.
