# Architecture

The workspace separates app behavior from hardware access.

## Layers

- `app-core` owns state, events, update logic, and drawing. It is `no_std` and can run in firmware or on the Mac simulator.
- `board-waveshare-c6` records hardware facts and will grow into the board support layer for display, touch, GPIO expansion, IMU, RTC, and storage.
- `sim` adapts native macOS button actions into app events and renders the app into an AppKit window.
- `firmware` initializes ESP32-C6 hardware and will adapt physical peripherals into the same app events.

## Event Flow

Inputs become `app_core::Event` values. Touch coordinates are interpreted by the shared app core, including Start and Stop hit-testing. The app updates its state and requests a redraw. Both simulator and firmware are responsible for delivering events and calling `App::render`.

The simulator runs a short AppKit timer only as a refresh cadence. Each refresh derives internal uptime from a monotonic host clock, then sends `Event::Tick` so the shared core can drive the stopwatch.

The tap highlight is a simulator-only overlay. The shared app core receives the tap coordinate for control hit-testing, but it does not render pointer feedback.

The Mac simulator shows render debug counters outside the device display. `core requests` count times `app-core` reported that visible app state changed, `core frames` count actual shared-core renders, and `sim redraws` count AppKit refreshes including simulator-only overlays.

Text rendering is part of `app-core::App::render`, using `u8g2-fonts` Helvetica variants. Font sizes are chosen in physical display pixels; because the target AMOLED is high density, the UI uses larger 24px Helvetica variants rather than desktop-style point sizes.

Simulator zoom changes the AppKit image view size only. The embedded framebuffer remains 466 x 466 pixels at both native scale and 2x zoom. The window layout always reserves the 2x display area so controls and window size stay stable.

## Hardware Direction

The firmware is set up for the no_std Espressif stack:

- `esp-hal` for ESP32-C6 peripheral access.
- Embassy-style async tasks when concurrent display, touch, storage, and network work becomes useful.
- `esp-radio` later, only when Wi-Fi, BLE, ESP-NOW, or 802.15.4 is needed.

The first real board task should be display and touch bring-up for the CO5300 AMOLED panel and FT6146 touch controller.
