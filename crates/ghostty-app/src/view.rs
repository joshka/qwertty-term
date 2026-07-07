//! The terminal `NSView`: hosts the render layer and conforms to
//! `NSTextInputClient` for the full macOS keyDown → `interpretKeyEvents` →
//! `insertText`/`setMarkedText` input dance (chunk R5).
//!
//! Structure follows the spike's `GhosttySpikeInputView`
//! (`spikes/appkit-input/src/view.rs`), promoted to production: the view backs
//! its `layer` with the renderer's [`IOSurfaceLayer`], accepts first responder,
//! and routes every encoded keystroke to the owning tab's PTY via the shared
//! [`Controller`]. Preedit state is stored (for a future inline render) and the
//! IME committed-text path sends composed text to the PTY.
//!
//! The view holds a `*const` back-reference to the [`Controller`] and its own
//! [`TabId`]; both are only touched on the main thread (AppKit guarantees
//! keyDown/draw run there), so the raw pointer is sound.

#![cfg(target_os = "macos")]

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSEvent, NSEventModifierFlags, NSTextInputClient, NSView};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSPoint, NSRange, NSRangePointer, NSRect,
    NSSize, NSString, NSUInteger,
};
use objc2_quartz_core::CALayer;

use crate::app::Controller;
use crate::input::preedit::Preedit;
use crate::input::translate::RawKeyEvent;
use crate::tabs::TabId;

/// Interior state for the terminal input view.
pub struct Ivars {
    /// Preedit / marked-text state machine.
    preedit: RefCell<Preedit>,
    /// The tab this view renders/inputs for.
    tab: TabId,
    /// Back-reference to the shared controller. Main-thread-only access.
    controller: *const Controller,
    /// The render layer hosted by this view (also assigned as `self.layer`).
    layer: Retained<ghostty_renderer::metal::IOSurfaceLayer>,
}

define_class!(
    // SAFETY:
    // - Superclass `NSView` has no subclassing requirement we violate: we add an
    //   ivar and implement key handling + `NSTextInputClient`.
    // - No `Drop` touches the objc runtime unsafely.
    #[unsafe(super(NSView))]
    #[name = "GhosttyTerminalView"]
    #[ivars = Ivars]
    #[thread_kind = MainThreadOnly]
    pub struct TerminalView;

    impl TerminalView {
        /// `keyDown:` — mirror upstream's structure: open the accumulator so IME
        /// `insertText` accumulates, run `interpretKeyEvents` (which drives the
        /// input context / dead-key / IME callbacks), then encode.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            let raw = raw_from_nsevent(event, false);
            self.ivars().preedit.borrow_mut().begin_key_event();

            // SAFETY: standard NSResponder call on the main thread.
            unsafe {
                let events = NSArray::from_slice(&[event]);
                let _: () = msg_send![self, interpretKeyEvents: &*events];
            }

            self.finish_key_event(&raw);
        }

        /// Accept first responder so we receive key events.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        /// The view draws via its layer (no `drawRect:`).
        #[unsafe(method(wantsUpdateLayer))]
        fn wants_update_layer(&self) -> bool {
            true
        }

        /// Flip the coordinate system so (0,0) is top-left, matching terminal
        /// mouse-report pixel space (and the render grid's row order).
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            self.route_mouse(event, MouseKind::Down, Some(MouseBtn::Left));
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            self.route_mouse(event, MouseKind::Up, Some(MouseBtn::Left));
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            self.route_mouse(event, MouseKind::Down, Some(MouseBtn::Right));
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            self.route_mouse(event, MouseKind::Up, Some(MouseBtn::Right));
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            self.route_mouse(event, MouseKind::Drag, Some(MouseBtn::Left));
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            // Report scroll as button 4 (up) / 5 (down) press, like xterm.
            let dy = event.scrollingDeltaY();
            if dy == 0.0 {
                return;
            }
            let btn = if dy > 0.0 { MouseBtn::WheelUp } else { MouseBtn::WheelDown };
            self.route_mouse(event, MouseKind::Down, Some(btn));
        }
    }

    unsafe impl NSTextInputClient for TerminalView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, _replacement_range: NSRange) {
            let s = any_to_string(string);
            let sent = {
                let mut preedit = self.ivars().preedit.borrow_mut();
                // Commit: inside a keyDown it accumulates (drained in
                // finish_key_event); outside, send immediately.
                preedit.commit(&s).map(str::to_owned)
            };
            if let Some(text) = sent {
                self.send_text(&text);
            }
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(
            &self,
            string: &AnyObject,
            _selected_range: NSRange,
            _replacement_range: NSRange,
        ) {
            let s = any_to_string(string);
            self.ivars().preedit.borrow_mut().set_marked(&s);
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            self.ivars().preedit.borrow_mut().unmark();
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            self.ivars().preedit.borrow().is_composing()
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            let len = self.ivars().preedit.borrow().marked_text().len();
            if len == 0 {
                NSRange::new(NSUInteger::MAX, 0)
            } else {
                NSRange::new(0, len)
            }
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            // NSTextInputClient's marked-text selection, not the terminal's
            // mouse selection (that's engine-side; see crate::selection /
            // crate::app::Controller::mouse_to_tab). This app never has a
            // selected range within IME marked text, so always report empty.
            NSRange::new(NSUInteger::MAX, 0)
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes_for_marked_text(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            NSArray::new()
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(
            &self,
            _range: NSRange,
            _actual_range: NSRangePointer,
        ) -> Option<Retained<NSAttributedString>> {
            None
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, _range: NSRange, _actual_range: NSRangePointer) -> NSRect {
            // IME box placement from real cell geometry is a follow-on; the
            // preedit state machine is what R5 wires. Return a zero rect.
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(0.0, 0.0))
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index_for_point(&self, _point: NSPoint) -> NSUInteger {
            0
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, _selector: Sel) {
            // Suppress the default NSBeep for unhandled selectors.
        }
    }
);

impl TerminalView {
    /// Allocate the view for `tab`, wired to `controller`, hosting a fresh
    /// [`HostLayer`](ghostty_renderer::metal::IOSurfaceLayer).
    ///
    /// # Safety
    /// `controller` must outlive the view (the app owns both for the process
    /// lifetime) and only be dereferenced on the main thread.
    pub fn new(
        mtm: objc2::MainThreadMarker,
        tab: TabId,
        controller: *const Controller,
    ) -> Retained<Self> {
        let layer = ghostty_renderer::metal::IOSurfaceLayer::new();
        let this = Self::alloc(mtm).set_ivars(Ivars {
            preedit: RefCell::new(Preedit::new()),
            tab,
            controller,
            layer,
        });
        let this: Retained<Self> = unsafe { msg_send![super(this), init] };

        // Host the render layer: layer-backed view whose layer is our IOSurface
        // layer.
        this.setWantsLayer(true);
        let calayer: &CALayer = this.ivars().layer.as_layer();
        this.setLayer(Some(calayer));
        this
    }

    /// The tab this view belongs to.
    pub fn tab(&self) -> TabId {
        self.ivars().tab
    }

    /// The host layer to present into.
    pub fn host_layer(&self) -> &ghostty_renderer::metal::IOSurfaceLayer {
        &self.ivars().layer
    }

    /// Encode any committed text + the key event, then close the per-keyDown
    /// window. Split out so it can be driven by synthetic raw data in tests.
    fn finish_key_event(&self, raw: &RawKeyEvent) {
        let committed = self.ivars().preedit.borrow_mut().end_key_event();
        let composing = {
            let p = self.ivars().preedit.borrow();
            p.is_composing() || !committed.is_empty()
        };

        if !committed.is_empty() {
            for t in &committed {
                self.send_text(t);
            }
            return;
        }
        if composing {
            // Still composing: key consumed by the IME, don't encode.
            return;
        }

        // Normal key: encode via the controller (which reads the tab's live
        // terminal encode options + user option-as-alt config).
        self.with_controller(|c| c.encode_key_to_tab(self.ivars().tab, raw));
    }

    /// Send already-composed text to the tab's PTY (IME commit path). Bracketed
    /// if the program enabled bracketed paste is handled at the controller.
    fn send_text(&self, text: &str) {
        let text = text.to_owned();
        self.with_controller(move |c| c.send_text_to_tab(self.ivars().tab, &text));
    }

    /// Convert a mouse `NSEvent` to top-left device-pixel coordinates and route
    /// it through the controller (which drops it unless the program enabled
    /// mouse reporting). The view is flipped, so `locationInWindow` converted to
    /// view space already has a top-left origin; scale to device pixels.
    fn route_mouse(&self, event: &NSEvent, kind: MouseKind, button: Option<MouseBtn>) {
        let win_pt = event.locationInWindow();
        let view_pt = self.convertPoint_fromView(win_pt, None);
        let scale = self.window().map(|w| w.backingScaleFactor()).unwrap_or(2.0);
        let x = (view_pt.x * scale) as f32;
        let y = (view_pt.y * scale) as f32;

        let mods = mods_from_flags(event.modifierFlags());
        let (action, pressed) = match kind {
            MouseKind::Down => (ghostty_input::mouse::Action::Press, Some(true)),
            MouseKind::Up => (ghostty_input::mouse::Action::Release, Some(false)),
            MouseKind::Drag => (ghostty_input::mouse::Action::Motion, None),
        };
        let btn = button.map(MouseBtn::to_input);
        let tab = self.ivars().tab;
        self.with_controller(|c| c.mouse_to_tab(tab, action, btn, mods, x, y, pressed));
    }

    /// Run `f` with the controller, if the back-pointer is set.
    fn with_controller<R>(&self, f: impl FnOnce(&Controller) -> R) -> Option<R> {
        let ptr = self.ivars().controller;
        if ptr.is_null() {
            return None;
        }
        // SAFETY: main-thread-only; the controller outlives the view.
        Some(f(unsafe { &*ptr }))
    }
}

/// Extract a [`RawKeyEvent`] from a real `NSEvent`. Mirrors the field reads in
/// upstream `NSEvent+Extension.swift::ghosttyKeyEvent` + `ghosttyCharacters`,
/// including the raw `NX_DEVICER*` right-side modifier bits.
fn raw_from_nsevent(event: &NSEvent, is_up: bool) -> RawKeyEvent {
    let keycode: u16 = event.keyCode();
    let is_repeat: bool = if is_up { false } else { event.isARepeat() };
    let flags: NSEventModifierFlags = event.modifierFlags();
    let raw_flags = flags.0 as u64;

    let characters = event.characters();
    let unshifted = event.charactersByApplyingModifiers(NSEventModifierFlags::empty());

    let text = characters
        .map(|s| filter_ghostty_characters(&s.to_string()))
        .unwrap_or_default();
    let unshifted_codepoint = unshifted
        .and_then(|s| s.to_string().chars().next())
        .map(|c| c as u32)
        .unwrap_or(0);

    RawKeyEvent {
        keycode,
        is_repeat,
        is_up,
        shift: flags.contains(NSEventModifierFlags::Shift),
        ctrl: flags.contains(NSEventModifierFlags::Control),
        option: flags.contains(NSEventModifierFlags::Option),
        command: flags.contains(NSEventModifierFlags::Command),
        caps_lock: flags.contains(NSEventModifierFlags::CapsLock),
        // Right-side device bits from the raw modifier mask (upstream
        // `ghosttyMods` reads these NX_DEVICER*KEYMASK bits directly).
        shift_right: raw_flags & NX_DEVICERSHIFTKEYMASK != 0,
        ctrl_right: raw_flags & NX_DEVICERCTLKEYMASK != 0,
        option_right: raw_flags & NX_DEVICERALTKEYMASK != 0,
        command_right: raw_flags & NX_DEVICERCMDKEYMASK != 0,
        text,
        unshifted_codepoint,
    }
}

// The right-side device modifier masks (`<IOKit/hidsystem/IOLLEvent.h>`),
// carried in the low bits of `NSEvent.modifierFlags`. objc2's safe surface
// doesn't name them, so we test the raw bits like upstream `ghosttyMods`.
const NX_DEVICERSHIFTKEYMASK: u64 = 0x0000_0004;
const NX_DEVICERCTLKEYMASK: u64 = 0x0000_2000;
const NX_DEVICERALTKEYMASK: u64 = 0x0000_0040;
const NX_DEVICERCMDKEYMASK: u64 = 0x0000_0010;

/// Which kind of pointer transition a mouse handler represents.
#[derive(Clone, Copy)]
enum MouseKind {
    Down,
    Up,
    Drag,
}

/// The buttons the view reports, mapped onto `ghostty_input`'s button set.
#[derive(Clone, Copy)]
enum MouseBtn {
    Left,
    Right,
    WheelUp,
    WheelDown,
}

impl MouseBtn {
    fn to_input(self) -> ghostty_input::mouse::Button {
        use ghostty_input::mouse::Button;
        match self {
            MouseBtn::Left => Button::Left,
            MouseBtn::Right => Button::Right,
            // xterm reports wheel up/down as buttons 4/5.
            MouseBtn::WheelUp => Button::Four,
            MouseBtn::WheelDown => Button::Five,
        }
    }
}

/// Build `ghostty_input` [`Mods`](ghostty_input::key_mods::Mods) from
/// NSEvent modifier flags (shift/ctrl/alt/super only — what mouse encoding
/// uses).
fn mods_from_flags(flags: NSEventModifierFlags) -> ghostty_input::key_mods::Mods {
    ghostty_input::key_mods::Mods {
        shift: flags.contains(NSEventModifierFlags::Shift),
        ctrl: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        super_: flags.contains(NSEventModifierFlags::Command),
        ..Default::default()
    }
}

/// Port of `NSEvent.ghosttyCharacters`: drop single control chars (the encoder
/// handles those) and PUA function-key codepoints.
fn filter_ghostty_characters(chars: &str) -> String {
    let mut it = chars.chars();
    if let (Some(c), true) = (it.next(), it.clone().next().is_none()) {
        let cp = c as u32;
        if cp < 0x20 {
            return String::new();
        }
        if (0xF700..=0xF8FF).contains(&cp) {
            return String::new();
        }
    }
    chars.to_string()
}

/// Extract a Rust `String` from an NSString/NSAttributedString `id`.
fn any_to_string(obj: &AnyObject) -> String {
    // SAFETY: `obj` is the `id` AppKit passed to insertText/setMarkedText; it's
    // always an NSString or NSAttributedString.
    unsafe {
        let is_attr: bool = {
            let cls = objc2::class!(NSAttributedString);
            msg_send![obj, isKindOfClass: cls]
        };
        if is_attr {
            let s: Retained<NSString> = msg_send![obj, string];
            s.to_string()
        } else {
            let s: &NSString = &*(obj as *const AnyObject as *const NSString);
            s.to_string()
        }
    }
}
