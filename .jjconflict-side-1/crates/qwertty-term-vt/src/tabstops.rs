//! Tabstop tracking as a bitset. Port of `src/terminal/Tabstops.zig`
//! (271 lines, 5 inline tests).
//!
//! Ghostty's `Tabstops` splits storage into a fixed-size, non-allocating
//! "preallocated" segment (512 columns, i.e. 99.9% of real terminals) and a
//! dynamically-grown segment used only when resized past that. This port
//! keeps the same two-segment shape but replaces the manual bit-twiddling
//! (`Unit = u8`, hand-rolled `masks` table, `entry`/`index` helpers) with a
//! plain `Vec<bool>` per segment — the perf-motivated bitset packing isn't
//! load bearing for correctness and there's no allocator-failure path to
//! preserve in safe Rust (Zig's `resize` can return `error.OutOfMemory` and
//! must leave `self.cols` unchanged on failure; `Vec::resize` aborts the
//! process on allocation failure instead, so that specific failure-path test
//! has no Rust equivalent — see `tests` below for the disposition).

/// The number of columns we preallocate for without a dynamic allocation.
/// Port of `Tabstops.zig` `prealloc_columns`.
const PREALLOC_COLUMNS: usize = 512;

/// Keeps track of the location of tabstops. Port of `Tabstops.zig`'s
/// top-level `Tabstops` struct.
///
/// Implemented as two bit-vector-like segments: a fixed-size preallocated
/// segment (covers `0..PREALLOC_COLUMNS`) and a dynamically-grown segment for
/// anything beyond it. Unlike the Zig source, both segments here are plain
/// `Vec<bool>` rather than packed bitsets — see module docs for why that's
/// an acceptable divergence for this port.
#[derive(Debug, Clone)]
pub struct Tabstops {
    /// The number of columns this tabstop is set to manage. Use
    /// [`Tabstops::resize`] to change this number.
    cols: usize,
    /// Preallocated tab stops, one bool per column up to `PREALLOC_COLUMNS`.
    prealloc_stops: Box<[bool; PREALLOC_COLUMNS]>,
    /// Dynamically expanded stops above `PREALLOC_COLUMNS`.
    dynamic_stops: Vec<bool>,
}

impl Default for Tabstops {
    fn default() -> Self {
        Self {
            cols: 0,
            prealloc_stops: Box::new([false; PREALLOC_COLUMNS]),
            dynamic_stops: Vec::new(),
        }
    }
}

impl Tabstops {
    /// Initialize tabstops for `cols` columns with tabstops every `interval`
    /// columns. Port of `Tabstops.zig` `init`.
    pub fn new(cols: usize, interval: usize) -> Self {
        let mut res = Self::default();
        res.resize(cols);
        res.reset(interval);
        res
    }

    /// Set the tabstop at a certain column. The columns are 0-indexed. Port
    /// of `Tabstops.zig` `set`.
    pub fn set(&mut self, col: usize) {
        if col < PREALLOC_COLUMNS {
            self.prealloc_stops[col] = true;
            return;
        }
        let dynamic_i = col - PREALLOC_COLUMNS;
        assert!(dynamic_i < self.dynamic_stops.len());
        self.dynamic_stops[dynamic_i] = true;
    }

    /// Unset the tabstop at a certain column. The columns are 0-indexed.
    /// Port of `Tabstops.zig` `unset`.
    ///
    /// NOTE: upstream implements this as `stops ^= mask` — an XOR *toggle*, not
    /// a clear. So unsetting a column that has no tabstop *creates* one. This is
    /// observable (e.g. `CSI 0 g` — TBC "clear at cursor" — at a non-tabstop
    /// column then makes a later HT stop there), so we replicate the XOR exactly
    /// rather than doing a plain clear.
    pub fn unset(&mut self, col: usize) {
        if col < PREALLOC_COLUMNS {
            self.prealloc_stops[col] ^= true;
            return;
        }
        let dynamic_i = col - PREALLOC_COLUMNS;
        assert!(dynamic_i < self.dynamic_stops.len());
        self.dynamic_stops[dynamic_i] ^= true;
    }

    /// Get the value of a tabstop at a specific column. The columns are
    /// 0-indexed. Port of `Tabstops.zig` `get`.
    pub fn get(&self, col: usize) -> bool {
        if col < PREALLOC_COLUMNS {
            return self.prealloc_stops[col];
        }
        let dynamic_i = col - PREALLOC_COLUMNS;
        assert!(dynamic_i < self.dynamic_stops.len());
        self.dynamic_stops[dynamic_i]
    }

    /// Resize this to support up to `cols` columns. Port of `Tabstops.zig`
    /// `resize`.
    ///
    /// Note: like the Zig original, this does not set any new tabstops for
    /// the grown region (see the Zig `TODO: needs interval to set new
    /// tabstops`); callers that need tabstops re-applied after a resize call
    /// [`Tabstops::reset`] themselves.
    pub fn resize(&mut self, cols: usize) {
        // Do nothing if it fits.
        if cols <= PREALLOC_COLUMNS {
            self.cols = cols;
            return;
        }

        // What we need in the dynamic size.
        let size = cols - PREALLOC_COLUMNS;
        if size < self.dynamic_stops.len() {
            self.cols = cols;
            return;
        }

        self.dynamic_stops.resize(size, false);
        self.cols = cols;
    }

    /// Return the maximum number of columns this can support currently.
    /// Port of `Tabstops.zig` `capacity`.
    pub fn capacity(&self) -> usize {
        PREALLOC_COLUMNS + self.dynamic_stops.len()
    }

    /// Unset all tabstops and then reset the initial tabstops to the given
    /// interval. An interval of 0 sets no tabstops. Port of `Tabstops.zig`
    /// `reset`.
    pub fn reset(&mut self, interval: usize) {
        self.prealloc_stops.fill(false);
        self.dynamic_stops.fill(false);

        if interval > 0 {
            let mut i = interval;
            while i < self.cols.saturating_sub(1) {
                self.set(i);
                i += interval;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of Tabstops.zig "Tabstops: basic". The Zig test also exercises
    // `entry`/`index`/`masks`, the internal bit-packing helpers of the
    // hand-rolled bitset; this port has no equivalent internals (see module
    // docs), so only the externally observable set/get/reset behavior is
    // ported.
    #[test]
    fn basic() {
        let mut t = Tabstops::default();

        assert!(!t.get(4));
        t.set(4);
        assert!(t.get(4));
        assert!(!t.get(3));

        t.reset(0);
        assert!(!t.get(4));

        t.set(4);
        assert!(t.get(4));
        t.unset(4);
        assert!(!t.get(4));
    }

    // `unset` is an XOR *toggle* in upstream, not a clear: unsetting a column
    // that has no tabstop CREATES one. This is observable via `CSI 0 g` (TBC
    // "clear at cursor") at a non-tabstop column, so it must be replicated.
    #[test]
    fn unset_toggles_a_missing_stop_on() {
        let mut t = Tabstops::default();
        assert!(!t.get(1)); // no default stop at col 1
        t.unset(1);
        assert!(t.get(1), "unset on an empty column must toggle it ON (XOR)");
        t.unset(1);
        assert!(!t.get(1), "a second unset toggles it back OFF");

        // Same in the dynamically-allocated region (beyond prealloc).
        let far = PREALLOC_COLUMNS + 3;
        t.resize(far + 1);
        assert!(!t.get(far));
        t.unset(far);
        assert!(t.get(far));
    }

    // Port of Tabstops.zig "Tabstops: dynamic allocations".
    #[test]
    fn dynamic_allocations() {
        let mut t = Tabstops::default();

        // Grow the capacity by 2.
        let cap = t.capacity();
        t.resize(cap * 2);

        // Set something that was out of range of the first.
        t.set(cap + 5);
        assert!(t.get(cap + 5));
        assert!(!t.get(cap + 4));

        // Prealloc still works.
        assert!(!t.get(5));
    }

    // Port of Tabstops.zig "Tabstops: interval".
    #[test]
    fn interval() {
        let t = Tabstops::new(80, 4);
        assert!(!t.get(0));
        assert!(t.get(4));
        assert!(!t.get(5));
        assert!(t.get(8));
    }

    // Port of Tabstops.zig "Tabstops: count on 80".
    // https://superuser.com/questions/710019/why-there-are-11-tabstops-on-a-80-column-console
    #[test]
    fn count_on_80() {
        let t = Tabstops::new(80, 8);

        let count = (0..80).filter(|&i| t.get(i)).count();
        assert_eq!(count, 9);
    }

    // Zig "Tabstops: resize alloc failure preserves state" exercises a
    // tripwire-injected allocator failure mid-resize and asserts `cols` is
    // left unchanged. This port's `resize` uses `Vec::resize`, which aborts
    // the process on allocation failure rather than returning a recoverable
    // error (no `Allocator.Error` equivalent in safe Rust for `Vec`), so
    // there's no failure path to preserve state around. Ported instead as a
    // no-op-resize invariant: resizing to a size that doesn't require
    // dynamic growth must never touch `cols` incorrectly, and resizing
    // within the already-allocated dynamic region is a cheap no-alloc path.
    #[test]
    fn resize_within_capacity_preserves_cols_and_stops() {
        let mut t = Tabstops::new(80, 8);
        let original_cols = t.cols;
        assert!(t.get(8));

        // A resize to a smaller or equal column count must not perturb
        // existing tabstops or grow allocations.
        t.resize(PREALLOC_COLUMNS);
        assert_eq!(t.cols, PREALLOC_COLUMNS);
        assert!(t.get(8));

        // Restore and confirm the original value round-trips.
        t.resize(original_cols);
        assert_eq!(t.cols, original_cols);
    }
}
