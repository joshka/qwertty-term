//! The AppKit split container and divider views.
//!
//! # Layout mechanism: hand-rolled, not `NSSplitView`
//!
//! Slice 1 lays panes out with a plain flipped container [`SplitContainer`]
//! (`NSView`) that holds each pane's [`TerminalView`](crate::view::TerminalView)
//! as a subview at an explicit frame, plus a thin [`SplitDivider`] (`NSView`)
//! between adjacent panes. The frames come from the pure
//! [`SplitTree::layout`](crate::splits::SplitTree::layout); the controller sets
//! them.
//!
//! We deliberately do **not** use `NSSplitView`. Each pane is a layer-backed
//! `NSView` whose backing layer is a renderer
//! [`IOSurfaceLayer`](qwertty_term_renderer::metal::IOSurfaceLayer) with a bespoke
//! `contentsScale` + `contentsGravity` (`pin_surface_to_top`) already tuned in
//! R5 to kill the sub-cell "dark band". `NSSplitView` wants to own its
//! subviews' frames and inserts its own divider chrome, which fights that
//! per-layer geometry and the flipped top-left coordinate space the mouse-report
//! pixel math depends on. A hand-rolled container keeps the exact geometry
//! contract the single-pane path already relies on and makes the divider a plain
//! draggable strip we fully control — the same call into
//! [`SplitTree::layout`](crate::splits::SplitTree::layout) drives both the
//! single-pane (byte-identical to before) and multi-pane cases.
//!
//! The divider reports drag deltas back to the controller, which mutates the
//! tree's ratio and re-lays-out (recomputing both adjacent panes' pixel rects →
//! per-surface engine + PTY resize).

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSBezierPath, NSColor, NSCursor, NSEvent, NSView};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect};

use crate::app::Controller;
use crate::splits::Axis;
use crate::tabs::TabId;

/// A plain flipped container that hosts the tab's pane views + dividers. Flipped
/// so its coordinate origin is top-left, matching each pane view's flipped space
/// and the tree layout's top-left rects.
pub struct ContainerIvars {
    /// Back-reference to the shared controller (main-thread-only access), used
    /// to trigger a relayout when AppKit resizes this container.
    controller: *const Controller,
    /// The tab this container lays out.
    tab: TabId,
}

define_class!(
    // SAFETY: NSView subclass; overrides `isFlipped` + `setFrameSize:`. No
    // unsafe Drop.
    #[unsafe(super(NSView))]
    #[name = "QwerttyTermSplitContainer"]
    #[ivars = ContainerIvars]
    #[thread_kind = MainThreadOnly]
    pub struct SplitContainer;

    impl SplitContainer {
        /// Top-left origin, matching the pane views and the tree layout rects.
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// AppKit resized the container (it is the window's content view, so
        /// this fires on window resize AND on content-area changes that never
        /// fire `windowDidResize:` — most importantly the native tab bar
        /// appearing/disappearing, which shrinks/grows the content area in
        /// place). Re-lay-out the panes to the new size; without this the pane
        /// frames go stale across the tab-bar transition and the bottom rows
        /// are clipped. (The pre-splits code was immune because the terminal
        /// view itself was the content view and followed automatically.)
        #[unsafe(method(setFrameSize:))]
        fn set_frame_size(&self, size: objc2_foundation::NSSize) {
            // SAFETY: standard super call for an overridden AppKit method.
            unsafe {
                let _: () = msg_send![super(self), setFrameSize: size];
            }
            let ivars = self.ivars();
            let tab = ivars.tab;
            if !ivars.controller.is_null() {
                // SAFETY: main-thread-only; the controller outlives the views.
                let controller = unsafe { &*ivars.controller };
                controller.container_resized(tab);
            }
        }
    }
);

impl SplitContainer {
    /// Create an empty container for `tab` with the given frame.
    pub fn new(
        mtm: MainThreadMarker,
        controller: *const Controller,
        tab: TabId,
        frame: NSRect,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ContainerIvars { controller, tab });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this
    }
}

/// Ivars for a draggable divider: the controller, the tab it belongs to, the
/// tree path of the split it controls, and its axis (which drag axis to honour).
pub struct DividerIvars {
    controller: *const Controller,
    tab: TabId,
    /// The path (`false`=first / `true`=second child at each level) from the
    /// tree root to the split this divider resizes.
    path: Vec<bool>,
    /// The split's axis: `Horizontal` → this is a vertical strip dragged left/
    /// right; `Vertical` → a horizontal strip dragged up/down.
    axis: Axis,
}

define_class!(
    // SAFETY: NSView subclass; overrides mouse handling + cursor. No unsafe Drop.
    #[unsafe(super(NSView))]
    #[name = "QwerttyTermSplitDivider"]
    #[ivars = DividerIvars]
    #[thread_kind = MainThreadOnly]
    pub struct SplitDivider;

    impl SplitDivider {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        /// Paint the divider a subtle chrome colour so the seam is visible.
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _dirty: NSRect) {
            let bounds = self.bounds();
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(0.20, 0.20, 0.20, 1.0);
            color.set();
            let path = NSBezierPath::bezierPathWithRect(bounds);
            path.fill();
        }

        /// A live drag: translate the pointer's motion along the divider's axis
        /// into a ratio change on the controlled split, then re-lay-out. We use
        /// the pointer's position in the *container* (superview) space and hand
        /// the controller an absolute coordinate; it converts to a ratio against
        /// the split's own span.
        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            let win_pt = event.locationInWindow();
            // Convert to the superview (container) coordinate space.
            let Some(container) = (unsafe { self.superview() }) else {
                return;
            };
            let pt = container.convertPoint_fromView(win_pt, None);
            let ivars = self.ivars();
            let coord: f64 = match ivars.axis {
                Axis::Horizontal => pt.x,
                Axis::Vertical => pt.y,
            };
            let (tab, path) = (ivars.tab, ivars.path.clone());
            self.with_controller(|c| c.drag_divider(tab, &path, coord));
        }

        /// Show a resize cursor while hovering the divider.
        #[unsafe(method(resetCursorRects))]
        fn reset_cursor_rects(&self) {
            let bounds = self.bounds();
            let cursor = match self.ivars().axis {
                // A horizontal split has a vertical divider → left/right resize.
                #[allow(deprecated)]
                Axis::Horizontal => NSCursor::resizeLeftRightCursor(),
                #[allow(deprecated)]
                Axis::Vertical => NSCursor::resizeUpDownCursor(),
            };
            self.addCursorRect_cursor(bounds, &cursor);
        }
    }
);

impl SplitDivider {
    /// Create a divider bound to a split path within `tab`, at the given frame.
    pub fn new(
        mtm: MainThreadMarker,
        controller: *const Controller,
        tab: TabId,
        path: Vec<bool>,
        axis: Axis,
        frame: NSRect,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DividerIvars {
            controller,
            tab,
            path,
            axis,
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };
        this
    }

    /// Run `f` with the controller if the back-pointer is set.
    fn with_controller<R>(&self, f: impl FnOnce(&Controller) -> R) -> Option<R> {
        let ptr = self.ivars().controller;
        if ptr.is_null() {
            return None;
        }
        // SAFETY: main-thread-only; the controller outlives the view.
        Some(f(unsafe { &*ptr }))
    }
}

/// Convert a [`crate::splits::Rect`] (top-left origin, tree/pixel space) into an
/// `NSRect` in the flipped container's point space. Since both the container and
/// the tree layout use a top-left origin, this is a straight field copy after
/// dividing device pixels back to points by `scale`.
pub fn ns_rect_from_tree(rect: crate::splits::Rect, scale: f64) -> NSRect {
    let s = if scale > 0.0 { scale } else { 1.0 };
    NSRect::new(
        NSPoint::new(rect.x / s, rect.y / s),
        objc2_foundation::NSSize::new(rect.w / s, rect.h / s),
    )
}
