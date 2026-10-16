//! The heap a process holds, counted (roll-dih8.11): xtask's global allocator is the system's, with
//! a count of the bytes it hands out and the most it has handed out at once, so that the process
//! that loads a journal and replays it (`timing --load`) can say what replay held at its peak. The
//! resident set `/usr/bin/time` reports holds more: memory freed but kept by the allocator for
//! sizes the rest of the run never asks for again.
//!
//! The one unsafe spot outside bead's two (`CONTRIBUTING.md`): an allocator is an unsafe
//! trait. Every call passes its arguments to the system allocator, whose contract is the trait's;
//! the counting touches only atomics.
#![allow(
    unsafe_code,
    reason = "xtask's counting allocator (CONTRIBUTING.md): GlobalAlloc is an unsafe trait"
)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The system allocator, counted.
pub(crate) struct Counting;

/// Bytes handed out and not yet given back.
static HELD: AtomicUsize = AtomicUsize::new(0);

/// The most [`HELD`] has been.
static PEAK: AtomicUsize = AtomicUsize::new(0);

/// `bytes` more held.
fn held_more(bytes: usize) {
    let now = HELD.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

/// `bytes` given back.
fn held_less(bytes: usize) {
    HELD.fetch_sub(bytes, Ordering::Relaxed);
}

/// The most this process has held on its heap at once, in bytes.
pub(crate) fn peak() -> usize {
    PEAK.load(Ordering::Relaxed)
}

/// What this process holds on its heap now, in bytes.
pub(crate) fn held() -> usize {
    HELD.load(Ordering::Relaxed)
}

// SAFETY: each method calls the same method of `System` with the arguments it was given, under the
// contract `GlobalAlloc` states for both; only the counts are added.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller keeps `alloc`'s contract, which is `System::alloc`'s.
        let block = unsafe { System.alloc(layout) };
        if !block.is_null() {
            held_more(layout.size());
        }
        block
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: the caller keeps `alloc_zeroed`'s contract, which is `System::alloc_zeroed`'s.
        let block = unsafe { System.alloc_zeroed(layout) };
        if !block.is_null() {
            held_more(layout.size());
        }
        block
    }

    unsafe fn dealloc(&self, block: *mut u8, layout: Layout) {
        // SAFETY: `block` came from this allocator, so from `System`, with this `layout`.
        unsafe { System.dealloc(block, layout) };
        held_less(layout.size());
    }

    unsafe fn realloc(&self, block: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        // SAFETY: `block` came from this allocator, so from `System`, with this `layout`, and the
        // caller keeps `realloc`'s contract for `size`.
        let moved = unsafe { System.realloc(block, layout, size) };
        if !moved.is_null() {
            if size >= layout.size() {
                held_more(size - layout.size());
            } else {
                held_less(layout.size() - size);
            }
        }
        moved
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_peak_counts_what_was_held_at_once() {
        let before = peak();
        let block = vec![0u8; 1 << 22];
        assert!(peak() >= before.max(1 << 22));
        drop(block);
        assert!(peak() >= 1 << 22, "a peak is kept once passed");
    }
}
