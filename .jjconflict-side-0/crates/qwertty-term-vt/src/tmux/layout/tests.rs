//! Ported inline tests from `tmux/layout.zig`.

use super::*;

/// Parse a bare layout from a `&str`, panicking on error (for the happy paths).
#[track_caller]
fn parse(s: &str) -> Layout {
    Layout::parse(s.as_bytes()).expect("layout should parse")
}

fn pane(l: &Layout) -> usize {
    match &l.content {
        Content::Pane(id) => *id,
        other => panic!("expected pane, got {other:?}"),
    }
}

fn horizontal(l: &Layout) -> &[Layout] {
    match &l.content {
        Content::Horizontal(v) => v,
        other => panic!("expected horizontal split, got {other:?}"),
    }
}

fn vertical(l: &Layout) -> &[Layout] {
    match &l.content {
        Content::Vertical(v) => v,
        other => panic!("expected vertical split, got {other:?}"),
    }
}

#[test]
fn simple_single_pane() {
    let l = parse("80x24,0,0,42");
    assert_eq!(l.width, 80);
    assert_eq!(l.height, 24);
    assert_eq!(l.x, 0);
    assert_eq!(l.y, 0);
    assert_eq!(pane(&l), 42);
}

#[test]
fn single_pane_with_offset() {
    let l = parse("40x12,10,5,7");
    assert_eq!(l.width, 40);
    assert_eq!(l.height, 12);
    assert_eq!(l.x, 10);
    assert_eq!(l.y, 5);
    assert_eq!(pane(&l), 7);
}

#[test]
fn single_pane_large_values() {
    let l = parse("1920x1080,100,200,999");
    assert_eq!(l.width, 1920);
    assert_eq!(l.height, 1080);
    assert_eq!(l.x, 100);
    assert_eq!(l.y, 200);
    assert_eq!(pane(&l), 999);
}

#[test]
fn horizontal_split_two_panes() {
    let l = parse("80x24,0,0{40x24,0,0,1,40x24,40,0,2}");
    assert_eq!(l.width, 80);
    assert_eq!(l.height, 24);
    let c = horizontal(&l);
    assert_eq!(c.len(), 2);
    assert_eq!((c[0].width, c[0].height, c[0].x, c[0].y), (40, 24, 0, 0));
    assert_eq!(pane(&c[0]), 1);
    assert_eq!((c[1].width, c[1].height, c[1].x, c[1].y), (40, 24, 40, 0));
    assert_eq!(pane(&c[1]), 2);
}

#[test]
fn vertical_split_two_panes() {
    let l = parse("80x24,0,0[80x12,0,0,1,80x12,0,12,2]");
    let c = vertical(&l);
    assert_eq!(c.len(), 2);
    assert_eq!((c[0].width, c[0].height, c[0].x, c[0].y), (80, 12, 0, 0));
    assert_eq!(pane(&c[0]), 1);
    assert_eq!((c[1].width, c[1].height, c[1].x, c[1].y), (80, 12, 0, 12));
    assert_eq!(pane(&c[1]), 2);
}

#[test]
fn horizontal_split_three_panes() {
    let l = parse("120x24,0,0{40x24,0,0,1,40x24,40,0,2,40x24,80,0,3}");
    assert_eq!(l.width, 120);
    let c = horizontal(&l);
    assert_eq!(c.len(), 3);
    assert_eq!(pane(&c[0]), 1);
    assert_eq!(pane(&c[1]), 2);
    assert_eq!(pane(&c[2]), 3);
}

#[test]
fn nested_horizontal_in_vertical() {
    let l = parse("80x24,0,0[80x12,0,0,1,80x12,0,12{40x12,0,12,2,40x12,40,12,3}]");
    let vc = vertical(&l);
    assert_eq!(vc.len(), 2);
    assert_eq!(pane(&vc[0]), 1);
    let hc = horizontal(&vc[1]);
    assert_eq!(hc.len(), 2);
    assert_eq!(pane(&hc[0]), 2);
    assert_eq!(pane(&hc[1]), 3);
}

#[test]
fn nested_vertical_in_horizontal() {
    let l = parse("80x24,0,0{40x24,0,0,1,40x24,40,0[40x12,40,0,2,40x12,40,12,3]}");
    let hc = horizontal(&l);
    assert_eq!(hc.len(), 2);
    assert_eq!(pane(&hc[0]), 1);
    let vc = vertical(&hc[1]);
    assert_eq!(vc.len(), 2);
    assert_eq!(pane(&vc[0]), 2);
    assert_eq!(pane(&vc[1]), 3);
}

#[test]
fn deeply_nested_layout() {
    let l = parse("80x24,0,0{40x24,0,0[40x12,0,0,1,40x12,0,12,2],40x24,40,0,3}");
    let h = horizontal(&l);
    assert_eq!(h.len(), 2);
    let v = vertical(&h[0]);
    assert_eq!(v.len(), 2);
    assert_eq!(pane(&v[0]), 1);
    assert_eq!(pane(&v[1]), 2);
    assert_eq!(pane(&h[1]), 3);
}

// ---- syntax errors --------------------------------------------------------

#[track_caller]
fn assert_syntax_error(s: &str) {
    assert_eq!(
        Layout::parse(s.as_bytes()),
        Err(ParseError::SyntaxError),
        "input {s:?}"
    );
}

#[test]
fn syntax_errors() {
    assert_syntax_error(""); // empty
    assert_syntax_error("x24,0,0,1"); // missing width
    assert_syntax_error("80x,0,0,1"); // missing height
    assert_syntax_error("80x24,,0,1"); // missing x
    assert_syntax_error("80x24,0,,1"); // missing y
    assert_syntax_error("80x24,0,0,"); // missing pane id
    assert_syntax_error("abcx24,0,0,1"); // non-numeric width
    assert_syntax_error("80x24,0,0,abc"); // non-numeric pane id
    assert_syntax_error("80x24,0,0{40x24,0,0,1"); // unclosed horizontal
    assert_syntax_error("80x24,0,0[40x24,0,0,1"); // unclosed vertical
    assert_syntax_error("80x24,0,0{40x24,0,0,1]"); // mismatched brackets
    assert_syntax_error("80x24,0,0[40x24,0,0,1}"); // mismatched brackets
    assert_syntax_error("80x24,0,0,1extra"); // trailing data
    assert_syntax_error("8024,0,0,1"); // no x separator
    assert_syntax_error("80x24,0,0"); // no content delimiter
}

// ---- parse_with_checksum --------------------------------------------------

#[test]
fn parse_with_checksum_valid() {
    let l = Layout::parse_with_checksum(b"f8f9,80x24,0,0{40x24,0,0,1,40x24,40,0,2}")
        .expect("valid checksum should parse");
    assert_eq!(l.width, 80);
    assert_eq!(l.height, 24);
}

#[test]
fn parse_with_checksum_mismatch() {
    assert_eq!(
        Layout::parse_with_checksum(b"0000,80x24,0,0{40x24,0,0,1,40x24,40,0,2}"),
        Err(ParseError::ChecksumMismatch)
    );
}

#[test]
fn parse_with_checksum_too_short() {
    assert_eq!(
        Layout::parse_with_checksum(b"bb62"),
        Err(ParseError::SyntaxError)
    );
    assert_eq!(
        Layout::parse_with_checksum(b""),
        Err(ParseError::SyntaxError)
    );
}

#[test]
fn parse_with_checksum_missing_comma() {
    assert_eq!(
        Layout::parse_with_checksum(b"bb62x159x48,0,0"),
        Err(ParseError::SyntaxError)
    );
}

// ---- checksum -------------------------------------------------------------

#[track_caller]
fn checksum_str(s: &[u8]) -> String {
    String::from_utf8(Checksum::calculate(s).as_string().to_vec()).unwrap()
}

#[test]
fn checksum_empty_string() {
    assert_eq!(Checksum::calculate(b"").0, 0);
    assert_eq!(checksum_str(b""), "0000");
}

#[test]
fn checksum_single_character() {
    assert_eq!(Checksum::calculate(b"A").0, 65);
    assert_eq!(checksum_str(b"A"), "0041");
}

#[test]
fn checksum_two_characters() {
    assert_eq!(Checksum::calculate(b"AB").0, 32866);
    assert_eq!(checksum_str(b"AB"), "8062");
}

#[test]
fn checksum_simple_layout() {
    assert_eq!(checksum_str(b"80x24,0,0,42"), "d962");
}

#[test]
fn checksum_horizontal_split_layout() {
    assert_eq!(checksum_str(b"80x24,0,0{40x24,0,0,1,40x24,40,0,2}"), "f8f9");
}

#[test]
fn checksum_as_string_padding_and_hex() {
    let hex = |v: u16| String::from_utf8(Checksum(v).as_string().to_vec()).unwrap();
    assert_eq!(hex(0x000f), "000f");
    assert_eq!(hex(0x1234), "1234");
    assert_eq!(hex(0xabcd), "abcd");
    assert_eq!(hex(0xffff), "ffff");
}

#[test]
fn checksum_wraparound() {
    assert_eq!(checksum_str(b"\xff\xff\xff\xff\xff\xff\xff\xff"), "03fc");
}

#[test]
fn checksum_deterministic() {
    let s = b"159x48,0,0{79x48,0,0,79x48,80,0}";
    assert_eq!(Checksum::calculate(s), Checksum::calculate(s));
}

#[test]
fn checksum_different_inputs_different_outputs() {
    assert_ne!(
        Checksum::calculate(b"80x24,0,0,1"),
        Checksum::calculate(b"80x24,0,0,2")
    );
}

#[test]
fn checksum_known_tmux_layout_bb62() {
    // From tmux docs: "bb62,159x48,0,0{79x48,0,0,79x48,80,0}".
    assert_eq!(checksum_str(b"159x48,0,0{79x48,0,0,79x48,80,0}"), "bb62");
}
