//! Wheel-scroll math and the mouse-wheel decision ladder, factored out of the
//! AppKit view so the branchy pixel→row accumulation and the reporting /
//! alternate-scroll / viewport ladder are unit-testable without a window.
//!
//! This is a direct port of upstream `src/Surface.zig`'s `scrollCallback`
//! (rev `2da015cd6`, lines 3405–3599): the [`ScrollAmount`] delta computation
//! (the precision-vs-discrete branch, the macOS ±1 clamp, and the sub-cell
//! [`WheelState::pending`] accumulator) and the three mutually-exclusive
//! outcomes (mouse reporting, alternate-scroll cursor keys, viewport scroll).
//!
//! Only the vertical (Y) axis drives scrollback; horizontal deltas are used
//! only for the mouse-reporting path (buttons 6/7) which the app already
//! handles via the existing `crate::input::mouse` encode, so this module is
//! Y-only.

/// The `mouse-scroll-multiplier` config: separate multipliers for precision
/// (trackpad, pixel) deltas and discrete (mouse wheel, tick) deltas. Port of
/// upstream `Config.MouseScrollMultiplier` (`precision: f64 = 1`,
/// `discrete: f64 = 3`; both clamped to `[0.01, 10_000.0]`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScrollMultiplier {
    pub precision: f64,
    pub discrete: f64,
}

impl Default for ScrollMultiplier {
    fn default() -> Self {
        // Upstream defaults (Config.zig `MouseScrollMultiplier`): precision 1,
        // discrete 3.
        ScrollMultiplier {
            precision: 1.0,
            discrete: 3.0,
        }
    }
}

impl ScrollMultiplier {
    /// Clamp both multipliers to upstream's `[0.01, 10_000.0]` range
    /// (`Config.zig` `finalize`). Applied when loading from user config.
    pub fn clamped(self) -> Self {
        ScrollMultiplier {
            precision: self.precision.clamp(0.01, 10_000.0),
            discrete: self.discrete.clamp(0.01, 10_000.0),
        }
    }
}

/// Per-surface wheel accumulator: the sub-cell pixel remainder that hasn't yet
/// crossed a full-cell threshold. Port of the `mouse.pending_scroll_y` field
/// upstream keeps per `Surface`.
#[derive(Debug, Clone, Copy, Default)]
pub struct WheelState {
    /// Pixels of vertical scroll accumulated but not yet turned into a row
    /// delta (upstream `mouse.pending_scroll_y`).
    pub pending: f64,
}

impl WheelState {
    /// Compute the integer row delta for one wheel event, updating the pending
    /// remainder. Returns the number of rows to act on (sign: negative = down,
    /// positive = up, matching upstream `ScrollAmount.delta`), or `0` when the
    /// accumulated offset is still below a full cell.
    ///
    /// Direct port of `scrollCallback`'s `y` block (Surface.zig 3437–3492):
    ///
    /// - precision (trackpad): `yoff * multiplier.precision` (pixels, no
    ///   cell-size scaling).
    /// - discrete (wheel tick): on macOS, clamp `|yoff|` up to at least 1 (the
    ///   `yoff_max` step that stops slow 0.1-magnitude ticks from stalling),
    ///   then `yoff_max * cell_height * multiplier.discrete`.
    /// - accumulate into `pending`; if `|pending| < cell_height`, keep it and
    ///   emit 0; else `delta = trunc(pending / cell_height)` and carry the
    ///   remainder.
    pub fn row_delta(
        &mut self,
        yoff: f64,
        precision: bool,
        cell_height: f64,
        mult: ScrollMultiplier,
    ) -> isize {
        if yoff == 0.0 || cell_height <= 0.0 {
            return 0;
        }

        let yoff_adjusted = if precision {
            yoff * mult.precision
        } else {
            // macOS: ramp a slow (|yoff| < 1) tick up to a full ±1 so a single
            // detent always crosses a cell. (Upstream applies this only on
            // Darwin; this crate is macOS-only, so it is unconditional here.)
            let yoff_max = if yoff > 0.0 {
                yoff.max(1.0)
            } else {
                yoff.min(-1.0)
            };
            yoff_max * cell_height * mult.discrete
        };

        let poff = self.pending + yoff_adjusted;
        if poff.abs() < cell_height {
            self.pending = poff;
            return 0;
        }

        let amount = poff / cell_height;
        // Carry the sub-cell remainder (upstream `poff - amount * cell_size`
        // where `amount` is the *truncated* row count).
        let delta = amount.trunc();
        self.pending = poff - delta * cell_height;
        delta as isize
    }
}

/// What a wheel event should do, after the decision ladder resolves. Port of
/// the three mutually-exclusive branches at the bottom of `scrollCallback`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WheelOutcome {
    /// No effect (zero delta).
    None,
    /// Mouse reporting is active: emit `|delta|` button-4 (up) / button-5
    /// (down) press reports; do NOT move the viewport. The caller re-uses the
    /// existing `crate::input::mouse` encode path for byte-identical output.
    Report { count: usize, up: bool },
    /// Alternate screen + mode 1007 (and no explicit mouse reporting): emit
    /// `count` cursor-up (`up = true`) / cursor-down arrow key presses, per
    /// DECCKM. Does NOT move the viewport.
    AltScrollKeys { count: usize, up: bool },
    /// Scroll the scrollback viewport by `delta` rows (positive = *up* into
    /// history — already sign-flipped from upstream's viewport convention; see
    /// [`decide`]).
    Viewport { rows_up: isize },
}

/// Resolve a computed row `delta` (negative = down, positive = up, upstream
/// `ScrollAmount.delta` convention) into an outcome, given the surface's live
/// mode state. Order and conditions mirror `scrollCallback` exactly:
///
/// 1. **Alternate-scroll** (Surface.zig 3533–3535): requires ALL of alt screen
///    active, mouse reporting `none`, and mode 1007 set → cursor keys.
/// 2. **Mouse reporting** (3568): buttons 4/5.
/// 3. **Viewport** (3590–3595): `scrollViewport(delta * -1)`. Upstream's
///    viewport delta is positive-*down*; we return `rows_up` (positive = up)
///    so the caller passes it to a "rows up from bottom" offset model.
pub fn decide(
    delta: isize,
    reporting_active: bool,
    alt_screen: bool,
    alt_scroll_mode: bool,
) -> WheelOutcome {
    if delta == 0 {
        return WheelOutcome::None;
    }
    // `up_right` when delta >= 0 (upstream ScrollAmount.direction).
    let up = delta > 0;
    let count = delta.unsigned_abs();

    // (1) Alternate-scroll: alt screen AND no explicit reporting AND mode 1007.
    if alt_screen && !reporting_active && alt_scroll_mode {
        return WheelOutcome::AltScrollKeys { count, up };
    }

    // (2) Mouse reporting: buttons 4 (up) / 5 (down).
    if reporting_active {
        return WheelOutcome::Report { count, up };
    }

    // (3) Viewport scroll. Upstream: `scrollViewport(.delta = y.delta * -1)`
    // (its viewport delta is positive-down). We express the result as
    // rows-up-from-bottom, which is simply `delta` (positive = up).
    WheelOutcome::Viewport { rows_up: delta }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CH: f64 = 16.0;

    #[test]
    fn discrete_single_tick_scrolls_by_multiplier_rows() {
        // A single up tick (yoff = +1), discrete, default multiplier 3 →
        // 1 * 16 * 3 = 48 px → 48/16 = 3 rows up. No remainder.
        let mut w = WheelState::default();
        let d = w.row_delta(1.0, false, CH, ScrollMultiplier::default());
        assert_eq!(d, 3);
        assert_eq!(w.pending, 0.0);
    }

    #[test]
    fn discrete_slow_tick_ramps_to_at_least_one() {
        // macOS reports slow ticks as ~0.1; the ±1 clamp makes even a 0.1 tick
        // cross a cell: max(0.1,1)=1 → 48 px → 3 rows.
        let mut w = WheelState::default();
        let d = w.row_delta(0.1, false, CH, ScrollMultiplier::default());
        assert_eq!(d, 3);
    }

    #[test]
    fn discrete_down_tick_is_negative() {
        let mut w = WheelState::default();
        let d = w.row_delta(-1.0, false, CH, ScrollMultiplier::default());
        assert_eq!(d, -3);
    }

    #[test]
    fn precision_accumulates_below_a_cell() {
        // Precision deltas are raw pixels * precision (1.0). 10 px < 16 px cell
        // → no scroll, remainder saved.
        let mult = ScrollMultiplier::default();
        let mut w = WheelState::default();
        assert_eq!(w.row_delta(10.0, true, CH, mult), 0);
        assert_eq!(w.pending, 10.0);
        // Another 10 px → 20 total → 1 row, 4 px remainder.
        assert_eq!(w.row_delta(10.0, true, CH, mult), 1);
        assert_eq!(w.pending, 4.0);
    }

    #[test]
    fn precision_remainder_carries_across_direction_change() {
        let mult = ScrollMultiplier::default();
        let mut w = WheelState::default();
        // +10 px pending, then -30 px → poff = -20 → -1 row, remainder -4.
        assert_eq!(w.row_delta(10.0, true, CH, mult), 0);
        assert_eq!(w.row_delta(-30.0, true, CH, mult), -1);
        assert_eq!(w.pending, -4.0);
    }

    #[test]
    fn zero_yoff_is_noop() {
        let mut w = WheelState::default();
        assert_eq!(w.row_delta(0.0, false, CH, ScrollMultiplier::default()), 0);
        assert_eq!(w.pending, 0.0);
    }

    #[test]
    fn ladder_alt_scroll_requires_all_three() {
        // Up delta, alt screen, no reporting, mode 1007 → cursor-up keys.
        assert_eq!(
            decide(3, false, true, true),
            WheelOutcome::AltScrollKeys { count: 3, up: true }
        );
        // Missing mode 1007 → falls through to viewport.
        assert_eq!(
            decide(3, false, true, false),
            WheelOutcome::Viewport { rows_up: 3 }
        );
        // Not alt screen → viewport.
        assert_eq!(
            decide(-2, false, false, true),
            WheelOutcome::Viewport { rows_up: -2 }
        );
    }

    #[test]
    fn ladder_reporting_wins_over_viewport_but_not_when_alt_scroll_applies() {
        // Reporting active, primary screen → report buttons.
        assert_eq!(
            decide(2, true, false, true),
            WheelOutcome::Report { count: 2, up: true }
        );
        // Reporting active blocks the alt-scroll branch (that branch requires
        // reporting == none), so on alt screen with reporting on we report.
        assert_eq!(
            decide(-1, true, true, true),
            WheelOutcome::Report {
                count: 1,
                up: false
            }
        );
    }

    #[test]
    fn ladder_zero_delta_is_none() {
        assert_eq!(decide(0, false, false, true), WheelOutcome::None);
    }

    #[test]
    fn multiplier_clamps_to_upstream_range() {
        let m = ScrollMultiplier {
            precision: 0.0,
            discrete: 99_999.0,
        }
        .clamped();
        assert_eq!(m.precision, 0.01);
        assert_eq!(m.discrete, 10_000.0);
    }
}
