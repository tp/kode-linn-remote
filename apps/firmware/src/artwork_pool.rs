//! Three-slot pool for decoded album-artwork pixels.
//!
//! The decoded artwork lives in statically allocated buffers and is handed to
//! the renderer as a `&'static` view via `HifiArtwork::from_static_pixels`. We
//! cannot allocate a fresh `Box` per load — the device has run out of heap
//! doing exactly that — so the buffers must be statically reserved.
//!
//! ## This no longer fits the retired C6 board
//!
//! `HIFI_ARTWORK_SIZE` is now 330 px, because that is the size of the Now
//! Playing artwork slot on the Kode Dot's 410x502 panel. One slot is therefore
//! 330 x 330 x 2 = 213 KiB, and three of them are 638 KiB — more than the
//! ESP32-C6's entire 512 KiB of SRAM. Together with the artwork decode and
//! HTTP buffers this file's pool will not link for that target.
//!
//! That is a known consequence, not an oversight. `apps/firmware` is legacy: it
//! targets the retired round Waveshare C6 board, whose 466x466 geometry already
//! does not match what `app-core` renders, and there is no Kode Dot firmware
//! yet. `cargo check` still passes because checking does not link. The real fix
//! arrives with the ESP32-P4 port, which has 32 MB of PSRAM and wants a
//! PSRAM-backed pool rather than `.bss` slots — and by then the pool needs a
//! second, smaller size for the picker's tile artwork anyway.
//!
//! ## Soundness invariant
//!
//! At any instant, three distinct live `&'static` views of slots are
//! possible:
//!
//! 1. **App-held**: the artwork currently displayed, owned by
//!    `App.hifi_screen.artwork`.
//! 2. **Channel-queued**: at most one `Event::HifiArtwork(_)` sitting in
//!    `FIRMWARE_EVENTS` (capacity 1) waiting for the main loop to receive it.
//! 3. **In-flight**: the producer task's future state between
//!    `HifiArtwork::from_static_pixels` and `Channel::send().await`
//!    returning. Backpressure on a full channel makes this window real.
//!
//! With 3 slots and strict round-robin acquisition, the slot we're about to
//! overwrite is at least 3 acquires old — older than any of the three
//! holders above could possibly reference. The `&'static mut` produced by
//! [`ArtworkPool::acquire`] therefore aliases nothing.
//!
//! ## When this assumption breaks
//!
//! Increase [`POOL_SIZE`] if any of the following changes:
//!
//! * `FIRMWARE_EVENTS` capacity grows beyond 1 → add one slot per extra
//!   queue position.
//! * The app caches more than one prior artwork (e.g. fade transitions,
//!   history) → add one slot per retained artwork.
//! * A second producer task is added that can also acquire → the round-robin
//!   counter is no longer a sufficient invariant; replace this whole module
//!   with an explicit free/ack pool (drop-guarded) backed by an atomic
//!   bitmask.

use app_core::{ArtworkPixel, HIFI_ARTWORK_PIXELS};
use embedded_graphics::{pixelcolor::Rgb565, prelude::RgbColor};

/// Live-view holders that pin a `&'static` to a slot:
/// App (1) + `FIRMWARE_EVENTS` capacity (1) + in-flight producer frame (1).
const POOL_SIZE: usize = 3;

static mut SLOT_0: [ArtworkPixel; HIFI_ARTWORK_PIXELS] = [Rgb565::BLACK; HIFI_ARTWORK_PIXELS];
static mut SLOT_1: [ArtworkPixel; HIFI_ARTWORK_PIXELS] = [Rgb565::BLACK; HIFI_ARTWORK_PIXELS];
static mut SLOT_2: [ArtworkPixel; HIFI_ARTWORK_PIXELS] = [Rgb565::BLACK; HIFI_ARTWORK_PIXELS];

pub struct ArtworkPool {
    next_index: usize,
}

impl ArtworkPool {
    pub const fn new() -> Self {
        Self { next_index: 0 }
    }

    pub fn acquire(&mut self) -> &'static mut [ArtworkPixel; HIFI_ARTWORK_PIXELS] {
        let index = self.next_index;
        self.next_index = (self.next_index + 1) % POOL_SIZE;
        // SAFETY: see module-level invariant. With POOL_SIZE = 3 (matching
        // App + channel-queued + in-flight) and strict round-robin, the slot
        // we are about to mutate cannot be referenced by any live `&'static`
        // view: it was last handed out at least POOL_SIZE acquires ago, and
        // by then every slot's prior view has been displaced from each of
        // those three holders.
        unsafe {
            match index {
                0 => &mut *core::ptr::addr_of_mut!(SLOT_0),
                1 => &mut *core::ptr::addr_of_mut!(SLOT_1),
                2 => &mut *core::ptr::addr_of_mut!(SLOT_2),
                _ => unreachable!(),
            }
        }
    }
}
