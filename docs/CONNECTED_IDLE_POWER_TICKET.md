# Connected idle power follow-up

Status: deferred until display, touch, buttons, and HIFI runtime are stable

Design and measure a low-power connected-idle mode for the handheld remote. The
goal is to keep wake latency feeling instant while reducing average battery
current as much as possible.

## Context

Fully dropping Wi-Fi makes the device feel like a fresh start. Reconnecting can
take several seconds because the device may need to scan, associate, obtain or
refresh IP state, reconnect TCP, and rebuild HIFI runtime state. For a remote
control, a 3-4 second wake path is too slow.

The desired mode is therefore connected idle rather than deep sleep:

- Keep Wi-Fi associated.
- Avoid full Wi-Fi scans on normal wake.
- Prefer keeping the Linn/LPEC runtime warm, or reconnect without scanning if a
  long-lived TCP socket proves too expensive.
- Turn the AMOLED off or put it in sleep mode.
- Stop redraws and reduce app tick work while the display is off.
- Wake the UI from touch interrupt and the clean app button.

The important power model is average idle current, not feature count. A small
ESP32-C6 board can still lose badly to a phone if it idles at tens of milliamps
continuously. Phones get long standby life through a tightly integrated power
stack: display off, app CPUs mostly asleep, radio firmware handling low-power
network state, batching/coalescing of network events, and aggressive peripheral
power gating.

## Proposed Design

Start with a conservative two-level policy:

1. **Active**
   - Display on.
   - Normal UI tick cadence.
   - Touch path active.
   - HIFI status/artwork/runtime handling active.

2. **Idle connected**
   - Display brightness off and/or panel sleep command sent.
   - No routine rendering.
   - App tick frequency reduced to the minimum required for runtime health.
   - Wi-Fi remains associated with power-save behavior enabled where supported.
   - Touch/button wake re-enters Active.
   - HIFI runtime either keeps a quiet TCP session or reconnects lazily without a
     full Wi-Fi reconnect.

Keep deep sleep or full power-off as a separate explicit mode for long press,
low battery, or very long idle timeout. It is allowed to have slow wake.

## Implementation Steps

- Add firmware state for `Active` vs `IdleConnected`.
- Add a display idle timeout.
- Implement display-off/display-sleep behavior in the CO5300 driver.
- Stop rendering while idle and invalidate the render state on wake.
- Move touch from constant polling to the interrupt-driven follow-up once GPIO
  ownership is clear.
- Poll the clean app button initially; consider interrupt wake later.
- Enable and test ESP Wi-Fi power-save settings compatible with the current
  `esp-radio` stack.
- Decide whether LPEC should keep TCP connected in idle or reconnect on wake.
- Add logging for state transitions, reconnect time, and command latency after
  wake.

## Measurement Plan

Measure current with a real battery path or inline meter for at least these
states:

- Booting and connecting.
- Display on, HIFI screen active.
- Display on, mostly idle.
- Display off, Wi-Fi associated, TCP connected.
- Display off, Wi-Fi associated, TCP disconnected but reconnectable.
- Deep sleep / explicit off, if implemented.

Record:

- Average current.
- Peak current during Wi-Fi reconnect and display wake.
- Time from touch/button wake to visible UI.
- Time from wake to first successful HIFI command.
- Whether the access point's DTIM/beacon behavior changes idle current.

Use the rough planning formula:

```text
runtime hours ~= battery_mAh * 0.85 / average_current_mA
```

For phone-sized packs, remember that an iPhone Pro Max-class battery is roughly
17-18 Wh. A similar 1S LiPo in a custom case is mechanically possible, but use a
normal protected 3.7 V LiPo pack compatible with the board's charger rather than
an actual phone replacement battery.

## Acceptance Criteria

- Waking from connected idle feels immediate for normal UI interaction.
- Wi-Fi does not perform a full fresh-start reconnect on normal wake.
- Display-off connected idle current is measured and documented.
- HIFI command latency after wake is measured and documented.
- The firmware still supports an explicit slow wake/off mode separately from
  connected idle.
- Verification passes with `cargo fmt --check`, `cargo check`,
  `cargo test -p app-core`, and
  `cargo check -p firmware --target riscv32imac-unknown-none-elf --release`.

## Open Questions

- What is the actual idle current of this board with the AMOLED off and Wi-Fi
  associated?
- Is holding the Linn TCP/LPEC connection cheaper or more expensive than
  reconnecting on wake?
- Which `esp-radio` Wi-Fi power-save knobs are available and stable for the
  current dependency versions?
- Does the access point's DTIM interval materially affect latency or current?
- How should the PWR button interact with explicit off mode versus app UI?
