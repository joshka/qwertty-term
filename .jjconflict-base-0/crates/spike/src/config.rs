//! Minimal TOML user config for the spike window: `theme`, `copy-on-select`,
//! and optional `font-size`/`font-family` overrides.
//!
//! This is intentionally small — it is not the eventual `ghostty-config`
//! crate from the rewrite plan (`docs/rewrite-prompt.md`'s config decision
//! table), just enough to give the demo window the maintainer's two
//! actually-used settings (theme, copy-on-select) plus the font knobs the
//! window already half-supports via env vars.
//!
//! Load order: `~/.config/qwertty-term/config.toml`, created with a commented
//! example on first run if missing. Parsing is lenient — unknown keys are
//! ignored (`serde` default) and a malformed file falls back to defaults
//! rather than failing the window startup.

use std::{env, fs, path::PathBuf};

use serde::Deserialize;

/// The config keys the spike window understands. Field names are the TOML
/// keys directly (hyphenated, matching ghostty's own option-name convention
/// per the rewrite prompt's config decision table).
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub(crate) struct Config {
    /// A ghostty theme name (or absolute path to a theme file). See
    /// [`crate::window::theme_file`] for resolution/parsing.
    pub(crate) theme: Option<String>,
    /// When true, finishing a mouse selection immediately copies it to the
    /// system clipboard, in addition to the existing explicit-copy shortcut.
    #[serde(rename = "copy-on-select")]
    pub(crate) copy_on_select: bool,
    #[serde(rename = "font-size")]
    pub(crate) font_size: Option<f32>,
    #[serde(rename = "font-family")]
    pub(crate) font_family: Option<String>,
}

const EXAMPLE_CONFIG: &str = r#"# qwertty-term config
#
# This file is created automatically on first run. Uncomment and edit any of
# the lines below; unknown keys are ignored.

# Theme name, resolved against (in order):
#   1. an absolute path, used as-is
#   2. ~/.config/ghostty/themes/<name>
#   3. the shared ghostty themes directory (see the QWERTTY_TERM_THEMES_DIR
#      override in the README's config section)
# theme = "GruvboxDarkHard"

# Copy the mouse selection to the clipboard as soon as the drag finishes,
# without needing an explicit copy shortcut.
# copy-on-select = false

# Terminal font size in points.
# font-size = 14.0

# Substring to prefer when picking among discovered terminal fonts (falls
# back to the default discovery order when unset or no match is found).
# font-family = "JetBrainsMono Nerd Font Mono"
"#;

/// Load the config from `~/.config/qwertty-term/config.toml`, creating the
/// file (with a commented example, all settings left at their defaults) if
/// it does not exist yet. Returns [`Config::default`] if `$HOME` is unset,
/// the file can't be read, or it fails to parse.
pub(crate) fn load() -> Config {
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

    match toml::from_str(&contents) {
        Ok(config) => config,
        Err(err) => {
            eprintln!("failed to parse {}: {err}", path.display());
            Config::default()
        }
    }
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
    if let Some(dir) = env::var_os("QWERTTY_TERM_CONFIG_DIR") {
        return Some(PathBuf::from(dir).join("config.toml"));
    }
    let home = env::var_os("HOME")?;
    Some(
        PathBuf::from(home)
            .join(".config")
            .join("qwertty-term")
            .join("config.toml"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_copy_on_select_off_and_no_overrides() {
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
        let config: Config = toml::from_str(toml).unwrap();
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
        let toml = r#"
            theme = "Nord"
            some-future-option = "whatever"
        "#;
        let config: Config = toml::from_str(toml).unwrap();
        assert_eq!(config.theme.as_deref(), Some("Nord"));
    }

    #[test]
    fn missing_keys_default() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn malformed_toml_does_not_panic() {
        let result: Result<Config, _> = toml::from_str("theme = [this is not valid");
        assert!(result.is_err());
    }
}
