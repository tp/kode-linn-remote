# Architecture

The workspace separates app behavior from hardware access.

## Layers

- `app-core` owns state, events, update logic, and drawing. It is `no_std` and can run in firmware or on the Mac simulator. State transitions live in the crate root; presentation details live in `app-core::ui`.
- `app-config` owns application configuration data and parsing. Committed code reads the same keys everywhere; personal values live in `config/local.env`, which is gitignored. Keep `config/local.env.example` as the committed template.
- `app-runtime` owns app side effects behind small traits. It is `no_std` by default and turns `app-core` commands into service calls such as hi-fi pin invocation. Protocol implementations live here or in protocol crates; platform socket setup does not.
- `linn-lpec` owns Linn's LPEC command formatting and message parsing. It is a protocol crate, not a platform networking crate.
- `linn-ci-gateway` owns Linn CI Gateway request paths and WebSocket JSON message formatting. It is based on the DSM-hosted Swagger schema at `/api/swagger.yaml` and is also `no_std`.
- `board-waveshare-c6` records hardware facts and will grow into the board support layer for display, touch, GPIO expansion, IMU, RTC, and storage.
- `sim` adapts native macOS controls into app events, renders the app into an AppKit window, and provides host networking adapters to `app-runtime`.
- `firmware` initializes ESP32-C6 hardware and will adapt physical peripherals and embedded networking into the same app events and runtime traits.

## Event Flow

Inputs become `app_core::Event` values. Touch coordinates are interpreted by the shared app core, including Start and Stop hit-testing. The app updates its state and requests a redraw. Both simulator and firmware are responsible for delivering events and calling `App::render`.

App-triggered side effects flow the other way. `App::update` can return an `app_core::Command` alongside the redraw flag. Platform apps pass those commands into `app-runtime`; the runtime calls a domain trait such as `HifiController`, and the selected controller implementation performs the protocol work. This keeps `app-core` deterministic and reusable while still letting the app own what a button press means.

The simulator runs a short AppKit timer only as a refresh cadence. Each refresh derives internal uptime from a monotonic host clock, then sends `Event::Tick` so the shared core can drive the stopwatch.

The tap highlight is a simulator-only overlay. The shared app core receives the tap coordinate for control hit-testing, but it does not render pointer feedback.

The simulator can mask the rectangular framebuffer to the board's round visible display area. Circle mode is the default, and simulator taps outside the visible circle are ignored so desktop interaction matches the hardware shape.

The Mac simulator shows render debug counters outside the device display. `core requests` count times `app-core` reported that visible app state changed, `core frames` count actual shared-core renders, and `sim redraws` count AppKit refreshes including simulator-only overlays.

Text rendering uses generated `mplusfonts` bitmap fonts. The generated font data is subset to printable ASCII for now and uses compile-time rasterization, antialiasing, and kerning while keeping firmware rendering deterministic.

The UI is OLED-first: the screen background is true black, with small near-black surfaces and restrained action colors to keep power use low while preserving contrast. Palette and layout values are named constants in `app-core::ui`; keep those constants as the source of truth instead of repeating detailed color choices here.

Touch hit-testing intentionally uses each control's rectangular bounds, even when the visual shape is rounded. This keeps touch handling simple, forgiving, and consistent between simulator and firmware; taps in the small rounded-off corner areas still activate the control. Only switch to shape-accurate hit-testing if a future layout has overlapping controls or visible affordances that make rectangular targets misleading.

Simulator zoom changes the AppKit image view size only. The embedded framebuffer remains 466 x 466 pixels at both native scale and 2x zoom. The window layout always reserves the 2x display area so controls and window size stay stable.

## Hardware Direction

The firmware is set up for the no_std Espressif stack:

- `esp-hal` for ESP32-C6 peripheral access.
- `esp-wifi` for Wi-Fi/BLE radio support when networking is enabled.
- `embassy-net` for the no_std TCP/IP stack, with TCP sockets implementing embedded async I/O traits.
- `reqwless` or an equivalent embedded HTTP/WebSocket-capable client for HTTP-facing integrations.

The first real board task should be display and touch bring-up for the CO5300 AMOLED panel and FT6146 touch controller.

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
