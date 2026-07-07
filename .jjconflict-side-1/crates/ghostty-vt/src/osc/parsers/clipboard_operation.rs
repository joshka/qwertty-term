//! OSC 52: get/set clipboard contents. Port of
//! `osc/parsers/clipboard_operation.zig`.

use crate::osc::Command;

/// Parse OSC 52. Port of `clipboard_operation.zig` `parse`. Requires
/// unbounded capture (checked by the caller, mirroring `ensureAllocator`
/// having already invalidated the parser in Zig before this runs).
pub fn parse(rest: &str) -> Option<Command> {
    let data = rest.strip_prefix(';')?;
    if data.is_empty() {
        return None;
    }
    if let Some(payload) = data.strip_prefix(';') {
        return Some(Command::ClipboardContents {
            kind: b'c',
            data: payload.to_string(),
        });
    }
    if data.len() < 2 {
        return None;
    }
    let kind = data.as_bytes()[0];
    if data.as_bytes()[1] != b';' {
        return None;
    }
    Some(Command::ClipboardContents {
        kind,
        data: data[2..].to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    fn run(body: &str) -> Option<Command> {
        let mut p = osc::Parser::with_allocator();
        for c in body.bytes() {
            p.next(c);
        }
        p.end(None)
    }

    // Zig: clipboard_operation.zig "OSC 52: get/set clipboard".
    #[test]
    fn osc_52_get_set_clipboard() {
        assert_eq!(
            run("52;s;?"),
            Some(Command::ClipboardContents {
                kind: b's',
                data: "?".to_string()
            })
        );
    }

    // Zig: clipboard_operation.zig "OSC 52: get/set clipboard (optional parameter)".
    #[test]
    fn osc_52_get_set_clipboard_optional_parameter() {
        assert_eq!(
            run("52;;?"),
            Some(Command::ClipboardContents {
                kind: b'c',
                data: "?".to_string()
            })
        );
    }

    // Zig: clipboard_operation.zig "OSC 52: get/set clipboard with allocator".
    //
    // The Zig test's point is that this works *with* an allocator (vs the
    // unbounded-capture-required gate); the Rust port always uses
    // `with_allocator()` for OSC 52 in this test module, so this is
    // structurally identical to the basic test above.
    #[test]
    fn osc_52_get_set_clipboard_with_allocator() {
        assert_eq!(
            run("52;s;?"),
            Some(Command::ClipboardContents {
                kind: b's',
                data: "?".to_string()
            })
        );
    }

    // Zig: clipboard_operation.zig "OSC 52: clear clipboard".
    #[test]
    fn osc_52_clear_clipboard() {
        assert_eq!(
            run("52;;"),
            Some(Command::ClipboardContents {
                kind: b'c',
                data: String::new()
            })
        );
    }

    // Not a Zig-ported test: pins that OSC 52 without unbounded capture
    // (no allocator) fails, mirroring `ensureAllocator`.
    #[test]
    fn osc_52_without_allocator_fails() {
        let mut p = osc::Parser::new();
        for c in "52;s;?".bytes() {
            p.next(c);
        }
        assert_eq!(p.end(None), None);
    }
}
