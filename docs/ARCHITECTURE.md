# Architecture

The workspace separates app behavior from hardware access.

## Layers

- `app-core` owns state, events, update logic, and drawing. It is `no_std` and can run in firmware or on the Mac simulator. State transitions live in the crate root; presentation details live in `app-core::ui`.
- `app-config` owns application configuration data and parsing. Committed code reads the same keys everywhere; personal values live in `config/local.env`, which is gitignored. Keep `config/local.env.example` as the committed template.
- `app-runtime` owns app side effects behind small traits. It is `no_std` by default and turns `app-core` commands into service calls such as hi-fi pin invocation. Protocol implementations live here or in protocol crates; platform socket setup does not.
- `linn-lpec` owns Linn's LPEC command formatting and message parsing. It is a protocol crate, not a platform networking crate.
- `linn-ci-gateway` owns Linn CI Gateway request paths and WebSocket JSON message formatting. It is based on the DSM-hosted Swagger schema at `/api/swagger.yaml` and is also `no_std`.
- `board-kode-dot` records Kode Dot hardware facts and owns the input model. It is the single source of truth for display geometry: `app-core` re-exports `DISPLAY_SIZE` from it rather than repeating the number. Facts inherited from the vendor's ESP32-S3 documentation are tagged `Confidence::Provisional` because the board this project targets is the ESP32-P4 revision.
- `board-waveshare-c6` records hardware facts for the retired round board the project started on, used only by the legacy firmware.
- `sim` adapts native macOS controls into app events, renders the app into an AppKit window, and provides host networking adapters to `app-runtime`.
- `firmware` initializes ESP32-C6 hardware for the retired Waveshare board. It is legacy: `esp-hal` has no ESP32-P4 support, so there is no Kode Dot firmware yet. See `docs/KODE_DOT_PORT_TICKET.md`.

## Event Flow

Inputs become `app_core::Event` values. Touch coordinates are interpreted by the shared app core, including Start and Stop hit-testing. The app updates its state and requests a redraw. Both simulator and firmware are responsible for delivering events and calling `App::render`.

App-triggered side effects flow the other way. `App::update` can return an `app_core::Command` alongside the redraw flag. Platform apps pass those commands into `app-runtime`; the runtime calls a domain trait such as `HifiController`, and the selected controller implementation performs the protocol work. This keeps `app-core` deterministic and reusable while still letting the app own what a button press means.

The simulator runs a short AppKit timer only as a refresh cadence. Each refresh derives internal uptime from a monotonic host clock, then sends `Event::Tick` so the shared core can drive the stopwatch.

The tap highlight is a simulator-only overlay. The shared app core receives the tap coordinate for control hit-testing, but it does not render pointer feedback.

## Input Model

The Kode Dot has a four-way pad and two control buttons alongside its
touchscreen, so every screen must be operable without touching the panel.

Rather than give each screen a hand-written navigation table, each screen
publishes the rectangles of its focusable controls in reading order and
`app-core::ui::focus` moves between them geometrically. One implementation
therefore covers a stacked pair of launcher cards, a row of transport
controls, and a 3x2 grid of pins, with nothing to keep in sync when a layout
constant changes.

`Select` activates the focused control by replaying it as a tap at the
control's centre, so pad and touch share a single dispatch path and cannot
drift apart. `Back` goes up one level: out of a HiFi subpage first, then out to
the launcher. Running the pad off the edge of a HiFi page turns to the next
page, which is how the three pages are reached without a dedicated key.

The focus ring is drawn as an overlay after the screen paints, so it needs no
cooperation from each screen's dirty-region cache. Moving the ring forces a
full repaint, which is what erases the outline from its previous position.

A touch moves the ring to whatever was pressed, but only once the pad has been
used — a touch-only session never sprouts a ring it did not ask for.

The Mac simulator shows render debug counters outside the device display. `core requests` count times `app-core` reported that visible app state changed, `core frames` count actual shared-core renders, and `sim redraws` count AppKit refreshes including simulator-only overlays.

Text rendering uses generated `mplusfonts` bitmap fonts. The generated font data is subset to printable ASCII for now and uses compile-time rasterization, antialiasing, and kerning while keeping firmware rendering deterministic.

The UI is OLED-first: the screen background is true black, with small near-black surfaces and restrained action colors to keep power use low while preserving contrast. Palette and layout values are named constants in `app-core::ui`; keep those constants as the source of truth instead of repeating detailed color choices here.

Touch hit-testing intentionally uses each control's rectangular bounds, even when the visual shape is rounded. This keeps touch handling simple, forgiving, and consistent between simulator and firmware; taps in the small rounded-off corner areas still activate the control. Only switch to shape-accurate hit-testing if a future layout has overlapping controls or visible affordances that make rectangular targets misleading.

Simulator zoom changes the AppKit image view size only. The embedded framebuffer remains 410 x 502 pixels at both native scale and 2x zoom. The window layout always reserves the 2x display area so controls and window size stay stable.

The simulator can also render every screen to PNG and exit (`--snapshot`), which is how layout work gets reviewed without a window or hardware.

Because the panel is 410 px wide, its centre line falls on an odd x. Centred widgets therefore need even half-widths to stay on the display controller's 2-px write grid; `Painter` asserts this for scratch-blitted bounds, and `Framebuffer::fill_solid` reproduces the controller's window expansion so the simulator catches drift before hardware does.

## Hardware Direction

`apps/firmware` is built on the no_std Espressif stack (`esp-hal`, `esp-radio`, `embassy-net`, `reqwless`), which is why it still targets the retired Waveshare ESP32-C6 board.

**That stack does not reach the Kode Dot.** As of 2026-08-17, released `esp-hal` (1.1.2) has no `esp32p4` chip feature — support exists on `main` but is unpublished — and bare-metal Rust additionally has no MIPI-DSI driver and no way to reach the ESP32-C5 co-processor's radio (`esp-radio` covers on-die radios only).

The Kode Dot firmware is therefore planned against **std Rust on ESP-IDF**:

- `esp-idf-hal` / `esp-idf-svc` for peripherals, which carry ESP32-P4 support and `esp_wifi_remote` co-processor Wi-Fi as of their 2026-03-10 releases.
- `esp_lcd` through bindgen for the MIPI-DSI panel.
- ESP-IDF's own TCP/IP stack and HTTP client in place of `embassy-net` and `reqwless`.

This trades embassy and `no_std` on device for Espressif's drivers. `app-core` stays `no_std` and is unaffected — it needs only a framebuffer blit and an event source.

The first real board task is confirming the panel resolution, display controller and interface for the ESP32-P4 revision, since the vendor documentation still describes the ESP32-S3 revision. See `docs/KODE_DOT_PORT_TICKET.md`.

## Network Direction

`app-core` should not open TCP or HTTP connections directly. On the simulator, sockets come from `std::net`; on firmware, sockets come from the ESP radio driver plus an embedded TCP/IP stack. Those APIs have different ownership, buffering, and async constraints, so they belong behind runtime traits instead of inside shared UI state.

Each platform provides network primitives, not Linn behavior:

- `AsyncTcpConnector::connect(endpoint) -> embedded-io-async stream`
- `embedded_io_async::Read`, `Write`, and `ErrorType`

The host simulator implements those traits with `std::net::TcpStream`. Firmware uses `esp-radio` and `embassy-net` sockets directly in its constrained event loop, while shared runtime code layers LPEC protocol behavior and artwork loading on embedded async I/O streams.

Local, changing, or secret values should not be hard-coded in platform crates. Use `config/local.env` for development values such as `LINN_HOST`, `LINN_LPEC_PORT`, `WIFI_SSID`, and `WIFI_PASSWORD`. The simulator loads that file through `app-config`; firmware can later use the same parser with build-time generated input or non-volatile storage.

For Linn control there are two protocol lanes:

- LPEC: a small TCP line protocol. `app-runtime::lpec` owns the async session state, CRLF line framing, shared artwork loading, and `linn-lpec` owns command formatting/parsing.
- CI Gateway: JSON over WebSocket with HTTP setup/documentation. `linn-ci-gateway` owns the request envelope and V2 paths. Add a CI Gateway `HifiController` implementation rather than changing `app-core`. The likely firmware stack is `esp-wifi` + `embassy-net` + an embedded HTTP/WebSocket client, with fixed-size buffers.
