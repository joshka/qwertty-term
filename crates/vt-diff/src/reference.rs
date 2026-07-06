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
pub struct ReferenceTerminal {
    handle: ffi::GhosttyTerminal,
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
        Self { handle }
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

    /// Raw plain-text dump of the active screen (scrollback + visible grid)
    /// as produced by the libghostty-vt formatter with `trim = true`:
    /// trailing whitespace is trimmed from non-blank lines, but trailing
    /// blank lines for empty grid rows may be present.
    pub fn raw_text(&self) -> String {
        let options = ffi::GhosttyFormatterTerminalOptions {
            emit: ffi::GHOSTTY_FORMATTER_FORMAT_PLAIN,
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
            String::from_utf8(buf).expect("plain-text formatter output is UTF-8")
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

    fn cursor(&self) -> CursorPos {
        CursorPos {
            row: self.get_u16(ffi::GHOSTTY_TERMINAL_DATA_CURSOR_Y),
            col: self.get_u16(ffi::GHOSTTY_TERMINAL_DATA_CURSOR_X),
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
