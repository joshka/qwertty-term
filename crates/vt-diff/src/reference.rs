//! Safe wrapper around the libghostty-vt reference terminal.

use std::ffi::c_void;

use crate::ffi;
use crate::oracle::{CursorPos, Oracle, normalize_screen_text};

/// The Zig-built libghostty-vt terminal, used as the reference oracle in
/// differential tests.
///
/// Single-threaded by design: the C API has no internal locking and the
/// handle is only touched from the owning thread (`!Send`/`!Sync` is
/// enforced by the raw pointer field).
///
/// Wires `GHOSTTY_TERMINAL_OPT_WRITE_PTY` so reply bytes (DECRQM, DSR/DA,
/// kitty-keyboard query, DECRQSS, ...) are captured in `reply_buf` instead of
/// being silently dropped (the library's documented default with no
/// callback registered). `reply_buf` is heap-allocated and never moved after
/// construction, so the raw pointer handed to the C callback as userdata
/// stays valid for the terminal's lifetime.
pub struct ReferenceTerminal {
    handle: ffi::GhosttyTerminal,
    // The `Box` is load-bearing, not redundant: the C callback's userdata is
    // a raw pointer to the `Vec<u8>` handle itself (not its buffer), so the
    // handle needs a stable heap address that survives `ReferenceTerminal`
    // being moved (e.g. returned by value from `with_scrollback`). A bare
    // `Vec<u8>` field would move with the struct, invalidating the pointer
    // registered in `install_write_pty`.
    #[allow(clippy::box_collection)]
    reply_buf: Box<Vec<u8>>,
}

/// `GhosttyTerminalWritePtyFn`: appends the reply bytes to the `Vec<u8>`
/// pointed to by `userdata`. `data` is only valid for the call's duration,
/// so it is copied immediately.
unsafe extern "C" fn write_pty_trampoline(
    _terminal: ffi::GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
) {
    // SAFETY: userdata is the `&mut Vec<u8>` behind `ReferenceTerminal::reply_buf`,
    // set up in `with_scrollback` and valid until the terminal is freed;
    // `data`/`len` describe a slice valid for this call.
    unsafe {
        let buf = &mut *(userdata as *mut Vec<u8>);
        buf.extend_from_slice(std::slice::from_raw_parts(data, len));
    }
}

impl ReferenceTerminal {
    /// Create a terminal with the given grid size and no scrollback.
    ///
    /// Zero scrollback keeps the formatter's screen dump identical to the
    /// visible grid, which is the comparison space of the harness. Use
    /// [`ReferenceTerminal::with_scrollback`] when history matters.
    pub fn new(cols: u16, rows: u16) -> Self {
        Self::with_scrollback(cols, rows, 0)
    }

    /// Create a terminal with the given grid size and scrollback capacity.
    ///
    /// Despite the C header saying "lines", `max_scrollback` is a byte
    /// budget for scrollback page memory (rounded up to the page size);
    /// zero keeps no history. See docs/analysis/libghostty-vt-c-api.md.
    pub fn with_scrollback(cols: u16, rows: u16, max_scrollback: usize) -> Self {
        assert!(cols > 0 && rows > 0, "grid dimensions must be non-zero");
        let options = ffi::GhosttyTerminalOptions {
            cols,
            rows,
            max_scrollback,
        };
        let mut handle: ffi::GhosttyTerminal = std::ptr::null_mut();
        // SAFETY: null allocator selects the default allocator; `handle`
        // is a valid out-pointer; options are validated above.
        let result = unsafe { ffi::ghostty_terminal_new(std::ptr::null(), &mut handle, options) };
        assert_eq!(
            result,
            ffi::GHOSTTY_SUCCESS,
            "ghostty_terminal_new failed: {result}"
        );
        assert!(!handle.is_null());
        let mut terminal = Self {
            handle,
            reply_buf: Box::new(Vec::new()),
        };
        terminal.install_write_pty();
        terminal
    }

    /// Register `write_pty_trampoline` with `reply_buf`'s address as
    /// userdata. Must run after `reply_buf` is in its final heap location
    /// (i.e. after the `Box` is created) and before any `feed`.
    fn install_write_pty(&mut self) {
        let userdata: *mut c_void = (&mut *self.reply_buf as *mut Vec<u8>) as *mut c_void;
        // SAFETY: handle is valid; the trampoline matches
        // `GhosttyTerminalWritePtyFn`'s signature; userdata points at the
        // boxed `Vec<u8>` which outlives the terminal (freed in `Drop`
        // after `ghostty_terminal_free`, which is the last use of the
        // handle).
        let result = unsafe {
            ffi::ghostty_terminal_set(
                self.handle,
                ffi::GHOSTTY_TERMINAL_OPT_WRITE_PTY,
                write_pty_trampoline as *const () as *const c_void,
            )
        };
        assert_eq!(
            result,
            ffi::GHOSTTY_SUCCESS,
            "ghostty_terminal_set(WRITE_PTY) failed: {result}"
        );
        // The userdata option is separate from the callback option; set it
        // too so the trampoline receives the right pointer.
        let result = unsafe {
            ffi::ghostty_terminal_set(self.handle, ffi::GHOSTTY_TERMINAL_OPT_USERDATA, userdata)
        };
        assert_eq!(
            result,
            ffi::GHOSTTY_SUCCESS,
            "ghostty_terminal_set(USERDATA) failed: {result}"
        );
    }

    /// Accumulated reply bytes (DSR/DA/CPR/DECRQM/DECRQSS/kitty-keyboard
    /// query/...), in order, since construction. The Rust mirror of
    /// [`RustTerminal::output`](crate::RustTerminal::output).
    pub fn output(&self) -> &[u8] {
        &self.reply_buf
    }

    /// Resize the grid. Cell pixel size is fixed at 1x1; the harness never
    /// exercises pixel-based reports.
    pub fn resize(&mut self, cols: u16, rows: u16) {
        // SAFETY: handle is valid for the lifetime of self.
        let result = unsafe { ffi::ghostty_terminal_resize(self.handle, cols, rows, 1, 1) };
        assert_eq!(
            result,
            ffi::GHOSTTY_SUCCESS,
            "ghostty_terminal_resize failed: {result}"
        );
    }

    fn get_u16(&self, key: std::ffi::c_int) -> u16 {
        let mut value: u16 = 0;
        // SAFETY: handle is valid; `key` selects a u16-typed datum and
        // `value` is a valid out-pointer of that type.
        let result =
            unsafe { ffi::ghostty_terminal_get(self.handle, key, &raw mut value as *mut c_void) };
        assert_eq!(
            result,
            ffi::GHOSTTY_SUCCESS,
            "ghostty_terminal_get({key}) failed: {result}"
        );
        value
    }

    /// Read a `bool *`-typed datum (1 byte). Using [`get_u16`](Self::get_u16)
    /// here would read an adjacent byte as garbage.
    fn get_bool(&self, key: std::ffi::c_int) -> bool {
        let mut value: bool = false;
        // SAFETY: handle is valid; `key` selects a bool-typed datum and
        // `value` is a valid out-pointer of that type.
        let result =
            unsafe { ffi::ghostty_terminal_get(self.handle, key, &raw mut value as *mut c_void) };
        assert_eq!(
            result,
            ffi::GHOSTTY_SUCCESS,
            "ghostty_terminal_get({key}) failed: {result}"
        );
        value
    }

    /// Read an enum-typed datum (ghostty C enums are `int`-width).
    fn get_enum(&self, key: std::ffi::c_int) -> std::ffi::c_int {
        let mut value: std::ffi::c_int = 0;
        // SAFETY: handle is valid; `key` selects an int-width enum datum and
        // `value` is a valid out-pointer of that type.
        let result =
            unsafe { ffi::ghostty_terminal_get(self.handle, key, &raw mut value as *mut c_void) };
        assert_eq!(
            result,
            ffi::GHOSTTY_SUCCESS,
            "ghostty_terminal_get({key}) failed: {result}"
        );
        value
    }

    /// Raw plain-text dump of the active screen (scrollback + visible grid)
    /// as produced by the libghostty-vt formatter with `trim = true`:
    /// trailing whitespace is trimmed from non-blank lines, but trailing
    /// blank lines for empty grid rows may be present.
    pub fn raw_text(&self) -> String {
        self.format_with_emit(ffi::GHOSTTY_FORMATTER_FORMAT_PLAIN)
    }

    /// Styled dump of the active screen via the libghostty-vt **VT** formatter:
    /// the screen re-emitted as VT sequences INCLUDING SGR attributes. Mirror of
    /// [`RustTerminal::formatter_vt_text`](crate::RustTerminal::formatter_vt_text).
    pub fn raw_text_vt(&self) -> String {
        self.format_with_emit(ffi::GHOSTTY_FORMATTER_FORMAT_VT)
    }

    /// Run the libghostty-vt terminal formatter with the given emit format
    /// (`trim = true`) and return its output.
    fn format_with_emit(&self, emit: std::ffi::c_int) -> String {
        let options = ffi::GhosttyFormatterTerminalOptions {
            emit,
            trim: true,
            ..Default::default()
        };

        let mut formatter: ffi::GhosttyFormatter = std::ptr::null_mut();
        // SAFETY: handle is valid and outlives the formatter (freed below);
        // options is a properly sized struct with a null selection.
        let result = unsafe {
            ffi::ghostty_formatter_terminal_new(
                std::ptr::null(),
                &mut formatter,
                self.handle,
                options,
            )
        };
        assert_eq!(
            result,
            ffi::GHOSTTY_SUCCESS,
            "ghostty_formatter_terminal_new failed: {result}"
        );

        // Query the required size, then format into an exactly-sized buffer.
        let mut needed: usize = 0;
        // SAFETY: NULL buffer queries the required size into `needed`.
        let result = unsafe {
            ffi::ghostty_formatter_format_buf(formatter, std::ptr::null_mut(), 0, &mut needed)
        };
        let text = if result == ffi::GHOSTTY_SUCCESS && needed == 0 {
            String::new()
        } else {
            assert_eq!(
                result,
                ffi::GHOSTTY_OUT_OF_SPACE,
                "size query returned unexpected result: {result}"
            );
            let mut buf = vec![0u8; needed];
            let mut written: usize = 0;
            // SAFETY: buf has `needed` writable bytes; the terminal is not
            // mutated between the size query and this call.
            let result = unsafe {
                ffi::ghostty_formatter_format_buf(
                    formatter,
                    buf.as_mut_ptr(),
                    buf.len(),
                    &mut written,
                )
            };
            assert_eq!(
                result,
                ffi::GHOSTTY_SUCCESS,
                "ghostty_formatter_format_buf failed: {result}"
            );
            buf.truncate(written);
            String::from_utf8(buf).expect("formatter output is UTF-8")
        };

        // SAFETY: formatter is valid and not used after this call.
        unsafe { ffi::ghostty_formatter_free(formatter) };
        text
    }
}

impl Oracle for ReferenceTerminal {
    fn feed(&mut self, bytes: &[u8]) {
        // SAFETY: handle is valid; data/len describe a live slice. The API
        // never fails on malformed input.
        unsafe { ffi::ghostty_terminal_vt_write(self.handle, bytes.as_ptr(), bytes.len()) };
    }

    fn text(&self) -> String {
        normalize_screen_text(&self.raw_text())
    }

    fn styled_text(&self) -> String {
        self.raw_text_vt()
    }

    fn cursor(&self) -> CursorPos {
        CursorPos {
            row: self.get_u16(ffi::GHOSTTY_TERMINAL_DATA_CURSOR_Y),
            col: self.get_u16(ffi::GHOSTTY_TERMINAL_DATA_CURSOR_X),
        }
    }

    fn term_state(&self) -> crate::TermState {
        crate::TermState {
            pending_wrap: self.get_bool(ffi::GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP),
            alt_screen: self.get_enum(ffi::GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN)
                == ffi::GHOSTTY_TERMINAL_SCREEN_ALTERNATE,
            cursor_visible: self.get_bool(ffi::GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE),
        }
    }
}

impl Drop for ReferenceTerminal {
    fn drop(&mut self) {
        // SAFETY: handle is valid; nothing borrows from it (formatters are
        // created and freed within single calls).
        unsafe { ffi::ghostty_terminal_free(self.handle) };
    }
}
