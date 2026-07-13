//! `+import-ghostty-config`: convert a real Ghostty config into qwertty-term's
//! TOML config.
//!
//! Ghostty's config is a line-oriented `key = value` file (repeatable keys allowed).
//! We keep upstream's kebab-case key names verbatim (so muscle memory transfers),
//! emit the keys qwertty-term supports as TOML, and leave every other key as a
//! `# ` comment that preserves the original value — so nothing is silently lost
//! and the user can see exactly what didn't carry over. See
//! `docs/analysis/import-ghostty-config.md` for the full 204-key mapping table;
//! this slice maps the common keys and comments the long tail (extended
//! incrementally as more options are wired).

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
    let entries = parse_ghostty_config(contents);
    let mut out = vec![
        "# Imported from a Ghostty config by `qwertty-term +import-ghostty-config`.".to_string(),
    ];
    let mut keybinds: Vec<String> = Vec::new();
    let mut unsupported: Vec<String> = Vec::new();

    for (key, value) in entries {
        match key.as_str() {
            // Repeatable keybind → a single TOML array (trigger/action grammar is
            // byte-identical between the two tools; only TOML quoting changes).
            "keybind" => keybinds.push(value),

            // String-valued keys we support: emit quoted.
            "theme"
            | "font-family"
            | "right-click-action"
            | "quick-terminal-position"
            | "quick-terminal-size"
            | "bell-features"
            | "unfocused-split-fill" => out.push(format!("{key} = {}", toml_string(&value))),

            // Numeric keys: emit bare (Ghostty and TOML share the literal form).
            "font-size" | "unfocused-split-opacity" | "quick-terminal-animation-duration" => {
                out.push(format!("{key} = {value}"))
            }

            // Boolean keys: normalize `true`/`false`.
            "mouse-hide-while-typing"
            | "quick-terminal-autohide"
            | "clipboard-paste-protection"
            | "clipboard-paste-bracketed-safe"
            | "clipboard-trim-trailing-spaces"
            | "selection-clear-on-typing"
            | "desktop-notifications" => out.push(format!("{key} = {}", value == "true")),

            // `copy-on-select` is a tri-state enum upstream (`false`/`true`/
            // `clipboard`); our field is a bool, so `clipboard`/`true` → true
            // (issue #22 tracks widening it to the enum to preserve `clipboard`).
            "copy-on-select" => {
                let on = matches!(value.as_str(), "clipboard" | "true");
                out.push(format!("copy-on-select = {on}"));
            }

            // Anything else: keep it as a comment so nothing is silently dropped.
            _ => unsupported.push(format!("# unsupported (not yet): {key} = {value}")),
        }
    }

    if !keybinds.is_empty() {
        let array = keybinds
            .iter()
            .map(|k| toml_string(k))
            .collect::<Vec<_>>()
            .join(", ");
        out.push(format!("keybind = [{array}]"));
    }

    if !unsupported.is_empty() {
        out.push(String::new());
        out.push("# Keys not yet supported by qwertty-term (preserved as comments):".to_string());
        out.extend(unsupported);
    }

    out.push(String::new());
    out.join("\n")
}

/// Serialize a string as a TOML basic string (quoted, with escapes) — used so a
/// `keybind` value's backslashes (`text:\x1b\r`) round-trip correctly.
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
}
