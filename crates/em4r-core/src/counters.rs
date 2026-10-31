//! Per-frame counters and phase detection shared between the hooks, the
//! frame statistics and the profiler.

use std::sync::atomic::{AtomicI64, AtomicU32, Ordering};

/// Draw calls (glDrawElements/glDrawArrays/glBegin) since the last frame.
pub static DRAW_CALLS: AtomicU32 = AtomicU32::new(0);

/// QPC timestamp of the last buffer swap (0 = no frame yet).
pub static LAST_PRESENT_QPC: AtomicI64 = AtomicI64::new(0);

/// Without a new frame for this long, the game counts as loading/stalled.
pub const STALL_MS: f64 = 250.0;

#[inline]
pub fn add_draw_call() {
    DRAW_CALLS.fetch_add(1, Ordering::Relaxed);
}

/// Read and reset the draw-call counter (called once per frame).
#[inline]
pub fn take_draw_calls() -> u32 {
    DRAW_CALLS.swap(0, Ordering::Relaxed)
}

#[inline]
pub fn mark_present(now_qpc: i64) {
    LAST_PRESENT_QPC.store(now_qpc, Ordering::Relaxed);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Phase {
    /// Frames are being presented regularly.
    Playing = 0,
    /// No frame for more than [`STALL_MS`]: loading screen or a hang.
    Loading = 1,
}

impl Phase {
    pub const ALL: [Phase; 2] = [Phase::Playing, Phase::Loading];

    pub fn label(self) -> &'static str {
        match self {
            Phase::Playing => "Spiel läuft (Bilder werden gezeichnet)",
            Phase::Loading => "Laden / Hänger (über 250 ms ohne neues Bild)",
        }
    }
}

/// Pure classification, see [`phase_now`].
pub fn classify(last_present: i64, now: i64, freq: i64) -> Phase {
    if last_present == 0 {
        return Phase::Loading;
    }
    let ms = (now - last_present) as f64 * 1000.0 / freq as f64;
    if ms > STALL_MS { Phase::Loading } else { Phase::Playing }
}

pub fn phase_now() -> Phase {
    classify(
        LAST_PRESENT_QPC.load(Ordering::Relaxed),
        crate::util::qpc_now(),
        crate::util::qpc_freq(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phase_classification() {
        let f = 1000; // 1 tick = 1 ms
        assert_eq!(classify(0, 5000, f), Phase::Loading);
        assert_eq!(classify(4900, 5000, f), Phase::Playing);
        assert_eq!(classify(4700, 5000, f), Phase::Loading);
    }
}
