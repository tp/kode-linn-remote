# Hi-Fi Runtime Driver Refactor

## Problem

Hi-fi side-effect orchestration is split between `apps/sim` and `apps/firmware`.
Both platforms decide when to poll Linn status/events, when to load artwork, how
to track last/pending artwork URIs, and how to turn runtime results back into
`app-core` events. This duplicates behavior and makes simulator/firmware drift
likely.

`app-core` should not own this work. It must stay deterministic, `no_std`, and
side-effect free. It should continue to accept `Event::HifiStatus` and
`Event::HifiArtwork`, update state, render, and emit `Command::Hifi(...)`.

## Goal

Move shared hi-fi orchestration into `crates/app-runtime` behind a small driver
API. Platform apps should provide networking, time, buffers, and logging policy;
the runtime should own the common behavior.

## Proposed Shape

Add an `app_runtime::hifi` or `app_runtime::lpec` driver that owns:

- LPEC session state.
- Last loaded artwork URI.
- Pending artwork URI.
- Status/event polling cadence decisions.
- Artwork load decisions.
- Command handling follow-up, including immediate event poll after commands.
- Conversion of successful status/artwork work into `app_core::Event` values.

Platform apps provide:

- `TcpConnector` implementation.
- Linn endpoint.
- Current uptime or elapsed ticks.
- Artwork HTTP/decode buffers, fixed on firmware and heap-backed or reusable on
  simulator.
- A callback or return value for events to apply to `App`.
- Logging of recoverable errors.

Avoid passing sockets or platform networking directly into `app-core`.

## Sketch

Possible API:

```rust
pub struct HifiRuntime<C> {
    controller: LpecHifi<C>,
    session: LpecSession,
    last_artwork_uri: heapless::String<HIFI_URI_LEN>,
    pending_artwork_uri: heapless::String<HIFI_URI_LEN>,
    next_poll_ms: u64,
}

pub enum HifiRuntimeOutput<E> {
    Event(app_core::Event),
    Error(E),
}

impl<C: TcpConnector> HifiRuntime<C> {
    pub fn tick(
        &mut self,
        uptime_ms: u64,
        screen: app_core::Screen,
        buffers: Option<ArtworkBuffers<'_>>,
    ) -> Option<HifiRuntimeOutput<C::Error>>;

    pub fn handle_command(
        &mut self,
        command: app_core::HifiCommand,
        buffers: Option<ArtworkBuffers<'_>>,
    ) -> Option<HifiRuntimeOutput<C::Error>>;
}
```

The exact API can differ, but it should make the platform loop small and keep
the state machine in one place.

## Acceptance Criteria

- Firmware and simulator no longer keep separate last/pending artwork URI state.
- Firmware and simulator use the same hi-fi status/artwork decision logic.
- `app-core` remains `no_std` and does not depend on networking, time sources,
  alloc-backed HTTP, or platform sockets.
- Firmware can still use fixed artwork buffers.
- Simulator can still use host TCP and does not need firmware-only buffer
  plumbing unless useful for parity.
- Errors remain observable enough to diagnose network, HTTP, buffer, and decode
  failures.

## Verification

- `cargo fmt --check`
- `cargo check`
- `cargo test -p app-core`
- `cargo test -p app-runtime`
- `cargo check -p firmware --target riscv32imac-unknown-none-elf --release`

