//! Three-slot pool for decoded album-artwork pixels.
//!
//! The decoded artwork (96×96 Rgb565 = 18 KiB) lives in statically allocated
//! buffers and is handed to the renderer as a `&'static` view via
//! `HifiArtwork::from_static_pixels`. We cannot allocate a fresh `Box` per
//! load — the device has run out of heap doing exactly that — so the buffers
//! must be statically reserved.
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
