//! OSC 8: hyperlinks. Port of `osc/parsers/hyperlink.zig`.

use crate::osc::Command;

/// Parse OSC 8. Port of `hyperlink.zig` `parse`.
pub fn parse(rest: &str) -> Option<Command> {
    let data = rest.strip_prefix(';')?;
    // data is "<key=value:key=value:...>;<uri>".
    let s = data.find(';')?;
    let uri = &data[s + 1..];
    let kvs = &data[..s];

    let mut id: Option<String> = None;
    for kv in kvs.split(':') {
        // A completely empty segment (from a leading/trailing/doubled
        // colon) has no '=' and is just skipped, per the Zig source's
        // "break" on missing '=' -- but note Zig's loop actually keys off
        // NUL-terminator positions from a mutated buffer; the equivalent
        // observable behavior here is: split on ':', and for each
        // non-empty segment, split on the first '=' to get key/value.
        if kv.is_empty() {
            continue;
        }
        let Some(eq) = kv.find('=') else {
            // Incomplete key (no '='): logged and ignored in Zig once it
            // fails to find '=' -- but Zig's loop actually *stops*
            // iterating entirely at that point (`orelse break`). Match
            // that: an incomplete key ends kv parsing right there.
            break;
        };
        let key = &kv[..eq];
        let value = &kv[eq + 1..];
        if key == "id" && !value.is_empty() {
            id = Some(value.to_string());
        }
        // else: unknown key, logged and ignored.
    }

    if uri.is_empty() {
        if id.is_some() {
            return None;
        }
        return Some(Command::HyperlinkEnd);
    }

    Some(Command::HyperlinkStart {
        id,
        uri: uri.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    fn run(body: &str) -> Option<Command> {
        let mut p = osc::Parser::new();
        for c in body.bytes() {
            p.next(c);
        }
        p.end(Some(0x1b))
    }

    // Zig: hyperlink.zig "OSC 8: hyperlink".
    #[test]
    fn osc_8_hyperlink() {
        assert_eq!(
            run("8;;http://example.com"),
            Some(Command::HyperlinkStart {
                id: None,
                uri: "http://example.com".to_string()
            })
        );
    }

    // Zig: hyperlink.zig "OSC 8: hyperlink with id set".
    #[test]
    fn osc_8_hyperlink_with_id_set() {
        assert_eq!(
            run("8;id=foo;http://example.com"),
            Some(Command::HyperlinkStart {
                id: Some("foo".to_string()),
                uri: "http://example.com".to_string()
            })
        );
    }

    // Zig: hyperlink.zig "OSC 8: hyperlink with empty id".
    #[test]
    fn osc_8_hyperlink_with_empty_id() {
        assert_eq!(
            run("8;id=;http://example.com"),
            Some(Command::HyperlinkStart {
                id: None,
                uri: "http://example.com".to_string()
            })
        );
    }

    // Zig: hyperlink.zig "OSC 8: hyperlink with incomplete key".
    #[test]
    fn osc_8_hyperlink_with_incomplete_key() {
        assert_eq!(
            run("8;id;http://example.com"),
            Some(Command::HyperlinkStart {
                id: None,
                uri: "http://example.com".to_string()
            })
        );
    }

    // Zig: hyperlink.zig "OSC 8: hyperlink with empty key".
    #[test]
    fn osc_8_hyperlink_with_empty_key() {
        assert_eq!(
            run("8;=value;http://example.com"),
            Some(Command::HyperlinkStart {
                id: None,
                uri: "http://example.com".to_string()
            })
        );
    }

    // Zig: hyperlink.zig "OSC 8: hyperlink with empty key and id".
    #[test]
    fn osc_8_hyperlink_with_empty_key_and_id() {
        assert_eq!(
            run("8;=value:id=foo;http://example.com"),
            Some(Command::HyperlinkStart {
                id: Some("foo".to_string()),
                uri: "http://example.com".to_string()
            })
        );
    }

    // Zig: hyperlink.zig "OSC 8: hyperlink with empty uri".
    #[test]
    fn osc_8_hyperlink_with_empty_uri() {
        assert_eq!(run("8;id=foo;"), None);
    }

    // Zig: hyperlink.zig "OSC 8: hyperlink end".
    #[test]
    fn osc_8_hyperlink_end() {
        assert_eq!(run("8;;"), Some(Command::HyperlinkEnd));
    }
}
