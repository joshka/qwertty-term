//! Unit tests for the tmux layout → `SplitTree` converter and the window → tab
//! reconciler (ADR 006 slice 5b). Fully headless: builds `Layout` trees from the
//! same layout strings the engine's `layout` parser tests use, converts them,
//! and asserts the resulting split structure, ratios, and reconcile diff across
//! a notification-like sequence of window sets.

use super::*;
use qwertty_term_vt::tmux::layout::Layout;

use crate::splits::{Axis, Node, Rect};

// ---- helpers ---------------------------------------------------------------

/// Parse a bare (checksum-less) layout string into a `Layout` tree.
fn layout(s: &str) -> Layout {
    Layout::parse(s.as_bytes()).expect("valid layout string")
}

/// Build a window with the given id from a bare layout string.
fn window(id: usize, s: &str) -> Window {
    let l = layout(s);
    Window {
        id,
        width: l.width,
        height: l.height,
        layout: l,
    }
}

/// Identity surface mapping: pane id `n` → `SurfaceId(n)`. Lets the converter
/// tests assert against pane ids directly.
fn ident(id: usize) -> SurfaceId {
    SurfaceId(id as u64)
}

fn sid(n: u64) -> SurfaceId {
    SurfaceId(n)
}

// ---- converter: leaf + 2-way splits ----------------------------------------

#[test]
fn single_pane_is_a_leaf() {
    // "80x24,0,0,42": one pane, id 42.
    let tree = layout_to_split_tree(&layout("80x24,0,0,42"), ident);
    assert_eq!(tree.surfaces(), vec![sid(42)]);
    assert_eq!(tree.focused(), sid(42));
    assert!(matches!(tree.root(), Node::Leaf(id) if *id == sid(42)));
}

#[test]
fn horizontal_split_maps_to_axis_horizontal() {
    // "{…}" is side-by-side → Axis::Horizontal. Two equal 40-wide panes → 0.5.
    let tree = layout_to_split_tree(&layout("80x24,0,0{40x24,0,0,1,40x24,40,0,2}"), ident);
    assert_eq!(tree.surfaces(), vec![sid(1), sid(2)]);
    let Node::Split(s) = tree.root() else {
        panic!("expected a split root");
    };
    assert_eq!(s.axis, Axis::Horizontal);
    assert!((s.ratio - 0.5).abs() < 1e-9);
    assert!(matches!(&s.first, Node::Leaf(id) if *id == sid(1)));
    assert!(matches!(&s.second, Node::Leaf(id) if *id == sid(2)));
}

#[test]
fn vertical_split_maps_to_axis_vertical() {
    // "[…]" is stacked → Axis::Vertical. Two equal 12-tall panes → 0.5.
    let tree = layout_to_split_tree(&layout("80x24,0,0[80x12,0,0,1,80x12,0,12,2]"), ident);
    assert_eq!(tree.surfaces(), vec![sid(1), sid(2)]);
    let Node::Split(s) = tree.root() else {
        panic!("expected a split root");
    };
    assert_eq!(s.axis, Axis::Vertical);
    assert!((s.ratio - 0.5).abs() < 1e-9);
}

#[test]
fn horizontal_ratio_is_width_weighted() {
    // Left pane 20 wide, right pane 60 wide (of 80) → ratio 20/80 = 0.25.
    let tree = layout_to_split_tree(&layout("80x24,0,0{20x24,0,0,1,60x24,20,0,2}"), ident);
    let Node::Split(s) = tree.root() else {
        panic!("expected a split root");
    };
    assert_eq!(s.axis, Axis::Horizontal);
    assert!((s.ratio - 0.25).abs() < 1e-9);
    // Laid out in a 80-wide container (no divider): 20 / 60.
    let lay = tree.layout(Rect::new(0.0, 0.0, 80.0, 24.0), 0.0);
    assert!((lay.panes[&sid(1)].w - 20.0).abs() < 1e-9);
    assert!((lay.panes[&sid(2)].w - 60.0).abs() < 1e-9);
}

// ---- converter: n-ary → right-leaning binary chain -------------------------

#[test]
fn three_way_horizontal_is_right_leaning_chain() {
    // "120x24,0,0{40x24,0,0,1,40x24,40,0,2,40x24,80,0,3}": three equal columns.
    let tree = layout_to_split_tree(
        &layout("120x24,0,0{40x24,0,0,1,40x24,40,0,2,40x24,80,0,3}"),
        ident,
    );
    assert_eq!(tree.surfaces(), vec![sid(1), sid(2), sid(3)]);

    // Root: first = pane 1, ratio = 40/120 = 1/3, second = a nested split.
    let Node::Split(root) = tree.root() else {
        panic!("expected split root");
    };
    assert_eq!(root.axis, Axis::Horizontal);
    assert!((root.ratio - 1.0 / 3.0).abs() < 1e-9);
    assert!(matches!(&root.first, Node::Leaf(id) if *id == sid(1)));

    // Nested: first = pane 2, ratio = 40/80 = 0.5, second = pane 3.
    let Node::Split(nested) = &root.second else {
        panic!("expected nested split");
    };
    assert_eq!(nested.axis, Axis::Horizontal);
    assert!((nested.ratio - 0.5).abs() < 1e-9);
    assert!(matches!(&nested.first, Node::Leaf(id) if *id == sid(2)));
    assert!(matches!(&nested.second, Node::Leaf(id) if *id == sid(3)));

    // Geometrically: three equal thirds of 120.
    let lay = tree.layout(Rect::new(0.0, 0.0, 120.0, 24.0), 0.0);
    assert!((lay.panes[&sid(1)].w - 40.0).abs() < 1e-6);
    assert!((lay.panes[&sid(2)].w - 40.0).abs() < 1e-6);
    assert!((lay.panes[&sid(3)].w - 40.0).abs() < 1e-6);
}

// ---- converter: nested mixed-axis layouts ----------------------------------

#[test]
fn nested_vertical_inside_horizontal() {
    // "80x24,0,0{40x24,0,0,1,40x24,40,0[40x12,40,0,2,40x12,40,12,3]}":
    // left column = pane 1; right column = panes 2/3 stacked.
    let tree = layout_to_split_tree(
        &layout("80x24,0,0{40x24,0,0,1,40x24,40,0[40x12,40,0,2,40x12,40,12,3]}"),
        ident,
    );
    assert_eq!(tree.surfaces(), vec![sid(1), sid(2), sid(3)]);

    let Node::Split(root) = tree.root() else {
        panic!("expected split root");
    };
    assert_eq!(root.axis, Axis::Horizontal);
    assert!((root.ratio - 0.5).abs() < 1e-9); // 40/80
    assert!(matches!(&root.first, Node::Leaf(id) if *id == sid(1)));
    let Node::Split(right) = &root.second else {
        panic!("expected nested vertical split");
    };
    assert_eq!(right.axis, Axis::Vertical);
    assert!((right.ratio - 0.5).abs() < 1e-9); // 12/24

    // Geometry: left half full-height, right half split top/bottom.
    let lay = tree.layout(Rect::new(0.0, 0.0, 80.0, 24.0), 0.0);
    assert!((lay.panes[&sid(1)].w - 40.0).abs() < 1e-6);
    assert!((lay.panes[&sid(1)].h - 24.0).abs() < 1e-6);
    assert!((lay.panes[&sid(2)].h - 12.0).abs() < 1e-6);
    assert!((lay.panes[&sid(3)].h - 12.0).abs() < 1e-6);
}

#[test]
fn nested_horizontal_inside_vertical() {
    // "80x24,0,0[80x12,0,0,1,80x12,0,12{40x12,0,12,2,40x12,40,12,3}]":
    // top row = pane 1; bottom row = panes 2|3 side-by-side.
    let tree = layout_to_split_tree(
        &layout("80x24,0,0[80x12,0,0,1,80x12,0,12{40x12,0,12,2,40x12,40,12,3}]"),
        ident,
    );
    let Node::Split(root) = tree.root() else {
        panic!("expected split root");
    };
    assert_eq!(root.axis, Axis::Vertical);
    let Node::Split(bottom) = &root.second else {
        panic!("expected nested horizontal split");
    };
    assert_eq!(bottom.axis, Axis::Horizontal);
    assert_eq!(tree.surfaces(), vec![sid(1), sid(2), sid(3)]);
}

// ---- converter: ratio clamping ---------------------------------------------

#[test]
fn extreme_ratio_is_clamped() {
    // A 2-wide pane beside a 200-wide pane → 2/202 ≈ 0.0099, clamped to 0.1.
    let tree = layout_to_split_tree(&layout("202x24,0,0{2x24,0,0,1,200x24,2,0,2}"), ident);
    let Node::Split(s) = tree.root() else {
        panic!("expected split root");
    };
    assert!((s.ratio - 0.1).abs() < 1e-9); // clamped up to MIN_RATIO
}

#[test]
fn surface_mapping_is_applied() {
    // A non-identity map: pane 1 → surface 100, pane 2 → surface 200.
    let map = |id: usize| SurfaceId((id as u64) * 100);
    let tree = layout_to_split_tree(&layout("80x24,0,0{40x24,0,0,1,40x24,40,0,2}"), map);
    assert_eq!(tree.surfaces(), vec![sid(100), sid(200)]);
    assert_eq!(tree.focused(), sid(100)); // leftmost leaf
}

// ---- reconciler: create / grow / drop --------------------------------------

fn split_tree_for(plan: &ReconcilePlan, window_id: usize) -> &SplitTree {
    plan.ops
        .iter()
        .find_map(|op| match op {
            ReconcileOp::SetSplitTree { window_id: w, tree } if *w == window_id => Some(tree),
            _ => None,
        })
        .expect("a SetSplitTree op for the window")
}

#[test]
fn first_reconcile_creates_a_tab_per_window() {
    let mut r = Reconciler::new();
    let windows = vec![window(1, "80x24,0,0,10"), window(2, "80x24,0,0,20")];
    let plan = r.reconcile(&windows);

    // Two CreateTabs (windows 1, 2), no removals, and a tree for each.
    assert!(plan.ops.contains(&ReconcileOp::CreateTab { window_id: 1 }));
    assert!(plan.ops.contains(&ReconcileOp::CreateTab { window_id: 2 }));
    assert!(
        !plan
            .ops
            .iter()
            .any(|o| matches!(o, ReconcileOp::RemoveTab { .. }))
    );
    assert!(plan.dropped_surfaces.is_empty());
    assert_eq!(r.tabs(), &[1, 2]);

    // Each window's tree is a single mapped leaf.
    let s10 = r.surface_of(10).unwrap();
    let s20 = r.surface_of(20).unwrap();
    assert_eq!(split_tree_for(&plan, 1).surfaces(), vec![s10]);
    assert_eq!(split_tree_for(&plan, 2).surfaces(), vec![s20]);
}

#[test]
fn layout_change_reuses_surviving_surface_and_creates_new() {
    let mut r = Reconciler::new();
    // Window 1 starts with one pane (id 10).
    let p1 = r.reconcile(&[window(1, "80x24,0,0,10")]);
    let s10 = r.surface_of(10).unwrap();
    assert_eq!(split_tree_for(&p1, 1).surfaces(), vec![s10]);

    // A %layout-change splits it into panes 10 | 11.
    let p2 = r.reconcile(&[window(1, "80x24,0,0{40x24,0,0,10,40x24,40,0,11}")]);
    // No new/removed tabs; just a replaced split tree.
    assert!(!p2.ops.iter().any(|o| matches!(
        o,
        ReconcileOp::CreateTab { .. } | ReconcileOp::RemoveTab { .. }
    )));
    // Surviving pane 10 keeps its surface; pane 11 gets a fresh one.
    assert_eq!(r.surface_of(10).unwrap(), s10, "surviving pane reused");
    let s11 = r.surface_of(11).unwrap();
    assert_ne!(s11, s10);
    let tree = split_tree_for(&p2, 1);
    assert_eq!(tree.surfaces(), vec![s10, s11]);
    assert!(p2.dropped_surfaces.is_empty());
}

#[test]
fn closing_a_pane_drops_its_surface() {
    let mut r = Reconciler::new();
    // Two panes 10 | 11.
    r.reconcile(&[window(1, "80x24,0,0{40x24,0,0,10,40x24,40,0,11}")]);
    let s10 = r.surface_of(10).unwrap();
    let s11 = r.surface_of(11).unwrap();

    // Pane 11 closes → window collapses to a single pane 10.
    let plan = r.reconcile(&[window(1, "80x24,0,0,10")]);
    assert_eq!(plan.dropped_surfaces, vec![s11]);
    assert_eq!(r.surface_of(10).unwrap(), s10, "surviving pane reused");
    assert!(r.surface_of(11).is_none(), "closed pane surface freed");
    assert_eq!(split_tree_for(&plan, 1).surfaces(), vec![s10]);
}

#[test]
fn removing_a_window_removes_its_tab_and_drops_surfaces() {
    let mut r = Reconciler::new();
    r.reconcile(&[window(1, "80x24,0,0,10"), window(2, "80x24,0,0,20")]);
    let s20 = r.surface_of(20).unwrap();

    // Window 2 closes.
    let plan = r.reconcile(&[window(1, "80x24,0,0,10")]);
    assert!(plan.ops.contains(&ReconcileOp::RemoveTab { window_id: 2 }));
    assert_eq!(r.tabs(), &[1]);
    assert_eq!(plan.dropped_surfaces, vec![s20]);
    assert!(r.surface_of(20).is_none());
}

#[test]
fn adding_a_window_creates_a_tab_and_keeps_the_others() {
    let mut r = Reconciler::new();
    r.reconcile(&[window(1, "80x24,0,0,10")]);
    let s10 = r.surface_of(10).unwrap();

    // A new window 2 appears.
    let plan = r.reconcile(&[window(1, "80x24,0,0,10"), window(2, "80x24,0,0,20")]);
    assert!(plan.ops.contains(&ReconcileOp::CreateTab { window_id: 2 }));
    assert!(
        !plan
            .ops
            .iter()
            .any(|o| matches!(o, ReconcileOp::RemoveTab { .. }))
    );
    assert_eq!(
        r.surface_of(10).unwrap(),
        s10,
        "existing window's pane reused"
    );
    assert_eq!(r.tabs(), &[1, 2]);
}

#[test]
fn full_notification_sequence() {
    // Drive a whole session: open, split, add window, close pane, close window.
    let mut r = Reconciler::new();

    // 1. Session opens with one window, one pane.
    r.reconcile(&[window(1, "80x24,0,0,10")]);
    assert_eq!(r.tabs(), &[1]);

    // 2. Window 1 splits horizontally into 10 | 11.
    let p = r.reconcile(&[window(1, "80x24,0,0{40x24,0,0,10,40x24,40,0,11}")]);
    let s10 = r.surface_of(10).unwrap();
    let s11 = r.surface_of(11).unwrap();
    assert_eq!(split_tree_for(&p, 1).surfaces(), vec![s10, s11]);

    // 3. A second window opens with pane 20.
    let p = r.reconcile(&[
        window(1, "80x24,0,0{40x24,0,0,10,40x24,40,0,11}"),
        window(2, "80x24,0,0,20"),
    ]);
    assert!(p.ops.contains(&ReconcileOp::CreateTab { window_id: 2 }));
    assert_eq!(r.tabs(), &[1, 2]);
    let s20 = r.surface_of(20).unwrap();

    // 4. Pane 11 closes in window 1; window 2 unchanged.
    let p = r.reconcile(&[window(1, "80x24,0,0,10"), window(2, "80x24,0,0,20")]);
    assert_eq!(p.dropped_surfaces, vec![s11]);
    assert_eq!(split_tree_for(&p, 1).surfaces(), vec![s10]);
    assert_eq!(split_tree_for(&p, 2).surfaces(), vec![s20]);

    // 5. Window 1 closes; only window 2 remains.
    let p = r.reconcile(&[window(2, "80x24,0,0,20")]);
    assert!(p.ops.contains(&ReconcileOp::RemoveTab { window_id: 1 }));
    assert_eq!(p.dropped_surfaces, vec![s10]);
    assert_eq!(r.tabs(), &[2]);
    assert_eq!(
        r.surface_of(20).unwrap(),
        s20,
        "untouched window's pane reused"
    );
}

#[test]
fn op_ordering_is_removals_creations_then_trees() {
    let mut r = Reconciler::new();
    r.reconcile(&[window(1, "80x24,0,0,10"), window(2, "80x24,0,0,20")]);
    // Drop window 1, add window 3.
    let plan = r.reconcile(&[window(2, "80x24,0,0,20"), window(3, "80x24,0,0,30")]);

    // First op is the removal, then the creation, then SetSplitTrees.
    let kinds: Vec<&str> = plan
        .ops
        .iter()
        .map(|o| match o {
            ReconcileOp::RemoveTab { .. } => "remove",
            ReconcileOp::CreateTab { .. } => "create",
            ReconcileOp::SetSplitTree { .. } => "set",
        })
        .collect();
    assert_eq!(kinds, vec!["remove", "create", "set", "set"]);
}
