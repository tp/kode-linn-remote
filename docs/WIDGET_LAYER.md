# Widget layer guide

A practical guide to how `app-core::ui` draws screens, why it is shaped the way
it is, and how to add or change a widget without breaking partial-update
behaviour or the OLED panel's quirks.

Audience: Claude/agents working in this codebase, plus the human maintainer.
Read this before touching anything under `crates/app-core/src/ui/`.

## TL;DR

- We render with **retained, smart-skipping widgets**: every frame walks the
  full screen, but each widget decides for itself whether it actually pushes
  pixels. The "smart" part is comparing current state against
  `last_rendered.*` fields stored on the screen state.
- The `Painter` dispatches widgets and owns one tunable lever — the `scratch`
  buffer — that turns a widget's draw calls into a single blit. Use it for
  text. Don't use it for animations that overdraw cleanly.
- Mutually-exclusive widgets share a `Slot`. The slot clears its bounds when
  the kind changes; the widget itself just paints its content.
- Three things will silently corrupt the screen if you get them wrong:
  1. A scratch widget that doesn't override `should_draw` will blit black over
     itself every frame state hasn't changed.
  2. A scratch widget with bounds that aren't 2-px aligned will paint shifted
     edge pixels (CO5300 write-window expansion).
  3. Updating a `last_rendered` field before drawing succeeds means a failed
     frame leaves the cache lying about what's on screen. (See
     `RENDER_CACHE_TICKET.md` for the planned fix.)

## Files at a glance

- [crates/app-core/src/ui/widget.rs](../crates/app-core/src/ui/widget.rs) —
  `Widget<A>` trait, `Slot` helper.
- [crates/app-core/src/ui/painter.rs](../crates/app-core/src/ui/painter.rs) —
  `Painter`, scratch dispatch, 2-px alignment check.
- [crates/app-core/src/ui/render.rs](../crates/app-core/src/ui/render.rs) —
  `App::render` entry point, screen-transition clear.
- [crates/app-core/src/ui/components.rs](../crates/app-core/src/ui/components.rs)
  — reusable primitives: buttons, panels, spinner dots, duration text, wifi
  icon, progress bar.
- [crates/app-core/src/ui/style.rs](../crates/app-core/src/ui/style.rs) —
  palette and radii. **Always import colours from here**, never inline literals.
- [crates/app-core/src/ui/screens/](../crates/app-core/src/ui/screens/) — one
  module per screen, each owning its `Layout`, `State`, widgets, and
  `render` function.

## Architectural shape

```
App::update(event)              → mutates screen state, returns render flag
  ↓
App::render(display, scratch)
  ↓
screens::<screen>::render(state, display, scratch, layout)
  ↓
Painter::draw(&Widget)          → maybe routes through scratch, blits to display
  ↓
DrawTarget<Color = Rgb565>      → CO5300 panel (firmware) or AppKit (sim)
```

State, layout, and presentation are kept apart on purpose:

- **`State`** holds *behavioural* state: playback status, elapsed seconds,
  loading flags, etc. It is mutated by events; rendering only reads it.
- **`Layout`** is screen-fixed geometry computed once per screen size.
  Widgets and hit-testing both consume the same `Layout`.
- **`LastRendered`** (a substruct of `State` today) holds *presentation*
  facts: "we already drew this title", "the volume wedge sits at 42%", etc.
  This is the "render cache". Long-term it should move out of `State` (see
  `RENDER_CACHE_TICKET.md`) but for now treat it as write-only inside
  `render` and read-only outside.

## The `Widget` trait

```rust
pub(super) trait Widget<A> {
    fn bounds(&self) -> Rectangle;
    fn draw<D: DrawTarget<Color = Rgb565>>(&self, target: &mut D) -> Result<(), D::Error>;
    fn use_scratch(&self) -> bool { false }
    fn should_draw(&self) -> bool { true }
}
```

Four hooks, each with a job:

- `bounds()` — the rectangle the widget owns. Used by the painter to size the
  scratch slice, and by `Slot` to clear on kind change.
- `draw()` — paints in **absolute** coordinates. The painter handles any
  translation needed for the scratch path; the widget itself never sees
  translated coordinates.
- `use_scratch()` — opt into the off-screen framebuffer. Returns `true` for
  text and progress-bar redraws; `false` for everything that overdraws
  cleanly or paints into a slot we just cleared.
- `should_draw()` — the smart-skip switch. **Override this whenever you would
  early-return inside `draw()`**, and *especially* if `use_scratch() == true`.

The `A` type parameter is reserved for a future per-screen action enum if we
ever move hit-testing onto the trait. Today it's unused.

## The `Painter`

`Painter::draw(&widget)` does this:

1. Call `widget.should_draw()` — if `false`, return immediately.
2. If `widget.use_scratch() == false`: pass the display straight into
   `widget.draw()`. Done.
3. If `use_scratch() == true`:
   - Debug-assert `bounds` is 2-px aligned.
   - Carve a slice of the shared scratch buffer sized to the widget's bounds.
   - Wrap it in a `FrameBuf`, **clear to black**, translate by `-bounds.top_left`,
     and call `widget.draw()` against the translated framebuf.
   - Blit the framebuf to the display via a single `fill_contiguous` over
     `bounds`.

Two consequences:

- **Scratch widgets compose cheaply.** Drawing text + clearing the underlying
  rectangle costs one panel write rather than one-per-glyph. That's why
  `CenteredTextBand` calls `clear_rect` *and* draws text: against the
  pre-cleared scratch the clear is a no-op, but it keeps the widget correct
  on the non-scratch test paths.
- **Scratch is a snapshot, not an overlay.** The framebuf starts black. If
  your widget draws nothing, you blit a black rectangle over whatever was
  there before. Hence the `should_draw` gotcha.

The scratch buffer is shared across widgets. Size it with
`RECOMMENDED_SCRATCH_PIXELS` (currently 384 × 40 = 15,360 px = 30 KB). Any
widget bounds larger than that will fall back to a direct draw.

## The `Slot`

A `Slot` is a fixed rectangle that owns mutually-exclusive content over time —
e.g. the centre of the hi-fi screen, which holds spinner / play / pause /
buffering / artwork.

```rust
pub(super) struct Slot {
    pub bounds: Rectangle,
    pub previous_kind: Option<u8>,
}

slot.clear_if_kind_changed(display, new_kind)?;  // paints black if kind changed
```

The pattern in `screens::hifi::render`:

1. Compute `play_kind` from current state.
2. Call `clear_if_kind_changed` — this is a no-op when nothing changed, and a
   single `fill_solid` over `bounds` when it did.
3. Dispatch the widget for the new kind. That widget can smart-skip on
   `already_drawn = state.play_slot.previous_kind == Some(MY_KIND)`.
4. Record `state.play_slot.previous_kind = Some(play_kind)`.

Use `Slot` whenever exactly one of N things lives in a region. Don't use it
for stacked widgets that all draw every frame — that's just regular draws.

## Partial-update patterns

These are the recipes that already exist in the codebase. New widgets should
fit one of them; if they don't, that's worth a conversation before merging.

### 1. Static chrome (draw once, ever)

`StaticChrome` in [launcher.rs](../crates/app-core/src/ui/screens/launcher.rs):
title text and the two giant buttons never change after first render.

```rust
fn should_draw(&self) -> bool { !self.already_drawn }
```

Set `already_drawn = state.static_drawn` going in, then `state.static_drawn =
true` after the painter call returns. The screen-transition clear in
`App::render` invalidates this implicitly (because the State is reconstructed
on screen entry — check this when adding a new screen).

### 2. Diff-rendered region (paint only the delta)

`VolumeArc` in [hifi.rs](../crates/app-core/src/ui/screens/hifi.rs:577): when
volume goes 40 → 42, paint a 2-percentage-point arc in the *active* colour;
when it goes 42 → 40, paint that arc in the *track* colour. First frame paints
the whole arc.

```rust
match self.previous_percent {
    None             => /* full arc */,
    Some(p) if p == self.percent => /* skip */,
    Some(p)          => /* paint just the delta sub-arc */,
}
```

Use this when the widget is much bigger than the typical change region.

### 3. Animated overdraw (no clear, dots cover dots)

The eight-dot spinner in [components.rs](../crates/app-core/src/ui/components.rs:213).
Each phase paints dots at the same eight positions; new dots cover old dots
exactly. **Skip the clear** — clearing causes visible flicker.

`Spinner` (the widget wrapper in `hifi.rs`) layers smart-skip on top:

```rust
fn draw(&self, target) {
    if self.previous_phase == Some(self.phase) { return Ok(()); }
    draw_spinner_dots(target, self.center, self.phase)
}
```

Note that `previous_phase` is fed in conditionally — only when the slot's
previous kind was *also* the spinner. If the slot just transitioned from
artwork to spinner, the dots haven't been on screen, so we paint the full
phase regardless.

### 4. Periodic counter (scratch + full re-render of small region)

`TimerDisplay` and `ProgressBarWidget` in `hifi.rs:799`/`842`. These re-render
the whole "hh:mm:ss" / progress bar every time the value changes, but they go
through scratch so it's a single blit. The `should_draw` override is what
prevents wasted blits on unchanged frames:

```rust
fn use_scratch(&self) -> bool { true }
fn should_draw(&self) -> bool { self.previous_elapsed != Some(self.elapsed) }
```

### 5. Text band (scratch + change-only redraw)

`CenteredTextBand` in `hifi.rs:881`. Title and artist update rarely but can
contain anything. Use scratch (single blit, no glyph flicker), and gate on a
string compare:

```rust
let song_unchanged = state.last_rendered.title.as_str() == song_text;
let song = CenteredTextBand { /*...*/, unchanged: song_unchanged, /*...*/ };
painter.draw(&song)?;
state.last_rendered.title.clear();
let _ = state.last_rendered.title.push_str(song_text);
```

The widget calls `clear_rect` inside `draw()` even though scratch is
pre-cleared — that keeps the widget correct on non-scratch test paths.

### 6. Slot transitions (clear + redraw)

See `Slot` above. The widget for each kind can use any of patterns 1–5
internally; the slot just guarantees the bounds are clean when it lands.

## The `last_rendered` cache pattern

Each screen owns a `LastRendered` struct in its `State`. Convention:

- One field per smart-skipping widget input. Use `Option<T>` so `None` means
  "first frame, paint unconditionally".
- **Read** the field when constructing the widget. Pass it in as
  `previous_*`.
- **Write** the field *after* the painter call returns. (This is the bug
  `RENDER_CACHE_TICKET.md` plans to fix — today we update the cache even on
  partial-failure paths. Be aware of it; don't make it worse.)
- **Invalidate** when assumptions change. The hi-fi loading→content
  transition clears every cache field because the spinner-only frame
  short-circuited and skipped them all:

  ```rust
  if state.last_rendered.loading_visible {
      state.last_rendered.title.clear();
      state.last_rendered.elapsed_seconds = None;
      // ...
      state.last_rendered.loading_visible = false;
  }
  ```

When you add a new field, audit transitions: which state changes invalidate
it? Slot kind transitions? Screen entry? Loading→content? Wire those up in
the relevant transition block, not in some generic place.

## The CO5300 2-pixel alignment rule

The panel's write-window protocol expands odd coordinates and odd extents,
shifting edge pixels. Direct draw calls go through the display driver's
per-primitive alignment path, so they're fine. Scratch widgets don't —
they're one big `fill_contiguous`, and the panel sees the bounds as-is.

Rule: any widget with `use_scratch() == true` must have bounds where
`top_left.x`, `top_left.y`, `size.width`, `size.height` are **all even**.
This is debug-asserted in `Painter::draw` and there's a regression test
`scratched_widget_bounds_are_two_aligned` in
[hifi.rs:1020](../crates/app-core/src/ui/screens/hifi.rs#L1020) that catches
layout regressions.

If you add a scratch widget, add it to that test.

## Common pitfalls

| Symptom | Cause |
| --- | --- |
| Text disappears after first frame | Scratch widget without `should_draw` override; black blit overwrites. |
| Edge column/row of text shifted by 1 px | Bounds not 2-px aligned. Check the layout. |
| Spinner flickers / strobes | You added a clear before drawing the dots, or you're routing it through scratch. |
| Stale text after screen change | New screen's `State` was reused; either reconstruct on entry or invalidate `last_rendered`. |
| Ghost pixels at edge of slot | Slot bounds smaller than the widget actually paints. Centre on the slot, not on some other layout point. (See the loading-spinner regression: `loading_spinner_pixels_are_cleared_on_transition_out_of_loading`.) |
| Spinner doesn't animate | Either the runtime isn't requesting frames (`on_tick` returns false) or the platform main loop is blocked on I/O. The widget itself is fine. |

## Adding a new widget

A checklist:

1. Decide which pattern (1–6 above) it fits.
2. Define a struct in the screen module with the inputs *plus* `previous_*`
   fields for whatever it'll smart-skip on.
3. Implement `Widget<Action>`:
   - `bounds()` returns the dirty rectangle (must be 2-px aligned if
     scratched).
   - `should_draw()` returns false when `previous == current`.
   - `use_scratch()` if you redraw a region wholesale (text, progress bar).
   - `draw()` paints in absolute coords.
4. Add a `LastRendered` field for it.
5. In `render()`: build the widget, call `painter.draw(&widget)?`, then write
   the `last_rendered` field.
6. If it interacts with a slot or screen-level transition, invalidate the
   field in the matching transition block.
7. Tests: add a "smart-skips when state unchanged" test (count draw calls
   like `loading_spinner_smart_skips_when_phase_unchanged` does), and a
   "ghost pixel on transition" test if it changes visible footprint.

## Adding a new screen

Mirror the existing screens:

- `Layout` struct + `layout(bounds)` function. Keep all geometry constants at
  module top.
- `State` struct with whatever it tracks plus `last_rendered: LastRendered`.
- `render(state, display, scratch, layout)` function. Construct widgets;
  dispatch via the painter.
- `handle_touch(layout, point) -> Option<Command|Screen>` for input.
- Wire it into `ScreenLayouts`, the `ActiveScreen` enum, and
  `App::render`'s match.
- Add a `*_button_centers` test helper for hit-testing tests.

The **screen-transition clear** in `App::render` (currently a full
`display.clear(BLACK)`) is what guarantees you don't inherit pixels from the
previous screen. Don't try to optimise it away on a per-screen basis without
also reasoning about every other screen's first-frame assumptions.

## Testing

Two kinds of test live in this layer:

- **Layout/hit-test tests** — pure functions over `Rectangle`. Cheap, fast,
  no draw target needed.
- **Render tests with a fake DrawTarget** — see
  `loading_spinner_pixels_are_cleared_on_transition_out_of_loading` for the
  pattern. Build a `RecordingDisplay` (counts pixels written, or stores a
  framebuffer), drive `render()`, then assert on what landed.

The painter's scratch path is `Infallible` inside, so test drawing through
the same `Painter` you'd use in production rather than swapping in a custom
target — it'll exercise the same alignment and dispatch logic.

## When in doubt

- Before adding a new pattern, look for the closest existing widget and copy
  its shape.
- Don't push behavioural state into `LastRendered`. Don't pull presentation
  state into `State`.
- If you find yourself wanting to clear a region "just in case", you've
  probably missed a `Slot` opportunity or forgotten a `should_draw`
  override.
- If a widget needs more than ~30 KB of scratch (the shared buffer), that's
  a sign the widget is too big — split it, or use direct draws with
  per-primitive alignment.
