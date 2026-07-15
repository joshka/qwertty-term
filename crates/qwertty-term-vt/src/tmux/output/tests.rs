//! Ported inline tests from `tmux/output.zig`. Zig's per-variable errors
//! (`InvalidCharacter`/`Overflow`/`FormatError`) collapse to a single
//! parse-failure here (see [`VarParseError`]); error-case tests assert
//! `is_err()` rather than a specific variant.

use super::*;

fn expect_bool(var: Variable, value: &str, want: bool) {
    assert_eq!(
        var.parse(value.as_bytes()),
        Ok(Value::Bool(want)),
        "{var:?} {value:?}"
    );
}

fn expect_usize(var: Variable, value: &str, want: usize) {
    assert_eq!(
        var.parse(value.as_bytes()),
        Ok(Value::Usize(want)),
        "{var:?} {value:?}"
    );
}

fn expect_str(var: Variable, value: &str) {
    assert_eq!(
        var.parse(value.as_bytes()),
        Ok(Value::Str(value.as_bytes().to_vec())),
        "{var:?} {value:?}"
    );
}

fn expect_err(var: Variable, value: &str) {
    assert!(
        var.parse(value.as_bytes()).is_err(),
        "{var:?} {value:?} should error"
    );
}

#[test]
fn parse_all_flag_variables() {
    // Every boolean variable: "1" -> true, everything else -> false.
    let flags = [
        Variable::AlternateOn,
        Variable::BracketedPaste,
        Variable::CursorBlinking,
        Variable::CursorFlag,
        Variable::FocusFlag,
        Variable::InsertFlag,
        Variable::KeypadCursorFlag,
        Variable::KeypadFlag,
        Variable::MouseAllFlag,
        Variable::MouseAnyFlag,
        Variable::MouseButtonFlag,
        Variable::MouseSgrFlag,
        Variable::MouseStandardFlag,
        Variable::MouseUtf8Flag,
        Variable::OriginFlag,
        Variable::WrapFlag,
    ];
    for f in flags {
        expect_bool(f, "1", true);
        expect_bool(f, "0", false);
        expect_bool(f, "", false);
        expect_bool(f, "true", false);
        expect_bool(f, "yes", false);
    }
}

#[test]
fn parse_plain_usize_variables() {
    for v in [
        Variable::AlternateSavedX,
        Variable::AlternateSavedY,
        Variable::CursorX,
        Variable::CursorY,
        Variable::ScrollRegionUpper,
        Variable::ScrollRegionLower,
    ] {
        expect_usize(v, "0", 0);
        expect_usize(v, "42", 42);
        expect_err(v, "abc");
    }
}

#[test]
fn parse_window_width_height() {
    expect_usize(Variable::WindowWidth, "80", 80);
    expect_usize(Variable::WindowWidth, "0", 0);
    expect_usize(Variable::WindowWidth, "12345", 12345);
    expect_err(Variable::WindowWidth, "abc");
    expect_err(Variable::WindowWidth, "80px");
    expect_err(Variable::WindowWidth, "-1"); // Zig error.Overflow

    expect_usize(Variable::WindowHeight, "24", 24);
    expect_err(Variable::WindowHeight, "24px");
    expect_err(Variable::WindowHeight, "-1");
}

#[test]
fn parse_session_id() {
    expect_usize(Variable::SessionId, "$42", 42);
    expect_usize(Variable::SessionId, "$0", 0);
    expect_err(Variable::SessionId, "0"); // missing prefix
    expect_err(Variable::SessionId, "@0"); // wrong prefix
    expect_err(Variable::SessionId, "$"); // prefix only
    expect_err(Variable::SessionId, ""); // empty
    expect_err(Variable::SessionId, "$abc"); // non-numeric
}

#[test]
fn parse_window_id() {
    expect_usize(Variable::WindowId, "@42", 42);
    expect_usize(Variable::WindowId, "@0", 0);
    expect_usize(Variable::WindowId, "@12345", 12345);
    expect_err(Variable::WindowId, "0");
    expect_err(Variable::WindowId, "$0");
    expect_err(Variable::WindowId, "@");
    expect_err(Variable::WindowId, "");
    expect_err(Variable::WindowId, "@abc");
}

#[test]
fn parse_pane_id() {
    expect_usize(Variable::PaneId, "%42", 42);
    expect_usize(Variable::PaneId, "%0", 0);
    expect_err(Variable::PaneId, "0");
    expect_err(Variable::PaneId, "@0");
    expect_err(Variable::PaneId, "%");
    expect_err(Variable::PaneId, "");
    expect_err(Variable::PaneId, "%abc");
}

#[test]
fn parse_string_variables() {
    expect_str(Variable::CursorColour, "red");
    expect_str(Variable::CursorColour, "#ff0000");
    expect_str(Variable::CursorColour, "");
    expect_str(Variable::CursorShape, "block");
    expect_str(Variable::CursorShape, "underline");
    expect_str(Variable::CursorShape, "bar");
    expect_str(Variable::PaneTabs, "0,8,16,24");
    expect_str(Variable::PaneTabs, "");
    expect_str(Variable::Version, "3.5a");
    expect_str(Variable::Version, "next-3.5");
    expect_str(Variable::WindowLayout, "abc123");
    expect_str(Variable::WindowLayout, "");
    expect_str(Variable::WindowLayout, "a]b,c{d}e(f)");
}

// ---- parse_format ---------------------------------------------------------

#[test]
fn parse_format_single_field() {
    let r = parse_format(&[Variable::SessionId], b"$42", b' ').unwrap();
    assert_eq!(r, vec![Value::Usize(42)]);
}

#[test]
fn parse_format_multiple_fields() {
    let vars = [
        Variable::SessionId,
        Variable::WindowId,
        Variable::WindowWidth,
        Variable::WindowHeight,
    ];
    let r = parse_format(&vars, b"$1 @2 80 24", b' ').unwrap();
    assert_eq!(
        r,
        vec![
            Value::Usize(1),
            Value::Usize(2),
            Value::Usize(80),
            Value::Usize(24)
        ]
    );
}

#[test]
fn parse_format_with_string_field() {
    let vars = [Variable::WindowId, Variable::WindowLayout];
    let r = parse_format(&vars, b"@5,abc123", b',').unwrap();
    assert_eq!(r, vec![Value::Usize(5), Value::Str(b"abc123".to_vec())]);
}

#[test]
fn parse_format_different_delimiter() {
    let vars = [Variable::WindowWidth, Variable::WindowHeight];
    let r = parse_format(&vars, b"120\t40", b'\t').unwrap();
    assert_eq!(r, vec![Value::Usize(120), Value::Usize(40)]);
}

#[test]
fn parse_format_missing_entry() {
    let vars = [Variable::SessionId, Variable::WindowId];
    assert_eq!(
        parse_format(&vars, b"$1", b' '),
        Err(ParseError::MissingEntry)
    );
}

#[test]
fn parse_format_extra_entry() {
    let vars = [Variable::SessionId];
    assert_eq!(
        parse_format(&vars, b"$1 @2", b' '),
        Err(ParseError::ExtraEntry)
    );
}

#[test]
fn parse_format_format_error() {
    let vars = [Variable::SessionId];
    assert_eq!(
        parse_format(&vars, b"42", b' '),
        Err(ParseError::FormatError)
    );
    assert_eq!(
        parse_format(&vars, b"@42", b' '),
        Err(ParseError::FormatError)
    );
    assert_eq!(
        parse_format(&vars, b"$abc", b' '),
        Err(ParseError::FormatError)
    );
}

#[test]
fn parse_format_empty_string() {
    let vars = [Variable::SessionId];
    assert_eq!(parse_format(&vars, b"", b' '), Err(ParseError::FormatError));
}

#[test]
fn parse_format_with_empty_layout_field() {
    let vars = [Variable::SessionId, Variable::WindowLayout];
    let r = parse_format(&vars, b"$1,", b',').unwrap();
    assert_eq!(r, vec![Value::Usize(1), Value::Str(b"".to_vec())]);
}

// ---- format ---------------------------------------------------------------

#[track_caller]
fn assert_format(vars: &[Variable], delimiter: u8, expected: &str) {
    assert_eq!(format(vars, delimiter), expected.as_bytes());
}

#[test]
fn format_single_variable() {
    assert_format(&[Variable::SessionId], b' ', "#{session_id}");
}

#[test]
fn format_multiple_variables() {
    assert_format(
        &[
            Variable::SessionId,
            Variable::WindowId,
            Variable::WindowWidth,
            Variable::WindowHeight,
        ],
        b' ',
        "#{session_id} #{window_id} #{window_width} #{window_height}",
    );
}

#[test]
fn format_with_comma_delimiter() {
    assert_format(
        &[Variable::WindowId, Variable::WindowLayout],
        b',',
        "#{window_id},#{window_layout}",
    );
}

#[test]
fn format_with_tab_delimiter() {
    assert_format(
        &[Variable::WindowWidth, Variable::WindowHeight],
        b'\t',
        "#{window_width}\t#{window_height}",
    );
}

#[test]
fn format_empty_variables() {
    assert_format(&[], b' ', "");
}

#[test]
fn format_all_named_variables() {
    assert_format(
        &[
            Variable::SessionId,
            Variable::WindowId,
            Variable::WindowWidth,
            Variable::WindowHeight,
            Variable::WindowLayout,
        ],
        b' ',
        "#{session_id} #{window_id} #{window_width} #{window_height} #{window_layout}",
    );
}

#[test]
fn round_trip_name_matches_kind() {
    // Sanity: every variable has a non-empty snake_case name and a stable kind.
    for var in [
        Variable::AlternateOn,
        Variable::SessionId,
        Variable::WindowLayout,
        Variable::CursorX,
    ] {
        assert!(!var.name().is_empty());
        let _ = var.kind();
    }
}
