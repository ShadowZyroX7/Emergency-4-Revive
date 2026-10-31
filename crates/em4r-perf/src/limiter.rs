//! Precise frame limiter: a high-resolution waitable timer for the bulk of
//! the wait, then a short spin for the last ~1 ms.

use em4r_core::ffi::*;
use em4r_core::util::{qpc_freq, qpc_now};

pub struct Limiter {
    period: i64,
    next: i64,
    timer: HANDLE,
}

// The timer handle is only used from the render thread.
unsafe impl Send for Limiter {}

/// Pure scheduling step: given the previous deadline and "now", return
/// (new deadline, ticks to wait). Late frames re-anchor instead of trying to
/// catch up with a burst of unthrottled frames.
pub fn schedule(next: i64, now: i64, period: i64) -> (i64, i64) {
    if next == 0 {
        return (now + period, 0);
    }
    if now >= next {
        // Late. Slightly late: keep the cadence. Very late: re-anchor.
        let new_next = if now - next > period { now + period } else { next + period };
        return (new_next, 0);
    }
    (next + period, next - now)
}

impl Limiter {
    pub fn new(fps: u32) -> Option<Self> {
        if fps == 0 {
            return None;
        }
        // SAFETY: plain Win32 calls; null handle is handled.
        let mut timer = unsafe {
            CreateWaitableTimerExW(
                core::ptr::null_mut(),
                core::ptr::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS,
            )
        };
        if timer.is_null() {
            // Older Windows: normal timer (1 ms with timeBeginPeriod).
            timer = unsafe { CreateWaitableTimerExW(core::ptr::null_mut(), core::ptr::null(), 0, TIMER_ALL_ACCESS) };
        }
        Some(Self { period: qpc_freq() / fps as i64, next: 0, timer })
    }

    /// Block until the next frame deadline.
    pub fn wait(&mut self) {
        let (next, mut wait_ticks) = schedule(self.next, qpc_now(), self.period);
        let deadline = if wait_ticks > 0 { self.next } else { 0 };
        self.next = next;
        if wait_ticks <= 0 {
            return;
        }
        let freq = qpc_freq();
        // Sleep on the timer, leaving ~1 ms for the spin.
        let hundred_ns = wait_ticks * 10_000_000 / freq;
        if hundred_ns > 15_000 && !self.timer.is_null() {
            let due: i64 = -(hundred_ns - 10_000); // negative = relative
            // SAFETY: valid timer handle.
            unsafe {
                if SetWaitableTimer(self.timer, &due, 0, core::ptr::null_mut(), core::ptr::null_mut(), FALSE) != 0 {
                    WaitForSingleObject(self.timer, INFINITE);
                }
            }
        }
        loop {
            wait_ticks = deadline - qpc_now();
            if wait_ticks <= 0 {
                break;
            }
            core::hint::spin_loop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_frame_anchors() {
        assert_eq!(schedule(0, 1000, 100), (1100, 0));
    }

    #[test]
    fn early_frame_waits() {
        assert_eq!(schedule(1100, 1030, 100), (1200, 70));
    }

    #[test]
    fn slightly_late_keeps_cadence() {
        assert_eq!(schedule(1100, 1150, 100), (1200, 0));
    }

    #[test]
    fn very_late_reanchors() {
        assert_eq!(schedule(1100, 1500, 100), (1600, 0));
    }

    #[test]
    fn limiter_holds_rate() {
        let mut l = Limiter::new(200).unwrap(); // 5 ms
        l.wait();
        let start = qpc_now();
        for _ in 0..20 {
            l.wait();
        }
        let ms = em4r_core::util::qpc_to_ms(qpc_now() - start);
        assert!((95.0..140.0).contains(&ms), "20 frames at 200 fps took {ms} ms");
    }
}
