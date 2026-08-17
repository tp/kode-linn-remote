# Render cache ownership follow-up

Status: follow-up after the current display smart-rendering patch lands

The load-bearing reason for this refactor is **transactional correctness**, not aesthetics. Today the smart-skip cache lives inside screen state and is updated as draw calls return. If a frame partially fails, gets pre-empted by an external clear, or is rendered to a second target, the cache lies about what's on screen and subsequent frames smart-skip widgets that are actually missing. Moving the cache out of `App` state into per-target session objects is the structural prerequisite for fixing that — but the move alone is not the fix; the commit-on-success behaviour is.

The current smart-rendering direction is sound. What needs to change is *where* the cache lives and *when* it is updated.

## Context

The UI is moving from redraw-everything rendering toward retained, widget-level rendering. That is the right tradeoff for the CO5300 panel: routine updates such as elapsed time, progress, spinner phases, and text changes should avoid full-screen clears and unnecessary SPI writes.

The current implementation keeps per-screen `last_rendered` data inside screen state, and `App::render` tracks `last_rendered_screen`. This works for the main firmware path where one `App` renders to one persistent display. It is less robust as a general contract:

- If a draw call fails midway through a frame, earlier cache fields may already have been updated.
- If the display is externally cleared or reset, screen state still believes old widgets are visible.
- If the same `App` is rendered to a simulator framebuffer, screenshot buffer, or test display after rendering to hardware, the retained cache from the first target can incorrectly suppress drawing to the second target.
- If `App::render` records a screen as rendered immediately after a clear, a later screen-render failure can leave the target partially blank while future renders skip transition clearing.

The core issue is ownership: the cache describes the relationship between one render target and the last successful pixels written to that target. It is not pure app state.

## Proposed Design

Introduce an explicit render cache/session object owned alongside the display or framebuffer, not embedded in `App`.

Possible shape:

```rust
pub struct RenderSession {
    last_screen: Option<Screen>,
    launcher: LauncherRenderCache,
    stopwatch: StopwatchRenderCache,
    hifi: HifiRenderCache,
}
```

`App::render` would take the session explicitly:

```rust
pub fn render<D>(
    &self,
    display: &mut D,
    scratch: &mut [Rgb565],
    session: &mut RenderSession,
) -> Result<(), RenderError<D::Error>>
where
    D: DrawTarget<Color = Rgb565>;
```

Each screen renderer should receive its mutable screen-specific render cache:

```rust
screens::hifi::render(
    state,
    display,
    scratch,
    layout,
    session.hifi(),
)
```

Keep the logical screen state (`HifiStatus`, artwork, stopwatch elapsed time, loading state, etc.) in the screen state. Move only display-derived facts into the render cache: last rendered text, last rendered progress width, last rendered spinner phase, previous slot kind, and similar fields.

## Transactional Cache Updates

Cache updates should commit only after successful drawing. A simple pattern is:

1. Build widgets from app state plus the current render cache.
2. Draw into the target.
3. Record intended cache updates in locals or a temporary cache.
4. Commit the temporary cache to `RenderSession` only after all draw calls succeed.

For the first version, it is acceptable to be more conservative:

- If any render step fails, invalidate the affected screen cache.
- If a screen transition clear succeeds but screen rendering fails, keep `last_screen` unset or invalidate the destination screen cache.
- On the next render attempt, force a full fresh draw for that screen.

This avoids partial-display/clean-cache mismatches without requiring a complex transaction layer immediately.

## Invalidation API

The session should expose explicit invalidation methods for **target-level** events:

```rust
impl RenderSession {
    pub fn invalidate_all(&mut self);
    pub fn invalidate_screen(&mut self, screen: Screen);
    pub fn note_external_clear(&mut self);
}
```

Use these when:

- Firmware reinitializes or clears the panel outside `App::render`.
- The simulator replaces or clears a framebuffer.
- A render error occurs.
- A display driver reset or brightness/power transition invalidates visible pixels.

**Keep screen-internal invalidation screen-local.** Slot-kind transitions
(e.g. spinner → play icon in the hi-fi play slot) and the loading→content
fan-out that today clears `title`, `artist`, `elapsed_seconds`, etc. all
depend on knowledge that lives inside the screen's render function — only
`hifi::render` knows about `PLAY_SLOT_*`. Resist the temptation to
centralize those into `RenderSession`. The session API is for things the
render function cannot detect (target swaps, external clears, render
errors). Slot transitions stay in the screen module, just operating on
`&mut HifiRenderCache` instead of `&mut State.last_rendered`.

## Migration Plan

- Add `RenderSession` and `Default`/`new` constructors in `app-core`.
- Move `App::last_rendered_screen` into `RenderSession`.
- Move launcher `static_drawn` / `network_status_drawn` / `spinner_phase_drawn` into a `LauncherRenderCache`.
- Move hi-fi `LastRendered` and `play_slot.previous_kind` into a `HifiRenderCache`.
- Audit which fields are genuine target-state vs. derivable booleans before relocating. In hi-fi specifically:
  - **Genuine cache** (must move): `volume_percent`, `spinner_phase`, `elapsed_seconds`, `duration_seconds`, `progress_filled_px`, `title`, `artist`, `artwork_uri`, `play_slot.previous_kind`.
  - **First-frame flags** worth consolidating: `has_rendered`, `pin_buttons_drawn`, `loading_visible` are three variants of "we got past stage X once". Consider collapsing to a single `first_full_frame_done: bool` plus a `previously_loading: bool` for the loading→content invalidation gate, rather than carrying three near-duplicates forward.
- Keep `State` fields that are real app behavior in `State`: status, artwork, loading, elapsed-time bookkeeping, and current uptime.
- Update firmware and simulator to own one `RenderSession` next to their display/framebuffer.
- Update tests to construct a fresh `RenderSession` per display target.
- Add a regression test that renders the same `App` to two separate test displays with separate sessions and verifies both receive text pixels.
- Add a regression test that simulates a render failure after a partial draw, then retries and verifies the retry does not smart-skip the missing widgets.

## Acceptance Criteria

- `App` no longer stores target-specific render cache state.
- Each render target owns its own `RenderSession`.
- Rendering the same `App` to two different targets does not suppress drawing on either target.
- A failed render invalidates enough cache state for a later retry to produce a correct frame.
- External display clears can be represented by invalidating the session.
- Hi-fi smart rendering still skips unchanged widgets during normal successful frames.
- The current scratch text persistence and loading-spinner cleanup regressions remain covered by tests.
- Verification passes with `cargo fmt --check`, `cargo check`, `cargo test -p app-core`, and `cargo check -p firmware --target riscv32imac-unknown-none-elf --release`.

## Non-goals

- **Not a retained widget tree.** Widgets stay as values constructed during
  `render()`. They compare current props against the `&RenderCache` passed
  in. We are not introducing per-widget identity, lifetime tracking, or a
  diffing tree — that is overkill for an embedded UI of this size and would
  swamp the actual fix (transactional cache commits) in plumbing.
- **Not a dirty-region transport optimization.** SPI-side dirty rectangles
  / DMA sequencing are a separate concern. Keep this ticket about
  correctness of retained state; performance work layers on top later.

## Notes

- This should stay in `crates/app-core`; the cache is target-specific, but it is still shared UI behavior and must remain `no_std`.
- Avoid making the firmware display driver responsible for widget dirtiness. Firmware should own the session and pass it through.
- A conservative invalidate-on-error implementation is preferable to a clever partial transaction if the latter complicates the embedded path. (Restated for emphasis: invalidate the affected screen cache on error and force a fresh draw next frame; do not try to roll back partial commits.)
- See [WIDGET_LAYER.md](WIDGET_LAYER.md) for the current widget/painter contract and the partial-update patterns the cache needs to keep working through this refactor.
