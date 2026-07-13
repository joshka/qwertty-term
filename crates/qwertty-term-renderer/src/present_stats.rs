//! Present-smoothness instrumentation (#141 DOOM-fire judder).
//!
//! The fps lane measures *throughput* (frames produced), which can be high while
//! the animation still visibly judders — because judder is about *evenness of
//! sampling*, not frame count. This module accumulates, per presented frame, two
//! independent quantities and reports their jitter, so a smoothness change is
//! measurable instead of eyeballed:
//!
//! - **Present-cadence jitter** — stddev of the wall-clock interval between
//!   presents. With #139's vsync `CADisplayLink` this should be ~0 (presents land
//!   on the refresh); a large value means present *timing* is uneven.
//! - **Animation-step variance** — the coefficient of variation of the per-present
//!   *content change* (a caller-supplied signature such as mean frame luma). Smooth
//!   animation advances by an even amount each present (low CV); "1, then 3, then 1
//!   steps apart" chunking shows up as a high CV *even when the cadence is perfect*.
//!
//! Separating the two localizes the fix: even cadence + high content CV points at
//! uneven *sampling* (io/apply burstiness), not the present path. Pure and
//! caller-clocked (takes a monotonic timestamp + signature) so it unit-tests
//! without a display or real time.

/// Running mean + variance via Welford's algorithm (single-pass, numerically
/// stable). Tracks only what the report needs — no sample buffer.
#[derive(Debug, Clone, Default)]
struct Running {
    n: u64,
    mean: f64,
    m2: f64,
}

impl Running {
    fn push(&mut self, x: f64) {
        self.n += 1;
        let delta = x - self.mean;
        self.mean += delta / self.n as f64;
        self.m2 += delta * (x - self.mean);
    }

    /// Population standard deviation (0 for < 2 samples).
    fn stddev(&self) -> f64 {
        if self.n < 2 {
            0.0
        } else {
            (self.m2 / self.n as f64).sqrt()
        }
    }
}

/// Accumulates per-present timing + content signatures and reports jitter
/// statistics. Feed one `record` per presented frame.
#[derive(Debug, Clone, Default)]
pub struct PresentStats {
    prev_ts_ns: Option<u64>,
    prev_signature: Option<f64>,
    interval_ms: Running,
    content_step: Running,
}

impl PresentStats {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one presented frame: its monotonic timestamp (nanoseconds) and a
    /// content signature (any scalar that tracks animation progress — e.g. the
    /// mean luma of the presented frame). The first call seeds; each subsequent
    /// call accumulates one inter-present interval and one absolute content step.
    pub fn record(&mut self, timestamp_ns: u64, content_signature: f64) {
        if let Some(prev) = self.prev_ts_ns {
            // saturating: a non-monotonic clock hiccup contributes a 0 interval
            // rather than a huge wraparound.
            let interval_ns = timestamp_ns.saturating_sub(prev);
            self.interval_ms.push(interval_ns as f64 / 1.0e6);
        }
        if let Some(prev_sig) = self.prev_signature {
            self.content_step.push((content_signature - prev_sig).abs());
        }
        self.prev_ts_ns = Some(timestamp_ns);
        self.prev_signature = Some(content_signature);
    }

    /// The number of presented frames recorded (one more than the interval
    /// count once at least one frame is seeded).
    #[must_use]
    pub fn frames(&self) -> u64 {
        if self.prev_ts_ns.is_some() {
            self.interval_ms.n + 1
        } else {
            0
        }
    }

    /// Snapshot the jitter statistics.
    #[must_use]
    pub fn report(&self) -> PresentReport {
        let content_mean = self.content_step.mean;
        let content_stddev = self.content_step.stddev();
        PresentReport {
            frames: self.frames(),
            present_interval_ms_mean: self.interval_ms.mean,
            present_interval_ms_stddev: self.interval_ms.stddev(),
            content_step_mean: content_mean,
            content_step_stddev: content_stddev,
            // Normalized judder: stddev relative to mean step size. Scale-free, so
            // it compares across content/brightness. ~0 = perfectly even sampling.
            content_step_cv: if content_mean.abs() > f64::EPSILON {
                content_stddev / content_mean
            } else {
                0.0
            },
        }
    }
}

/// A snapshot of present-smoothness statistics ([`PresentStats::report`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PresentReport {
    /// Presented frames recorded.
    pub frames: u64,
    /// Mean interval between presents (ms). ~1000/refresh with vsync.
    pub present_interval_ms_mean: f64,
    /// Stddev of the present interval (ms) — **present-cadence jitter**; ~0 when
    /// presents are vsync-locked.
    pub present_interval_ms_stddev: f64,
    /// Mean absolute per-present content change (in signature units).
    pub content_step_mean: f64,
    /// Stddev of the per-present content change.
    pub content_step_stddev: f64,
    /// Coefficient of variation of the content step (`stddev/mean`) — the
    /// **animation-step evenness / judder** metric. ~0 = even sampling; high =
    /// uneven (chunky) sampling even if the cadence is perfect.
    pub content_step_cv: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    const HZ120_NS: u64 = 8_333_333; // one 120 Hz refresh

    #[test]
    fn empty_and_single_are_zero() {
        let mut s = PresentStats::new();
        assert_eq!(s.report().frames, 0);
        s.record(0, 10.0);
        let r = s.report();
        assert_eq!(r.frames, 1);
        assert_eq!(r.present_interval_ms_stddev, 0.0);
        assert_eq!(r.content_step_cv, 0.0);
    }

    #[test]
    fn even_cadence_and_even_steps_have_near_zero_jitter() {
        // Perfect vsync cadence, animation advancing exactly one step each present.
        let mut s = PresentStats::new();
        for i in 0..10u64 {
            s.record(i * HZ120_NS, i as f64 * 1.0); // luma climbs by 1.0 each frame
        }
        let r = s.report();
        assert_eq!(r.frames, 10);
        assert!(r.present_interval_ms_stddev < 1e-6, "cadence jitter {r:?}");
        assert!(r.content_step_cv < 1e-9, "content CV should be ~0, {r:?}");
        assert!((r.present_interval_ms_mean - 8.3333).abs() < 0.01);
    }

    #[test]
    fn even_cadence_but_chunky_steps_flag_high_content_cv() {
        // Presents are perfectly vsync-spaced, but the animation content jumps by
        // 1, 3, 1, 3, ... steps — the DOOM-fire "chunking" the fps lane can't see.
        let mut s = PresentStats::new();
        let steps = [1.0, 3.0, 1.0, 3.0, 1.0, 3.0, 1.0, 3.0];
        let mut luma = 0.0;
        let mut ts = 0u64;
        s.record(ts, luma);
        for step in steps {
            ts += HZ120_NS;
            luma += step;
            s.record(ts, luma);
        }
        let r = s.report();
        // Cadence is still perfect...
        assert!(
            r.present_interval_ms_stddev < 1e-6,
            "cadence should be even {r:?}"
        );
        // ...but the content step is uneven — this is the judder signal.
        assert!(
            r.content_step_cv > 0.4,
            "chunky sampling should raise content CV, got {r:?}"
        );
        assert_eq!(r.content_step_mean, 2.0); // mean of 1 and 3
    }

    #[test]
    fn uneven_cadence_flags_present_jitter() {
        // Content advances evenly, but presents land at jittery offsets.
        let mut s = PresentStats::new();
        let offsets = [0u64, 8_000_000, 20_000_000, 24_000_000, 40_000_000];
        for (i, &ts) in offsets.iter().enumerate() {
            s.record(ts, i as f64);
        }
        let r = s.report();
        assert!(
            r.present_interval_ms_stddev > 1.0,
            "jittery cadence should raise interval stddev, got {r:?}"
        );
        assert!(r.content_step_cv < 1e-9, "content steps are even, {r:?}");
    }
}
