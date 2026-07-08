//! Built-in split keybinds — the split subset of real Ghostty's default keybind
//! block, hardcoded as a small static table (the same shape as
//! [`crate::tabkeys`]).
//!
//! This is deliberately *not* the full `Binding.zig` keybind system (deferred).
//! It is the narrowest thing that makes split navigation match a macOS
//! maintainer's expectations.
//!
//! Ported from upstream `src/config/Config.zig` (Ghostty commit `2da015cd6`),
//! the default keybind block (lines ~6625-6667):
//!
//! - `new_split:right` / `new_split:down`. Upstream binds these to
//!   `ctrl+shift+o` / `ctrl+shift+e` on **all** platforms (Config.zig 6625-6632
//!   — there is no macOS override). The maintainer daily-drives macOS and asked
//!   for the iTerm2/macOS-native `cmd+d` / `cmd+shift+d` instead, so those are
//!   the primary bindings here; the upstream `ctrl+shift+o`/`e` chords are kept
//!   as aliases so muscle memory from upstream still works. This mirrors how
//!   [`crate::tabkeys`] added `cmd+shift+[`/`]` as a maintainer alias over
//!   upstream's defaults.
//! - `goto_split:previous` / `goto_split:next` → `ctrl+super+[` / `ctrl+super+]`
//!   (Config.zig 6636-6645) — matched exactly.
//! - `goto_split:{up,down,left,right}` → `ctrl+alt+arrow` (Config.zig 6649-6667)
//!   — matched exactly.
//!
//! Structured so a future config-driven keybind chunk can absorb it: the table
//! is data ([`DEFAULT_SPLIT_BINDINGS`]) and [`resolve`] is a pure function over
//! the same AppKit-free [`TabMods`](crate::tabkeys::TabMods) bitset the tab
//! table uses.

use qwertty_term_input::key::Key;

use crate::splits::{Direction, Sequential};
use crate::tabkeys::TabMods;

/// A split action a built-in binding maps to. Executed against the focused
/// tab's split tree by [`crate::app::Controller`].
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

/// One entry in the built-in table: an exact `(key, mods)` trigger → action.
#[derive(Debug, Clone, Copy)]
pub struct SplitKeyBinding {
    pub key: Key,
    pub mods: TabMods,
    pub action: SplitAction,
}

/// The built-in split keybind table. A future config-driven keybind chunk
/// should treat this as the *default* set it layers user overrides on top of.
///
/// [`resolve`] requires an exact `(key, mods)` match; no two entries share a
/// trigger.
pub static DEFAULT_SPLIT_BINDINGS: &[SplitKeyBinding] = &[
    // --- new split (maintainer cmd+d / cmd+shift+d; see module docs) ---
    SplitKeyBinding {
        key: Key::KeyD,
        mods: MODS_CMD,
        action: SplitAction::NewSplit(Direction::Right),
    },
    SplitKeyBinding {
        key: Key::KeyD,
        mods: MODS_CMD_SHIFT,
        action: SplitAction::NewSplit(Direction::Down),
    },
    // --- new split (upstream ctrl+shift+o / ctrl+shift+e aliases) ---
    SplitKeyBinding {
        key: Key::KeyO,
        mods: MODS_CTRL_SHIFT,
        action: SplitAction::NewSplit(Direction::Right),
    },
    SplitKeyBinding {
        key: Key::KeyE,
        mods: MODS_CTRL_SHIFT,
        action: SplitAction::NewSplit(Direction::Down),
    },
    // --- goto split previous/next (ctrl+super+[ / ctrl+super+], Config.zig 6636-6645) ---
    SplitKeyBinding {
        key: Key::BracketLeft,
        mods: MODS_CTRL_SUPER,
        action: SplitAction::GotoAdjacent(Sequential::Previous),
    },
    SplitKeyBinding {
        key: Key::BracketRight,
        mods: MODS_CTRL_SUPER,
        action: SplitAction::GotoAdjacent(Sequential::Next),
    },
    // --- directional goto split (ctrl+alt+arrow, Config.zig 6649-6667) ---
    SplitKeyBinding {
        key: Key::ArrowUp,
        mods: MODS_CTRL_ALT,
        action: SplitAction::GotoSplit(Direction::Up),
    },
    SplitKeyBinding {
        key: Key::ArrowDown,
        mods: MODS_CTRL_ALT,
        action: SplitAction::GotoSplit(Direction::Down),
    },
    SplitKeyBinding {
        key: Key::ArrowLeft,
        mods: MODS_CTRL_ALT,
        action: SplitAction::GotoSplit(Direction::Left),
    },
    SplitKeyBinding {
        key: Key::ArrowRight,
        mods: MODS_CTRL_ALT,
        action: SplitAction::GotoSplit(Direction::Right),
    },
    // --- toggle zoom (cmd+shift+enter; upstream `ctrlOrSuper(shift)+enter`,
    //     Config.zig 6857-6861 → super+shift on macOS) ---
    SplitKeyBinding {
        key: Key::Enter,
        mods: MODS_CMD_SHIFT,
        action: SplitAction::ToggleZoom,
    },
    // --- resize split (cmd+ctrl+shift+arrows, 10px; Config.zig 6671-6695) ---
    SplitKeyBinding {
        key: Key::ArrowUp,
        mods: MODS_CMD_CTRL_SHIFT,
        action: SplitAction::ResizeSplit(Direction::Up),
    },
    SplitKeyBinding {
        key: Key::ArrowDown,
        mods: MODS_CMD_CTRL_SHIFT,
        action: SplitAction::ResizeSplit(Direction::Down),
    },
    SplitKeyBinding {
        key: Key::ArrowLeft,
        mods: MODS_CMD_CTRL_SHIFT,
        action: SplitAction::ResizeSplit(Direction::Left),
    },
    SplitKeyBinding {
        key: Key::ArrowRight,
        mods: MODS_CMD_CTRL_SHIFT,
        action: SplitAction::ResizeSplit(Direction::Right),
    },
    // --- equalize splits (cmd+ctrl+=; Config.zig 7050-7054) ---
    SplitKeyBinding {
        key: Key::Equal,
        mods: MODS_CMD_CTRL,
        action: SplitAction::EqualizeSplits,
    },
];

/// The fixed pixel step a `resize_split` chord moves the divider by (upstream's
/// default `.{ direction, 10 }`, Config.zig 6671-6695). In *points*; the
/// controller scales to device pixels for the tree op.
pub const RESIZE_STEP_PT: f64 = 10.0;

// Modifier combos used above. `super_` is Cmd on macOS.
const MODS_CMD: TabMods = TabMods {
    shift: false,
    ctrl: false,
    alt: false,
    super_: true,
};
const MODS_CMD_SHIFT: TabMods = TabMods {
    shift: true,
    ctrl: false,
    alt: false,
    super_: true,
};
const MODS_CTRL_SHIFT: TabMods = TabMods {
    shift: true,
    ctrl: true,
    alt: false,
    super_: false,
};
const MODS_CTRL_SUPER: TabMods = TabMods {
    shift: false,
    ctrl: true,
    alt: false,
    super_: true,
};
const MODS_CTRL_ALT: TabMods = TabMods {
    shift: false,
    ctrl: true,
    alt: true,
    super_: false,
};
const MODS_CMD_CTRL_SHIFT: TabMods = TabMods {
    shift: true,
    ctrl: true,
    alt: false,
    super_: true,
};
const MODS_CMD_CTRL: TabMods = TabMods {
    shift: false,
    ctrl: true,
    alt: false,
    super_: true,
};

/// Resolve a physical key + modifier state to a built-in split action, or `None`
/// if the chord is not a split binding. Exact match on both key and the four
/// modifiers, so unrelated chords (and the tab chords in [`crate::tabkeys`])
/// never collide here.
pub fn resolve(key: Key, mods: TabMods) -> Option<SplitAction> {
    DEFAULT_SPLIT_BINDINGS
        .iter()
        .find(|b| b.key == key && b.mods == mods)
        .map(|b| b.action)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cmd_d_is_new_split_right_and_cmd_shift_d_is_down() {
        assert_eq!(
            resolve(Key::KeyD, MODS_CMD),
            Some(SplitAction::NewSplit(Direction::Right))
        );
        assert_eq!(
            resolve(Key::KeyD, MODS_CMD_SHIFT),
            Some(SplitAction::NewSplit(Direction::Down))
        );
    }

    #[test]
    fn upstream_ctrl_shift_o_e_aliases_still_work() {
        assert_eq!(
            resolve(Key::KeyO, MODS_CTRL_SHIFT),
            Some(SplitAction::NewSplit(Direction::Right))
        );
        assert_eq!(
            resolve(Key::KeyE, MODS_CTRL_SHIFT),
            Some(SplitAction::NewSplit(Direction::Down))
        );
    }

    #[test]
    fn ctrl_alt_arrows_are_directional_goto() {
        assert_eq!(
            resolve(Key::ArrowUp, MODS_CTRL_ALT),
            Some(SplitAction::GotoSplit(Direction::Up))
        );
        assert_eq!(
            resolve(Key::ArrowDown, MODS_CTRL_ALT),
            Some(SplitAction::GotoSplit(Direction::Down))
        );
        assert_eq!(
            resolve(Key::ArrowLeft, MODS_CTRL_ALT),
            Some(SplitAction::GotoSplit(Direction::Left))
        );
        assert_eq!(
            resolve(Key::ArrowRight, MODS_CTRL_ALT),
            Some(SplitAction::GotoSplit(Direction::Right))
        );
    }

    #[test]
    fn cmd_shift_enter_toggles_zoom() {
        assert_eq!(
            resolve(Key::Enter, MODS_CMD_SHIFT),
            Some(SplitAction::ToggleZoom)
        );
    }

    #[test]
    fn cmd_ctrl_shift_arrows_resize() {
        assert_eq!(
            resolve(Key::ArrowUp, MODS_CMD_CTRL_SHIFT),
            Some(SplitAction::ResizeSplit(Direction::Up))
        );
        assert_eq!(
            resolve(Key::ArrowDown, MODS_CMD_CTRL_SHIFT),
            Some(SplitAction::ResizeSplit(Direction::Down))
        );
        assert_eq!(
            resolve(Key::ArrowLeft, MODS_CMD_CTRL_SHIFT),
            Some(SplitAction::ResizeSplit(Direction::Left))
        );
        assert_eq!(
            resolve(Key::ArrowRight, MODS_CMD_CTRL_SHIFT),
            Some(SplitAction::ResizeSplit(Direction::Right))
        );
    }

    #[test]
    fn cmd_ctrl_equal_equalizes() {
        assert_eq!(
            resolve(Key::Equal, MODS_CMD_CTRL),
            Some(SplitAction::EqualizeSplits)
        );
    }

    #[test]
    fn ctrl_super_brackets_are_prev_next() {
        assert_eq!(
            resolve(Key::BracketLeft, MODS_CTRL_SUPER),
            Some(SplitAction::GotoAdjacent(Sequential::Previous))
        );
        assert_eq!(
            resolve(Key::BracketRight, MODS_CTRL_SUPER),
            Some(SplitAction::GotoAdjacent(Sequential::Next))
        );
    }

    #[test]
    fn unrelated_chords_do_not_resolve() {
        // Plain cmd+d without shift is right, but cmd+d with ctrl is nothing.
        assert_eq!(
            resolve(
                Key::KeyD,
                TabMods {
                    shift: false,
                    ctrl: true,
                    alt: false,
                    super_: true
                }
            ),
            None
        );
        // A bare arrow (no mods) must never be a split chord — it has to reach
        // the PTY encoder as a cursor key.
        assert_eq!(resolve(Key::ArrowUp, TabMods::default()), None);
        // cmd+w (close) is not a split binding.
        assert_eq!(resolve(Key::KeyW, MODS_CMD), None);
    }

    #[test]
    fn table_has_no_duplicate_triggers() {
        for (i, a) in DEFAULT_SPLIT_BINDINGS.iter().enumerate() {
            for b in &DEFAULT_SPLIT_BINDINGS[i + 1..] {
                assert!(
                    !(a.key == b.key && a.mods == b.mods),
                    "duplicate trigger: {a:?} and {b:?}"
                );
            }
        }
    }

    #[test]
    fn does_not_collide_with_tab_bindings() {
        // No split chord may also resolve as a tab chord — they share the view's
        // performKeyEquivalent path. cmd+d, ctrl+alt+arrows, ctrl+super+brackets
        // are all distinct from the tab table's ctrl+tab / cmd+digits /
        // cmd+shift+brackets.
        for b in DEFAULT_SPLIT_BINDINGS {
            assert_eq!(
                crate::tabkeys::resolve(b.key, b.mods),
                None,
                "split binding {b:?} also resolves as a tab action"
            );
        }
    }
}
