//! Rust-side tests that drive the C ABI in-process, the way `vt-diff` binds
//! and exercises libghostty-vt. These call the `extern "C"` entry points
//! directly (they are `pub` in this crate), so they cover the real ABI surface
//! — pointer handling, buffer conventions, panic guards — not a Rust-only
//! shortcut.

use super::*;
use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

// --- clipboard callback capture -------------------------------------------
//
// The C ABI hands the callback a raw `*mut c_void` userdata; the spike driver
// (and these tests) point it at a capture cell. `extern "C" fn` cannot be a
// closure, so we route through a thread-local, which is sound here because the
// apartment is single-threaded (see the threading contract).

thread_local! {
    static CLIPBOARD_CAPTURE: RefCell<Vec<(GhosttyRsClipboard, String)>> =
        const { RefCell::new(Vec::new()) };
}

extern "C" fn capture_clipboard(
    _userdata: *mut c_void,
    kind: GhosttyRsClipboard,
    data: *const c_char,
) {
    let s = unsafe { CStr::from_ptr(data) }
        .to_string_lossy()
        .into_owned();
    CLIPBOARD_CAPTURE.with(|c| c.borrow_mut().push((kind, s)));
}

fn make_app(clipboard: GhosttyRsWriteClipboardCb) -> *mut FfiApp {
    let config = GhosttyRsRuntimeConfig {
        userdata: std::ptr::null_mut(),
        wakeup_cb: None,
        write_clipboard_cb: clipboard,
    };
    let app = unsafe { ghostty_rs_app_new(&config) };
    assert!(!app.is_null());
    app
}

fn make_surface(app: *mut FfiApp) -> *mut FfiSurface {
    let config = GhosttyRsSurfaceConfig {
        cols: 20,
        rows: 5,
        max_scrollback: 100,
    };
    let surface = unsafe { ghostty_rs_surface_new(app, &config) };
    assert!(!surface.is_null());
    surface
}

/// Read the screen text via the two-call buffer convention.
fn read_text(surface: *mut FfiSurface) -> String {
    let mut needed: usize = 0;
    let rc = unsafe { ghostty_rs_surface_read_text(surface, std::ptr::null_mut(), 0, &mut needed) };
    assert_eq!(rc, GhosttyRsResult::Success);
    let mut buf = vec![0u8; needed + 1];
    let mut written: usize = 0;
    let rc = unsafe {
        ghostty_rs_surface_read_text(
            surface,
            buf.as_mut_ptr() as *mut c_char,
            buf.len(),
            &mut written,
        )
    };
    assert_eq!(rc, GhosttyRsResult::Success);
    assert_eq!(written, needed);
    String::from_utf8(buf[..written].to_vec()).unwrap()
}

#[test]
fn init_is_success() {
    assert_eq!(ghostty_rs_init(), GhosttyRsResult::Success);
}

#[test]
fn app_surface_lifecycle() {
    let app = make_app(None);
    let surface = make_surface(app);
    unsafe { ghostty_rs_app_tick(app) };
    unsafe { ghostty_rs_surface_free(surface) };
    unsafe { ghostty_rs_app_free(app) };
}

#[test]
fn null_handles_are_safe() {
    // Every free is a NULL no-op.
    unsafe { ghostty_rs_surface_free(std::ptr::null_mut()) };
    unsafe { ghostty_rs_app_free(std::ptr::null_mut()) };
    unsafe { ghostty_rs_app_tick(std::ptr::null_mut()) };
    // Constructors reject NULL config -> NULL handle.
    assert!(unsafe { ghostty_rs_app_new(std::ptr::null()) }.is_null());
    assert!(unsafe { ghostty_rs_surface_new(std::ptr::null_mut(), std::ptr::null()) }.is_null());
    // Write/read reject NULL surface.
    let mut out = 0usize;
    assert_eq!(
        unsafe { ghostty_rs_surface_write_pty_bytes(std::ptr::null_mut(), b"x".as_ptr(), 1) },
        GhosttyRsResult::NullArgument
    );
    assert_eq!(
        unsafe {
            ghostty_rs_surface_read_text(std::ptr::null_mut(), std::ptr::null_mut(), 0, &mut out)
        },
        GhosttyRsResult::NullArgument
    );
}

#[test]
fn zero_grid_rejected() {
    let app = make_app(None);
    let config = GhosttyRsSurfaceConfig {
        cols: 0,
        rows: 5,
        max_scrollback: 0,
    };
    assert!(unsafe { ghostty_rs_surface_new(app, &config) }.is_null());
    unsafe { ghostty_rs_app_free(app) };
}

#[test]
fn write_pty_bytes_and_read_text() {
    let app = make_app(None);
    let surface = make_surface(app);
    let bytes = b"hi\r\nthere";
    let rc = unsafe { ghostty_rs_surface_write_pty_bytes(surface, bytes.as_ptr(), bytes.len()) };
    assert_eq!(rc, GhosttyRsResult::Success);

    let text = read_text(surface);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "hi");
    assert_eq!(lines[1], "there");

    unsafe { ghostty_rs_surface_free(surface) };
    unsafe { ghostty_rs_app_free(app) };
}

#[test]
fn read_text_out_of_space() {
    let app = make_app(None);
    let surface = make_surface(app);
    let bytes = b"hello";
    unsafe { ghostty_rs_surface_write_pty_bytes(surface, bytes.as_ptr(), bytes.len()) };

    let mut needed = 0usize;
    unsafe { ghostty_rs_surface_read_text(surface, std::ptr::null_mut(), 0, &mut needed) };
    assert!(needed >= 5);

    // A too-small buffer returns OutOfSpace and still reports the need.
    let mut small = vec![0u8; 2];
    let mut written = 0usize;
    let rc = unsafe {
        ghostty_rs_surface_read_text(
            surface,
            small.as_mut_ptr() as *mut c_char,
            small.len(),
            &mut written,
        )
    };
    assert_eq!(rc, GhosttyRsResult::OutOfSpace);
    assert_eq!(written, needed);

    unsafe { ghostty_rs_surface_free(surface) };
    unsafe { ghostty_rs_app_free(app) };
}

#[test]
fn key_round_trip() {
    let app = make_app(None);
    let surface = make_surface(app);

    // Type "hi" as two printable key presses; the encoder emits the UTF-8
    // bytes, which are fed back into the engine.
    for ch in ["h", "i"] {
        let text = CString::new(ch).unwrap();
        let event = GhosttyRsInputKey {
            action: GhosttyRsInputAction::Press,
            mods: GhosttyRsInputMods { bits: 0 },
            text: text.as_ptr(),
            unshifted_codepoint: ch.chars().next().unwrap() as u32,
            composing: false,
        };
        let rc = unsafe { ghostty_rs_surface_key(surface, event) };
        assert_eq!(rc, GhosttyRsResult::Success);
    }

    let text = read_text(surface);
    assert_eq!(text.lines().next().unwrap(), "hi");

    unsafe { ghostty_rs_surface_free(surface) };
    unsafe { ghostty_rs_app_free(app) };
}

#[test]
fn clipboard_callback_fires_on_osc52() {
    CLIPBOARD_CAPTURE.with(|c| c.borrow_mut().clear());
    let app = make_app(Some(capture_clipboard));
    let surface = make_surface(app);

    // OSC 52 write to the standard clipboard ('c'): base64("hi") == "aGk=".
    let osc = b"\x1b]52;c;aGk=\x07";
    let rc = unsafe { ghostty_rs_surface_write_pty_bytes(surface, osc.as_ptr(), osc.len()) };
    assert_eq!(rc, GhosttyRsResult::Success);

    let captured = CLIPBOARD_CAPTURE.with(|c| c.borrow().clone());
    assert_eq!(captured.len(), 1);
    assert_eq!(captured[0].0, GhosttyRsClipboard::Standard);
    // Raw (still base64) per the terminal-core policy.
    assert_eq!(captured[0].1, "aGk=");

    unsafe { ghostty_rs_surface_free(surface) };
    unsafe { ghostty_rs_app_free(app) };
}

#[test]
fn pty_reply_drain_on_device_status_report() {
    let app = make_app(None);
    let surface = make_surface(app);

    // DA1 request: `ESC [ c` -> engine queues a device-attributes reply.
    let req = b"\x1b[c";
    unsafe { ghostty_rs_surface_write_pty_bytes(surface, req.as_ptr(), req.len()) };

    let mut needed = 0usize;
    unsafe { ghostty_rs_surface_take_pty_reply(surface, std::ptr::null_mut(), 0, &mut needed) };
    assert!(needed > 0, "expected a DA1 reply to be queued");

    let mut buf = vec![0u8; needed];
    let mut written = 0usize;
    let rc = unsafe {
        ghostty_rs_surface_take_pty_reply(surface, buf.as_mut_ptr(), buf.len(), &mut written)
    };
    assert_eq!(rc, GhosttyRsResult::Success);
    assert_eq!(written, needed);
    assert!(buf.starts_with(b"\x1b["));

    unsafe { ghostty_rs_surface_free(surface) };
    unsafe { ghostty_rs_app_free(app) };
}
