//! tmux Viewer layout → `SplitTree` converter + window → tab reconciler
//! (ADR 006 slice 5b — the pure, unit-testable logic layer).
//!
//! ADR 006 ratified **Option (a)**: a tmux *window* maps to a native *tab*, and
//! a tmux *pane* maps to a *split* within that tab. This module is the
//! AppKit-free half of that mapping:
//!
//! - [`layout_to_split_tree`] walks a tmux [`Layout`] tree (as tracked by
//!   [`tmux_viewer::Window::layout`](crate::tmux_viewer::Window)) and produces
//!   an app [`SplitTree`], preserving the split *structure* rather than
//!   flattening to rects.
//! - [`Reconciler`] diffs a Viewer's window set across `%layout-change` /
//!   `list-windows` refreshes into a [`ReconcilePlan`] of tab create/remove ops
//!   plus a fresh `SplitTree` per surviving window, while keeping a **stable**
//!   `pane_id → SurfaceId` map so surviving panes reuse their surfaces and only
//!   genuinely-new panes allocate.
//!
//! It creates no native tabs, splits, or surfaces and touches no control pty —
//! that is slice 5b-native + 5c. Everything here is a pure function of the
//! Viewer's window model, so it is fully headless and unit-tested.
//!
//! ## Axis / ratio semantics
//!
//! tmux and the app's [`SplitTree`] agree on axis orientation, so the mapping is
//! 1:1 (no transposition):
//!
//! - a tmux **`{…}` = [`Content::Horizontal`]** is a left-to-right (side-by-side)
//!   split → [`Axis::Horizontal`] (upstream `layout.zig`; `Axis::Horizontal` is
//!   "children side by side" per `splits.rs`);
//! - a tmux **`[…]` = [`Content::Vertical`]** is a top-to-bottom (stacked) split
//!   → [`Axis::Vertical`].
//!
//! tmux splits are n-ary; `SplitTree` splits are binary. An n-ary split
//! `[c0, c1, …, cn]` becomes a **right-leaning** binary chain: at each level the
//! first child is `c0` and the second child is the recursively-built chain of
//! the remaining children. The split's `ratio` is `dim(c0) / sum(dim(c0..cn))`
//! where `dim` is the child's **width** for a horizontal split and **height**
//! for a vertical one (tmux carries absolute cell geometry per node). Each ratio
//! is clamped into `[0.1, 0.9]` via [`clamp_ratio`] — the same clamp every other
//! `SplitTree` op applies — so an extreme tmux layout never collapses a pane to
//! zero. Upstream `viewer.zig` does not itself build a split tree (it emits an
//! opaque `windows` action and leaves surface mapping to the apprt), so these
//! semantics are defined against the tmux `Layout` format and the app's existing
//! `SplitTree` geometry, not ported line-for-line.

use std::collections::{HashMap, HashSet};

use qwertty_term_vt::tmux::layout::{Content, Layout};

use crate::splits::{Axis, Node, Split, SplitTree, SurfaceId, clamp_ratio};
use crate::tmux_viewer::Window;

/// Convert a tmux [`Layout`] tree into an app [`SplitTree`], mapping each pane
/// leaf's tmux id to a [`SurfaceId`] via `surface_of`. See the module docs for
/// the axis/ratio semantics. `surface_of` must return a surface for every pane
/// id in the layout (the [`Reconciler`] guarantees this by assigning surfaces
/// before it converts). The focused surface is the tree's leftmost leaf; the
/// caller can re-focus afterwards from tmux's active-pane signal.
pub fn layout_to_split_tree(layout: &Layout, surface_of: impl Fn(usize) -> SurfaceId) -> SplitTree {
    let root = node_of(layout, &surface_of);
    let focused = leftmost(&root);
    SplitTree::from_node(root, focused)
}

/// Build the [`Node`] for one layout node.
fn node_of(layout: &Layout, surface_of: &impl Fn(usize) -> SurfaceId) -> Node {
    match &layout.content {
        Content::Pane(id) => Node::Leaf(surface_of(*id)),
        Content::Horizontal(children) => chain(children, Axis::Horizontal, surface_of),
        Content::Vertical(children) => chain(children, Axis::Vertical, surface_of),
    }
}

/// Build a right-leaning binary split chain from an n-ary tmux split's children.
/// The layout parser never yields an empty child list, but guard defensively:
/// an empty list degenerates to a leaf of surface id 0 rather than panicking.
fn chain(children: &[Layout], axis: Axis, surface_of: &impl Fn(usize) -> SurfaceId) -> Node {
    debug_assert!(!children.is_empty(), "tmux split has no children");
    match children {
        [] => Node::Leaf(SurfaceId(0)),
        [only] => node_of(only, surface_of),
        [head, tail @ ..] => {
            let dim = |l: &Layout| match axis {
                Axis::Horizontal => l.width,
                Axis::Vertical => l.height,
            };
            let head_dim = dim(head);
            let total: usize = children.iter().map(dim).sum();
            let ratio = if total == 0 {
                0.5
            } else {
                clamp_ratio(head_dim as f64 / total as f64)
            };
            Node::Split(Box::new(Split {
                axis,
                ratio,
                first: node_of(head, surface_of),
                second: chain(tail, axis, surface_of),
            }))
        }
    }
}

/// The leftmost (first/top) leaf of a node — the default focus.
fn leftmost(node: &Node) -> SurfaceId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split(s) => leftmost(&s.first),
    }
}

/// Collect a layout's pane leaf ids in layout order, de-duplicated (first
/// occurrence wins — mirrors the Viewer's own leaf-collection order).
fn pane_ids_in_order(layout: &Layout, out: &mut Vec<usize>) {
    match &layout.content {
        Content::Pane(id) => {
            if !out.contains(id) {
                out.push(*id);
            }
        }
        Content::Horizontal(children) | Content::Vertical(children) => {
            for child in children {
                pane_ids_in_order(child, out);
            }
        }
    }
}

/// One native-surface reconciliation op the caller (slice 5b-native) applies.
/// The ops are ordered so a caller can apply them top-to-bottom: removals
/// first, then creations, then a `SetSplitTree` for every currently-present
/// window (both surviving and freshly created).
#[derive(Debug, Clone, PartialEq)]
pub enum ReconcileOp {
    /// Create a native tab for this tmux window id.
    CreateTab { window_id: usize },
    /// Remove the native tab for this (now-gone) tmux window id.
    RemoveTab { window_id: usize },
    /// Bind/replace the split tree of this window's tab.
    SetSplitTree { window_id: usize, tree: SplitTree },
}

/// The plan produced by one [`Reconciler::reconcile`] call: the ordered ops plus
/// the pane surfaces that disappeared (so the native layer can free their
/// renderer surfaces). `dropped_surfaces` is sorted for determinism.
#[derive(Debug, Clone, PartialEq)]
pub struct ReconcilePlan {
    pub ops: Vec<ReconcileOp>,
    pub dropped_surfaces: Vec<SurfaceId>,
}

/// Diffs a tmux Viewer's window set into native-tab intent while keeping a
/// stable `pane_id → SurfaceId` assignment. One [`Reconciler`] is owned per
/// Viewer (per `tmux -CC` connection). It holds no AppKit state — it produces
/// [`ReconcilePlan`]s the native layer turns into real tabs/splits.
#[derive(Debug, Clone, Default)]
pub struct Reconciler {
    /// Stable pane_id → SurfaceId assignment, preserved across layout changes so
    /// surviving panes keep their surface (and thus their renderer + terminal).
    surfaces: HashMap<usize, SurfaceId>,
    /// Monotonic surface-id allocator; ids are never reused (matches `SurfaceId`
    /// semantics in `splits.rs`).
    next_id: u64,
    /// The current native tab set, in the window order last reconciled.
    tabs: Vec<usize>,
}

impl Reconciler {
    /// A fresh reconciler with no tabs and no surfaces.
    pub fn new() -> Reconciler {
        Reconciler::default()
    }

    /// The stable surface id assigned to a pane id, if one has been assigned.
    pub fn surface_of(&self, pane_id: usize) -> Option<SurfaceId> {
        self.surfaces.get(&pane_id).copied()
    }

    /// The tmux pane id a native [`SurfaceId`] is bound to, if any — the reverse
    /// of [`surface_of`](Self::surface_of). The native layer (slice 5b-native)
    /// uses this to resolve a split-tree leaf back to the Viewer's pane
    /// `Terminal` it must render. Surface ids are unique, so at most one pane
    /// matches.
    pub fn pane_of_surface(&self, surface: SurfaceId) -> Option<usize> {
        self.surfaces
            .iter()
            .find_map(|(pane_id, sid)| (*sid == surface).then_some(*pane_id))
    }

    /// The current native tab set (window ids), in reconcile order.
    pub fn tabs(&self) -> &[usize] {
        &self.tabs
    }

    /// Reconcile the current Viewer window set into a native-surface plan.
    ///
    /// - assigns a fresh [`SurfaceId`] to every pane id not yet seen (in window
    ///   then layout order, so allocation is deterministic);
    /// - drops surfaces for panes no longer present in any window;
    /// - emits `RemoveTab` for gone windows, `CreateTab` for new ones;
    /// - emits `SetSplitTree` for every present window with its converted tree.
    ///
    /// Mirrors [`Viewer::sync_layouts`](crate::tmux_viewer::Viewer)'s pane diff,
    /// at the native-surface layer.
    pub fn reconcile(&mut self, windows: &[Window]) -> ReconcilePlan {
        let new_ids: Vec<usize> = windows.iter().map(|w| w.id).collect();

        // 1. Assign surfaces for any new pane, in deterministic order.
        let mut live_panes: HashSet<usize> = HashSet::new();
        for w in windows {
            let mut ids = Vec::new();
            pane_ids_in_order(&w.layout, &mut ids);
            for pane_id in ids {
                live_panes.insert(pane_id);
                self.surfaces.entry(pane_id).or_insert_with(|| {
                    let id = SurfaceId(self.next_id);
                    self.next_id += 1;
                    id
                });
            }
        }

        // 2. Drop surfaces whose pane is gone.
        let mut dropped: Vec<SurfaceId> = Vec::new();
        self.surfaces.retain(|pane_id, sid| {
            if live_panes.contains(pane_id) {
                true
            } else {
                dropped.push(*sid);
                false
            }
        });
        dropped.sort();

        // 3. Tab diff: removals first, then creations.
        let mut ops: Vec<ReconcileOp> = Vec::new();
        for &old in &self.tabs {
            if !new_ids.contains(&old) {
                ops.push(ReconcileOp::RemoveTab { window_id: old });
            }
        }
        for &new in &new_ids {
            if !self.tabs.contains(&new) {
                ops.push(ReconcileOp::CreateTab { window_id: new });
            }
        }

        // 4. A split tree for every present window (surviving + new). All pane
        //    ids are in `self.surfaces` from step 1, so the lookup is total.
        for w in windows {
            let tree = layout_to_split_tree(&w.layout, |pid| self.surfaces[&pid]);
            ops.push(ReconcileOp::SetSplitTree {
                window_id: w.id,
                tree,
            });
        }

        self.tabs = new_ids;
        ReconcilePlan {
            ops,
            dropped_surfaces: dropped,
        }
    }
}

#[cfg(test)]
mod tests;
