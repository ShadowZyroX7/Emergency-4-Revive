//! Frame statistics: FPS, 1 % lows, frametime spikes, draw calls, memory.

/// Summary of one measurement interval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub frames: usize,
    pub fps_avg: f64,
    /// FPS equivalent of the 99th-percentile frametime ("1 % low").
    pub fps_1pct_low: f64,
    pub frametime_max_ms: f64,
    pub draws_avg: f64,
}

/// Compute a summary from frametimes (ms) and draw calls per frame.
pub fn summarize(frametimes_ms: &[f32], draws: &[u32]) -> Option<Summary> {
    if frametimes_ms.is_empty() {
        return None;
    }
    let total: f64 = frametimes_ms.iter().map(|&t| t as f64).sum();
    let mut sorted: Vec<f32> = frametimes_ms.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let max = *sorted.last().unwrap_or(&0.0) as f64;
    // Nearest-rank 99th percentile.
    let rank = ((sorted.len() as f64 * 0.99).ceil() as usize).clamp(1, sorted.len());
    let p99 = sorted[rank - 1] as f64;
    let draws_avg = if draws.is_empty() {
        0.0
    } else {
        draws.iter().map(|&d| d as f64).sum::<f64>() / draws.len() as f64
    };
    Some(Summary {
        frames: frametimes_ms.len(),
        fps_avg: if total > 0.0 { frametimes_ms.len() as f64 * 1000.0 / total } else { 0.0 },
        fps_1pct_low: if p99 > 0.0 { 1000.0 / p99 } else { 0.0 },
        frametime_max_ms: max,
        draws_avg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_60fps() {
        let ft = vec![1000.0 / 60.0; 120];
        let s = summarize(&ft, &[100; 120]).unwrap();
        assert!((s.fps_avg - 60.0).abs() < 0.01);
        assert!((s.fps_1pct_low - 60.0).abs() < 0.01);
        assert_eq!(s.draws_avg, 100.0);
    }

    #[test]
    fn spike_shows_in_low_and_max() {
        let mut ft = vec![10.0f32; 99];
        ft.push(100.0);
        let s = summarize(&ft, &[]).unwrap();
        assert!((s.frametime_max_ms - 100.0).abs() < 1e-6);
        // p99 of 100 samples = 99th value = 10 ms; one spike in 100 frames
        // is exactly the 1 % boundary.
        assert!((s.fps_1pct_low - 100.0).abs() < 1e-6);
        let mut ft2 = vec![10.0f32; 98];
        ft2.extend([100.0, 100.0]);
        let s2 = summarize(&ft2, &[]).unwrap();
        assert!((s2.fps_1pct_low - 10.0).abs() < 1e-6);
    }

    #[test]
    fn empty_is_none() {
        assert!(summarize(&[], &[]).is_none());
    }
}
