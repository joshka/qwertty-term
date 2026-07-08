//! The split tree: a binary tree of surfaces within one tab.
//!
//! Pure, AppKit-free logic (unit-tested without a window), ported *by design*
//! from upstream Ghostty's `macos/Sources/Features/Splits/SplitTree.swift`
//! (commit `2da015cd6`). Upstream implements splits entirely in the Swift app
//! layer as an immutable value tree; this is the same model, adapted to Rust
//! ownership.
//!
//! # Model
//!
//! A [`SplitTree`] is a binary tree whose leaves are [`SurfaceId`]s (opaque
//! keys into the tab's `HashMap<SurfaceId, Surface>` — the AppKit + engine
//! bundle lives there, not in the tree). An internal node is a [`Split`] with a
//! [`Axis`] and a `ratio` in `[0.1, 0.9]` giving the fraction of the container
//! the first (left/top) child occupies. A single-leaf tree is the byte-identical
//! analog of today's one-surface-per-tab: one leaf, no splits.
//!
//! ```text
//!   Split{ Horizontal, 0.5 }        A | B         (side by side)
//!     ├── Leaf(A)
//!     └── Split{ Vertical, 0.5 }    ┌───┬───┐
//!         ├── Leaf(B)              A│ B ├───┤
//!         └── Leaf(C)              │   │ C │
//!                                  └───┴───┘
//! ```
//!
//! # Operations mirrored from upstream
//!
//! - **insert** ([`SplitTree::split`]): replace a target leaf with a split whose
//!   two children are the old leaf and the new leaf, at ratio `0.5` — upstream
//!   `inserting(view:at:direction:)`.
//! - **remove/collapse** ([`SplitTree::close`]): drop a leaf; its parent split
//!   collapses so the sibling absorbs the parent's whole rect (no ratio
//!   redistribution) — upstream `removing(_:)`.
//! - **directional focus** ([`SplitTree::neighbor`]): compute each leaf's pixel
//!   rect, then pick the spatially-nearest leaf in the requested direction —
//!   upstream's `Spatial` nearest-neighbour walk.
//! - **prev/next focus** ([`SplitTree::adjacent`]): flatten leaves in in-order
//!   and step with wrap — upstream `.previous`/`.next`.
//! - **layout** ([`SplitTree::layout`]): recursively divide a pixel rect by each
//!   split's axis + ratio into per-leaf rects — upstream `Spatial` slots (the
//!   divider gaps are inserted by the caller, since they're AppKit geometry).
//!
//! # Deferred (see `docs/analysis/splits.md`)
//!
//! Zoom (`toggle_split_zoom` + the `zoomed` node), equalize, and
//! programmatic `resize_split` keybinds are out of slice 1; the tree carries no
//! `zoomed` field yet.

use std::collections::HashMap;

/// An opaque per-surface key: a leaf in the split tree and a lookup key into the
/// tab's surface map. Monotonic within a tab; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SurfaceId(pub u64);

/// The axis a split divides its container along.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    /// Children side by side (a vertical divider): `left | right`. Upstream
    /// `Direction.horizontal`.
    Horizontal,
    /// Children stacked (a horizontal divider): `top / bottom`. Upstream
    /// `Direction.vertical`.
    Vertical,
}

/// The direction a new split places the *new* surface relative to the focused
/// one, and the direction directional-focus navigation moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

impl Direction {
    /// The split axis this direction implies: left/right split horizontally
    /// (vertical divider), up/down split vertically (horizontal divider).
    pub fn axis(self) -> Axis {
        match self {
            Direction::Left | Direction::Right => Axis::Horizontal,
            Direction::Up | Direction::Down => Axis::Vertical,
        }
    }

    /// Whether the *new* surface goes into the first (left/top) child slot. For
    /// `Left`/`Up` the new surface takes the first slot and the existing one is
    /// pushed to the second; for `Right`/`Down` the existing surface stays
    /// first. Matches upstream `inserting`'s left/right assignment.
    pub fn new_is_first(self) -> bool {
        matches!(self, Direction::Left | Direction::Up)
    }
}

/// Prev/next flatten-order navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sequential {
    Previous,
    Next,
}

/// The minimum / maximum ratio a divider can take (upstream clamps to
/// `[0.1, 0.9]` so neither pane collapses to nothing).
pub const MIN_RATIO: f64 = 0.1;
pub const MAX_RATIO: f64 = 0.9;

/// Clamp a ratio into the allowed divider range.
pub fn clamp_ratio(r: f64) -> f64 {
    r.clamp(MIN_RATIO, MAX_RATIO)
}

/// A node in the split tree: either a surface leaf or an internal split.
#[derive(Debug, Clone, PartialEq)]
pub enum Node {
    /// A single surface.
    Leaf(SurfaceId),
    /// A division of the container into two child sub-trees.
    Split(Box<Split>),
}

/// An internal split node.
#[derive(Debug, Clone, PartialEq)]
pub struct Split {
    /// The axis this split divides along.
    pub axis: Axis,
    /// Fraction of the container the first (left/top) child occupies,
    /// `[0.1, 0.9]`. The second (right/bottom) child gets `1 - ratio`.
    pub ratio: f64,
    /// The first (left / top) child.
    pub first: Node,
    /// The second (right / bottom) child.
    pub second: Node,
}

/// A rectangle in pixels (or points — the tree is unit-agnostic; the caller
/// picks). `x`/`y` is the top-left origin, `w`/`h` the size.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Rect { x, y, w, h }
    }

    fn min_x(&self) -> f64 {
        self.x
    }
    fn max_x(&self) -> f64 {
        self.x + self.w
    }
    fn min_y(&self) -> f64 {
        self.y
    }
    fn max_y(&self) -> f64 {
        self.y + self.h
    }
    fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.min_x() && px < self.max_x() && py >= self.min_y() && py < self.max_y()
    }
}

/// One divider between two panes, produced by [`SplitTree::layout`]. The rect is
/// the thin strip the divider occupies; `axis` tells the caller which cursor /
/// drag orientation to use, and `path` locates the split whose ratio a drag on
/// this divider mutates.
#[derive(Debug, Clone, PartialEq)]
pub struct Divider {
    /// The divider's pixel rect (a thin strip between the two panes).
    pub rect: Rect,
    /// The axis of the split this divider belongs to (`Horizontal` → a vertical
    /// draggable strip; `Vertical` → a horizontal one).
    pub axis: Axis,
    /// Path from the root to the split node this divider controls, as a sequence
    /// of child choices (`false` = first, `true` = second). [`SplitTree::resize`]
    /// consumes it to mutate exactly that split's ratio.
    pub path: Vec<bool>,
}

/// The full pixel layout of a tree within a container rect: every leaf's rect
/// and every divider strip.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// Each surface's pixel rect.
    pub panes: HashMap<SurfaceId, Rect>,
    /// The divider strips between panes.
    pub dividers: Vec<Divider>,
}

/// The split tree for one tab: the node tree plus the currently focused leaf.
#[derive(Debug, Clone, PartialEq)]
pub struct SplitTree {
    root: Node,
    focused: SurfaceId,
}

impl SplitTree {
    /// A fresh single-leaf tree — the one-surface tab, behaviourally identical
    /// to the pre-splits `Tab`.
    pub fn leaf(id: SurfaceId) -> Self {
        SplitTree {
            root: Node::Leaf(id),
            focused: id,
        }
    }

    /// The currently focused surface.
    pub fn focused(&self) -> SurfaceId {
        self.focused
    }

    /// Make `id` the focused surface if it is present in the tree.
    pub fn focus(&mut self, id: SurfaceId) -> bool {
        if self.contains(id) {
            self.focused = id;
            true
        } else {
            false
        }
    }

    /// All surface ids in in-order (left-to-right, top-to-bottom flatten order).
    pub fn surfaces(&self) -> Vec<SurfaceId> {
        let mut out = Vec::new();
        collect_leaves(&self.root, &mut out);
        out
    }

    /// The number of surfaces (leaves) in the tree.
    pub fn len(&self) -> usize {
        self.surfaces().len()
    }

    pub fn is_empty(&self) -> bool {
        false // a tree always has at least one leaf
    }

    /// Whether `id` is a leaf of the tree.
    pub fn contains(&self, id: SurfaceId) -> bool {
        self.surfaces().contains(&id)
    }

    /// Split the *focused* surface: replace its leaf with a split whose children
    /// are the old surface and `new`, placed per `direction` at ratio `0.5`.
    /// The new surface becomes focused (upstream focuses the freshly-created
    /// split surface). Mirrors `inserting(view:at:direction:)`.
    pub fn split(&mut self, new: SurfaceId, direction: Direction) {
        let target = self.focused;
        let axis = direction.axis();
        let (first, second) = if direction.new_is_first() {
            (Node::Leaf(new), Node::Leaf(target))
        } else {
            (Node::Leaf(target), Node::Leaf(new))
        };
        let replacement = Node::Split(Box::new(Split {
            axis,
            ratio: 0.5,
            first,
            second,
        }));
        replace_leaf(&mut self.root, target, replacement);
        self.focused = new;
    }

    /// Close a surface: remove its leaf and collapse its parent split so the
    /// sibling absorbs the space (no ratio change). Returns `Some(new_focus)`
    /// with the surface that should receive focus afterwards, or `None` if the
    /// closed surface was the tab's last one (the caller closes the tab).
    ///
    /// If the closed surface was focused, focus moves to the sibling sub-tree's
    /// nearest leaf (upstream picks the next focus target from the collapsing
    /// sibling); otherwise the existing focus is preserved. Mirrors
    /// `removing(_:)`.
    pub fn close(&mut self, id: SurfaceId) -> Option<SurfaceId> {
        // Last surface: nothing to collapse into — signal tab close.
        if matches!(&self.root, Node::Leaf(leaf) if *leaf == id) {
            return None;
        }
        if !self.contains(id) {
            // Not ours; leave focus untouched.
            return Some(self.focused);
        }

        // The leaf that should inherit focus if we're closing the focused one:
        // the leftmost leaf of the sibling that survives the collapse.
        let sibling_focus = sibling_leaf(&self.root, id);

        remove_leaf(&mut self.root, id);

        if self.focused == id {
            self.focused = sibling_focus.unwrap_or_else(|| {
                // Fallback: any surviving leaf (shouldn't be needed — a
                // collapse always leaves a sibling).
                self.surfaces()[0]
            });
        }
        Some(self.focused)
    }

    /// The surface spatially adjacent to the focused one in `direction`, given
    /// the current pixel `layout`, or `None` if there is no pane that way.
    /// Mirrors upstream's spatial nearest-neighbour: filter the leaves strictly
    /// on the far side of the focused pane along the axis, then pick the one
    /// whose centre is closest (Euclidean).
    pub fn neighbor(&self, direction: Direction, layout: &Layout) -> Option<SurfaceId> {
        let from = *layout.panes.get(&self.focused)?;
        let (fcx, fcy) = from.center();

        let mut best: Option<(f64, SurfaceId)> = None;
        for (&id, rect) in &layout.panes {
            if id == self.focused {
                continue;
            }
            // Keep only panes that lie on the correct side. Use a small overlap
            // test on the cross axis so a pane that only touches a corner isn't
            // considered a left/right (resp. up/down) neighbour.
            let on_side = match direction {
                Direction::Left => rect.max_x() <= from.min_x() + EPS && overlaps_y(rect, &from),
                Direction::Right => rect.min_x() >= from.max_x() - EPS && overlaps_y(rect, &from),
                Direction::Up => rect.max_y() <= from.min_y() + EPS && overlaps_x(rect, &from),
                Direction::Down => rect.min_y() >= from.max_y() - EPS && overlaps_x(rect, &from),
            };
            if !on_side {
                continue;
            }
            let (cx, cy) = rect.center();
            let dist = (cx - fcx).powi(2) + (cy - fcy).powi(2);
            match best {
                Some((bd, _)) if bd <= dist => {}
                _ => best = Some((dist, id)),
            }
        }
        best.map(|(_, id)| id)
    }

    /// The surface before/after the focused one in in-order flatten order, with
    /// wrap-around. `None` only for a single-leaf tree. Mirrors upstream
    /// `.previous`/`.next`.
    pub fn adjacent(&self, seq: Sequential) -> Option<SurfaceId> {
        let leaves = self.surfaces();
        if leaves.len() < 2 {
            return None;
        }
        let idx = leaves.iter().position(|&l| l == self.focused)?;
        let n = leaves.len();
        let next = match seq {
            Sequential::Next => (idx + 1) % n,
            Sequential::Previous => (idx + n - 1) % n,
        };
        Some(leaves[next])
    }

    /// The surface whose pane rect contains the pixel point `(px, py)` in the
    /// given `layout`, or `None` if the point is on a divider / outside. Used for
    /// click-to-focus.
    pub fn hit_test(&self, px: f64, py: f64, layout: &Layout) -> Option<SurfaceId> {
        layout
            .panes
            .iter()
            .find(|(_, r)| r.contains(px, py))
            .map(|(&id, _)| id)
    }

    /// Resize the split identified by a divider `path` by `delta` (in the same
    /// units as the container passed to [`layout`](Self::layout)); positive
    /// `delta` grows the first child. The ratio is re-derived from the split's
    /// own pixel span so a drag maps 1:1 to cursor motion, then clamped. `span`
    /// is the split container's extent along its axis (width for a horizontal
    /// split, height for a vertical one) — the caller has it from the layout.
    pub fn resize(&mut self, path: &[bool], delta: f64, span: f64) {
        if span <= 0.0 {
            return;
        }
        if let Some(Node::Split(split)) = node_at_path_mut(&mut self.root, path) {
            let new = split.ratio + delta / span;
            split.ratio = clamp_ratio(new);
        }
    }

    /// Set the ratio of the split at `path` directly (used by an absolute
    /// divider drag that computes the new ratio itself). Clamped.
    pub fn set_ratio(&mut self, path: &[bool], ratio: f64) {
        if let Some(Node::Split(split)) = node_at_path_mut(&mut self.root, path) {
            split.ratio = clamp_ratio(ratio);
        }
    }

    /// The pixel rect the split at `path` (false=first / true=second at each
    /// level) occupies within `container`, and its axis — the geometry a divider
    /// drag needs to convert a pointer position into a ratio. `None` if the path
    /// doesn't lead to a split node.
    pub fn split_rect(&self, path: &[bool], container: Rect, divider: f64) -> Option<(Rect, Axis)> {
        let mut node = &self.root;
        let mut rect = container;
        for &second in path {
            let Node::Split(s) = node else {
                return None;
            };
            let (first_rect, second_rect) =
                child_rects(rect, s.axis, clamp_ratio(s.ratio), divider);
            if second {
                node = &s.second;
                rect = second_rect;
            } else {
                node = &s.first;
                rect = first_rect;
            }
        }
        match node {
            Node::Split(s) => Some((rect, s.axis)),
            Node::Leaf(_) => None,
        }
    }

    /// Compute every leaf's pixel rect and every divider strip within
    /// `container`. `divider` is the divider thickness in the same units; each
    /// split reserves that strip between its two children (the children shrink
    /// to make room), matching a real hand-rolled split container. Window resize
    /// is just calling this again with a new `container` — ratios are preserved,
    /// so panes redistribute proportionally.
    pub fn layout(&self, container: Rect, divider: f64) -> Layout {
        let mut panes = HashMap::new();
        let mut dividers = Vec::new();
        layout_node(
            &self.root,
            container,
            divider,
            &mut Vec::new(),
            &mut panes,
            &mut dividers,
        );
        Layout { panes, dividers }
    }
}

/// A tiny epsilon for the neighbour side tests (pixel rects derived from ratios
/// won't be bit-exact).
const EPS: f64 = 0.5;

fn overlaps_x(a: &Rect, b: &Rect) -> bool {
    a.min_x() < b.max_x() - EPS && a.max_x() > b.min_x() + EPS
}
fn overlaps_y(a: &Rect, b: &Rect) -> bool {
    a.min_y() < b.max_y() - EPS && a.max_y() > b.min_y() + EPS
}

/// In-order leaf collection.
fn collect_leaves(node: &Node, out: &mut Vec<SurfaceId>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Split(s) => {
            collect_leaves(&s.first, out);
            collect_leaves(&s.second, out);
        }
    }
}

/// Replace the leaf `target` anywhere in the tree with `replacement`.
fn replace_leaf(node: &mut Node, target: SurfaceId, replacement: Node) -> bool {
    match node {
        Node::Leaf(id) => {
            if *id == target {
                *node = replacement;
                true
            } else {
                false
            }
        }
        Node::Split(s) => {
            // Try the first child; if it consumed the replacement, stop.
            if replace_leaf(&mut s.first, target, replacement.clone()) {
                return true;
            }
            replace_leaf(&mut s.second, target, replacement)
        }
    }
}

/// Remove the leaf `target`, collapsing its parent split so the sibling takes
/// the parent's place. Operates in place on the root (the root itself is never
/// the target here — the caller handles the last-leaf case).
fn remove_leaf(node: &mut Node, target: SurfaceId) {
    let Node::Split(split) = node else {
        return;
    };

    // Is a direct child the target leaf? Then collapse to the other child.
    let first_is_target = matches!(&split.first, Node::Leaf(id) if *id == target);
    let second_is_target = matches!(&split.second, Node::Leaf(id) if *id == target);
    if first_is_target {
        let sibling = std::mem::replace(&mut split.second, Node::Leaf(target));
        *node = sibling;
        return;
    }
    if second_is_target {
        let sibling = std::mem::replace(&mut split.first, Node::Leaf(target));
        *node = sibling;
        return;
    }

    // Otherwise recurse into whichever subtree contains it.
    if subtree_contains(&split.first, target) {
        remove_leaf(&mut split.first, target);
    } else {
        remove_leaf(&mut split.second, target);
    }
}

fn subtree_contains(node: &Node, target: SurfaceId) -> bool {
    match node {
        Node::Leaf(id) => *id == target,
        Node::Split(s) => subtree_contains(&s.first, target) || subtree_contains(&s.second, target),
    }
}

/// The leaf that should inherit focus when `target` is removed: the leftmost
/// leaf of `target`'s sibling subtree (the one that collapses up into the
/// parent's slot).
fn sibling_leaf(node: &Node, target: SurfaceId) -> Option<SurfaceId> {
    let Node::Split(split) = node else {
        return None;
    };
    let first_is_target = matches!(&split.first, Node::Leaf(id) if *id == target);
    let second_is_target = matches!(&split.second, Node::Leaf(id) if *id == target);
    if first_is_target {
        return Some(leftmost_leaf(&split.second));
    }
    if second_is_target {
        return Some(leftmost_leaf(&split.first));
    }
    if subtree_contains(&split.first, target) {
        sibling_leaf(&split.first, target)
    } else {
        sibling_leaf(&split.second, target)
    }
}

fn leftmost_leaf(node: &Node) -> SurfaceId {
    match node {
        Node::Leaf(id) => *id,
        Node::Split(s) => leftmost_leaf(&s.first),
    }
}

/// Follow a `false=first / true=second` path to a node, mutably.
fn node_at_path_mut<'a>(mut node: &'a mut Node, path: &[bool]) -> Option<&'a mut Node> {
    for &second in path {
        match node {
            Node::Split(s) => {
                node = if second { &mut s.second } else { &mut s.first };
            }
            Node::Leaf(_) => return None,
        }
    }
    Some(node)
}

/// The two child rects a split of `axis`/`ratio` produces within `rect`,
/// reserving `divider` px between them. The single source of truth for split
/// geometry (used by both [`SplitTree::layout`] and
/// [`SplitTree::split_rect`]).
fn child_rects(rect: Rect, axis: Axis, ratio: f64, divider: f64) -> (Rect, Rect) {
    match axis {
        Axis::Horizontal => {
            let avail = (rect.w - divider).max(0.0);
            let first_w = avail * ratio;
            let second_w = avail - first_w;
            (
                Rect::new(rect.x, rect.y, first_w, rect.h),
                Rect::new(rect.x + first_w + divider, rect.y, second_w, rect.h),
            )
        }
        Axis::Vertical => {
            let avail = (rect.h - divider).max(0.0);
            let first_h = avail * ratio;
            let second_h = avail - first_h;
            (
                Rect::new(rect.x, rect.y, rect.w, first_h),
                Rect::new(rect.x, rect.y + first_h + divider, rect.w, second_h),
            )
        }
    }
}

/// The divider strip between the two child rects of a split.
fn divider_rect(rect: Rect, axis: Axis, ratio: f64, divider: f64) -> Rect {
    match axis {
        Axis::Horizontal => {
            let first_w = (rect.w - divider).max(0.0) * ratio;
            Rect::new(rect.x + first_w, rect.y, divider, rect.h)
        }
        Axis::Vertical => {
            let first_h = (rect.h - divider).max(0.0) * ratio;
            Rect::new(rect.x, rect.y + first_h, rect.w, divider)
        }
    }
}

/// Recursively lay a node out within `rect`, reserving `divider` px between each
/// split's children.
fn layout_node(
    node: &Node,
    rect: Rect,
    divider: f64,
    path: &mut Vec<bool>,
    panes: &mut HashMap<SurfaceId, Rect>,
    dividers: &mut Vec<Divider>,
) {
    match node {
        Node::Leaf(id) => {
            panes.insert(*id, rect);
        }
        Node::Split(s) => {
            let ratio = clamp_ratio(s.ratio);
            let (first_rect, second_rect) = child_rects(rect, s.axis, ratio, divider);
            dividers.push(Divider {
                rect: divider_rect(rect, s.axis, ratio, divider),
                axis: s.axis,
                path: path.clone(),
            });
            path.push(false);
            layout_node(&s.first, first_rect, divider, path, panes, dividers);
            path.pop();
            path.push(true);
            layout_node(&s.second, second_rect, divider, path, panes, dividers);
            path.pop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(n: u64) -> SurfaceId {
        SurfaceId(n)
    }

    #[test]
    fn single_leaf_is_the_whole_container() {
        let tree = SplitTree::leaf(s(0));
        assert_eq!(tree.len(), 1);
        assert_eq!(tree.focused(), s(0));
        let layout = tree.layout(Rect::new(0.0, 0.0, 100.0, 100.0), 4.0);
        assert_eq!(layout.panes[&s(0)], Rect::new(0.0, 0.0, 100.0, 100.0));
        assert!(layout.dividers.is_empty());
    }

    #[test]
    fn split_right_places_new_surface_in_second_slot_and_focuses_it() {
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        assert_eq!(tree.len(), 2);
        assert_eq!(tree.focused(), s(1));
        let layout = tree.layout(Rect::new(0.0, 0.0, 104.0, 100.0), 4.0);
        // 104 - 4 divider = 100 usable, 50/50 → each 50 wide.
        assert_eq!(layout.panes[&s(0)], Rect::new(0.0, 0.0, 50.0, 100.0));
        assert_eq!(layout.panes[&s(1)], Rect::new(54.0, 0.0, 50.0, 100.0));
        assert_eq!(layout.dividers.len(), 1);
        assert_eq!(layout.dividers[0].axis, Axis::Horizontal);
        assert_eq!(layout.dividers[0].rect, Rect::new(50.0, 0.0, 4.0, 100.0));
    }

    #[test]
    fn split_left_places_new_surface_first() {
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Left);
        let layout = tree.layout(Rect::new(0.0, 0.0, 104.0, 100.0), 4.0);
        // New (1) is first/left; old (0) second/right.
        assert_eq!(layout.panes[&s(1)].x, 0.0);
        assert!(layout.panes[&s(0)].x > layout.panes[&s(1)].x);
    }

    #[test]
    fn split_down_stacks_vertically() {
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Down);
        let layout = tree.layout(Rect::new(0.0, 0.0, 100.0, 104.0), 4.0);
        assert_eq!(layout.panes[&s(0)], Rect::new(0.0, 0.0, 100.0, 50.0));
        assert_eq!(layout.panes[&s(1)], Rect::new(0.0, 54.0, 100.0, 50.0));
        assert_eq!(layout.dividers[0].axis, Axis::Vertical);
    }

    #[test]
    fn three_panes_split_right_then_down() {
        // Split right (0 | 1), focus is on 1, then split it down → 0 | (1 / 2).
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        tree.split(s(2), Direction::Down);
        assert_eq!(tree.len(), 3);
        assert_eq!(tree.focused(), s(2));
        let surfaces = tree.surfaces();
        assert_eq!(surfaces, vec![s(0), s(1), s(2)]);
    }

    #[test]
    fn directional_neighbor_walks_the_grid() {
        // 0 | (1 / 2)
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        tree.split(s(2), Direction::Down);
        let layout = tree.layout(Rect::new(0.0, 0.0, 200.0, 200.0), 0.0);

        // From 2 (bottom-right), left goes to 0.
        tree.focus(s(2));
        assert_eq!(tree.neighbor(Direction::Left, &layout), Some(s(0)));
        // From 2, up goes to 1.
        assert_eq!(tree.neighbor(Direction::Up, &layout), Some(s(1)));
        // From 2, right/down have no neighbour.
        assert_eq!(tree.neighbor(Direction::Right, &layout), None);
        assert_eq!(tree.neighbor(Direction::Down, &layout), None);

        // From 0 (full left column), right goes to the nearest of 1/2. Its
        // centre is at mid-height, so 1 (top) and 2 (bottom) are equidistant;
        // either is acceptable — assert it's one of them.
        tree.focus(s(0));
        let right = tree.neighbor(Direction::Right, &layout);
        assert!(right == Some(s(1)) || right == Some(s(2)));

        // From 1 (top-right), left goes to 0.
        tree.focus(s(1));
        assert_eq!(tree.neighbor(Direction::Left, &layout), Some(s(0)));
        assert_eq!(tree.neighbor(Direction::Down, &layout), Some(s(2)));
    }

    #[test]
    fn adjacent_wraps_in_flatten_order() {
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        tree.split(s(2), Direction::Down);
        // Order: [0, 1, 2].
        tree.focus(s(0));
        assert_eq!(tree.adjacent(Sequential::Next), Some(s(1)));
        assert_eq!(tree.adjacent(Sequential::Previous), Some(s(2))); // wrap
        tree.focus(s(2));
        assert_eq!(tree.adjacent(Sequential::Next), Some(s(0))); // wrap
        assert_eq!(tree.adjacent(Sequential::Previous), Some(s(1)));
    }

    #[test]
    fn adjacent_single_leaf_is_none() {
        let tree = SplitTree::leaf(s(0));
        assert_eq!(tree.adjacent(Sequential::Next), None);
    }

    #[test]
    fn close_collapses_parent_and_sibling_absorbs_space() {
        // 0 | 1, close 1 → 0 fills the whole container.
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        let new_focus = tree.close(s(1));
        assert_eq!(new_focus, Some(s(0)));
        assert_eq!(tree.len(), 1);
        let layout = tree.layout(Rect::new(0.0, 0.0, 100.0, 100.0), 4.0);
        assert_eq!(layout.panes[&s(0)], Rect::new(0.0, 0.0, 100.0, 100.0));
        assert!(layout.dividers.is_empty());
    }

    #[test]
    fn close_middle_pane_collapses_to_two() {
        // 0 | (1 / 2). Close 1 (the middle in flatten order) → 0 | 2.
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        tree.split(s(2), Direction::Down);
        // Focus the middle pane (1), then close it.
        tree.focus(s(1));
        let new_focus = tree.close(s(1));
        assert_eq!(tree.len(), 2);
        assert_eq!(tree.surfaces(), vec![s(0), s(2)]);
        // Focus moved to the sibling (2).
        assert_eq!(new_focus, Some(s(2)));
        // 2 now fills the right column.
        let layout = tree.layout(Rect::new(0.0, 0.0, 204.0, 200.0), 4.0);
        assert_eq!(layout.panes[&s(2)], Rect::new(104.0, 0.0, 100.0, 200.0));
    }

    #[test]
    fn close_last_surface_returns_none() {
        let mut tree = SplitTree::leaf(s(0));
        assert_eq!(tree.close(s(0)), None);
    }

    #[test]
    fn close_unfocused_pane_preserves_focus() {
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        // Focus 0, close 1.
        tree.focus(s(0));
        let new_focus = tree.close(s(1));
        assert_eq!(new_focus, Some(s(0)));
        assert_eq!(tree.focused(), s(0));
    }

    #[test]
    fn resize_moves_the_divider_ratio() {
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        // The single split is at the root, path [].
        tree.resize(&[], 20.0, 100.0); // +20px of 100px span → ratio 0.7
        let layout = tree.layout(Rect::new(0.0, 0.0, 100.0, 100.0), 0.0);
        assert!((layout.panes[&s(0)].w - 70.0).abs() < 1e-9);
        assert!((layout.panes[&s(1)].w - 30.0).abs() < 1e-9);
    }

    #[test]
    fn resize_clamps_to_bounds() {
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        tree.resize(&[], 1000.0, 100.0); // way past 0.9
        let layout = tree.layout(Rect::new(0.0, 0.0, 100.0, 100.0), 0.0);
        assert!((layout.panes[&s(0)].w - 90.0).abs() < 1e-9); // clamped to 0.9
    }

    #[test]
    fn divider_path_targets_the_correct_split() {
        // 0 | (1 / 2): root split (path []) is horizontal; the nested split
        // (path [true]) is vertical.
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        tree.split(s(2), Direction::Down);
        let layout = tree.layout(Rect::new(0.0, 0.0, 200.0, 200.0), 4.0);
        // Two dividers: the root vertical strip and the nested horizontal one.
        assert_eq!(layout.dividers.len(), 2);
        let root_div = layout.dividers.iter().find(|d| d.path.is_empty()).unwrap();
        assert_eq!(root_div.axis, Axis::Horizontal);
        let nested_div = layout
            .dividers
            .iter()
            .find(|d| d.path == vec![true])
            .unwrap();
        assert_eq!(nested_div.axis, Axis::Vertical);
    }

    #[test]
    fn hit_test_finds_the_pane_under_a_point() {
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        let layout = tree.layout(Rect::new(0.0, 0.0, 104.0, 100.0), 4.0);
        assert_eq!(tree.hit_test(10.0, 10.0, &layout), Some(s(0)));
        assert_eq!(tree.hit_test(60.0, 10.0, &layout), Some(s(1)));
        // On the divider → no pane.
        assert_eq!(tree.hit_test(52.0, 10.0, &layout), None);
    }

    #[test]
    fn split_rect_reports_the_split_container_geometry() {
        // 0 | (1 / 2): root split (path []) spans the whole container; the
        // nested split (path [true]) spans the right column.
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        tree.split(s(2), Direction::Down);
        let container = Rect::new(0.0, 0.0, 204.0, 200.0);
        let (root_rect, root_axis) = tree.split_rect(&[], container, 4.0).unwrap();
        assert_eq!(root_axis, Axis::Horizontal);
        assert_eq!(root_rect, container);
        let (nested_rect, nested_axis) = tree.split_rect(&[true], container, 4.0).unwrap();
        assert_eq!(nested_axis, Axis::Vertical);
        // The right column: x starts after the left pane (100) + divider (4).
        assert_eq!(nested_rect.x, 104.0);
        assert_eq!(nested_rect.w, 100.0);
        // A path to a leaf (not a split) is None.
        assert!(tree.split_rect(&[false], container, 4.0).is_none());
    }

    #[test]
    fn window_resize_preserves_ratios() {
        let mut tree = SplitTree::leaf(s(0));
        tree.split(s(1), Direction::Right);
        tree.resize(&[], 20.0, 100.0); // ratio 0.7
        // Re-layout at double width; the 70/30 split is preserved.
        let layout = tree.layout(Rect::new(0.0, 0.0, 200.0, 100.0), 0.0);
        assert!((layout.panes[&s(0)].w - 140.0).abs() < 1e-9);
        assert!((layout.panes[&s(1)].w - 60.0).abs() < 1e-9);
    }
}
