## Project Notes

- Keep shared application behavior in `crates/app-core`; it must stay `no_std` and usable by both firmware and the simulator.
- Keep macOS/AppKit-only code in `apps/sim`; simulator conveniences should enter the core only as `Event` values.
- Keep ESP32-C6 hardware facts and board-support code in `crates/board-waveshare-c6` or `apps/firmware`.
- Prefer deterministic embedded rendering paths over host-only APIs in shared code.

## Verification

- Format with `cargo fmt --check`.
- Check host code with `cargo check`.
- Test shared logic with `cargo test -p app-core`.
- Check firmware changes with `cargo check -p firmware --target riscv32imac-unknown-none-elf --release`.

## Resources

- https://docs.espressif.com/projects/rust/book/
- https://github.com/esp-rs/awesome-esp-rust
- https://github.com/rust-embedded/awesome-embedded-rust
