# Architecture

The workspace separates app behavior from hardware access.

## Layers

- `app-core` owns state, events, update logic, and drawing. It is `no_std` and can run in firmware or on the Mac simulator. State transitions live in the crate root; presentation details live in `app-core::ui`.
- `board-waveshare-c6` records hardware facts and will grow into the board support layer for display, touch, GPIO expansion, IMU, RTC, and storage.
- `sim` adapts native macOS controls into app events and renders the app into an AppKit window.
- `firmware` initializes ESP32-C6 hardware and will adapt physical peripherals into the same app events.

## Event Flow

Inputs become `app_core::Event` values. Touch coordinates are interpreted by the shared app core, including Start and Stop hit-testing. The app updates its state and requests a redraw. Both simulator and firmware are responsible for delivering events and calling `App::render`.

The simulator runs a short AppKit timer only as a refresh cadence. Each refresh derives internal uptime from a monotonic host clock, then sends `Event::Tick` so the shared core can drive the stopwatch.

The tap highlight is a simulator-only overlay. The shared app core receives the tap coordinate for control hit-testing, but it does not render pointer feedback.

The Mac simulator shows render debug counters outside the device display. `core requests` count times `app-core` reported that visible app state changed, `core frames` count actual shared-core renders, and `sim redraws` count AppKit refreshes including simulator-only overlays.

Text rendering uses generated `mplusfonts` bitmap fonts. The generated font data is subset to printable ASCII for now and uses compile-time rasterization, antialiasing, and kerning while keeping firmware rendering deterministic.

The UI is OLED-first: the screen background is true black, with small near-black surfaces and restrained action colors to keep power use low while preserving contrast. Palette and layout values are named constants in `app-core::ui`; keep those constants as the source of truth instead of repeating detailed color choices here.

Touch hit-testing intentionally uses each control's rectangular bounds, even when the visual shape is rounded. This keeps touch handling simple, forgiving, and consistent between simulator and firmware; taps in the small rounded-off corner areas still activate the control. Only switch to shape-accurate hit-testing if a future layout has overlapping controls or visible affordances that make rectangular targets misleading.

Simulator zoom changes the AppKit image view size only. The embedded framebuffer remains 466 x 466 pixels at both native scale and 2x zoom. The window layout always reserves the 2x display area so controls and window size stay stable.

## Hardware Direction

The firmware is set up for the no_std Espressif stack:

- `esp-hal` for ESP32-C6 peripheral access.
- Embassy-style async tasks when concurrent display, touch, storage, and network work becomes useful.
- `esp-radio` later, only when Wi-Fi, BLE, ESP-NOW, or 802.15.4 is needed.

The first real board task should be display and touch bring-up for the CO5300 AMOLED panel and FT6146 touch controller.
