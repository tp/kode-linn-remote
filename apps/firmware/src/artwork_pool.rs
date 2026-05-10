//! Two-slot ping-pong pool for decoded album-artwork pixels.
//!
//! The decoded artwork (96×96 Rgb565 = 18 KiB) lives in two statically
//! allocated buffers and is handed to the renderer as a `&'static` view via
//! `HifiArtwork::from_static_pixels`. We cannot allocate a fresh `Box` per
//! load — the device has run out of heap doing exactly that — so the buffers
//! must be statically reserved.
//!
//! ## Soundness invariant
//!
//! Each call to [`ArtworkPool::acquire`] hands out a `&'static mut` to the
//! next slot, alternating slot 0 → 1 → 0 → 1 → … The previous shared view of
//! that slot (held inside an `Event::HifiArtwork(_)` and ultimately the
//! `App`'s screen state) must already have been dropped before we return
//! here.
//!
//! This is upheld by construction in the firmware:
//!
//! * Slots alternate every load, so two same-slot acquires are at least one
//!   load apart.
//! * The single in-flight `Event::HifiArtwork` is consumed and replaces the
//!   `App`'s previous artwork during `App::update`, dropping the prior
//!   `&'static` view.
//! * `firmware_runtime_task` runs on a single executor task; there are no
//!   concurrent acquires.
//!
//! If those properties ever change (e.g. an event queue depth >1, an
//! artwork cache that retains old views, multi-task acquires), the
//! `ArtworkPool` needs to be replaced with a real reference-counted or
//! drop-guarded pool.

use app_core::{ArtworkPixel, HIFI_ARTWORK_PIXELS};
use embedded_graphics::{pixelcolor::Rgb565, prelude::RgbColor};

static mut SLOT_A: [ArtworkPixel; HIFI_ARTWORK_PIXELS] = [Rgb565::BLACK; HIFI_ARTWORK_PIXELS];
static mut SLOT_B: [ArtworkPixel; HIFI_ARTWORK_PIXELS] = [Rgb565::BLACK; HIFI_ARTWORK_PIXELS];

pub struct ArtworkPool {
    next_is_b: bool,
}

impl ArtworkPool {
    pub const fn new() -> Self {
        Self { next_is_b: false }
    }

    pub fn acquire(&mut self) -> &'static mut [ArtworkPixel; HIFI_ARTWORK_PIXELS] {
        let slot_b = self.next_is_b;
        self.next_is_b = !self.next_is_b;
        // SAFETY: see module-level invariant. Slots strictly alternate, and
        // any prior `&'static` view of this slot has been dropped by the time
        // we wrap back around.
        unsafe {
            if slot_b {
                &mut *core::ptr::addr_of_mut!(SLOT_B)
            } else {
                &mut *core::ptr::addr_of_mut!(SLOT_A)
            }
        }
    }
}
