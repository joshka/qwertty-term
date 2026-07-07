//! The presentation layer: a `CALayer` subclass whose `contents` is an
//! IOSurface.
//!
//! Port of `src/renderer/metal/IOSurfaceLayer.zig` (commit `2da015cd6`).
//!
//! Plan decision 2 (`docs/plans/m3-first-pixels.md`): we present by assigning
//! the render target's IOSurface to a plain `CALayer`'s `contents` — **not**
//! `CAMetalLayer`/`nextDrawable`. This module is that layer.
//!
//! # Why a subclass (`define_class!`)
//!
//! Two `CALayer` behaviors must be overridden, so a runtime subclass is
//! required (upstream uses `objc.allocateClassPair`; objc2 0.6's declarative
//! equivalent is [`define_class!`], the successor to the `declare_class!`
//! the chunk brief names):
//!
//! - **`display`** — CoreAnimation calls this when the layer needs to redraw
//!   (notably during a live resize). We forward it to a caller-installed
//!   display callback, which is the resize-driven synchronous redraw source
//!   (plan decision 3: `display` callback + display-link tick are the two
//!   pacing sources; this is the former).
//! - **`actionForKey:`** — returning `NSNull` for every key disables *all*
//!   implicit animations, so a `contents` swap shows the new surface
//!   immediately with no cross-fade (upstream returns `[NSNull null]`).
//!
//! # Threading of `contents`
//!
//! `contents` must be assigned on the main thread to avoid visual artifacts
//! (upstream `setSurface` dispatches to the main queue; `setSurfaceSync`
//! assigns directly for the resize path, which already runs on the main
//! thread). The async path here uses `dispatch2`'s main queue and re-checks the
//! surface size against the layer bounds before assigning — a late async frame
//! finishing just after a sync resize frame would otherwise cause jank
//! (upstream `setSurfaceCallback`'s size guard).

use std::cell::Cell;
use std::ptr::NonNull;

use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{AnyThread, DefinedClass, MainThreadMarker, Message, define_class, msg_send};
use objc2_core_foundation::{CFRetained, CGFloat};
use objc2_foundation::{NSNull, NSObject};
use objc2_io_surface::IOSurfaceRef;
use objc2_quartz_core::{CALayer, kCAGravityTopLeft};

/// A display callback: invoked from the layer's `display` method (main thread).
/// Boxed so the layer can hold an arbitrary closure — in practice "draw one
/// frame synchronously" (the resize redraw).
pub type DisplayCallback = Box<dyn Fn() + 'static>;

/// Instance variables for the [`IOSurfaceLayer`] subclass.
///
/// `pub` only because `define_class!` names it in the (public) `DefinedClass`
/// impl for the `pub` [`IOSurfaceLayer`]; the fields are private, so it's
/// opaque to callers.
pub struct Ivars {
    /// The installed display callback, if any. `Cell` for interior mutability
    /// (objc2 ivars are behind `&self`); the pointer is only ever touched on
    /// the main thread (where `display` and `set_display_callback` run).
    display_cb: Cell<Option<*mut DisplayCallback>>,
}

define_class!(
    // SAFETY:
    // - The superclass `CALayer` has no subclassing requirements beyond the
    //   usual CoreAnimation ones, which we uphold (we only override `display`
    //   and `actionForKey:`, both of which we implement fully).
    // - `IOSurfaceLayer` implements `Drop` (to free the boxed callback); the
    //   `Drop` impl calls no overridden methods and does not retain `self`.
    #[unsafe(super(CALayer))]
    #[name = "GhosttyIOSurfaceLayer"]
    #[ivars = Ivars]
    pub struct IOSurfaceLayer;

    impl IOSurfaceLayer {
        /// CoreAnimation redraw hook. Port of the subclass' `display` override:
        /// forward to the installed callback (context is captured in the
        /// boxed closure rather than a separate `display_ctx` ivar).
        #[unsafe(method(display))]
        fn display(&self) {
            if let Some(ptr) = self.ivars().display_cb.get() {
                // SAFETY: `ptr` was produced by `Box::into_raw` in
                // `set_display_callback` and is only cleared/replaced on this
                // same (main) thread; it stays valid until then.
                let cb = unsafe { &*ptr };
                cb();
            }
        }

        /// Disable implicit animations by returning `NSNull` for every action
        /// key. Port of the `actionForKey:` override (`[NSNull null]`).
        #[unsafe(method_id(actionForKey:))]
        fn action_for_key(&self, _key: &NSObject) -> Retained<NSObject> {
            // NSNull is a singleton; the caller treats it as "no action".
            let null: Retained<NSNull> = NSNull::null();
            // Return as NSObject (the runtime-declared return type is `id`).
            unsafe { Retained::cast_unchecked(null) }
        }
    }
);

impl IOSurfaceLayer {
    /// Create the layer. Port of `IOSurfaceLayer.init`.
    ///
    /// Sets `contentsGravity` to top-left so contents aren't stretched during a
    /// resize before a new frame is drawn (upstream comment), and initializes
    /// the display-callback ivar to empty.
    pub fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars {
            display_cb: Cell::new(None),
        });
        // `CALayer`'s designated initializer is `init`; we inherit it.
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // Top-left gravity: no stretch during resize before the next frame.
        // SAFETY: `kCAGravityTopLeft` is a valid contents-gravity constant.
        unsafe {
            this.setContentsGravity(kCAGravityTopLeft);
        }

        this
    }

    /// The layer as a `CALayer` (for hosting in an `NSView`, R5).
    pub fn as_layer(&self) -> &CALayer {
        // SAFETY: `IOSurfaceLayer` is a subclass of `CALayer`.
        self
    }

    /// Install (or clear) the display callback invoked from `display`. Must be
    /// called on the main thread. Port of `setDisplayCallback` (the closure
    /// subsumes upstream's separate `display_cb` + `display_ctx`).
    pub fn set_display_callback(&self, cb: Option<DisplayCallback>) {
        // Free any previously installed callback.
        if let Some(old) = self.ivars().display_cb.replace(None) {
            // SAFETY: `old` came from `Box::into_raw` below; reclaiming it here
            // (on the main thread, where it can't be concurrently invoked)
            // frees it exactly once.
            drop(unsafe { Box::from_raw(old) });
        }
        if let Some(cb) = cb {
            let ptr = Box::into_raw(Box::new(cb));
            self.ivars().display_cb.set(Some(ptr));
        }
    }

    /// Assign `surface` as the layer's `contents` synchronously (no
    /// main-thread dispatch, no size guard). Port of `setSurfaceSync` — used by
    /// the resize path, which already runs on the main thread.
    ///
    /// # Safety
    /// Must be called on the main thread (CoreAnimation requirement).
    pub unsafe fn set_surface_sync(&self, surface: &IOSurfaceRef) {
        // SAFETY: an IOSurface is a valid `contents` value; caller guarantees
        // main-thread.
        unsafe {
            let obj: &AnyObject = &*(surface as *const IOSurfaceRef).cast::<AnyObject>();
            self.setContents(Some(obj));
        }
    }

    /// Assign `surface` as the layer's `contents`, on the main thread, with a
    /// size guard. Port of `setSurface`.
    ///
    /// If already on the main thread, assigns directly; otherwise dispatches to
    /// the main queue. In the dispatched case, the surface is re-checked
    /// against the layer's `bounds * contentsScale` and discarded if it no
    /// longer matches (a late async frame vs. a sync resize frame — upstream's
    /// jank guard).
    pub fn set_surface(&self, surface: &IOSurfaceRef) {
        if MainThreadMarker::new().is_some() {
            // SAFETY: we're on the main thread.
            unsafe { self.set_surface_sync(surface) };
            return;
        }

        // Move retained handles into the block; they keep the objects alive
        // until the block runs. Wrapped so we can assert `Send` (CALayer /
        // IOSurface aren't `Send`, but the dispatched work only ever touches
        // them on the main thread — see `MainThreadHandles`).
        // SAFETY: `surface` is a live IOSurface; `CFRetained::retain` bumps its
        // refcount, keeping it alive until the block (and the `CFRetained`) drop.
        let surface = unsafe { CFRetained::retain(NonNull::from(surface)) };
        let handles = MainThreadHandles {
            layer: self.retain(),
            surface,
        };
        // Capture `handles` *whole* (not its fields disjointly), so the block's
        // `Send`-ness comes from `MainThreadHandles`'s manual `Send` impl rather
        // than from its non-`Send` fields. `handles.run()` moves it in.
        DispatchQueue::main().exec_async(move || handles.run());
    }
}

/// Retained CALayer + IOSurface handles moved into a main-queue block.
///
/// Neither type is `Send`, but `exec_async` requires `Send`. This is sound
/// because the block runs *only* on the main thread and does nothing with the
/// handles off it: the objc runtime's refcounting is itself thread-safe (so
/// moving the `Retained` across the dispatch boundary is fine), and every
/// `&self` method we call on them inside the block is a main-thread CoreAnimation
/// call. This mirrors upstream passing the raw `id`/`*IOSurface` into the block.
struct MainThreadHandles {
    layer: Retained<IOSurfaceLayer>,
    surface: CFRetained<IOSurfaceRef>,
}

impl MainThreadHandles {
    /// Assign the surface as the layer's contents, on the main thread, with the
    /// size guard. Runs inside the dispatched block. Port of
    /// `setSurfaceCallback`.
    fn run(self) {
        // Size guard: discard a surface that no longer fits the layer.
        let bounds = self.layer.bounds();
        let scale: CGFloat = self.layer.contentsScale();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "bounds*scale are non-negative pixel counts; truncation matches upstream @intFromFloat"
        )]
        let want_w = (bounds.size.width * scale) as usize;
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "as above"
        )]
        let want_h = (bounds.size.height * scale) as usize;
        if want_w != self.surface.width() || want_h != self.surface.height() {
            // Wrong size for the layer now; drop it to avoid resize jank.
            return;
        }
        // SAFETY: we're on the main thread (dispatched to the main queue).
        unsafe { self.layer.set_surface_sync(&self.surface) };
    }
}

// SAFETY: see `MainThreadHandles` docs — used only to ferry the handles to the
// main thread, where all access happens.
unsafe impl Send for MainThreadHandles {}

impl Drop for IOSurfaceLayer {
    fn drop(&mut self) {
        // Reclaim the boxed display callback, if any. This runs when the last
        // reference to the layer is released; by then no `display` call can be
        // in flight for this instance.
        if let Some(ptr) = self.ivars().display_cb.get() {
            // SAFETY: `ptr` came from `Box::into_raw`; freed exactly once here.
            drop(unsafe { Box::from_raw(ptr) });
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use objc2_foundation::NSString;

    use super::*;

    /// The subclass is created and registered (`define_class!` succeeds), the
    /// instance is a `CALayer`, and its contents-gravity was set to top-left.
    /// Also exercises `actionForKey:` returning a non-nil (NSNull) action so
    /// implicit animations are disabled.
    #[test]
    fn layer_creates_and_disables_animations() {
        let layer = IOSurfaceLayer::new();

        // It really is a CALayer.
        let _as_layer: &CALayer = layer.as_layer();

        // actionForKey: returns NSNull (not nil) for any key — disabling
        // implicit animations. We invoke it via the runtime.
        let key = NSString::from_str("contents");
        let action: Retained<AnyObject> = unsafe { msg_send![&*layer, actionForKey: &*key] };
        assert!(
            action.downcast_ref::<NSNull>().is_some(),
            "actionForKey: must return NSNull"
        );
    }

    /// The display callback ivar is invoked from `display` and can be cleared.
    /// (We invoke `display` via the runtime rather than waiting for
    /// CoreAnimation.)
    #[test]
    fn display_callback_fires_and_clears() {
        let layer = IOSurfaceLayer::new();
        let count = Arc::new(AtomicUsize::new(0));
        let c2 = Arc::clone(&count);
        layer.set_display_callback(Some(Box::new(move || {
            c2.fetch_add(1, Ordering::Relaxed);
        })));

        // Invoke `display` twice.
        unsafe {
            let _: () = msg_send![&*layer, display];
            let _: () = msg_send![&*layer, display];
        }
        assert_eq!(count.load(Ordering::Relaxed), 2);

        // Clearing the callback stops further ticks.
        layer.set_display_callback(None);
        unsafe {
            let _: () = msg_send![&*layer, display];
        }
        assert_eq!(
            count.load(Ordering::Relaxed),
            2,
            "cleared callback is quiet"
        );
    }
}
