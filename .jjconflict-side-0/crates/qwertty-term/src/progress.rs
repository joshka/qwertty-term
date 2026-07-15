//! OSC 9;4 (ConEmu) progress-bar display model.
//!
//! A program reports progress with `OSC 9 ; 4 ; state ; value ST`. The VT engine
//! parses it into a [`ProgressReport`](qwertty_term_vt::osc::ProgressReport) and
//! latches the latest; the app drains it each pace tick
//! (`Engine::take_progress_report`), gates it on `progress-style`, and renders
//! an in-surface progress bar — mirroring upstream's `SurfaceProgressBar`
//! overlay (a view over the terminal, not a Metal draw), so this lives in the
//! app crate rather than the read-only renderer.
//!
//! This module is the pure display mapping (report → bar geometry + color +
//! indeterminate flag), unit-testable without AppKit. The auto-clear timer and
//! the actual `CALayer` drawing live in the app.

use std::time::Duration;

use qwertty_term_vt::osc::{ProgressReport, ProgressState};

/// How long a progress bar lingers with no further updates before it
/// auto-clears (matching upstream's 15-second `SurfaceProgressBar` timer).
pub const AUTO_CLEAR: Duration = Duration::from_secs(15);

/// The visual category of a progress bar, driving its fill color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressCategory {
    /// Normal in-progress (accent color).
    Normal,
    /// The operation reported an error (red).
    Error,
    /// The operation is paused (orange).
    Paused,
}

/// The app-side display state derived from an OSC 9;4 report: enough to draw the
/// bar without re-deriving upstream's rules at the draw site.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressDisplay {
    pub category: ProgressCategory,
    /// Fill fraction in `[0.0, 1.0]` (meaningful only when `!indeterminate`).
    pub fraction: f64,
    /// Whether the bar is indeterminate (no known value) — drawn as a full
    /// faded/animated bar rather than a fixed fill.
    pub indeterminate: bool,
}

impl ProgressDisplay {
    /// Map an OSC 9;4 report to a display state, or `None` when the report
    /// clears the bar (`Remove`). Mirrors upstream's `SurfaceProgressBar`:
    /// error is red, pause is orange and shown full, a set value fills to its
    /// percentage, and a missing value (or the indeterminate state) is drawn
    /// indeterminate.
    pub fn from_report(report: ProgressReport) -> Option<Self> {
        let frac = |v: Option<u8>| (v.unwrap_or(0) as f64 / 100.0).clamp(0.0, 1.0);
        Some(match report.state {
            ProgressState::Remove => return None,
            ProgressState::Indeterminate => Self {
                category: ProgressCategory::Normal,
                fraction: 0.0,
                indeterminate: true,
            },
            ProgressState::Pause => Self {
                category: ProgressCategory::Paused,
                // Upstream shows a paused bar full.
                fraction: 1.0,
                indeterminate: false,
            },
            ProgressState::Set => Self {
                category: ProgressCategory::Normal,
                fraction: frac(report.progress),
                // A `set` without a value is drawn indeterminate.
                indeterminate: report.progress.is_none(),
            },
            ProgressState::Error => Self {
                category: ProgressCategory::Error,
                fraction: frac(report.progress),
                indeterminate: report.progress.is_none(),
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(state: ProgressState, progress: Option<u8>) -> ProgressReport {
        ProgressReport { state, progress }
    }

    #[test]
    fn remove_clears_the_bar() {
        assert_eq!(
            ProgressDisplay::from_report(report(ProgressState::Remove, None)),
            None
        );
    }

    #[test]
    fn set_fills_to_percentage() {
        let d = ProgressDisplay::from_report(report(ProgressState::Set, Some(50))).unwrap();
        assert_eq!(d.category, ProgressCategory::Normal);
        assert!(!d.indeterminate);
        assert!((d.fraction - 0.5).abs() < 1e-9);
    }

    #[test]
    fn set_over_100_clamps() {
        let d = ProgressDisplay::from_report(report(ProgressState::Set, Some(200))).unwrap();
        assert!((d.fraction - 1.0).abs() < 1e-9);
    }

    #[test]
    fn error_is_red_and_pause_is_orange_full() {
        let e = ProgressDisplay::from_report(report(ProgressState::Error, Some(80))).unwrap();
        assert_eq!(e.category, ProgressCategory::Error);
        assert!((e.fraction - 0.8).abs() < 1e-9);

        let p = ProgressDisplay::from_report(report(ProgressState::Pause, Some(40))).unwrap();
        assert_eq!(p.category, ProgressCategory::Paused);
        assert!((p.fraction - 1.0).abs() < 1e-9);
    }

    #[test]
    fn indeterminate_and_valueless_set_are_indeterminate() {
        let i = ProgressDisplay::from_report(report(ProgressState::Indeterminate, None)).unwrap();
        assert!(i.indeterminate);
        let s = ProgressDisplay::from_report(report(ProgressState::Set, None)).unwrap();
        assert!(s.indeterminate);
    }
}
