# Kode Dot Port

## Status

The shared app core and the simulator target the Kode Dot. Firmware does not.

This is no longer blocked on upstream — the earlier revision of this ticket
said it was, and that was true only for the bare-metal path. A viable firmware
route exists today (see **Firmware Path**). What remains is that first
bring-up needs hardware, which arrives with the November 2026 batch, and Kode
has not yet published documentation for the ESP32-P4 revision.

Last upstream re-check: **2026-08-17**.

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

## Firmware Path

**Decision: target std Rust on ESP-IDF via `esp-idf-svc`, not bare-metal
`esp-hal`.**

### Why not `esp-hal` (the original plan)

Verified 2026-08-17 against crates.io: `esp-hal` **1.1.2** (published
2026-08-05, current) exposes chip features `esp32`, `esp32c2`, `esp32c3`,
`esp32c5`, `esp32c6`, `esp32c61`, `esp32h2`, `esp32s2`, `esp32s3`. There is
still **no `esp32p4`**. The workspace resolves 1.1.1.

Support is in development but unreleased — `esp32p4` is present in
`esp-metadata/devices` on esp-hal's `main`, and the repo README already lists
the P4.

A release alone would not unblock us. Bare-metal would still be missing:

- a **MIPI-DSI** display driver, which the P4 almost certainly needs for this
  panel (the S3 revision's CO5300 QSPI driver likely does not transfer)
- **Wi-Fi**, which on the P4 comes from the ESP32-C5 co-processor, not an
  on-die radio — `esp-radio` does not apply, and there is no bare-metal Rust
  `esp-hosted` link

That is three unshipped dependencies, not one.

### Why `esp-idf-svc` works today

Binding to ESP-IDF's C means inheriting Espressif's drivers rather than
reimplementing them. Verified 2026-08-17:

- `riscv32imafc-esp-espidf` is present in the installed Rust toolchain
- `esp-idf-hal` **0.46.2** and `esp-idf-svc` **0.52.1**, both released
  2026-03-10
- ESP32-P4 support merged: `esp-idf-hal` #467, `esp-idf-svc` #465
- Co-processor Wi-Fi merged **2026-02-24**, two weeks before those releases:
  - `esp-idf-hal` #572 — "Make modem peripheral work on esp32p4 with
    esp_wifi_remote"
  - `esp-idf-svc` #640 — "Support esp_wifi_remote for esp32p4"

So `EspWifi` in ordinary Rust, with `esp_wifi_remote` RPCing over SDIO to the
co-processor. `esp_lcd` covers MIPI-DSI through bindgen. Kode's own ESP-IDF
driver libraries (`kode_bq27220-idf`, `kode_max31329-idf`) are linkable for
the fuel gauge and RTC.

It also emits an ESP-IDF-format image, which is what kodeOS chainloads.

### Cost of this choice

FreeRTOS, `alloc`, and a C build system via `embuild`/`esp-idf-sys`. Embassy
goes away on device.

`app-core` is `no_std` and indifferent — it needs a framebuffer blit and an
event source, which is the seam `app-runtime` already defines. The port is
confined to `apps/firmware` (~1,959 lines, still targeting the retired
Waveshare C6 board and not runnable: it renders against
`board-waveshare-c6::DISPLAY_SIZE` of 466 x 466 while `app-core` now produces
410 x 502).

## kodeOS Integration

kodeOS is a launcher and boot-selector, **not a runtime**. It provides no
services to a running app. Established from Kode's published sources
(`kodediy/kodedot_examples`, `LagoESP/KodeOS-Loader`) on 2026-08-17 — note
all of it describes the **S3** revision.

Mechanism on the S3:

- One app slot. `partitions_app.csv` declares
  `app, factory, 0x400000, 0x800000` (8 MB), with `nvs` at `0x9000`,
  `otadata` at `0x10000`, and a `storage` SPIFFS at `0xC00000`.
- Apps flash **only** to `0x400000`, preserving bootloader and partitions.
- Boot selection uses the ESP-IDF OTA machinery. Every app links
  `-Wl,--wrap=esp_ota_mark_app_valid_cancel_rollback`; the BSP supplies
  `custom_ota_override.cpp` to service that wrap.
- After hand-off the app owns the SoC outright.

Consequences for us:

- **Nothing to conform to at the API level.** Producing a kodeOS app is a
  partition-table and linker concern, satisfied by the `esp-idf-svc` path.
- **Nothing gained either.** Display init, touch, and Wi-Fi are already ours.
  `kodedot_bsp` is vendored per-app Arduino_GFX + LVGL glue we would not link.
- **Wi-Fi is each app's own job.** The only cross-app convention is a
  plaintext `/wifi.txt` on the microSD (`SSID=` / `PASSWORD=` lines, up to
  three networks). Kode's `WiFiManager.cpp` is 257 lines of SD parsing and
  `WiFi.begin()` retries with zero hardware access — reimplement in Rust, do
  not port. Match the file format for consistency with other apps on the
  device, but prefer NVS if storing credentials in the clear on removable
  media is unacceptable.

**Open decision:** keep kodeOS and flash to the app slot, or replace it by
flashing to `0x0`. This is a dedicated appliance, not a multi-app device, and
a launcher on every boot is arguably a liability. Recommend keeping it through
bring-up for the recovery and reflash tooling, then revisiting.

## Open Questions For First Bring-Up

Everything marked `Confidence::Provisional` in `crates/board-kode-dot`:

1. **Display controller and interface.** The S3 revision used a CO5300 over
   QuadSPI. The P4 has a MIPI-DSI host and probably drives the panel that way,
   which would also invalidate the 2-px write-window alignment the simulator's
   `Framebuffer::fill_solid` currently mirrors.
2. **Touch controller.** CST820 over I2C at 0x15 on the S3 revision.
3. **Button wiring.** On the S3 revision the pad sat behind a TCA95xx I/O
   expander at 0x20 with one key wired straight to GPIO0. The P4 revision
   advertises "two control buttons", which `board-kode-dot` models as `Select`
   and `Back`; confirm which physical key is which.
4. **Wireless co-processor part.** Kode's product page says **ESP32-C5**;
   several press write-ups say C6. This matters: `esp_wifi_remote`'s
   well-trodden slave is the C6 (as on the P4-Function-EV-Board), and C5 slave
   support in `esp-hosted-mcu` is the newer, less proven path. The risk sits
   in the ESP-IDF C component and is the same for Rust or C++.
5. **kodeOS P4 partition layout.** The offsets above are S3-era and assume a
   16 MB flash; the P4 board has 32 MB. The loader also hardcodes
   `--chip esp32s3`. Expect the app base address to move.

Settled, and no longer open:

- **Panel geometry.** 410 x 502, portrait. Kode publishes it as "502x410" —
  the native scan resolution, long side first — but the panel is mounted with
  the screen above the pad, so the framebuffer is 410 wide. Do not transpose
  it.
- **Memory.** 32 MB PSRAM and 32 MB flash.

## Next Actions

Nothing here is blocked on upstream. It is blocked on hardware and vendor
documentation.

- **Now, optional, needs no Kode docs:** stand up an `esp-idf-svc` skeleton on
  any ESP32-P4 devkit to de-risk the toolchain — `embuild`/`sdkconfig`, the
  `app-core` framebuffer blit seam, and `EspWifi` over `esp_wifi_remote`.
  Everything except the panel and pin map is board-independent.
- **On hardware arrival (November 2026 batch):** resolve the five open
  questions above and set `board-kode-dot` confidences accordingly.
- **Do not file upstream issues.** `esp-hal` P4 is already in progress on
  `main`, and the `esp-idf-svc` route needs nothing from anyone.
- **Re-check when revisiting:** whether `esp-hal` has cut a release containing
  `esp32p4`, and whether Kode has published P4 documentation, a P4 BSP, or a
  P4 build of the loader.
