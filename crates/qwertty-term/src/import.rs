//! `+import-ghostty-config`: convert a real Ghostty config into qwertty-term's
//! TOML config.
//!
//! Ghostty's config is a line-oriented `key = value` file (repeatable keys allowed).
//! We keep upstream's kebab-case key names verbatim (so muscle memory transfers),
//! emit every key that maps 1:1 onto a real qwertty-term `Config` field as TOML,
//! flag the handful of keys whose *value format* differs from ours as a
//! `# needs manual conversion` comment, and preserve every other key as a
//! `# unsupported (not yet)` comment — so nothing is silently lost and the user
//! can see exactly what did and didn't carry over. See
//! `docs/analysis/import-ghostty-config.md` for the full 204-key mapping table.
//!
//! The [`SUPPORTED`] table is the single source of truth for what carries over;
//! a drift-guard test asserts every entry names a real `Config` field.

use std::collections::HashMap;

/// How a supported key's value is rendered as TOML.
#[derive(Clone, Copy)]
enum Kind {
    /// Quoted string (`theme`, `font-family`, …). Covers `Option<String>` /
    /// `String` fields — including the ones we parse into an enum later
    /// (`confirm-close-surface`, `bell-features`), which accept any string.
    Str,
    /// Bare number (`font-size`, `window-width`, …). Emitted only when the value
    /// actually parses as a number; otherwise it falls through to a comment.
    Num,
    /// Boolean, normalized to `true`/`false`.
    Bool,
}

/// Ghostty keys whose value maps directly onto a real qwertty-term `Config`
/// field. `keybind` and `copy-on-select` are handled specially (see `convert`)
/// and are intentionally absent here. Keep this in sync with the `Config` struct
/// in `config.rs`; the `every_supported_key_is_a_real_config_field` test fails
/// if an entry names a field that doesn't exist.
const SUPPORTED: &[(&str, Kind)] = &[
    // Fonts & text
    ("font-family", Kind::Str),
    ("font-size", Kind::Num),
    // Colors / theme
    ("theme", Kind::Str),
    ("cursor-color", Kind::Str),
    // Mouse & clipboard
    ("mouse-hide-while-typing", Kind::Bool),
    ("clipboard-paste-protection", Kind::Bool),
    ("clipboard-paste-bracketed-safe", Kind::Bool),
    ("clipboard-trim-trailing-spaces", Kind::Bool),
    ("right-click-action", Kind::Str),
    // Selection
    ("selection-clear-on-typing", Kind::Bool),
    // Splits
    ("unfocused-split-opacity", Kind::Num),
    ("unfocused-split-fill", Kind::Str),
    // Quick terminal
    ("quick-terminal-position", Kind::Str),
    ("quick-terminal-size", Kind::Str),
    ("quick-terminal-autohide", Kind::Bool),
    // Bell
    ("bell-features", Kind::Str),
    // Notifications
    ("desktop-notifications", Kind::Bool),
    ("notify-on-command-finish", Kind::Str),
    ("notify-on-command-finish-action", Kind::Str),
    // Confirm-close / resize overlay
    ("confirm-close-surface", Kind::Str),
    ("resize-overlay", Kind::Str),
    ("resize-overlay-position", Kind::Str),
    // Window / lifecycle
    ("quit-after-last-window-closed", Kind::Bool),
    ("window-width", Kind::Num),
    ("window-height", Kind::Num),
    ("window-position-x", Kind::Num),
    ("window-position-y", Kind::Num),
];

/// Real qwertty-term fields whose Ghostty value *format* differs from ours, so a
/// verbatim copy wouldn't parse. We emit these as comments carrying the original
/// value plus a note, rather than a broken setting.
const NEEDS_MANUAL: &[(&str, &str)] = &[
    (
        "mouse-scroll-multiplier",
        "qwertty-term uses a [mouse-scroll-multiplier] table (precision/discrete), not a scalar",
    ),
    (
        "resize-overlay-duration",
        "qwertty-term expects a plain number, not a Ghostty duration string (e.g. `250ms`)",
    ),
    (
        "quick-terminal-animation-duration",
        "qwertty-term expects a plain number, not a Ghostty duration string (e.g. `250ms`)",
    ),
    (
        "notify-on-command-finish-after",
        "qwertty-term expects a plain number, not a Ghostty duration string (e.g. `250ms`)",
    ),
];

/// Parse Ghostty's `key = value` line syntax into ordered `(key, value)` pairs.
/// Blank lines and `#` comments are skipped; a fully double-quoted value has its
/// surrounding quotes stripped; the split is on the *first* `=` (so a `keybind`
/// value like `shift+enter=text:…` keeps its own `=`).
pub fn parse_ghostty_config(contents: &str) -> Vec<(String, String)> {
    let mut entries = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_string();
        let mut value = value.trim().to_string();
        if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
            value = value[1..value.len() - 1].to_string();
        }
        if !key.is_empty() {
            entries.push((key, value));
        }
    }
    entries
}

/// Convert a Ghostty config's text into a qwertty-term `config.toml` string.
pub fn convert(contents: &str) -> String {
    let supported: HashMap<&str, Kind> = SUPPORTED.iter().copied().collect();
    let manual: HashMap<&str, &str> = NEEDS_MANUAL.iter().copied().collect();

    let mut out = vec![
        "# Imported from a Ghostty config by `qwertty-term +import-ghostty-config`.".to_string(),
    ];
    let mut keybinds: Vec<String> = Vec::new();
    // Scalar settings, deduped last-wins while preserving first-seen order (a
    // repeated Ghostty scalar would otherwise emit a duplicate TOML key).
    let mut scalar_order: Vec<String> = Vec::new();
    let mut scalar_line: HashMap<String, String> = HashMap::new();
    let mut comments: Vec<String> = Vec::new();

    let mut set_scalar = |key: &str, line: String| {
        if !scalar_line.contains_key(key) {
            scalar_order.push(key.to_string());
        }
        scalar_line.insert(key.to_string(), line);
    };

    for (key, value) in parse_ghostty_config(contents) {
        match key.as_str() {
            // Repeatable keybind → a single TOML array (trigger/action grammar is
            // byte-identical between the two tools; only TOML quoting changes).
            "keybind" => keybinds.push(value),

            // `copy-on-select` is a tri-state enum upstream (`false`/`true`/
            // `clipboard`); our field is a bool, so `clipboard`/`true` → true
            // (issue #22 tracks widening it to the enum to preserve `clipboard`).
            "copy-on-select" => {
                let on = matches!(value.as_str(), "clipboard" | "true");
                set_scalar("copy-on-select", format!("copy-on-select = {on}"));
            }

            other if supported.contains_key(other) => match supported[other] {
                Kind::Str => set_scalar(other, format!("{other} = {}", toml_string(&value))),
                Kind::Bool => set_scalar(other, format!("{other} = {}", value == "true")),
                Kind::Num => {
                    if value.parse::<f64>().is_ok() {
                        set_scalar(other, format!("{other} = {value}"));
                    } else {
                        comments.push(format!(
                            "# unsupported value (not a number): {other} = {value}"
                        ));
                    }
                }
            },

            other if manual.contains_key(other) => {
                comments.push(format!(
                    "# needs manual conversion ({}): {other} = {value}",
                    manual[other]
                ));
            }

            // Anything else: keep it as a comment so nothing is silently dropped.
            _ => comments.push(format!("# unsupported (not yet): {key} = {value}")),
        }
    }

    for key in &scalar_order {
        out.push(scalar_line[key].clone());
    }

    if !keybinds.is_empty() {
        let array = keybinds
            .iter()
            .map(|k| toml_string(k))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!("keybind = [{array}]"));
    }

    if !comments.is_empty() {
        out.push(String::new());
        out.push("# Not carried over (preserved here for reference):".to_string());
        out.extend(comments);
    }

    out.push(String::new());
    out.join("\n")
}

/// Serialize a string as a TOML string (quoted, with escapes handled) — used so
/// a `keybind` value's backslashes (`text:\x1b\r`) round-trip correctly (TOML
/// picks a literal single-quoted form for values with backslashes).
fn toml_string(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_splits_on_first_equals_and_strips_quotes() {
        let entries = parse_ghostty_config(
            "# a comment\nfont-family = \"FiraCode Nerd Font Mono\"\n\nkeybind = shift+enter=text:\\x1b\\r\n",
        );
        assert_eq!(
            entries,
            vec![
                (
                    "font-family".to_string(),
                    "FiraCode Nerd Font Mono".to_string()
                ),
                // First `=` splits key/value; the keybind's own `=` stays.
                (
                    "keybind".to_string(),
                    "shift+enter=text:\\x1b\\r".to_string()
                ),
            ]
        );
    }

    /// Acceptance test: the maintainer's real Ghostty config round-trips to clean
    /// TOML (docs/analysis/import-ghostty-config.md §5).
    #[test]
    fn converts_the_maintainers_real_config() {
        let ghostty = "\
# ghostty template header comment
font-family = FiraCode Nerd Font Mono
theme = Aardvark Ink
copy-on-select = clipboard
font-size = 16
keybind = shift+enter=text:\\x1b\\r
";
        let toml = convert(ghostty);
        assert!(
            toml.contains("font-family = \"FiraCode Nerd Font Mono\""),
            "{toml}"
        );
        assert!(toml.contains("theme = \"Aardvark Ink\""), "{toml}");
        assert!(toml.contains("copy-on-select = true"), "{toml}");
        assert!(toml.contains("font-size = 16"), "{toml}");
        // TOML emits a literal (single-quoted) string when the value has
        // backslashes, so the keybind's `\x1b\r` carries over unescaped.
        assert!(
            toml.contains(r"keybind = ['shift+enter=text:\x1b\r']"),
            "{toml}"
        );
        // And the emitted TOML parses back into our Config without error.
        assert!(crate::config::parse(&toml).is_ok(), "{toml}");
    }

    #[test]
    fn maps_bool_numeric_and_string_keys() {
        let toml = convert(
            "mouse-hide-while-typing = true\nwindow-width = 120\nright-click-action = paste\n",
        );
        assert!(toml.contains("mouse-hide-while-typing = true"), "{toml}");
        assert!(toml.contains("window-width = 120"), "{toml}");
        assert!(toml.contains("right-click-action = \"paste\""), "{toml}");
        assert!(crate::config::parse(&toml).is_ok(), "{toml}");
    }

    #[test]
    fn format_mismatched_keys_become_manual_comments() {
        let toml = convert("mouse-scroll-multiplier = 3.0\nresize-overlay-duration = 4s\n");
        assert!(
            toml.contains("# needs manual conversion")
                && toml.contains("mouse-scroll-multiplier = 3.0"),
            "{toml}"
        );
        assert!(toml.contains("resize-overlay-duration = 4s"), "{toml}");
        // The emitted doc is still valid (the mismatched keys are comments).
        assert!(crate::config::parse(&toml).is_ok(), "{toml}");
    }

    #[test]
    fn non_numeric_value_for_a_numeric_key_is_commented_not_emitted() {
        let toml = convert("font-size = huge\n");
        assert!(
            toml.contains("# unsupported value (not a number): font-size = huge"),
            "{toml}"
        );
        assert!(crate::config::parse(&toml).is_ok(), "{toml}");
    }

    #[test]
    fn unsupported_keys_become_comments() {
        let toml = convert("background-blur = true\ntheme = Nord\n");
        assert!(toml.contains("theme = \"Nord\""));
        assert!(
            toml.contains("# unsupported (not yet): background-blur = true"),
            "{toml}"
        );
    }

    #[test]
    fn multiple_keybinds_accumulate_into_one_array() {
        let toml = convert("keybind = ctrl+a=text:x\nkeybind = ctrl+b=text:y\n");
        assert!(
            toml.contains("keybind = [\"ctrl+a=text:x\", \"ctrl+b=text:y\"]"),
            "{toml}"
        );
    }

    #[test]
    fn repeated_scalar_key_is_deduped_last_wins() {
        let toml = convert("theme = First\ntheme = Second\n");
        assert!(toml.contains("theme = \"Second\""), "{toml}");
        assert!(!toml.contains("First"), "{toml}");
        // No duplicate `theme` key → the doc parses.
        assert!(crate::config::parse(&toml).is_ok(), "{toml}");
    }

    /// Drift guard: every key we claim to support (and every "needs manual" key)
    /// must name a *real* `Config` field. `Config` doesn't `deny_unknown_fields`,
    /// so a genuinely-unknown key parses fine — but a real field rejects a
    /// deliberately mistyped value, which an ignored unknown key would not. No
    /// single value is wrong for *every* field type (a struct field like
    /// `mouse-scroll-multiplier` accepts both a table `{}` and an empty seq
    /// `[]`), so we probe with two — `[]` and `false` — and require at least one
    /// to error: every real field rejects one of them, while an unknown key
    /// silently ignores both. `keybind` (a real array field) is intentionally
    /// not guarded here.
    #[test]
    fn every_supported_key_is_a_real_config_field() {
        let keys = SUPPORTED
            .iter()
            .map(|(k, _)| *k)
            .chain(NEEDS_MANUAL.iter().map(|(k, _)| *k))
            .chain(std::iter::once("copy-on-select"));
        for key in keys {
            let rejects_a_wrong_type = ["[]", "false"]
                .iter()
                .any(|v| crate::config::parse(&format!("{key} = {v}")).is_err());
            assert!(
                rejects_a_wrong_type,
                "`{key}` is not a real Config field — the import table has \
                 drifted from config.rs"
            );
        }
    }
}
