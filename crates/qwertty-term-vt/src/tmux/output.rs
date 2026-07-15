//! tmux format-variable parse/encode. Port of `terminal/tmux/output.zig`
//! (Ghostty `2da015cd6`). ADR 004 slice 3.
//!
//! tmux exposes pane/window/session state through format variables like
//! `#{cursor_x}`. We build a request string from a set of [`Variable`]s
//! ([`format`]) and parse the delimited response back into typed [`Value`]s
//! ([`parse_format`]).
//!
//! Upstream generates a struct type per variable-set at comptime
//! (`FormatStruct` + `parseFormatStruct`). Rust has no comptime type
//! generation, so this is the faithful *runtime* equivalent: `parse_format`
//! returns a `Vec<Value>` positionally aligned with the input `&[Variable]`.
//! Behaviour (which variables parse to bool/usize/string, the `$`/`@`/`%`
//! prefixes, and the missing/extra/format error cases) matches upstream exactly.

/// A tmux format variable we support. Port of `output.zig`'s `Variable` enum
/// (the subset relevant to control mode). Names map 1:1 to the tmux
/// `#{snake_case}` variables via [`Variable::name`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variable {
    /// 1 if pane is in alternate screen.
    AlternateOn,
    /// Saved cursor X in alternate screen.
    AlternateSavedX,
    /// Saved cursor Y in alternate screen.
    AlternateSavedY,
    /// 1 if bracketed paste mode is enabled.
    BracketedPaste,
    /// 1 if the cursor is blinking.
    CursorBlinking,
    /// Cursor colour in pane (named / `colour<N>` / `#RRGGBB` / empty).
    CursorColour,
    /// Pane cursor flag.
    CursorFlag,
    /// Cursor shape in pane (`block`/`underline`/`bar`/`default`).
    CursorShape,
    /// Cursor X position in pane.
    CursorX,
    /// Cursor Y position in pane.
    CursorY,
    /// 1 if focus reporting is enabled.
    FocusFlag,
    /// Pane insert flag.
    InsertFlag,
    /// Pane keypad cursor flag.
    KeypadCursorFlag,
    /// Pane keypad flag.
    KeypadFlag,
    /// Pane mouse all flag.
    MouseAllFlag,
    /// Pane mouse any flag.
    MouseAnyFlag,
    /// Pane mouse button flag.
    MouseButtonFlag,
    /// Pane mouse SGR flag.
    MouseSgrFlag,
    /// Pane mouse standard flag.
    MouseStandardFlag,
    /// Pane mouse UTF-8 flag.
    MouseUtf8Flag,
    /// Pane origin flag.
    OriginFlag,
    /// Unique pane ID prefixed with `%`.
    PaneId,
    /// Pane tab positions as a comma-separated list of columns (may be empty).
    PaneTabs,
    /// Bottom of scroll region in pane.
    ScrollRegionLower,
    /// Top of scroll region in pane.
    ScrollRegionUpper,
    /// Unique session ID prefixed with `$`.
    SessionId,
    /// Server version (e.g. `3.5a`).
    Version,
    /// Unique window ID prefixed with `@`.
    WindowId,
    /// Width of window.
    WindowWidth,
    /// Height of window.
    WindowHeight,
    /// Window layout description (`<checksum>,<layout>`).
    WindowLayout,
    /// Pane wrap flag.
    WrapFlag,
}

/// The kind of value a [`Variable`] parses to. Port of `Variable.Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Bool,
    Usize,
    Str,
}

/// A parsed tmux format value. The runtime counterpart of `Variable.Type`'s
/// per-variable static type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Bool(bool),
    Usize(usize),
    /// A verbatim string value (owned; tmux values are arbitrary bytes).
    Str(Vec<u8>),
}

/// A single [`Variable::parse`] failed (bad number, or a missing `$`/`@`/`%`
/// prefix). Upstream distinguishes Zig's `InvalidCharacter`/`Overflow`/
/// `FormatError`; those all collapse to `error.FormatError` at the
/// `parseFormatStruct` layer, so this port carries the single parse-failed
/// signal that the (only) consumer, [`parse_format`], acts on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarParseError;

/// Failure parsing a whole delimited format string. Port of `output.zig`'s
/// `ParseError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// Fewer delimited parts than variables.
    MissingEntry,
    /// More delimited parts than variables.
    ExtraEntry,
    /// A part failed to parse as its variable's type.
    FormatError,
}

impl Variable {
    /// The tmux `snake_case` name (as it appears inside `#{…}`). Port of the
    /// enum `@tagName`.
    pub fn name(self) -> &'static str {
        use Variable::*;
        match self {
            AlternateOn => "alternate_on",
            AlternateSavedX => "alternate_saved_x",
            AlternateSavedY => "alternate_saved_y",
            BracketedPaste => "bracketed_paste",
            CursorBlinking => "cursor_blinking",
            CursorColour => "cursor_colour",
            CursorFlag => "cursor_flag",
            CursorShape => "cursor_shape",
            CursorX => "cursor_x",
            CursorY => "cursor_y",
            FocusFlag => "focus_flag",
            InsertFlag => "insert_flag",
            KeypadCursorFlag => "keypad_cursor_flag",
            KeypadFlag => "keypad_flag",
            MouseAllFlag => "mouse_all_flag",
            MouseAnyFlag => "mouse_any_flag",
            MouseButtonFlag => "mouse_button_flag",
            MouseSgrFlag => "mouse_sgr_flag",
            MouseStandardFlag => "mouse_standard_flag",
            MouseUtf8Flag => "mouse_utf8_flag",
            OriginFlag => "origin_flag",
            PaneId => "pane_id",
            PaneTabs => "pane_tabs",
            ScrollRegionLower => "scroll_region_lower",
            ScrollRegionUpper => "scroll_region_upper",
            SessionId => "session_id",
            Version => "version",
            WindowId => "window_id",
            WindowWidth => "window_width",
            WindowHeight => "window_height",
            WindowLayout => "window_layout",
            WrapFlag => "wrap_flag",
        }
    }

    /// The value kind this variable parses to. Port of `Variable.Type`.
    pub fn kind(self) -> ValueKind {
        use Variable::*;
        match self {
            AlternateOn | BracketedPaste | CursorBlinking | CursorFlag | FocusFlag | InsertFlag
            | KeypadCursorFlag | KeypadFlag | MouseAllFlag | MouseAnyFlag | MouseButtonFlag
            | MouseSgrFlag | MouseStandardFlag | MouseUtf8Flag | OriginFlag | WrapFlag => {
                ValueKind::Bool
            }
            AlternateSavedX | AlternateSavedY | CursorX | CursorY | ScrollRegionLower
            | ScrollRegionUpper | SessionId | WindowId | PaneId | WindowWidth | WindowHeight => {
                ValueKind::Usize
            }
            CursorColour | CursorShape | PaneTabs | Version | WindowLayout => ValueKind::Str,
        }
    }

    /// Parse a raw value string into a [`Value`] per this variable's rules.
    /// Port of `Variable.parse`.
    ///
    /// - Flags: `true` iff the value is exactly `"1"`.
    /// - Plain numbers: base-10 `usize`.
    /// - `$`/`@`/`%`-prefixed ids: the prefix plus a base-10 `usize`.
    /// - Strings: returned verbatim.
    pub fn parse(self, value: &[u8]) -> Result<Value, VarParseError> {
        use Variable::*;
        Ok(match self {
            AlternateOn | BracketedPaste | CursorBlinking | CursorFlag | FocusFlag | InsertFlag
            | KeypadCursorFlag | KeypadFlag | MouseAllFlag | MouseAnyFlag | MouseButtonFlag
            | MouseSgrFlag | MouseStandardFlag | MouseUtf8Flag | OriginFlag | WrapFlag => {
                Value::Bool(value == b"1")
            }

            AlternateSavedX | AlternateSavedY | CursorX | CursorY | ScrollRegionLower
            | ScrollRegionUpper | WindowWidth | WindowHeight => {
                Value::Usize(parse_usize(value).ok_or(VarParseError)?)
            }

            SessionId => Value::Usize(parse_prefixed_id(value, b'$')?),
            WindowId => Value::Usize(parse_prefixed_id(value, b'@')?),
            PaneId => Value::Usize(parse_prefixed_id(value, b'%')?),

            CursorColour | CursorShape | PaneTabs | Version | WindowLayout => {
                Value::Str(value.to_vec())
            }
        })
    }
}

/// Parse a `<prefix><digits>` id (e.g. `$42`, `@0`, `%7`). The value must be at
/// least two bytes and start with `prefix`; the remainder must be a base-10
/// `usize`. Port of the `session_id`/`window_id`/`pane_id` arms.
fn parse_prefixed_id(value: &[u8], prefix: u8) -> Result<usize, VarParseError> {
    if value.len() >= 2 && value[0] == prefix {
        parse_usize(&value[1..]).ok_or(VarParseError)
    } else {
        Err(VarParseError)
    }
}

/// Parse a whole byte slice as a base-10 `usize` (non-empty, all ASCII digits,
/// no overflow), or `None`. Mirrors `std.fmt.parseInt(usize, …, 10)`'s success
/// domain (it rejects empty, non-digit, and overflow).
fn parse_usize(s: &[u8]) -> Option<usize> {
    if s.is_empty() {
        return None;
    }
    let mut v: usize = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return None;
        }
        v = v.checked_mul(10)?.checked_add((b - b'0') as usize)?;
    }
    Some(v)
}

/// Build a tmux format request string for `vars`, joined by `delimiter`:
/// `#{name0}<delim>#{name1}…`. Port of `format`/`comptimeFormat` (collapsed to
/// one runtime function).
pub fn format(vars: &[Variable], delimiter: u8) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, var) in vars.iter().enumerate() {
        if i != 0 {
            out.push(delimiter);
        }
        out.extend_from_slice(b"#{");
        out.extend_from_slice(var.name().as_bytes());
        out.push(b'}');
    }
    out
}

/// Parse a `delimiter`-separated format response into one [`Value`] per
/// variable, positionally aligned with `vars`. Port of `parseFormatStruct`:
/// too few parts is [`ParseError::MissingEntry`], too many is
/// [`ParseError::ExtraEntry`], and any field that fails to parse is
/// [`ParseError::FormatError`].
pub fn parse_format(vars: &[Variable], s: &[u8], delimiter: u8) -> Result<Vec<Value>, ParseError> {
    let mut parts = s.split(|&b| b == delimiter);
    let mut result = Vec::with_capacity(vars.len());
    for &var in vars {
        let part = parts.next().ok_or(ParseError::MissingEntry)?;
        let value = var.parse(part).map_err(|_| ParseError::FormatError)?;
        result.push(value);
    }
    if parts.next().is_some() {
        return Err(ParseError::ExtraEntry);
    }
    Ok(result)
}

#[cfg(test)]
mod tests;
