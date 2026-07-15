//! macOS-only: a minimal `NSView` subclass conforming to `NSTextInputClient`,
//! wired to the [`crate::translate`] encoder and [`crate::preedit`] state.
//!
//! This is the R5-shaped artifact: it proves an `objc2` `define_class!` NSView
//! can conform to `NSTextInputClient`, receive `keyDown:`, drive the same
//! keyDown -> `interpretKeyEvents` -> `insertText`/`setMarkedText` dance
//! upstream uses (`SurfaceView_AppKit.swift`), and feed
//! `qwertty_term_input::key_encode::encode`.
//!
//! What it deliberately does NOT do (out of spike scope): host a CALayer,
//! participate in the responder chain of a real window, or wire
//! `performKeyEquivalent`. Those are named in the analysis doc as R5 work; the
//! encoder/preedit plumbing they'd feed is what's proven here.
//!
//! Verification note: the interesting logic (translation, preedit state) is in
//! the AppKit-free modules and is unit-tested there. This view is the thin
//! Objective-C shell around them. `record_encoded` captures every encoded byte
//! sequence so a harness can assert on what a real (or synthetic) event
//! produced.

#![cfg(target_os = "macos")]

use std::cell::RefCell;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSTextInputClient, NSView};
use objc2_foundation::{
    NSArray, NSAttributedString, NSAttributedStringKey, NSPoint, NSRange, NSRangePointer, NSRect,
    NSString, NSUInteger,
};

use crate::preedit::Preedit;
use crate::translate::{self, Config, RawKeyEvent};

/// Interior state for the input view.
pub struct Ivars {
    /// Preedit / marked-text state machine.
    preedit: RefCell<Preedit>,
    /// Encoder config (option-as-alt, kitty flags).
    config: Config,
    /// Every PTY byte sequence this view encoded, in order. Lets tests/harness
    /// assert on output without a live PTY.
    encoded: RefCell<Vec<Vec<u8>>>,
    /// Text committed via `insertText` (the terminal `sendText` path), in order.
    committed_text: RefCell<Vec<String>>,
}

define_class!(
    // SAFETY:
    // - Superclass `NSView` has no subclassing requirements we violate: we only
    //   add an ivar and implement `NSTextInputClient` methods plus key handlers.
    // - No `Drop` impl touches the objc runtime unsafely.
    #[unsafe(super(NSView))]
    #[name = "GhosttySpikeInputView"]
    #[ivars = Ivars]
    #[thread_kind = MainThreadOnly]
    pub struct InputView;

    /// NSResponder key handling.
    impl InputView {
        /// `keyDown:` — mirrors upstream's structure: mark that we're in a key
        /// event (so `insertText` accumulates), run the interpret dance, then
        /// encode.
        ///
        /// In a real app this calls `self.interpretKeyEvents([event])`, which is
        /// what lets the input context drive dead-key / IME composition via the
        /// `NSTextInputClient` callbacks. Here we expose the same flow through
        /// [`InputView::handle_key_down_raw`] so it can be driven both by a real
        /// `NSEvent` and by synthetic test data.
        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &objc2_app_kit::NSEvent) {
            let raw = raw_from_nsevent(event, false);

            // Enter the key-event window so IME `insertText` accumulates.
            self.ivars().preedit.borrow_mut().begin_key_event();

            // Let the input context handle complex composition. This may call
            // back into our setMarkedText/insertText.
            // SAFETY: standard NSResponder call, valid on the main thread.
            unsafe {
                let events = NSArray::from_slice(&[event]);
                let _: () = msg_send![self, interpretKeyEvents: &*events];
            }

            self.finish_key_event(&raw);
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &objc2_app_kit::NSEvent) {
            let raw = raw_from_nsevent(event, true);
            let bytes = translate::encode_raw(&raw, &self.ivars().config);
            self.record_encoded(bytes);
        }

        /// Accept first responder so we actually receive key events.
        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }
    }

    /// NSTextInputClient conformance. Signatures come from
    /// `objc2_app_kit::NSTextInputClient`.
    unsafe impl NSTextInputClient for InputView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, _replacement_range: NSRange) {
            let s = any_to_string(string);
            let mut preedit = self.ivars().preedit.borrow_mut();
            match preedit.commit(&s) {
                // Committed while inside a keyDown: accumulated, drained later.
                None => {}
                // Committed outside a key event (rare): send immediately.
                Some(text) => {
                    drop(preedit);
                    self.ivars()
                        .committed_text
                        .borrow_mut()
                        .push(text.to_string());
                }
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
            // Spike: no terminal selection model; report empty.
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
            // Spike: IME box positioning is R5 work; return zero rect.
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                objc2_foundation::NSSize::new(0.0, 0.0),
            )
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index_for_point(&self, _point: NSPoint) -> NSUInteger {
            0
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, _selector: Sel) {
            // Suppress the default NSBeep for unhandled selectors. Real
            // performKeyEquivalent re-injection is R5 work.
        }
    }
);

impl InputView {
    /// Encode any accumulated committed text + the key event itself, then clear
    /// the per-keyDown window. Split out so tests can drive the same tail logic
    /// via [`InputView::handle_key_down_raw`].
    fn finish_key_event(&self, raw: &RawKeyEvent) {
        let committed = self.ivars().preedit.borrow_mut().end_key_event();
        let composing = {
            let p = self.ivars().preedit.borrow();
            p.is_composing() || !committed.is_empty()
        };

        if !committed.is_empty() {
            // IME committed text: record it as sent text (upstream sendText).
            for t in &committed {
                self.ivars().committed_text.borrow_mut().push(t.clone());
            }
            return;
        }

        if composing {
            // Still composing (preedit set, nothing committed): the key was
            // consumed by the IME; don't encode.
            return;
        }

        let bytes = translate::encode_raw(raw, &self.ivars().config);
        self.record_encoded(bytes);
    }

    fn record_encoded(&self, bytes: Vec<u8>) {
        if !bytes.is_empty() {
            self.ivars().encoded.borrow_mut().push(bytes);
        }
    }

    /// Snapshot of all encoded byte sequences so far.
    pub fn encoded(&self) -> Vec<Vec<u8>> {
        self.ivars().encoded.borrow().clone()
    }

    /// Snapshot of all committed (IME-sent) text so far.
    pub fn committed_text(&self) -> Vec<String> {
        self.ivars().committed_text.borrow().clone()
    }

    /// Current preedit string.
    pub fn preedit_text(&self) -> String {
        self.ivars().preedit.borrow().marked_text().to_string()
    }

    /// Test/harness seam: run the exact keyDown tail logic from synthetic raw
    /// data, WITHOUT a real NSEvent or input context. `ime` describes what the
    /// (absent) input context would have done via the NSTextInputClient
    /// callbacks during `interpretKeyEvents`.
    pub fn handle_key_down_raw(&self, raw: &RawKeyEvent, ime: ImeScript) {
        self.ivars().preedit.borrow_mut().begin_key_event();
        // Simulate what the input context callbacks would do.
        match ime {
            ImeScript::None => {}
            ImeScript::Mark(s) => self.ivars().preedit.borrow_mut().set_marked(&s),
            ImeScript::Commit(s) => {
                let mut p = self.ivars().preedit.borrow_mut();
                let _ = p.commit(&s);
            }
        }
        self.finish_key_event(raw);
    }
}

/// What the input context would drive via NSTextInputClient during a synthetic
/// keyDown, for the test seam. A real event loop replaces this with actual
/// `interpretKeyEvents` callbacks.
#[derive(Debug, Clone)]
pub enum ImeScript {
    /// No IME interaction (normal key).
    None,
    /// `setMarkedText(s)` — enter/continue preedit (e.g. dead-key start).
    Mark(String),
    /// `insertText(s)` — commit composed text.
    Commit(String),
}

impl InputView {
    /// Allocate + init the view with the given config.
    pub fn new(mtm: objc2::MainThreadMarker, config: Config) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            preedit: RefCell::new(Preedit::new()),
            config,
            encoded: RefCell::new(Vec::new()),
            committed_text: RefCell::new(Vec::new()),
        });
        // SAFETY: NSView's designated initializer path; we inherit `init`.
        unsafe { msg_send![super(this), init] }
    }
}

/// Extract a [`RawKeyEvent`] from a real `NSEvent`. Mirrors the field reads in
/// upstream `NSEvent+Extension.swift::ghosttyKeyEvent` + `ghosttyCharacters`.
fn raw_from_nsevent(event: &objc2_app_kit::NSEvent, is_up: bool) -> RawKeyEvent {
    use objc2_app_kit::NSEventModifierFlags;

    let keycode: u16 = event.keyCode();
    let is_repeat: bool = if is_up { false } else { event.isARepeat() };
    let flags: NSEventModifierFlags = event.modifierFlags();
    let characters: Option<Retained<NSString>> = event.characters();
    // characters(byApplyingModifiers:) for the unshifted codepoint.
    let unshifted: Option<Retained<NSString>> =
        event.charactersByApplyingModifiers(NSEventModifierFlags::empty());

    let text = characters
        .map(|s| filter_ghostty_characters(&s.to_string(), flags))
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
        // Right-side device bits aren't exposed by the safe objc2 API surface;
        // the spike leaves them Left (option-as-alt Left/Right variants are
        // covered by the pure translate tests instead). R5 reads the
        // NX_DEVICER* raw bits like upstream Ghostty.ghosttyMods.
        shift_right: false,
        ctrl_right: false,
        option_right: false,
        command_right: false,
        text,
        unshifted_codepoint,
    }
}

/// Port of `NSEvent.ghosttyCharacters`: drop single control chars (encoder
/// handles those) and PUA function-key codepoints.
fn filter_ghostty_characters(chars: &str, _flags: objc2_app_kit::NSEventModifierFlags) -> String {
    let mut it = chars.chars();
    if let (Some(c), None) = (it.next(), it.clone().next().is_none().then_some(())) {
        let cp = c as u32;
        // Single control char: encoder handles it, don't pass as text.
        if cp < 0x20 {
            return String::new();
        }
        // PUA function-key range: not text.
        if (0xF700..=0xF8FF).contains(&cp) {
            return String::new();
        }
    }
    chars.to_string()
}

/// Extract a Rust `String` from an NSString/NSAttributedString `id`.
fn any_to_string(obj: &AnyObject) -> String {
    // Try NSAttributedString first (has a `.string`), then NSString.
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
