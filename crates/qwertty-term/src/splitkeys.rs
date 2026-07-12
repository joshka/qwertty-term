//! Split action + the resize step constant.
//!
//! The former hardcoded `(key, mods) -> SplitAction` table has been retired:
//! split chords now resolve through the ported `Binding.zig`
//! [`Set`](qwertty_term_input::binding::Set) (upstream `default_set()` + the
//! user's `keybind` config), dispatched by
//! `crate::app::Controller::perform_keybind_chord`. Adopting the real default
//! keymap means the macOS split bindings are now upstream's exactly — `cmd+d` /
//! `cmd+shift+d` for new splits, `cmd+[` / `cmd+]` for previous/next split,
//! `cmd+alt+arrow` for directional focus, `cmd+ctrl+arrow` for resize (the
//! former non-mac placeholder aliases `ctrl+shift+o`/`e` and `ctrl+super+[`/`]`
//! are dropped; a user can re-add them via `keybind` config). This module keeps
//! just the action enum the split handler consumes and the resize step.

use crate::splits::{Direction, Sequential};

/// A split action a binding maps to. Executed against the focused tab's split
/// tree by [`crate::app::Controller`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitAction {
    /// Create a new split in the given direction, spawning a new surface.
    NewSplit(Direction),
    /// Move focus to the spatially-adjacent split in the given direction.
    GotoSplit(Direction),
    /// Move focus to the previous / next split in flatten order (wraps).
    GotoAdjacent(Sequential),
    /// Toggle zoom on the focused split (fills the tab, hides the rest).
    ToggleZoom,
    /// Resize the focused split's containing split in the given direction by a
    /// fixed pixel step.
    ResizeSplit(Direction),
    /// Reset every split ratio to its leaf-count weight.
    EqualizeSplits,
}

/// The per-keystroke split resize step, in points (upstream binds
/// `resize_split:*,10`). Scaled by the backing-scale factor when applied
/// (`crate::app`).
pub const RESIZE_STEP_PT: f64 = 10.0;
