# Runtime Platform Glue Cleanup

## Problem

`crates/app-runtime` currently contains both shared runtime behavior and
host/simulator glue. In particular:

- `app_runtime::hifi::worker` is `std`-only, uses `std::thread`,
  `std::sync::mpsc`, and `futures_executor::block_on`.
- `app_runtime::host_tcp::HostTcpConnector` is a host networking adapter built
  on blocking `std::net::TcpStream`.
- Firmware does not use either path. It has its own Embassy loop and its own
  network adapter in `apps/firmware`.

That makes the architecture harder to reason about. `app-runtime` looks like a
platform-neutral runtime crate, but it also owns simulator-specific execution
policy. The simulator then tests behavior that is only partly shared with
firmware: some hi-fi orchestration is shared through `HifiDriver`, while other
important details remain in platform loops or `std`-only worker code.

The goal is not just to remove `block_on`. The goal is to make the ownership
boundary obvious and ensure the simulator exercises the same runtime behavior
the device depends on.

## Goal

Bring hi-fi runtime execution into one clear architectural shape:

- Shared behavior lives in `crates/app-runtime`.
- Platform crates own scheduling, sockets, timers, task/thread setup, buffers,
  and logging.
- Simulator and firmware use the same runtime state machine and protocol logic
  wherever practical.
- The simulator remains useful as a behavioral test for firmware, not just as a
  desktop UI shell.

## Target Architecture

Keep these in `crates/app-runtime`:

- `HifiController`
- `AppRuntime`
- `HifiDriver`
- LPEC session/protocol integration
- artwork loading and decode logic that can run on embedded async I/O
- small platform-neutral request/result types if they express runtime behavior

Move these out of `crates/app-runtime`:

- `hifi::worker`
- `host_tcp::HostTcpConnector`
- any `std::thread` / `std::sync::mpsc` execution policy
- any host-only TCP setup
- the `futures-executor` dependency, except as a dev-dependency for tests if
  still needed

After the cleanup, `app-runtime` should not have simulator-only execution code.
It may still expose APIs that make host and firmware glue small.

## Proposed Work

1. Define the shared runtime driver boundary.

   Decide whether `HifiDriver` is the final shared state machine or whether it
   needs a small API adjustment first. It should own common decisions such as:

   - active/inactive hi-fi screen state
   - status poll cadence
   - command follow-up behavior
   - track-change artwork invalidation
   - pending artwork URI handling
   - pins fetch-once behavior
   - conversion of successful work into `app_core::Event`

2. Move the std worker into `apps/sim`.

   Create simulator-owned glue for:

   - background worker thread or host async runtime
   - request/response channels
   - calls into the shared `HifiDriver`
   - error logging policy

   A first pass can keep the current thread + `block_on` model, but it should
   live in `apps/sim` so the platform boundary is honest.

3. Move host TCP into simulator-owned code.

   Move `HostTcpConnector` to `apps/sim` or a small host-support crate used by
   the simulator. It should remain an implementation of
   `app_runtime::net::AsyncTcpConnector`, but it should not make
   `app-runtime` depend on host networking.

4. Reconcile firmware with the shared driver.

   Firmware currently has a separate `FirmwareHifiDriver` with behavior that
   overlaps `app_runtime::hifi::HifiDriver`. Remove that duplication or reduce
   it to firmware-only buffer/network constraints.

   The important outcome is that simulator and firmware share the same runtime
   decisions for polling, command follow-up, artwork invalidation, and pins.

5. Decide on the simulator execution model.

   Two acceptable end states:

   - Keep a blocking host worker: `std::thread`, blocking TCP, and local
     `block_on` in `apps/sim`.
   - Use a real host async runtime: likely Tokio, with Tokio TCP and async
     channels.

   Do not make `run_background async` as a standalone change. That only moves
   the `block_on` call outward unless the host networking and scheduling model
   also changes.

## Non-Goals

- Do not move platform socket ownership into `app-core`.
- Do not put AppKit, Embassy, ESP32-P4, or host TCP details into shared runtime
  logic.
- Do not require firmware to use heap-backed simulator conveniences.
- Do not introduce Tokio into firmware.
- Do not rewrite LPEC protocol parsing as part of this cleanup.

## Acceptance Criteria

- `crates/app-runtime` contains no `std::thread` or `std::sync::mpsc` worker
  code.
- `crates/app-runtime` contains no `std::net::TcpStream` host adapter.
- `apps/sim` owns its platform execution glue explicitly.
- `apps/firmware` owns its Embassy execution glue explicitly.
- Simulator and firmware both use shared `app-runtime` logic for hi-fi polling,
  command follow-up, artwork invalidation/loading decisions, and pins fetch
  behavior.
- `app-runtime` remains `no_std` by default.
- The sim can still connect to Linn over host TCP.
- Firmware can still use fixed buffers and Embassy networking.
- Tests cover the shared runtime behavior without depending on AppKit or ESP32
  hardware.

## Suggested Tests

Add or keep `app-runtime` tests for:

- activating the hi-fi screen requests an immediate status poll
- inactive hi-fi screen suppresses status/artwork/pins work
- status poll interval behavior
- status errors request a retry
- next/previous track commands invalidate artwork and defer status refresh
- changed artwork URI schedules exactly one artwork load
- empty artwork URI clears pending/last artwork state
- pins are fetched once per driver lifetime or until an explicit reset exists

These tests should use fake `HifiController` implementations and should not
depend on simulator threads or firmware Embassy tasks.

## Verification

- `cargo fmt --check`
- `cargo check`
- `cargo test -p app-core`
- `cargo test -p app-runtime`
- `cargo check -p firmware --target riscv32imac-unknown-none-elf --release`

