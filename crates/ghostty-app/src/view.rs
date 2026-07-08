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
use objc2_quartz_core::{CALayer, kCAGravityBottomLeft};

use crate::app::Controller;
use crate::input::keymap::key_from_macos_keycode;
use crate::input::preedit::Preedit;
use crate::input::translate::RawKeyEvent;
use crate::splitkeys;
use crate::splits::SurfaceId;
use crate::tabkeys::{self, TabMods};
use crate::tabs::TabId;

/// Interior state for the terminal input view.
pub struct Ivars {
    /// Preedit / marked-text state machine.
    preedit: RefCell<Preedit>,
    /// The tab this view renders/inputs for.
    tab: TabId,
    /// The surface (pane) this view *is* — one `TerminalView` per split pane.
    /// Keystrokes/mouse route to exactly this surface, so a split tab's input
    /// isolation is automatic: only the first-responder view (the focused pane)
    /// receives events.
    surface: SurfaceId,
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

        /// `performKeyEquivalent:` — intercept the built-in tab-navigation
        /// chords *before* `keyDown:` and the PTY encoder ever see them.
        ///
        /// This is the interception point (not `keyDown:`) for two reasons that
        /// the task's correctness notes call out:
        ///
        /// 1. On macOS, `ctrl+tab` / `ctrl+shift+tab` are frequently consumed as
        ///    key equivalents (or by the window's tab group) and never reach
        ///    `keyDown:`. `performKeyEquivalent:` runs earlier in the responder
        ///    chain and reliably catches them.
        /// 2. Returning `true` tells AppKit the event is fully handled, so it is
        ///    NOT forwarded to `keyDown:` → the encoder. That is exactly what
        ///    keeps `ctrl+tab` from ever sending `\t` / a CSI-u sequence to the
        ///    shell (real Ghostty consumes these chords).
        ///
        /// Crucially, [`tabkeys::resolve`] only matches the *exact* tab chords,
        /// so plain Tab, Shift+Tab, and Ctrl+I do not resolve here: we return
        /// `false` and AppKit proceeds to `keyDown:` → the encoder unchanged.
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            self.try_handle_tab_key(event)
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
            // Click focuses this pane first (so a click on an unfocused split
            // both focuses it and starts any selection/mouse-report there), then
            // routes the press. Focusing makes this view the first responder, so
            // subsequent keystrokes go to this pane.
            let (tab, surface) = (self.ivars().tab, self.ivars().surface);
            self.with_controller(|c| c.focus_surface_in_tab(tab, surface));
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
        surface: SurfaceId,
        controller: *const Controller,
    ) -> Retained<Self> {
        let layer = ghostty_renderer::metal::IOSurfaceLayer::new();
        let this = Self::alloc(mtm).set_ivars(Ivars {
            preedit: RefCell::new(Preedit::new()),
            tab,
            surface,
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

    /// The surface (pane) this view is.
    pub fn surface(&self) -> SurfaceId {
        self.ivars().surface
    }

    /// The host layer to present into.
    pub fn host_layer(&self) -> &ghostty_renderer::metal::IOSurfaceLayer {
        &self.ivars().layer
    }

    /// Pin the presented surface flush to the *visual top* of the view.
    ///
    /// The surface is sized to a whole number of cells, so it is up to one cell
    /// shorter than the view; the leftover strip is uncovered layer area. The
    /// renderer's layer defaults to `kCAGravityTopLeft`, but this view returns
    /// `isFlipped == true`, which makes AppKit set the backing layer's
    /// `geometryFlipped = true` — inverting the layer's Y axis. In that flipped
    /// geometry, `kCAGravityTopLeft` pins contents to the layer's max-Y corner,
    /// which is the *visual bottom*, so the uncovered strip lands at the *top* —
    /// exactly the reported dark band directly under the titlebar.
    ///
    /// Setting `kCAGravityBottomLeft` here pins contents to the layer's min-Y
    /// corner in flipped geometry = the *visual top*, so the surface sits flush
    /// under the titlebar (matching real Ghostty) and the sub-cell remainder
    /// moves to the bottom edge — where a terminal's partial last row belongs,
    /// and where the window's terminal-coloured background makes it invisible.
    ///
    /// Called once at view attachment; must run on the main thread.
    pub fn pin_surface_to_top(&self) {
        // SAFETY: `kCAGravityBottomLeft` is a valid contents-gravity constant;
        // main-thread call at attachment time.
        unsafe {
            self.ivars()
                .layer
                .as_layer()
                .setContentsGravity(kCAGravityBottomLeft);
        }
    }

    /// Whether the host layer's `contentsGravity` is the visual-top-pinning value
    /// installed by [`Self::pin_surface_to_top`] (smoke/test only).
    pub fn surface_pinned_to_top(&self) -> bool {
        let gravity = self.ivars().layer.as_layer().contentsGravity();
        // SAFETY: `kCAGravityBottomLeft` is a valid framework NSString constant.
        let want: &NSString = unsafe { kCAGravityBottomLeft };
        gravity.to_string() == want.to_string()
    }

    /// If `event` is one of the built-in tab-navigation chords, run its action
    /// against the controller and return `true` (consuming the event so it never
    /// reaches the PTY encoder). Otherwise return `false` so AppKit keeps
    /// dispatching (→ `keyDown:` → encoder). Called from `performKeyEquivalent:`.
    ///
    /// Uses the *physical* keycode (layout-independent) for the digit keys, so
    /// `cmd+1..9` work on non-US layouts — matching upstream's `physical:one..`.
    fn try_handle_tab_key(&self, event: &NSEvent) -> bool {
        let key = key_from_macos_keycode(event.keyCode());
        let mods = tab_mods_from_flags(event.modifierFlags());

        // Split chords take precedence (cmd+d, ctrl+alt+arrow, ctrl+super+[]).
        // They are disjoint from the tab table (asserted in
        // `splitkeys::tests::does_not_collide_with_tab_bindings`), so order only
        // matters for clarity. A resolved split chord acts on *this* view's tab
        // and never falls through to the encoder.
        if let Some(action) = splitkeys::resolve(key, mods) {
            let tab = self.ivars().tab;
            self.with_controller(|c| c.handle_split_action(tab, action));
            return true;
        }

        let Some(action) = tabkeys::resolve(key, mods) else {
            return false;
        };
        self.with_controller(|c| c.handle_tab_action(action));
        // Consume regardless of whether a tab switch happened (e.g. the 1-tab
        // no-op case): a resolved chord must never fall through to the encoder.
        true
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

        // Normal key: encode via the controller (which reads this surface's live
        // terminal encode options + user option-as-alt config). Routed to *this*
        // surface — input isolation is automatic because only the focused pane's
        // view is first responder.
        let (tab, surface) = (self.ivars().tab, self.ivars().surface);
        self.with_controller(|c| c.encode_key_to_surface(tab, surface, raw));
    }

    /// Send already-composed text to this surface's PTY (IME commit path).
    /// Bracketed if the program enabled bracketed paste is handled at the
    /// controller.
    fn send_text(&self, text: &str) {
        let text = text.to_owned();
        let (tab, surface) = (self.ivars().tab, self.ivars().surface);
        self.with_controller(move |c| c.send_text_to_surface(tab, surface, &text));
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
        let (tab, surface) = (self.ivars().tab, self.ivars().surface);
        self.with_controller(|c| {
            c.mouse_to_surface(tab, surface, action, btn, mods, x, y, pressed)
        });
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

/// Build the tab-keybind [`TabMods`] from NSEvent modifier flags. Distinct from
/// [`mods_from_flags`] (which yields the encoder's `ghostty_input` mods): this is
/// the four-modifier bitset the built-in tab table matches on.
fn tab_mods_from_flags(flags: NSEventModifierFlags) -> TabMods {
    TabMods {
        shift: flags.contains(NSEventModifierFlags::Shift),
        ctrl: flags.contains(NSEventModifierFlags::Control),
        alt: flags.contains(NSEventModifierFlags::Option),
        super_: flags.contains(NSEventModifierFlags::Command),
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
