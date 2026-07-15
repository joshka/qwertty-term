//! The Cmd+F search bar overlay: a small `NSVisualEffectView` pinned to the
//! top-right of the focused pane's view, holding an `NSTextField` for the needle
//! and a counter label (`"3/17"`).
//!
//! # Structure
//!
//! One [`SearchOverlay`] (`NSVisualEffectView`) per pane that has ever opened
//! search, created lazily and hosted as a subview of the pane's
//! [`TerminalView`](crate::view::TerminalView). Toggling search just shows/hides
//! it (`setHidden:`) and moves first-responder focus in/out of its text field.
//!
//! The overlay conforms to `NSTextFieldDelegate`: `controlTextDidChange:` fires
//! on every keystroke (→ incremental search over the pane's scrollback), and
//! `control:textView:doCommandBySelector:` catches Return (→ next match), Escape
//! (→ close), so those keys drive navigation instead of inserting text. The
//! Cmd+G / Cmd+Shift+G chords are handled by
//! [`crate::view::TerminalView`]'s `performKeyEquivalent:` (key equivalents
//! reach the whole responder chain even while the field is first responder), so
//! they work whether focus is in the field or the terminal.
//!
//! All state flows back to the [`Controller`](crate::app::Controller) via an
//! owned clone (the controller is `Rc`-backed and main-thread-only) plus the
//! `(tab, surface)` this overlay belongs to. The controller owns the actual
//! [`SearchState`](crate::search::SearchState) and the highlight/scroll effects.

#![cfg(target_os = "macos")]

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSControl, NSControlTextEditingDelegate, NSTextField, NSTextFieldDelegate,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use crate::app::Controller;
use crate::splits::SurfaceId;
use crate::tabs::TabId;

/// Overlay dimensions in points.
const OVERLAY_W: f64 = 280.0;
const OVERLAY_H: f64 = 32.0;
const MARGIN: f64 = 8.0;
const COUNTER_W: f64 = 60.0;

/// Interior state for the search overlay.
pub struct SearchOverlayIvars {
    /// Owned controller handle (Rc-backed, main-thread-only).
    controller: Controller,
    /// The pane this overlay searches.
    tab: TabId,
    surface: SurfaceId,
    /// The needle input field.
    field: Retained<NSTextField>,
    /// The "N/M" counter label.
    counter: Retained<NSTextField>,
}

define_class!(
    // SAFETY: NSVisualEffectView subclass implementing NSTextFieldDelegate; no
    // unsafe Drop. All access is main-thread-only (AppKit guarantee).
    #[unsafe(super(NSVisualEffectView))]
    #[name = "QwerttyTermSearchOverlay"]
    #[ivars = SearchOverlayIvars]
    #[thread_kind = MainThreadOnly]
    pub struct SearchOverlay;

    unsafe impl NSObjectProtocol for SearchOverlay {}

    unsafe impl NSTextFieldDelegate for SearchOverlay {}

    unsafe impl NSControlTextEditingDelegate for SearchOverlay {
        /// The needle changed: run an incremental search over the pane's
        /// scrollback. Fires on every keystroke into the field.
        #[unsafe(method(controlTextDidChange:))]
        fn control_text_did_change(&self, _notification: &NSNotification) {
            let needle = self.ivars().field.stringValue().to_string();
            let (tab, surface) = (self.ivars().tab, self.ivars().surface);
            self.ivars()
                .controller
                .search_set_needle(tab, surface, &needle);
        }

        /// Intercept Return / Escape while the field is first responder so they
        /// drive navigation / dismissal instead of inserting a newline or
        /// beeping. Returns `true` to tell AppKit the command was handled.
        ///
        /// Return → next match; Escape (`cancelOperation:`) → close the bar.
        /// Shift+Return would ideally be "previous", but AppKit collapses both
        /// to `insertNewline:`; Cmd+Shift+G covers previous, and Return-for-next
        /// is the common case, so Return always advances forward here.
        #[unsafe(method(control:textView:doCommandBySelector:))]
        fn do_command_by_selector(
            &self,
            _control: &NSControl,
            _text_view: &AnyObject,
            selector: Sel,
        ) -> bool {
            let (tab, surface) = (self.ivars().tab, self.ivars().surface);
            if selector == sel!(insertNewline:) {
                self.ivars().controller.search_navigate_next(tab, surface);
                true
            } else if selector == sel!(cancelOperation:) {
                self.ivars().controller.search_end(tab, surface);
                true
            } else {
                false
            }
        }
    }
);

impl SearchOverlay {
    /// Create a search overlay for `(tab, surface)`, wired to `controller`. The
    /// overlay is created hidden; [`SearchOverlay::open`] shows it and focuses
    /// the field. `pane_bounds` is the pane view's bounds (points) so the
    /// overlay can pin itself top-right.
    pub fn new(
        mtm: MainThreadMarker,
        controller: Controller,
        tab: TabId,
        surface: SurfaceId,
        pane_bounds: NSRect,
    ) -> Retained<Self> {
        let field = make_field(mtm);
        let counter = make_counter(mtm);

        let this = Self::alloc(mtm).set_ivars(SearchOverlayIvars {
            controller,
            tab,
            surface,
            field: field.clone(),
            counter: counter.clone(),
        });
        let frame = overlay_frame(pane_bounds);
        let this: Retained<Self> = unsafe { msg_send![super(this), initWithFrame: frame] };

        // Frosted rounded chrome.
        this.setMaterial(NSVisualEffectMaterial::HUDWindow);
        this.setBlendingMode(NSVisualEffectBlendingMode::WithinWindow);
        this.setState(NSVisualEffectState::Active);
        this.setWantsLayer(true);
        // The backing layer exists after setWantsLayer(true); corner radius is
        // a plain CALayer property.
        if let Some(layer) = this.layer() {
            layer.setCornerRadius(6.0);
        }

        // Lay out the field (left) + counter (right) inside the overlay.
        let (field_frame, counter_frame) = inner_frames();
        field.setFrame(field_frame);
        counter.setFrame(counter_frame);

        // The overlay is the field's delegate (incremental search + key
        // interception).
        let delegate_obj: &AnyObject = this.as_ref();
        // SAFETY: `this` conforms to NSTextFieldDelegate; setDelegate holds a
        // weak ref, and the overlay outlives the field (both dropped together
        // when the pane closes).
        unsafe {
            let _: () = msg_send![&*field, setDelegate: delegate_obj];
        }

        this.addSubview(&field);
        this.addSubview(&counter);
        this.setHidden(true);
        this
    }

    /// Show the overlay and make its field first responder (so typing goes to
    /// the needle). `pane_bounds` re-pins it top-right in case the pane resized.
    pub fn open(&self, pane_bounds: NSRect) {
        self.setFrame(overlay_frame(pane_bounds));
        self.setHidden(false);
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(&self.ivars().field));
        }
    }

    /// Hide the overlay and hand first-responder focus back to the pane's view
    /// (so keystrokes reach the PTY again).
    pub fn close(&self) {
        self.setHidden(true);
        // Return focus to the hosting terminal view (our superview).
        if let (Some(window), Some(sup)) = (self.window(), unsafe { self.superview() }) {
            window.makeFirstResponder(Some(&sup));
        }
    }

    /// Whether the overlay is currently shown.
    pub fn is_open(&self) -> bool {
        !self.isHidden()
    }

    /// The current needle text in the field.
    pub fn needle(&self) -> String {
        self.ivars().field.stringValue().to_string()
    }

    /// Update the "N/M" counter label text.
    pub fn set_counter(&self, text: &str) {
        self.ivars()
            .counter
            .setStringValue(&NSString::from_str(text));
    }

    /// Re-pin the overlay top-right for a new pane bounds (on pane resize).
    pub fn reposition(&self, pane_bounds: NSRect) {
        self.setFrame(overlay_frame(pane_bounds));
    }
}

/// The overlay's frame: pinned to the top-right of the pane, inset by `MARGIN`.
/// The pane view is flipped (top-left origin), so "top" is `y = MARGIN`.
fn overlay_frame(pane_bounds: NSRect) -> NSRect {
    let x = (pane_bounds.size.width - OVERLAY_W - MARGIN).max(MARGIN);
    NSRect::new(NSPoint::new(x, MARGIN), NSSize::new(OVERLAY_W, OVERLAY_H))
}

/// The field (left) and counter (right) frames within the overlay.
fn inner_frames() -> (NSRect, NSRect) {
    let field_w = OVERLAY_W - COUNTER_W - 3.0 * MARGIN;
    let field = NSRect::new(
        NSPoint::new(MARGIN, 5.0),
        NSSize::new(field_w, OVERLAY_H - 10.0),
    );
    let counter = NSRect::new(
        NSPoint::new(MARGIN + field_w + MARGIN, 5.0),
        NSSize::new(COUNTER_W, OVERLAY_H - 10.0),
    );
    (field, counter)
}

/// Build the needle input field.
fn make_field(mtm: MainThreadMarker) -> Retained<NSTextField> {
    let field = NSTextField::new(mtm);
    field.setBezeled(true);
    field.setEditable(true);
    field.setSelectable(true);
    field.setPlaceholderString(Some(&NSString::from_str("Search")));
    field
}

/// Build the "N/M" counter label (a non-editable text field).
fn make_counter(mtm: MainThreadMarker) -> Retained<NSTextField> {
    // A borderless, non-editable label.
    NSTextField::labelWithString(&NSString::from_str(""), mtm)
}
