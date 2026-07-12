//! Paste-safety classification for `clipboard-paste-protection`: decides
//! whether pasted data is "unsafe" and should require confirmation before it
//! reaches the pty. Pure + unit-tested; [`crate::app`] shows the confirmation
//! dialog and performs the paste.
//!
//! Port of upstream `input/paste.zig` `isSafe` + the paste-protection gate in
//! `Surface.zig` (~L5862): a paste is unsafe when it contains a newline (the
//! "copy/paste attack" — text with newlines auto-executes commands) or a
//! bracketed-paste **end** sequence (`ESC [ 201 ~`, which could break out of a
//! bracketed frame). Bracketed pastes (the running program enabled bracketed
//! paste) are trusted when `clipboard-paste-bracketed-safe` is set — unless the
//! data itself smuggles in the end sequence, which is never trusted.

/// The paste-protection configuration (upstream `clipboard-paste-protection`
/// + `clipboard-paste-bracketed-safe`, both default true).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PasteProtection {
    /// Require confirmation before pasting unsafe data.
    pub enabled: bool,
    /// Trust bracketed pastes (framed by the running program) as safe.
    pub bracketed_safe: bool,
}

impl Default for PasteProtection {
    fn default() -> Self {
        PasteProtection {
            enabled: true,
            bracketed_safe: true,
        }
    }
}

/// The bracketed-paste end sequence. Data containing this is never trusted (it
/// could prematurely close a bracketed frame).
const BRACKET_END: &str = "\x1b[201~";

/// Whether `data` is intrinsically safe to paste: no newline and no bracketed-
/// paste end sequence. Port of `paste.isSafe`.
pub fn is_safe(data: &str) -> bool {
    !data.contains('\n') && !data.contains(BRACKET_END)
}

/// Whether pasting `data` should be gated behind a confirmation dialog.
/// `bracketed` is whether the running program has bracketed paste mode on.
/// Port of the `Surface.zig` paste-protection block:
///
/// - protection off → never unsafe;
/// - bracketed + contains the end sequence → always unsafe;
/// - bracketed + `bracketed_safe` → safe;
/// - otherwise → unsafe iff not [`is_safe`].
pub fn is_unsafe(data: &str, bracketed: bool, cfg: PasteProtection) -> bool {
    if !cfg.enabled {
        return false;
    }
    if bracketed {
        if data.contains(BRACKET_END) {
            return true;
        }
        if cfg.bracketed_safe {
            return false;
        }
    }
    !is_safe(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(enabled: bool, bracketed_safe: bool) -> PasteProtection {
        PasteProtection {
            enabled,
            bracketed_safe,
        }
    }

    #[test]
    fn is_safe_flags_newline_and_bracket_end() {
        assert!(is_safe("ls -la"));
        assert!(!is_safe("rm -rf /\nyes"));
        assert!(!is_safe("payload\x1b[201~more"));
        assert!(is_safe("")); // empty is safe
    }

    #[test]
    fn protection_off_is_always_safe() {
        // Even a multiline paste is allowed when protection is disabled.
        assert!(!is_unsafe("a\nb", false, cfg(false, true)));
        assert!(!is_unsafe("a\nb", true, cfg(false, false)));
    }

    #[test]
    fn non_bracketed_multiline_is_unsafe() {
        let c = cfg(true, true);
        // Single line → safe; multiline → unsafe.
        assert!(!is_unsafe("echo hi", false, c));
        assert!(is_unsafe("echo hi\necho bye", false, c));
    }

    #[test]
    fn bracketed_is_trusted_when_configured() {
        // Bracketed + bracketed_safe → a multiline paste is safe (the program
        // frames it).
        assert!(!is_unsafe("a\nb", true, cfg(true, true)));
        // Bracketed but NOT trusting bracketed → falls back to is_safe.
        assert!(is_unsafe("a\nb", true, cfg(true, false)));
    }

    #[test]
    fn bracket_end_sequence_is_never_trusted() {
        // Even bracketed + bracketed_safe, an embedded end sequence is unsafe.
        assert!(is_unsafe("safe\x1b[201~evil", true, cfg(true, true)));
    }
}
