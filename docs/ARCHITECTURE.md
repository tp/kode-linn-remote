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

## Hardware Direction

The firmware is set up for the no_std Espressif stack:

- `esp-hal` for ESP32-C6 peripheral access.
- Embassy-style async tasks when concurrent display, touch, storage, and network work becomes useful.
- `esp-radio` later, only when Wi-Fi, BLE, ESP-NOW, or 802.15.4 is needed.

The first real board task should be display and touch bring-up for the CO5300 AMOLED panel and FT6146 touch controller.
