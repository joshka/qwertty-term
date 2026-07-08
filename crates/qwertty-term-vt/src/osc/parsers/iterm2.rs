//! OSC 1337: iTerm2 proprietary protocol. Port of
//! `osc/parsers/iterm2.zig`. Ghostty implements exactly two of iTerm2's 34
//! recognized keys (`Copy`, `CurrentDir`); the other 32 are recognized
//! (case-insensitively) but produce no command.

use crate::osc::Command;

/// All keys iTerm2 defines; only `Copy`/`CurrentDir` do anything (matches
/// `iterm2.zig` `Key`). Kept as a full enum (rather than just the two
/// implemented variants) so key recognition/case-insensitivity matches Zig
/// exactly: an unimplemented-but-recognized key logs and returns no
/// command, same observable result as an unrecognized key, but the
/// distinction matters for fidelity/parity should more keys be
/// implemented later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum Key {
    AddAnnotation,
    AddHiddenAnnotation,
    Block,
    Button,
    ClearCapturedOutput,
    ClearScrollback,
    Copy,
    CopyToClipboard,
    CurrentDir,
    CursorShape,
    Custom,
    Disinter,
    EndCopy,
    File,
    FileEnd,
    FilePart,
    HighlightCursorLine,
    MultipartFile,
    OpenURL,
    PopKeyLabels,
    PushKeyLabels,
    RemoteHost,
    ReportCellSize,
    ReportVariable,
    RequestAttention,
    RequestUpload,
    SetBackgroundImageFile,
    SetBadgeFormat,
    SetColors,
    SetKeyLabel,
    SetMark,
    SetProfile,
    SetUserVar,
    ShellIntegrationVersion,
    StealFocus,
    UnicodeVersion,
}

impl Key {
    /// ASCII case-insensitive lookup, matching Zig's
    /// `StaticStringMapWithEql(Key, std.ascii.eqlIgnoreCase)`.
    fn parse(s: &str) -> Option<Key> {
        let pairs: &[(&str, Key)] = &[
            ("AddAnnotation", Key::AddAnnotation),
            ("AddHiddenAnnotation", Key::AddHiddenAnnotation),
            ("Block", Key::Block),
            ("Button", Key::Button),
            ("ClearCapturedOutput", Key::ClearCapturedOutput),
            ("ClearScrollback", Key::ClearScrollback),
            ("Copy", Key::Copy),
            ("CopyToClipboard", Key::CopyToClipboard),
            ("CurrentDir", Key::CurrentDir),
            ("CursorShape", Key::CursorShape),
            ("Custom", Key::Custom),
            ("Disinter", Key::Disinter),
            ("EndCopy", Key::EndCopy),
            ("File", Key::File),
            ("FileEnd", Key::FileEnd),
            ("FilePart", Key::FilePart),
            ("HighlightCursorLine", Key::HighlightCursorLine),
            ("MultipartFile", Key::MultipartFile),
            ("OpenURL", Key::OpenURL),
            ("PopKeyLabels", Key::PopKeyLabels),
            ("PushKeyLabels", Key::PushKeyLabels),
            ("RemoteHost", Key::RemoteHost),
            ("ReportCellSize", Key::ReportCellSize),
            ("ReportVariable", Key::ReportVariable),
            ("RequestAttention", Key::RequestAttention),
            ("RequestUpload", Key::RequestUpload),
            ("SetBackgroundImageFile", Key::SetBackgroundImageFile),
            ("SetBadgeFormat", Key::SetBadgeFormat),
            ("SetColors", Key::SetColors),
            ("SetKeyLabel", Key::SetKeyLabel),
            ("SetMark", Key::SetMark),
            ("SetProfile", Key::SetProfile),
            ("SetUserVar", Key::SetUserVar),
            ("ShellIntegrationVersion", Key::ShellIntegrationVersion),
            ("StealFocus", Key::StealFocus),
            ("UnicodeVersion", Key::UnicodeVersion),
        ];
        pairs
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case(s))
            .map(|(_, k)| *k)
    }
}

/// Parse OSC 1337. Port of `iterm2.zig` `parse`.
pub fn parse(rest: &str) -> Option<Command> {
    let data = rest.strip_prefix(';')?;
    let (key_str, value) = match data.find('=') {
        Some(idx) => (&data[..idx], Some(&data[idx + 1..])),
        None => (data, None),
    };

    let key = Key::parse(key_str)?;

    match key {
        Key::Copy => {
            let value = value?;
            if value.is_empty() {
                return None;
            }
            let value = value.strip_prefix(':')?;
            if value.is_empty() {
                return None;
            }
            if value == "?" {
                return None;
            }
            Some(Command::ClipboardContents {
                kind: b'c',
                data: value.to_string(),
            })
        }
        Key::CurrentDir => {
            let value = value?;
            if value.is_empty() {
                return None;
            }
            Some(Command::ReportPwd {
                value: value.to_string(),
            })
        }
        _ => None,
    }
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
        p.end(Some(0x1b))
    }

    // Zig: "OSC: 1337: test valid unimplemented key with no value".
    #[test]
    fn valid_unimplemented_key_no_value() {
        assert_eq!(run("1337;SetBadgeFormat"), None);
    }

    // Zig: "OSC: 1337: test valid unimplemented key with empty value".
    #[test]
    fn valid_unimplemented_key_empty_value() {
        assert_eq!(run("1337;SetBadgeFormat="), None);
    }

    // Zig: "OSC: 1337: test valid unimplemented key with non-empty value".
    #[test]
    fn valid_unimplemented_key_non_empty_value() {
        assert_eq!(run("1337;SetBadgeFormat=abc123"), None);
    }

    // Zig: "OSC: 1337: test valid key with lower case and with no value".
    #[test]
    fn valid_key_lower_case_no_value() {
        assert_eq!(run("1337;setbadgeformat"), None);
    }

    // Zig: "OSC: 1337: test valid key with lower case and with empty value".
    #[test]
    fn valid_key_lower_case_empty_value() {
        assert_eq!(run("1337;setbadgeformat="), None);
    }

    // Zig: "OSC: 1337: test valid key with lower case and with non-empty value".
    #[test]
    fn valid_key_lower_case_non_empty_value() {
        assert_eq!(run("1337;setbadgeformat=abc123"), None);
    }

    // Zig: "OSC: 1337: test invalid key with no value".
    #[test]
    fn invalid_key_no_value() {
        assert_eq!(run("1337;BobrKurwa"), None);
    }

    // Zig: "OSC: 1337: test invalid key with empty value".
    #[test]
    fn invalid_key_empty_value() {
        assert_eq!(run("1337;BobrKurwa="), None);
    }

    // Zig: "OSC: 1337: test invalid key with non-empty value".
    #[test]
    fn invalid_key_non_empty_value() {
        assert_eq!(run("1337;BobrKurwa=abc123"), None);
    }

    // Zig: "OSC: 1337: test Copy with no value".
    #[test]
    fn copy_with_no_value() {
        assert_eq!(run("1337;Copy"), None);
    }

    // Zig: "OSC: 1337: test Copy with empty value".
    #[test]
    fn copy_with_empty_value() {
        assert_eq!(run("1337;Copy="), None);
    }

    // Zig: "OSC: 1337: test Copy with only prefix colon".
    #[test]
    fn copy_with_only_prefix_colon() {
        assert_eq!(run("1337;Copy=:"), None);
    }

    // Zig: "OSC: 1337: test Copy with question mark".
    #[test]
    fn copy_with_question_mark() {
        assert_eq!(run("1337;Copy=:?"), None);
    }

    // Zig: "OSC: 1337: test Copy with non-empty value that is invalid base64".
    //
    // Ported as `#[ignore]` with the Zig source's own rationale: "For
    // performance reasons, we don't check for valid base64 data right
    // now" (`iterm2.zig` `test "OSC: 1337: test Copy with non-empty value
    // that is invalid base64 data"` is itself skipped via `SkipZigTest`).
    #[test]
    #[ignore = "for performance reasons, we don't check for valid base64 data right now"]
    fn copy_with_invalid_base64() {
        assert_eq!(run("1337;Copy=:abc123"), None);
    }

    // Zig: "OSC: 1337: test Copy with non-empty value that is valid base64 but not prefixed with a colon".
    #[test]
    fn copy_with_valid_base64_not_prefixed() {
        assert_eq!(run("1337;Copy=YWJjMTIz"), None);
    }

    // Zig: "OSC: 1337: test Copy with non-empty value that is valid base64".
    #[test]
    fn copy_with_valid_base64() {
        assert_eq!(
            run("1337;Copy=:YWJjMTIz"),
            Some(Command::ClipboardContents {
                kind: b'c',
                data: "YWJjMTIz".to_string()
            })
        );
    }

    // Zig: "OSC: 1337: test CurrentDir with no value".
    #[test]
    fn current_dir_with_no_value() {
        assert_eq!(run("1337;CurrentDir"), None);
    }

    // Zig: "OSC: 1337: test CurrentDir with empty value".
    #[test]
    fn current_dir_with_empty_value() {
        assert_eq!(run("1337;CurrentDir="), None);
    }

    // Zig: "OSC: 1337: test CurrentDir with non-empty value".
    #[test]
    fn current_dir_with_non_empty_value() {
        assert_eq!(
            run("1337;CurrentDir=abc123"),
            Some(Command::ReportPwd {
                value: "abc123".to_string()
            })
        );
    }
}
