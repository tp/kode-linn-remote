## Project Notes

- This project targets the Kode Dot (ESP32-P4 revision, November 2026 batch). The vendor docs at docs.kode.diy still describe the older ESP32-S3 revision, so treat their pin maps and drivers as unverified.
- Keep shared application behavior in `crates/app-core`; it must stay `no_std` and usable by both firmware and the simulator.
- Keep display geometry in `crates/board-kode-dot`; `app-core` re-exports `DISPLAY_SIZE` from it. Do not repeat the resolution anywhere else — it is not yet confirmed for this board revision.
- Keep macOS/AppKit-only code in `apps/sim`; simulator conveniences should enter the core only as `Event` values.
- Every screen must be operable from the directional pad, not just touch. Publish focusable controls via `focus_targets` rather than writing per-screen navigation tables.
- The panel is 410 px wide, so its centre is at an odd x. Give centred widgets even half-widths or the painter's 2-px alignment assertion will fire.
- `apps/firmware` and `crates/board-waveshare-c6` are legacy: they target the retired round Waveshare board. `esp-hal` has no ESP32-P4 support yet, so there is no Kode Dot firmware.
- The display driver must compose frames in PSRAM and blit them; never satisfy the panel's even-window rule by widening individual fills, which destroys the row above.
- Prefer deterministic embedded rendering paths over host-only APIs in shared code.

## Verification

- Format with `cargo fmt --check`.
- Check host code with `cargo check`.
- Test shared logic with `cargo test -p app-core`.
- Check the legacy firmware with `cargo check -p firmware --target riscv32imac-unknown-none-elf --release`.
- Review layout changes with `cargo run -p sim -- --snapshot target/snapshots`.

## Resources

- https://kode.diy/product/kode-dot
- https://docs.kode.diy (ESP32-S3 revision)
- https://docs.espressif.com/projects/rust/book/
- https://github.com/esp-rs/awesome-esp-rust
- https://github.com/rust-embedded/awesome-embedded-rust
