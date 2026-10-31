//! Per-frame counters shared between the GL hooks and the frame statistics.

use std::sync::atomic::{AtomicU32, Ordering};

/// Draw calls (glDrawElements/glDrawArrays/glBegin) since the last frame.
pub static DRAW_CALLS: AtomicU32 = AtomicU32::new(0);

#[inline]
pub fn add_draw_call() {
    DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// Read and reset the draw-call counter (called once per frame).
#[inline]
pub fn take_draw_calls() -> u32 {
    DRAW_CALLS.swap(0, Ordering::Relaxed)
}
