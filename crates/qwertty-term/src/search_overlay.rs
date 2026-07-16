//! The Cmd+F search bar overlay: a small `NSVisualEffectView` pinned to the
//! top-right of the focused pane's view, holding an `NSTextField` for the
//! needle, a counter label (`"3/17"`), and — matching upstream Ghostty's search
//! bar — previous (↑), next (↓), and close (✕) buttons wired to the pane's
//! search navigation / dismissal.
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
//! Otherwise the field is a plain `NSTextField`, so it behaves like a standard
//! macOS text box while focused: its field editor handles caret/word/line
//! motion, selection, and the user's system Ctrl-emacs key bindings natively.
//! `TerminalView`'s `performKeyEquivalent:` detects that the field editor holds
//! focus and, instead of letting the terminal's keybind chords steal them,
//! forwards the standard editing chords (Cmd+A/C/X/V/Z) to the field editor —
//! so copy/paste/select-all act on the search text, not the terminal.
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
    NSButton, NSControl, NSControlTextEditingDelegate, NSImage, NSTextField, NSTextFieldDelegate,
    NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use crate::app::Controller;
use crate::splits::SurfaceId;
use crate::tabs::TabId;

/// Overlay dimensions in points.
const OVERLAY_W: f64 = 320.0;
const OVERLAY_H: f64 = 32.0;
const MARGIN: f64 = 8.0;
const COUNTER_W: f64 = 44.0;
/// Width of each nav/close button (square-ish).
const BTN_W: f64 = 24.0;
/// Small gap between the field, counter, and buttons.
const GAP: f64 = 4.0;

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
    /// Previous-match (↑), next-match (↓), and close (✕) buttons.
    prev: Retained<NSButton>,
    next: Retained<NSButton>,
    close: Retained<NSButton>,
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

    impl SearchOverlay {
        /// Previous-match button (the up chevron): step to the previous match,
        /// keeping keyboard focus in the field so typing continues.
        #[unsafe(method(searchPrev:))]
        fn search_prev(&self, _sender: &AnyObject) {
            let (tab, surface) = (self.ivars().tab, self.ivars().surface);
            self.ivars().controller.search_navigate_previous(tab, surface);
            self.refocus_field();
        }

        /// Next-match button (the down chevron): step to the next match, keeping
        /// keyboard focus in the field.
        #[unsafe(method(searchNext:))]
        fn search_next(&self, _sender: &AnyObject) {
            let (tab, surface) = (self.ivars().tab, self.ivars().surface);
            self.ivars().controller.search_navigate_next(tab, surface);
            self.refocus_field();
        }

        /// Close button (the ✕): end the search and return focus to the terminal.
        #[unsafe(method(searchClose:))]
        fn search_close(&self, _sender: &AnyObject) {
            let (tab, surface) = (self.ivars().tab, self.ivars().surface);
            self.ivars().controller.search_end(tab, surface);
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
        // Parity with upstream: prev = up chevron, next = down chevron, close =
        // ✕. Targets are wired to `this` after the object exists.
        let prev = make_button(mtm, "chevron.up", sel!(searchPrev:));
        let next = make_button(mtm, "chevron.down", sel!(searchNext:));
        let close = make_button(mtm, "xmark", sel!(searchClose:));

        let this = Self::alloc(mtm).set_ivars(SearchOverlayIvars {
            controller,
            tab,
            surface,
            field: field.clone(),
            counter: counter.clone(),
            prev: prev.clone(),
            next: next.clone(),
            close: close.clone(),
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

        // Lay out the field (left) + counter, then the prev/next/close buttons
        // (right) inside the overlay.
        let layout = inner_frames();
        field.setFrame(layout.field);
        counter.setFrame(layout.counter);
        prev.setFrame(layout.prev);
        next.setFrame(layout.next);
        close.setFrame(layout.close);

        // Wire the button actions to this overlay (created target-less above).
        let target: &AnyObject = this.as_ref();
        // SAFETY: `setTarget:` holds an unretained (weak) reference; the overlay
        // owns each button as a subview, so the target always outlives them.
        unsafe {
            prev.setTarget(Some(target));
            next.setTarget(Some(target));
            close.setTarget(Some(target));
        }

        // The overlay is the field's delegate (incremental search + key
        // interception).
        // SAFETY: `this` conforms to NSTextFieldDelegate; setDelegate holds a
        // weak ref, and the overlay outlives the field (both dropped together
        // when the pane closes).
        unsafe {
            let _: () = msg_send![&*field, setDelegate: target];
        }

        this.addSubview(&field);
        this.addSubview(&counter);
        this.addSubview(&prev);
        this.addSubview(&next);
        this.addSubview(&close);
        this.setHidden(true);
        this
    }

    /// Return keyboard focus to the needle field (after a nav-button click) so
    /// the user can keep typing without re-clicking the field.
    fn refocus_field(&self) {
        if let Some(window) = self.window() {
            window.makeFirstResponder(Some(&self.ivars().field));
        }
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

    /// Smoke/test: fire the previous / next / close buttons through the real
    /// target-action path (`performClick:`), exactly as a mouse click would.
    ///
    /// SAFETY: `performClick:` is a standard main-thread `NSControl` send; the
    /// button and its target (this overlay) are both alive here.
    pub fn click_prev(&self) {
        unsafe { self.ivars().prev.performClick(None) };
    }
    pub fn click_next(&self) {
        unsafe { self.ivars().next.performClick(None) };
    }
    pub fn click_close(&self) {
        unsafe { self.ivars().close.performClick(None) };
    }
}

/// The overlay's frame: pinned to the top-right of the pane, inset by `MARGIN`.
/// The pane view is flipped (top-left origin), so "top" is `y = MARGIN`.
fn overlay_frame(pane_bounds: NSRect) -> NSRect {
    let x = (pane_bounds.size.width - OVERLAY_W - MARGIN).max(MARGIN);
    NSRect::new(NSPoint::new(x, MARGIN), NSSize::new(OVERLAY_W, OVERLAY_H))
}

/// The laid-out frames of the overlay's controls, left→right:
/// `field | counter | prev | next | close`.
struct InnerFrames {
    field: NSRect,
    counter: NSRect,
    prev: NSRect,
    next: NSRect,
    close: NSRect,
}

/// Compute the control frames within the overlay. The three buttons hug the
/// right edge; the counter sits just left of them; the field takes the rest.
fn inner_frames() -> InnerFrames {
    let ctrl_h = OVERLAY_H - 10.0;
    let y = 5.0;
    let rect = |x: f64, w: f64| NSRect::new(NSPoint::new(x, y), NSSize::new(w, ctrl_h));

    // Buttons hug the right edge.
    let close_x = OVERLAY_W - MARGIN - BTN_W;
    let next_x = close_x - BTN_W;
    let prev_x = next_x - BTN_W;
    // Counter just left of the first button.
    let counter_x = prev_x - GAP - COUNTER_W;
    // Field fills the remaining left span.
    let field_w = counter_x - GAP - MARGIN;

    InnerFrames {
        field: rect(MARGIN, field_w),
        counter: rect(counter_x, COUNTER_W),
        prev: rect(prev_x, BTN_W),
        next: rect(next_x, BTN_W),
        close: rect(close_x, BTN_W),
    }
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

/// Build a borderless SF-Symbol nav/close button carrying `action`. `symbol` is
/// an SF Symbol name (e.g. `chevron.up`); its own name doubles as the
/// accessibility description. Falls back to a titled button if the symbol can't
/// be loaded (older systems / missing symbol). The target is wired separately
/// in [`SearchOverlay::new`] once the overlay object exists.
fn make_button(mtm: MainThreadMarker, symbol: &str, action: Sel) -> Retained<NSButton> {
    let sym = NSString::from_str(symbol);
    // `imageWithSystemSymbolName` is nil on failure (older systems / missing
    // symbol), which we handle by falling back to a titled button.
    let image = NSImage::imageWithSystemSymbolName_accessibilityDescription(&sym, Some(&sym));
    // SAFETY: standard AppKit class-method sends on the main thread; the button
    // is autoreleased/retained by objc2.
    let button = match image {
        Some(image) => unsafe {
            NSButton::buttonWithImage_target_action(&image, None, Some(action), mtm)
        },
        None => unsafe { NSButton::buttonWithTitle_target_action(&sym, None, Some(action), mtm) },
    };
    // Borderless, inline look (like upstream's SearchButtonStyle).
    button.setBordered(false);
    button
}
