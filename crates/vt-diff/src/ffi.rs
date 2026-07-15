//! Hand-written FFI declarations for the subset of the libghostty-vt C API
//! used by the differential harness.
//!
//! Source of truth: `include/ghostty/vt/{terminal,formatter,types}.h` in the
//! ghostty checkout (commit 77190bd02; the C API layouts are unchanged from the
//! prior `2da015cd6` pin — verified by the differential harness). Struct layouts
//! below must match the C definitions exactly; all enums are C `int` on the Zig side.
//!
//! See `docs/analysis/libghostty-vt-c-api.md` for API semantics, ownership
//! rules, and gotchas.

#![allow(dead_code)]

use std::ffi::{c_int, c_void};

/// `GhosttyResult` (types.h). Note: `zig-out/include/ghostty/vt/result.h` is
/// a stale artifact from an older build; `types.h` is authoritative.
pub const GHOSTTY_SUCCESS: c_int = 0;
pub const GHOSTTY_OUT_OF_MEMORY: c_int = -1;
pub const GHOSTTY_INVALID_VALUE: c_int = -2;
pub const GHOSTTY_OUT_OF_SPACE: c_int = -3;
pub const GHOSTTY_NO_VALUE: c_int = -4;

/// `GhosttyTerminalData` (terminal.h) — keys for `ghostty_terminal_get`.
pub const GHOSTTY_TERMINAL_DATA_COLS: c_int = 1;
pub const GHOSTTY_TERMINAL_DATA_ROWS: c_int = 2;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_X: c_int = 3;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_Y: c_int = 4;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_PENDING_WRAP: c_int = 5;
pub const GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN: c_int = 6;
pub const GHOSTTY_TERMINAL_DATA_CURSOR_VISIBLE: c_int = 7;
pub const GHOSTTY_TERMINAL_DATA_MOUSE_TRACKING: c_int = 11;
pub const GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS: c_int = 15;

/// `GhosttyTerminalScreen` (terminal.h) — value of `ACTIVE_SCREEN`.
pub const GHOSTTY_TERMINAL_SCREEN_ALTERNATE: c_int = 1;

/// `GhosttyFormatterFormat` (types.h).
pub const GHOSTTY_FORMATTER_FORMAT_PLAIN: c_int = 0;
pub const GHOSTTY_FORMATTER_FORMAT_VT: c_int = 1;
pub const GHOSTTY_FORMATTER_FORMAT_HTML: c_int = 2;

/// `GhosttyTerminalOption` (terminal.h) — keys for `ghostty_terminal_set`.
pub const GHOSTTY_TERMINAL_OPT_USERDATA: c_int = 0;
/// Callback invoked when the terminal needs to write reply bytes back to the
/// pty (DECRQM/DSR/DA/kitty-keyboard-query/etc.). Round-2 addition: wires up
/// the reply channel the harness previously left unregistered.
pub const GHOSTTY_TERMINAL_OPT_WRITE_PTY: c_int = 1;

/// Opaque `struct GhosttyTerminalImpl` behind `GhosttyTerminal`.
#[repr(C)]
pub struct GhosttyTerminalImpl {
    _opaque: [u8; 0],
}
/// `GhosttyTerminal` — opaque terminal handle.
pub type GhosttyTerminal = *mut GhosttyTerminalImpl;

/// Opaque `struct GhosttyFormatterImpl` behind `GhosttyFormatter`.
#[repr(C)]
pub struct GhosttyFormatterImpl {
    _opaque: [u8; 0],
}
/// `GhosttyFormatter` — opaque formatter handle.
pub type GhosttyFormatter = *mut GhosttyFormatterImpl;

/// `GhosttyAllocator` (allocator.h). We only ever pass NULL (default
/// allocator), so the struct body is left opaque.
#[repr(C)]
pub struct GhosttyAllocator {
    _opaque: [u8; 0],
}

/// `GhosttySelection` (selection.h). Only passed as a NULL pointer here.
#[repr(C)]
pub struct GhosttySelection {
    _opaque: [u8; 0],
}

/// `GhosttyTerminalOptions` (terminal.h). Passed **by value** to
/// `ghostty_terminal_new`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyTerminalOptions {
    /// Width in cells; must be > 0.
    pub cols: u16,
    /// Height in cells; must be > 0.
    pub rows: u16,
    /// Maximum scrollback lines retained.
    pub max_scrollback: usize,
}

/// `GhosttyFormatterScreenExtra` (formatter.h). Sized struct: `size` must be
/// `size_of::<Self>()`.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyFormatterScreenExtra {
    pub size: usize,
    pub cursor: bool,
    pub style: bool,
    pub hyperlink: bool,
    pub protection: bool,
    pub kitty_keyboard: bool,
    pub charsets: bool,
}

impl Default for GhosttyFormatterScreenExtra {
    fn default() -> Self {
        Self {
            size: size_of::<Self>(),
            cursor: false,
            style: false,
            hyperlink: false,
            protection: false,
            kitty_keyboard: false,
            charsets: false,
        }
    }
}

/// `GhosttyFormatterTerminalExtra` (formatter.h). Sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyFormatterTerminalExtra {
    pub size: usize,
    pub palette: bool,
    pub modes: bool,
    pub scrolling_region: bool,
    pub tabstops: bool,
    pub pwd: bool,
    pub keyboard: bool,
    pub screen: GhosttyFormatterScreenExtra,
}

impl Default for GhosttyFormatterTerminalExtra {
    fn default() -> Self {
        Self {
            size: size_of::<Self>(),
            palette: false,
            modes: false,
            scrolling_region: false,
            tabstops: false,
            pwd: false,
            keyboard: false,
            screen: GhosttyFormatterScreenExtra::default(),
        }
    }
}

/// `GhosttyFormatterTerminalOptions` (formatter.h). Passed **by value** to
/// `ghostty_formatter_terminal_new`. Sized struct.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct GhosttyFormatterTerminalOptions {
    pub size: usize,
    /// One of the `GHOSTTY_FORMATTER_FORMAT_*` constants.
    pub emit: c_int,
    /// Unwrap soft-wrapped lines.
    pub unwrap: bool,
    /// Trim trailing whitespace on non-blank lines.
    pub trim: bool,
    pub extra: GhosttyFormatterTerminalExtra,
    /// Restrict output to a selection; NULL formats the whole screen.
    pub selection: *const GhosttySelection,
}

impl Default for GhosttyFormatterTerminalOptions {
    fn default() -> Self {
        Self {
            size: size_of::<Self>(),
            emit: GHOSTTY_FORMATTER_FORMAT_PLAIN,
            unwrap: false,
            trim: false,
            extra: GhosttyFormatterTerminalExtra::default(),
            selection: std::ptr::null(),
        }
    }
}

/// `GhosttyTerminalWritePtyFn` (terminal.h). Called synchronously during
/// `ghostty_terminal_vt_write` whenever the reference engine produces reply
/// bytes (DECRQM `$y`, DSR/CPR, kitty-keyboard query, DECRQSS, etc.). `data`
/// is only valid for the duration of the call.
pub type GhosttyTerminalWritePtyFn = unsafe extern "C" fn(
    terminal: GhosttyTerminal,
    userdata: *mut c_void,
    data: *const u8,
    len: usize,
);

unsafe extern "C" {
    /// Create a terminal. `allocator` may be NULL for the default allocator.
    pub fn ghostty_terminal_new(
        allocator: *const GhosttyAllocator,
        terminal: *mut GhosttyTerminal,
        options: GhosttyTerminalOptions,
    ) -> c_int;

    /// Free a terminal. NULL is a no-op.
    pub fn ghostty_terminal_free(terminal: GhosttyTerminal);

    /// Set an option on the terminal (callbacks + userdata). `value` is
    /// passed directly for pointer-typed options (callbacks, userdata);
    /// NULL clears the option to its default.
    pub fn ghostty_terminal_set(
        terminal: GhosttyTerminal,
        option: c_int,
        value: *const c_void,
    ) -> c_int;

    /// Feed raw VT bytes. Never fails; malformed input is logged internally.
    pub fn ghostty_terminal_vt_write(terminal: GhosttyTerminal, data: *const u8, len: usize);

    /// Resize the grid (primary screen reflows).
    pub fn ghostty_terminal_resize(
        terminal: GhosttyTerminal,
        cols: u16,
        rows: u16,
        cell_width_px: u32,
        cell_height_px: u32,
    ) -> c_int;

    /// Read typed data; `out` must point at the type documented for `data`.
    pub fn ghostty_terminal_get(terminal: GhosttyTerminal, data: c_int, out: *mut c_void) -> c_int;

    /// Create a formatter over the terminal's active screen. The terminal
    /// must outlive the formatter.
    pub fn ghostty_formatter_terminal_new(
        allocator: *const GhosttyAllocator,
        formatter: *mut GhosttyFormatter,
        terminal: GhosttyTerminal,
        options: GhosttyFormatterTerminalOptions,
    ) -> c_int;

    /// Format current terminal state into `buf`. With `buf == NULL`, returns
    /// `GHOSTTY_OUT_OF_SPACE` and writes the required size to `out_written`.
    pub fn ghostty_formatter_format_buf(
        formatter: GhosttyFormatter,
        buf: *mut u8,
        buf_len: usize,
        out_written: *mut usize,
    ) -> c_int;

    /// Free a formatter. NULL is a no-op.
    pub fn ghostty_formatter_free(formatter: GhosttyFormatter);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Layouts must match the C structs compiled on this target
    /// (LP64 macOS: size_t = 8, int = 4, bool = 1).
    #[test]
    fn struct_layouts_match_c() {
        assert_eq!(size_of::<GhosttyTerminalOptions>(), 16);
        assert_eq!(size_of::<GhosttyFormatterScreenExtra>(), 16);
        assert_eq!(size_of::<GhosttyFormatterTerminalExtra>(), 32);
        assert_eq!(size_of::<GhosttyFormatterTerminalOptions>(), 56);
        assert_eq!(align_of::<GhosttyFormatterTerminalOptions>(), 8);
    }
}
