//! Regex URL detection for R7 clickable links (slice 2).
//!
//! Upstream detects non-OSC8 links by matching a URL regex over the visible
//! grid text (`config/url.zig` + `renderer/link.zig`). That regex relies on
//! Oniguruma look-behind/look-ahead (`(?<![,.])`, `(?=…\.)`) to strip trailing
//! sentence punctuation and balance parentheses — features the Rust `regex`
//! crate does not support. So this is a **documented deviation**: we match a
//! simpler scheme-anchored pattern and reproduce the two load-bearing trimming
//! heuristics (from `url.zig`'s doc comment) in post-processing:
//!
//! 1. A URL does not end with `.`, `,`, or `:` (sentence punctuation).
//! 2. A trailing `)` is dropped unless the span holds an unmatched `(` — so
//!    `https://en.wikipedia.org/wiki/Rust_(video_game)` keeps its parens but
//!    `(https://example.com)` does not.
//!
//! This finds the common cases (schemed URLs, `mailto:`/`tel:` and friends);
//! the many pathological cases upstream also punts on are out of scope. Detected
//! spans feed the same hover-underline path as OSC8 links (slice 1).

use regex::Regex;
use std::ops::Range;
use std::sync::OnceLock;

/// The URL matcher. Scheme-anchored so we never match bare words:
/// `scheme://…` for the network schemes, or `scheme:…` for the flat ones
/// (`mailto:`, `tel:`, `news:`, `magnet:`). The trailing class is upstream's
/// `scheme_url_chars` minus the look-around trimming, which we do afterwards.
fn url_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?xi)
            \b
            (?:
                (?: https? | ftp | ssh | git | file | ipfs | ipns | gemini | gopher ) ://
              | (?: mailto | tel | news | magnet ) :
            )
            [\w\-.~:/?\#@!$&*+,;=%]+
            # optional trailing bracketed word, e.g. Wikipedia's
            # `/Rust_(video_game)` — mirrors url.zig's bracketed-word suffix so a
            # balanced inner paren stays part of the URL.
            (?: [(\[] \w* [)\]] )?
            ",
        )
        .expect("URL regex compiles")
    })
}

/// Find every URL span in `line`, as byte ranges into `line`, with the
/// trailing-punctuation and parenthesis heuristics applied. Ranges are
/// non-overlapping and left-to-right. Callers map byte offsets to grid cells.
#[must_use]
pub fn find_urls(line: &str) -> Vec<Range<usize>> {
    let re = url_regex();
    let mut out = Vec::new();
    for m in re.find_iter(line) {
        let start = m.start();
        let mut end = m.end();
        // Trim trailing sentence punctuation / unbalanced close-paren.
        while end > start {
            let span = &line[start..end];
            match span.as_bytes()[span.len() - 1] {
                b',' | b'.' | b':' => end -= 1,
                b')' => {
                    let opens = span.bytes().filter(|&c| c == b'(').count();
                    let closes = span.bytes().filter(|&c| c == b')').count();
                    if opens < closes {
                        end -= 1;
                    } else {
                        break;
                    }
                }
                _ => break,
            }
        }
        // A scheme alone (e.g. "https://" then trimmed to nothing meaningful)
        // is still a valid span start; keep anything past the scheme colon.
        if end > start {
            out.push(start..end);
        }
    }
    out
}

/// Whether `col`-th byte offset falls inside any detected URL on `line`.
/// Convenience for hover hit-testing once byte offsets are mapped to cells.
#[must_use]
pub fn url_span_at(line: &str, byte_offset: usize) -> Option<Range<usize>> {
    find_urls(line)
        .into_iter()
        .find(|r| r.contains(&byte_offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans(line: &str) -> Vec<&str> {
        find_urls(line).into_iter().map(|r| &line[r]).collect()
    }

    #[test]
    fn plain_https_url() {
        assert_eq!(
            spans("see https://example.com now"),
            ["https://example.com"]
        );
    }

    #[test]
    fn url_with_path_and_query() {
        assert_eq!(
            spans("go https://a.test/p?x=1&y=2#frag ok"),
            ["https://a.test/p?x=1&y=2#frag"]
        );
    }

    #[test]
    fn trailing_period_is_trimmed() {
        // Rule 1: a URL at the end of a sentence drops the period.
        assert_eq!(spans("visit https://example.com."), ["https://example.com"]);
        assert_eq!(spans("a https://x.io, b"), ["https://x.io"]);
    }

    #[test]
    fn parenthesized_url_drops_wrapping_paren() {
        // Rule 2: "(https://example.com)" excludes the wrapping paren...
        assert_eq!(spans("(https://example.com)"), ["https://example.com"]);
    }

    #[test]
    fn url_with_balanced_inner_parens_keeps_them() {
        // ...but a balanced inner paren stays part of the URL.
        assert_eq!(
            spans("https://en.wikipedia.org/wiki/Rust_(video_game)"),
            ["https://en.wikipedia.org/wiki/Rust_(video_game)"]
        );
    }

    #[test]
    fn flat_schemes() {
        assert_eq!(spans("mail me: mailto:a@b.test end"), ["mailto:a@b.test"]);
        assert_eq!(spans("call tel:+15551234"), ["tel:+15551234"]);
    }

    #[test]
    fn multiple_urls_on_a_line() {
        assert_eq!(
            spans("http://a.test and http://b.test"),
            ["http://a.test", "http://b.test"]
        );
    }

    #[test]
    fn no_scheme_no_match() {
        assert!(find_urls("just some example.com words").is_empty());
        assert!(find_urls("no links here at all").is_empty());
    }

    #[test]
    fn hit_test() {
        let line = "x https://example.com y";
        let inside = line.find("example").unwrap();
        assert!(url_span_at(line, inside).is_some());
        assert!(url_span_at(line, 0).is_none()); // the 'x'
    }
}
