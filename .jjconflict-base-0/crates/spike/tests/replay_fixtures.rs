use std::{fs, path::Path};

use qwertty_term_spike::Engine;

#[test]
fn replay_fixtures_match_expected_screen() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/replay");
    for entry in fs::read_dir(&fixtures).expect("read replay fixture directory") {
        let entry = entry.expect("read replay fixture entry");
        let path = entry.path();
        if path.is_dir() {
            run_fixture(&path);
        }
    }
}

fn run_fixture(path: &Path) {
    let size = read_size(&path.join("size.txt"));
    let input = fs::read_to_string(path.join("input.esc")).expect("read fixture input");
    let expected = fs::read_to_string(path.join("expected.txt")).expect("read fixture expected");

    let mut engine = Engine::new(size.cols, size.rows);
    engine.write(&decode_escaped_stream(&input));

    assert_eq!(
        engine.screen_dump(),
        expected.trim_end_matches('\n'),
        "fixture {}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("<unknown>")
    );
}

struct Size {
    cols: usize,
    rows: usize,
}

fn read_size(path: &Path) -> Size {
    let text = fs::read_to_string(path).expect("read fixture size");
    let mut parts = text.split_whitespace();
    let cols = parts
        .next()
        .expect("fixture size cols")
        .parse()
        .expect("fixture size cols number");
    let rows = parts
        .next()
        .expect("fixture size rows")
        .parse()
        .expect("fixture size rows number");
    Size { cols, rows }
}

fn decode_escaped_stream(text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut chars = text.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            push_utf8(&mut out, ch);
            continue;
        }

        match chars.next() {
            Some('e') => out.push(0x1b),
            Some('n') => out.push(b'\n'),
            Some('r') => out.push(b'\r'),
            Some('t') => out.push(b'\t'),
            Some('\\') => out.push(b'\\'),
            Some('x') => {
                let hi = chars.next().expect("hex escape high nibble");
                let lo = chars.next().expect("hex escape low nibble");
                out.push(hex_byte(hi, lo));
            }
            Some(other) => {
                out.push(b'\\');
                push_utf8(&mut out, other);
            }
            None => out.push(b'\\'),
        }
    }
    out
}

fn push_utf8(out: &mut Vec<u8>, ch: char) {
    let mut buf = [0; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

fn hex_byte(hi: char, lo: char) -> u8 {
    let hi = hi.to_digit(16).expect("valid high hex nibble") as u8;
    let lo = lo.to_digit(16).expect("valid low hex nibble") as u8;
    (hi << 4) | lo
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_fixture_escape_notation() {
        assert_eq!(
            decode_escaped_stream(r"hi\e[31m\n\r\t\\\x21"),
            b"hi\x1b[31m\n\r\t\\!"
        );
    }
}
