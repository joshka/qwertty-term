//! C ABI over the qwertty-term engine (M5 FFI spike scope).
//!
//! This crate is the Rust side of the Rust -> C-ABI -> Swift/AppKit seam. It
//! mirrors the *shapes* of upstream `include/ghostty.h` (opaque handles, sized
//! `*_s` structs, `*_e` enums, a runtime-callback struct with wakeup +
//! clipboard callbacks) so upstream's Swift sources can be **adapted, not
//! rewritten** (see `docs/analysis/ffi-spike.md`). It does **not** reproduce
//! the whole upstream surface: the spike scope is app + surface lifecycle, one
//! key round-trip through [`qwertty_term_input`], raw PTY-byte write, a screen-text
//! dump (pre-M3 stand-in for `surface_draw`), and an OSC 52 clipboard callback.
//!
//! # Design notes
//!
//! - **Header is generated.** `cbindgen` produces `include/qwertty_term.h` from
//!   the `#[repr(C)]` types and `extern "C"` fns below; a checked-in copy is
//!   diffed by `tests/header_drift.rs` so drift is loud. Every type/fn that must
//!   appear in the header is reachable from an `extern "C"` fn signature.
//! - **Panic safety.** Every `extern "C"` entry point wraps its body in
//!   [`std::panic::catch_unwind`]; a panic unwinding across the FFI boundary is
//!   UB, so a caught panic becomes [`QWERTTY_TERM_PANIC`] (or a null handle for
//!   constructors) instead. Handler bodies must not hold `&mut` across a panic
//!   in a way that leaves a handle poisoned; the spike keeps bodies simple.
//! - **Threading.** See the module-level "Threading contract" section in the
//!   analysis doc. In short: nothing here is thread-safe. An app + its surfaces
//!   form a single-thread apartment (like upstream's main-thread-only apprt
//!   surface). The one exception is `wakeup_cb`, which the embedder must treat
//!   as callable from any thread (the spike never fires it off-thread, but the
//!   contract is documented so a real PTY reader thread can).
//!
//! # Ownership / buffer conventions (mirroring upstream)
//!
//! - Handles are owned by the caller between `_new` and `_free`; `_free(NULL)`
//!   is a no-op.
//! - `surface_read_text` uses the upstream caller-buffer + length convention:
//!   pass `buf == NULL` to query the required length (written to `*out_len`),
//!   then call again with a buffer of that size. The text is **not**
//!   NUL-terminated (the length is authoritative); a trailing NUL is written
//!   only if it fits, for C-string convenience.
//! - Strings handed to callbacks (clipboard data) are **not** owned by the
//!   callback; they are valid only for the duration of the call.

#![allow(clippy::missing_safety_doc)]

use std::ffi::{c_char, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

use qwertty_term_input::key::{Action as InputAction, Key, KeyEvent};
use qwertty_term_input::key_encode::{self, KittyFlags, Options as EncodeOptions};
use qwertty_term_input::key_mods::Mods;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Options as TerminalOptions, Terminal};

// ---------------------------------------------------------------------------
// Result codes (mirror vt-diff's GHOSTTY_* scheme; `_RS_` namespaced).
// ---------------------------------------------------------------------------

/// Result code for the qwertty-term C ABI. `0` is success; negatives are errors.
///
/// Mirrors the `GhosttyResult` convention the differential harness already
/// binds (`crates/vt-diff/src/ffi.rs`), namespaced `_RS_` to avoid clashing
/// with a co-linked libghostty-vt.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QwerttyTermResult {
    /// The call succeeded.
    Success = 0,
    /// A required pointer argument was NULL.
    NullArgument = -1,
    /// An argument was out of range or otherwise invalid.
    InvalidValue = -2,
    /// The caller-provided buffer was too small; `*out_len` holds the need.
    OutOfSpace = -3,
    /// A panic was caught at the FFI boundary (would have been UB to unwind).
    Panic = -100,
}

/// Success sentinel, also exposed as a macro-style constant in the header for
/// callers that prefer `== QWERTTY_TERM_SUCCESS`.
pub const QWERTTY_TERM_SUCCESS: i32 = QwerttyTermResult::Success as i32;
/// See [`QwerttyTermResult::Panic`].
pub const QWERTTY_TERM_PANIC: i32 = QwerttyTermResult::Panic as i32;

// ---------------------------------------------------------------------------
// Runtime config + callbacks (mirror ghostty_runtime_config_s shape).
// ---------------------------------------------------------------------------

/// Clipboard selection, mirroring upstream `ghostty_clipboard_e`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QwerttyTermClipboard {
    /// The standard system clipboard (OSC 52 `c`).
    Standard = 0,
    /// The primary selection clipboard (OSC 52 `p`).
    Selection = 1,
}

/// Wakeup callback: the engine asks the embedder to schedule a tick on its
/// event loop. **Callable from any thread** (see the threading contract). The
/// `void*` is the `userdata` from [`QwerttyTermRuntimeConfig`].
pub type QwerttyTermWakeupCb = Option<extern "C" fn(userdata: *mut c_void)>;

/// Write-clipboard callback: fired when the engine wants to set the clipboard
/// (OSC 52 write). `data` is the **raw, still-base64-encoded** OSC body (empty
/// string means "clear"), valid only for the duration of the call. Mirrors the
/// intent of upstream `ghostty_runtime_write_clipboard_cb` (simplified: no
/// mime/confirm plumbing in the spike). Main-thread-only.
pub type QwerttyTermWriteClipboardCb =
    Option<extern "C" fn(userdata: *mut c_void, kind: QwerttyTermClipboard, data: *const c_char)>;

/// Runtime configuration handed to [`qwertty_term_app_new`]. Mirrors the shape of
/// upstream `ghostty_runtime_config_s` (subset: userdata + the two callbacks in
/// spike scope). Passed by const pointer; copied internally, so the caller's
/// struct need not outlive the call.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct QwerttyTermRuntimeConfig {
    /// Opaque embedder pointer passed back to every callback.
    pub userdata: *mut c_void,
    /// See [`QwerttyTermWakeupCb`]. May be NULL.
    pub wakeup_cb: QwerttyTermWakeupCb,
    /// See [`QwerttyTermWriteClipboardCb`]. May be NULL.
    pub write_clipboard_cb: QwerttyTermWriteClipboardCb,
}

/// Surface configuration handed to [`qwertty_term_surface_new`]. Mirrors the role
/// of upstream `ghostty_surface_config_s` (spike subset: grid size + scrollback).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct QwerttyTermSurfaceConfig {
    /// Width in cells; must be > 0.
    pub cols: u16,
    /// Height in cells; must be > 0.
    pub rows: u16,
    /// Maximum scrollback lines retained.
    pub max_scrollback: usize,
}

// ---------------------------------------------------------------------------
// Input key event (mirror qwertty_term_input_key_s).
// ---------------------------------------------------------------------------

/// Key action, mirroring upstream `qwertty_term_input_action_e`.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QwerttyTermInputAction {
    /// Key released.
    Release = 0,
    /// Key pressed.
    Press = 1,
    /// Auto-repeat.
    Repeat = 2,
}

/// Modifier bitmask, mirroring upstream `qwertty_term_input_mods_e` (a packed set;
/// here a plain bitfield of the modifiers the spike encodes). LSB-first.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QwerttyTermInputMods {
    /// Packed bits: 1=shift, 2=ctrl, 4=alt, 8=super. Others reserved (0).
    pub bits: u16,
}

/// A key input event, mirroring upstream `qwertty_term_input_key_s`.
///
/// `text` is an optional UTF-8, NUL-terminated string of the codepoint(s) this
/// key generated (may be NULL / empty for non-text keys). It is borrowed for
/// the duration of the call only.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct QwerttyTermInputKey {
    /// Press / release / repeat.
    pub action: QwerttyTermInputAction,
    /// Active modifiers.
    pub mods: QwerttyTermInputMods,
    /// The text this key generated (UTF-8, NUL-terminated), or NULL.
    pub text: *const c_char,
    /// The unshifted codepoint (0 = none). Used when `text` is NULL.
    pub unshifted_codepoint: u32,
    /// True while mid dead-key composition (event should not encode).
    pub composing: bool,
}

// ---------------------------------------------------------------------------
// Opaque handles.
// ---------------------------------------------------------------------------

/// The app object. Owns the runtime config (callbacks + userdata) shared by its
/// surfaces. In the spike this is deliberately thin: upstream's app owns the
/// event loop; here it is the callback-holder + apartment root.
///
/// Exported as opaque `qwertty_term_app_t` (a pointer) in the header.
pub struct FfiApp {
    runtime: QwerttyTermRuntimeConfig,
}

/// The surface object: wraps a `Stream<TerminalHandler>` (the engine) plus a
/// back-pointer to its app so it can fire the app's clipboard/wakeup callbacks.
///
/// Exported as opaque `qwertty_term_surface_t` (a pointer) in the header.
pub struct FfiSurface {
    stream: Stream<TerminalHandler>,
    // Raw pointer, not `&App`: the C caller owns lifetime ordering (surface
    // must be freed before its app). Mirrors upstream, where a surface holds an
    // app handle the embedder promises to keep alive.
    app: *const FfiApp,
}

impl FfiSurface {
    /// Fire the app's write-clipboard callback with a raw (base64) OSC body.
    fn fire_clipboard(&self, kind: u8, data: &str) {
        // SAFETY: `app` was set from a live `*const FfiApp` at construction and
        // the caller contract requires the app to outlive the surface.
        let Some(app) = (unsafe { self.app.as_ref() }) else {
            return;
        };
        let Some(cb) = app.runtime.write_clipboard_cb else {
            return;
        };
        let selection = if kind == b'p' {
            QwerttyTermClipboard::Selection
        } else {
            QwerttyTermClipboard::Standard
        };
        // NUL-terminate for the C callback. `data` is base64 (or empty), so it
        // contains no interior NULs.
        let cstring = std::ffi::CString::new(data).unwrap_or_default();
        cb(app.runtime.userdata, selection, cstring.as_ptr());
    }
}

// ---------------------------------------------------------------------------
// catch_unwind helpers.
// ---------------------------------------------------------------------------

/// Run `f`, converting any panic into `QWERTTY_TERM_PANIC`. Used by fns that
/// return a result code.
fn guard_result(f: impl FnOnce() -> QwerttyTermResult) -> QwerttyTermResult {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => QwerttyTermResult::Panic,
    }
}

/// Run `f`, converting any panic into a null pointer. Used by constructors.
fn guard_ptr<T>(f: impl FnOnce() -> *mut T) -> *mut T {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(p) => p,
        Err(_) => std::ptr::null_mut(),
    }
}

/// Run `f`, swallowing any panic. Used by void fns (free, tick).
fn guard_void(f: impl FnOnce()) {
    let _ = catch_unwind(AssertUnwindSafe(f));
}

// ---------------------------------------------------------------------------
// Global init.
// ---------------------------------------------------------------------------

/// One-time global initialization. Mirrors upstream `ghostty_init`. In the
/// spike there is no global state to set up (no logging/allocator wiring yet),
/// so this is a no-op that exists to lock the call ordering into the ABI.
/// Returns [`QWERTTY_TERM_SUCCESS`].
#[unsafe(no_mangle)]
pub extern "C" fn qwertty_term_init() -> QwerttyTermResult {
    guard_result(|| QwerttyTermResult::Success)
}

// ---------------------------------------------------------------------------
// App lifecycle.
// ---------------------------------------------------------------------------

/// Create a new app from a runtime config. Returns NULL on a NULL `config` or a
/// caught panic. The returned handle must be freed with [`qwertty_term_app_free`]
/// after all its surfaces are freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qwertty_term_app_new(
    config: *const QwerttyTermRuntimeConfig,
) -> *mut FfiApp {
    guard_ptr(|| {
        if config.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: non-null checked; caller guarantees a valid, aligned struct.
        let runtime = unsafe { *config };
        Box::into_raw(Box::new(FfiApp { runtime }))
    })
}

/// Advance the app one tick. In the spike the engine is synchronous, so this is
/// a no-op stand-in for upstream's event-loop pump; it exists so the Swift
/// driver's run loop can call it. NULL is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qwertty_term_app_tick(app: *mut FfiApp) {
    guard_void(|| {
        let _ = app;
    });
}

/// Free an app. Must be called after all its surfaces are freed. NULL is a
/// no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qwertty_term_app_free(app: *mut FfiApp) {
    guard_void(|| {
        if !app.is_null() {
            // SAFETY: `app` came from `Box::into_raw` in `qwertty_term_app_new`
            // and is freed exactly once (caller contract).
            drop(unsafe { Box::from_raw(app) });
        }
    });
}

// ---------------------------------------------------------------------------
// Surface lifecycle.
// ---------------------------------------------------------------------------

/// Create a surface on `app` from a surface config. Returns NULL on a NULL
/// arg, an invalid (zero) grid size, or a caught panic.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qwertty_term_surface_new(
    app: *mut FfiApp,
    config: *const QwerttyTermSurfaceConfig,
) -> *mut FfiSurface {
    guard_ptr(|| {
        if app.is_null() || config.is_null() {
            return std::ptr::null_mut();
        }
        // SAFETY: non-null checked.
        let cfg = unsafe { *config };
        if cfg.cols == 0 || cfg.rows == 0 {
            return std::ptr::null_mut();
        }
        let terminal = Terminal::new(TerminalOptions {
            cols: cfg.cols,
            rows: cfg.rows,
            max_scrollback: cfg.max_scrollback,
            ..TerminalOptions::default()
        });
        let stream = Stream::new(TerminalHandler::new(terminal));
        Box::into_raw(Box::new(FfiSurface {
            stream,
            app: app as *const FfiApp,
        }))
    })
}

/// Free a surface. NULL is a no-op.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qwertty_term_surface_free(surface: *mut FfiSurface) {
    guard_void(|| {
        if !surface.is_null() {
            // SAFETY: from `Box::into_raw`, freed once (caller contract).
            drop(unsafe { Box::from_raw(surface) });
        }
    });
}

// ---------------------------------------------------------------------------
// PTY bytes in.
// ---------------------------------------------------------------------------

/// Feed raw PTY bytes (what a shell would print) into the surface's engine.
/// This is the path the eventual PTY reader thread's bytes take. After
/// processing, any OSC 52 clipboard write is fired via the app's callback, and
/// any engine reply bytes are queued internally (drain via
/// [`qwertty_term_surface_take_pty_reply`], not required by the spike driver).
///
/// Returns [`QwerttyTermResult::NullArgument`] on NULL, else `Success`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qwertty_term_surface_write_pty_bytes(
    surface: *mut FfiSurface,
    data: *const u8,
    len: usize,
) -> QwerttyTermResult {
    guard_result(|| {
        if surface.is_null() || (data.is_null() && len != 0) {
            return QwerttyTermResult::NullArgument;
        }
        // SAFETY: non-null checked; caller guarantees `data[..len]` is valid.
        let surface = unsafe { &mut *surface };
        let bytes = if len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(data, len) }
        };
        surface.stream.feed(bytes);
        drain_side_effects(surface);
        QwerttyTermResult::Success
    })
}

// ---------------------------------------------------------------------------
// Key event in.
// ---------------------------------------------------------------------------

/// Send a key event to the surface. The event is encoded to PTY bytes via
/// [`qwertty_term_input`] and fed straight back into the engine (mirroring how a
/// real surface routes key output to its PTY, then the PTY echoes it back). Any
/// resulting clipboard side effects are fired.
///
/// Spike scope: text-producing keys (via `text` or `unshifted_codepoint`) use
/// the encoder's printable path. Returns `NullArgument` on NULL args.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qwertty_term_surface_key(
    surface: *mut FfiSurface,
    event: QwerttyTermInputKey,
) -> QwerttyTermResult {
    guard_result(|| {
        if surface.is_null() {
            return QwerttyTermResult::NullArgument;
        }
        // SAFETY: non-null checked.
        let surface = unsafe { &mut *surface };

        // Decode the text pointer (borrowed for this call only).
        let text = if event.text.is_null() {
            String::new()
        } else {
            // SAFETY: caller guarantees a NUL-terminated string valid for the
            // call. Lossy so an invalid UTF-8 pointer can't panic the boundary.
            unsafe { std::ffi::CStr::from_ptr(event.text) }
                .to_string_lossy()
                .into_owned()
        };

        if event.composing {
            // Mid dead-key composition: no PTY output (mirrors upstream).
            return QwerttyTermResult::Success;
        }

        let mods = decode_mods(event.mods);

        // Printable-text path: a key that generated plain (non-control) text
        // writes that text straight to the PTY, exactly as upstream splits
        // `ghostty_surface_text` from `ghostty_surface_key`. The legacy key
        // *encoder* deliberately handles only special keys + control combos
        // (see the friction note in docs/analysis/ffi-spike.md), so printable
        // text must not be routed through it.
        let is_plain_text = !text.is_empty()
            && !mods.ctrl
            && !mods.alt
            && !mods.super_
            && !text.chars().any(|c| (c as u32) < 0x20 || c as u32 == 0x7f);

        let bytes = if is_plain_text {
            text.into_bytes()
        } else {
            let key_event = KeyEvent {
                action: match event.action {
                    QwerttyTermInputAction::Release => InputAction::Release,
                    QwerttyTermInputAction::Press => InputAction::Press,
                    QwerttyTermInputAction::Repeat => InputAction::Repeat,
                },
                // Map the unshifted codepoint back to a `Key` for the special
                // keys the legacy encoder recognizes; unmapped codepoints stay
                // `Unidentified` (encoder yields nothing, which is correct for
                // the spike's scope).
                key: key_from_codepoint(event.unshifted_codepoint),
                mods,
                consumed_mods: Mods::default(),
                composing: false,
                utf8: text,
                unshifted_codepoint: event.unshifted_codepoint,
            };
            let opts = EncodeOptions {
                kitty_flags: KittyFlags::DISABLED,
                ..EncodeOptions::default()
            };
            key_encode::encode(&key_event, &opts)
        };

        if !bytes.is_empty() {
            surface.stream.feed(&bytes);
            drain_side_effects(surface);
        }
        QwerttyTermResult::Success
    })
}

// ---------------------------------------------------------------------------
// Screen text out (pre-M3 stand-in for surface_draw).
// ---------------------------------------------------------------------------

/// Read the surface's visible screen as UTF-8 text (rows joined by `\n`,
/// trailing blanks on each row trimmed). Pre-M3 stand-in for `surface_draw`.
///
/// Buffer convention (mirrors upstream `*_text` / vt-diff's formatter):
/// - Call with `buf == NULL` to get the required byte length in `*out_len`.
/// - Call again with a buffer of at least that size. On success `*out_len` is
///   the number of bytes written (not counting a trailing NUL, which is written
///   only if it fits).
/// - Returns [`QwerttyTermResult::OutOfSpace`] (and sets `*out_len` to the need)
///   if `buf` is non-NULL but `buf_len` is too small.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qwertty_term_surface_read_text(
    surface: *mut FfiSurface,
    buf: *mut c_char,
    buf_len: usize,
    out_len: *mut usize,
) -> QwerttyTermResult {
    guard_result(|| {
        if surface.is_null() || out_len.is_null() {
            return QwerttyTermResult::NullArgument;
        }
        // SAFETY: non-null checked.
        let surface = unsafe { &mut *surface };
        let text = screen_text(&surface.stream.handler.terminal);
        let needed = text.len();

        // SAFETY: out_len non-null checked.
        unsafe { *out_len = needed };

        if buf.is_null() {
            // Size query.
            return QwerttyTermResult::Success;
        }
        if buf_len < needed {
            return QwerttyTermResult::OutOfSpace;
        }
        // SAFETY: buf is non-null and buf_len >= needed (checked).
        unsafe {
            std::ptr::copy_nonoverlapping(text.as_ptr(), buf as *mut u8, needed);
            // Write a trailing NUL if there's room, for C-string convenience.
            if buf_len > needed {
                *buf.add(needed) = 0;
            }
        }
        QwerttyTermResult::Success
    })
}

// ---------------------------------------------------------------------------
// PTY reply drain (engine -> pty; not exercised by the spike driver but part
// of the round-trip contract, so exposed and tested).
// ---------------------------------------------------------------------------

/// Drain engine reply bytes (DSR/DA/CPR/DECRQSS answers the engine queued for
/// the PTY). Same buffer convention as [`qwertty_term_surface_read_text`]. The
/// bytes are removed from the queue only when `buf` is non-NULL and large
/// enough (a size query with `buf == NULL` does not consume).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn qwertty_term_surface_take_pty_reply(
    surface: *mut FfiSurface,
    buf: *mut u8,
    buf_len: usize,
    out_len: *mut usize,
) -> QwerttyTermResult {
    guard_result(|| {
        if surface.is_null() || out_len.is_null() {
            return QwerttyTermResult::NullArgument;
        }
        // SAFETY: non-null checked.
        let surface = unsafe { &mut *surface };
        let pending = &surface.stream.handler.output;
        let needed = pending.len();
        // SAFETY: out_len non-null checked.
        unsafe { *out_len = needed };

        if buf.is_null() {
            return QwerttyTermResult::Success; // size query, no consume
        }
        if buf_len < needed {
            return QwerttyTermResult::OutOfSpace;
        }
        let reply = surface.stream.handler.take_output();
        // SAFETY: buf non-null and buf_len >= needed.
        unsafe {
            std::ptr::copy_nonoverlapping(reply.as_ptr(), buf, reply.len());
        }
        QwerttyTermResult::Success
    })
}

// ---------------------------------------------------------------------------
// Internal helpers.
// ---------------------------------------------------------------------------

/// Fire any pending clipboard side effect after feeding the engine. The reply
/// queue is left for [`qwertty_term_surface_take_pty_reply`] to drain.
fn drain_side_effects(surface: &mut FfiSurface) {
    if let Some((kind, data)) = surface.stream.handler.take_clipboard() {
        surface.fire_clipboard(kind, &data);
    }
}

/// Decode the C mods bitfield into a `qwertty_term_input::Mods`.
fn decode_mods(mods: QwerttyTermInputMods) -> Mods {
    Mods {
        shift: mods.bits & 0b0001 != 0,
        ctrl: mods.bits & 0b0010 != 0,
        alt: mods.bits & 0b0100 != 0,
        super_: mods.bits & 0b1000 != 0,
        ..Mods::default()
    }
}

/// Map a control codepoint to the `Key` variant the legacy encoder recognizes.
/// The spike only needs the handful of special keys a driver might send
/// (Enter/Tab/Escape/Backspace); everything else is `Unidentified` (the
/// encoder yields nothing, matching the pre-full-legacy-port seam).
fn key_from_codepoint(cp: u32) -> Key {
    match cp {
        0x0d => Key::Enter,
        0x09 => Key::Tab,
        0x1b => Key::Escape,
        0x7f | 0x08 => Key::Backspace,
        _ => Key::Unidentified,
    }
}

/// Render the terminal's visible active area to a plain-text string (rows
/// `\n`-joined, per-row trailing spaces trimmed, no trailing newline). This is
/// the pre-M3 text dump; a real `surface_draw` replaces it post-M3.
fn screen_text(terminal: &Terminal) -> String {
    let snap = terminal.snapshot();
    let active = &snap.all_rows[snap.active_start..];
    let mut out = String::new();
    for (i, row) in active.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let mut line = String::new();
        for cell in &row.cells {
            if cell.is_spacer() {
                continue;
            }
            line.push(cell.ch);
            for &c in &cell.combining {
                line.push(c);
            }
        }
        out.push_str(line.trim_end());
    }
    out
}

#[cfg(test)]
mod tests;
