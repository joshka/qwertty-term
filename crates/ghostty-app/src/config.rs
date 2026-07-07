//! Minimal TOML user config: `theme`, `copy-on-select`, and optional
//! `font-size`/`font-family` overrides.
//!
//! This is a trimmed copy of the spike's `config.rs` (`crates/spike/src/config.rs`).
//! The spike's version is `pub(crate)`, so it can't be reused through a path
//! dependency without modifying the spike (which is read-only reference material
//! for R5); this copy keeps the same TOML shape, keys, and load semantics so a
//! user's `~/.config/ghostty-rs/config.toml` works identically under both
//! binaries. It is intentionally *not* the eventual `ghostty-config` crate — see
//! `docs/rewrite-prompt.md`'s config decision table.
//!
//! Load order: `$GHOSTTY_RS_CONFIG_DIR/config.toml` if set, else
//! `~/.config/ghostty-rs/config.toml` (created with a commented example on first
//! run if missing). Parsing is lenient — unknown keys are ignored and a
//! malformed file falls back to defaults rather than failing startup.

use std::{env, fs, path::PathBuf};

use serde::Deserialize;

/// The config keys the app understands. Field names are the TOML keys directly
/// (hyphenated, matching ghostty's own option-name convention).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// A ghostty theme name (or absolute path to a theme file). Not resolved to
    /// colors in this chunk (theme-file parsing is deferred; see the deferrals
    /// note in `docs/analysis/renderer-r5.md`) — kept so a user's config round-
    /// trips and so the field is available when theme resolution lands.
    pub theme: Option<String>,
    /// When true, finishing a mouse selection immediately copies it to the
    /// system clipboard. Selection is deferred for R5 (documented), so this is
    /// stored and surfaced but not yet acted on.
    #[serde(rename = "copy-on-select")]
    pub copy_on_select: bool,
    #[serde(rename = "font-size")]
    pub font_size: Option<f32>,
    #[serde(rename = "font-family")]
    pub font_family: Option<String>,
}

const EXAMPLE_CONFIG: &str = r#"# ghostty-rs config
#
# This file is created automatically on first run. Uncomment and edit any of
# the lines below; unknown keys are ignored.

# Theme name (resolution is not wired in the current build; the value is kept
# for forward compatibility).
# theme = "GruvboxDarkHard"

# Copy the mouse selection to the clipboard as soon as the drag finishes
# (selection itself is not yet implemented in the native app).
# copy-on-select = false

# Terminal font size in points.
# font-size = 14.0

# Substring to prefer when picking among discovered terminal fonts.
# font-family = "JetBrainsMono Nerd Font Mono"
"#;

/// Load the config, creating the file with a commented example if it does not
/// exist. Returns [`Config::default`] if `$HOME` (and `$GHOSTTY_RS_CONFIG_DIR`)
/// are unset, the file can't be read, or it fails to parse.
pub fn load() -> Config {
    let Some(path) = config_path() else {
        return Config::default();
    };

    if !path.exists() {
        create_default_config(&path);
        return Config::default();
    }

    let Ok(contents) = fs::read_to_string(&path) else {
        return Config::default();
    };

    parse(&contents).unwrap_or_else(|err| {
        eprintln!("failed to parse {}: {err}", path.display());
        Config::default()
    })
}

/// Parse a TOML string into a [`Config`]. Split out so it is unit-testable
/// without touching the filesystem.
pub fn parse(contents: &str) -> Result<Config, toml::de::Error> {
    toml::from_str(contents)
}

fn create_default_config(path: &PathBuf) {
    if let Some(parent) = path.parent()
        && let Err(err) = fs::create_dir_all(parent)
    {
        eprintln!("failed to create {}: {err}", parent.display());
        return;
    }
    if let Err(err) = fs::write(path, EXAMPLE_CONFIG) {
        eprintln!("failed to write {}: {err}", path.display());
    }
}

fn config_path() -> Option<PathBuf> {
    if let Some(dir) = env::var_os("GHOSTTY_RS_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("config.toml"));
    }
    let home = env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("ghostty-rs")
            .join("config.toml"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_empty() {
        let config = Config::default();
        assert!(!config.copy_on_select);
        assert_eq!(config.theme, None);
        assert_eq!(config.font_size, None);
        assert_eq!(config.font_family, None);
    }

    #[test]
    fn parses_all_known_keys() {
        let toml = r#"
            theme = "Nord"
            copy-on-select = true
            font-size = 16.5
            font-family = "JetBrainsMono Nerd Font Mono"
        "#;
        let config = parse(toml).unwrap();
        assert_eq!(config.theme.as_deref(), Some("Nord"));
        assert!(config.copy_on_select);
        assert_eq!(config.font_size, Some(16.5));
        assert_eq!(
            config.font_family.as_deref(),
            Some("JetBrainsMono Nerd Font Mono")
        );
    }

    #[test]
    fn unknown_keys_are_ignored() {
        let config = parse("theme = \"Nord\"\nsome-future-option = \"x\"\n").unwrap();
        assert_eq!(config.theme.as_deref(), Some("Nord"));
    }

    #[test]
    fn malformed_toml_is_an_error_not_a_panic() {
        assert!(parse("theme = [this is not valid").is_err());
    }
}
