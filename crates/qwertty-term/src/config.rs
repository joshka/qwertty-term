//! Minimal TOML user config: `theme`, `copy-on-select`, and optional
//! `font-size`/`font-family` overrides.
//!
//! This is a trimmed copy of the spike's `config.rs` (`crates/spike/src/config.rs`).
//! The spike's version is `pub(crate)`, so it can't be reused through a path
//! dependency without modifying the spike (which is read-only reference material
//! for R5); this copy keeps the same TOML shape, keys, and load semantics so a
//! user's `~/.config/qwertty-term/config.toml` works identically under both
//! binaries. It is intentionally *not* the eventual `ghostty-config` crate — see
//! `docs/rewrite-prompt.md`'s config decision table.
//!
//! Load order: `$QWERTTY_TERM_CONFIG_DIR/config.toml` if set, else
//! `~/.config/qwertty-term/config.toml` (created with a commented example on first
//! run if missing). Parsing is lenient — unknown keys are ignored and a
//! malformed file falls back to defaults rather than failing startup.

use std::{env, fs, path::PathBuf};

use serde::Deserialize;

/// The config keys the app understands. Field names are the TOML keys directly
/// (hyphenated, matching ghostty's own option-name convention).
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// A ghostty theme name (or absolute path to a theme file), resolved via
    /// [`crate::theme::load_theme`] into the engine's startup palette + fg/bg/
    /// cursor/selection colors (see `crate::app::Controller::new`).
    pub theme: Option<String>,
    /// When true, finishing a mouse-drag selection immediately copies it to
    /// the system clipboard (see `crate::app::Controller::mouse_to_tab`).
    #[serde(rename = "copy-on-select")]
    pub copy_on_select: bool,
    #[serde(rename = "font-size")]
    pub font_size: Option<f32>,
    #[serde(rename = "font-family")]
    pub font_family: Option<String>,
    /// Per-axis wheel-scroll multipliers (`precision` for trackpad/pixel
    /// deltas, `discrete` for mouse-wheel ticks). Mirrors ghostty's
    /// `mouse-scroll-multiplier` (defaults precision 1.0, discrete 3.0). A TOML
    /// table: `[mouse-scroll-multiplier]` with `precision`/`discrete` keys.
    #[serde(rename = "mouse-scroll-multiplier")]
    pub mouse_scroll_multiplier: MouseScrollMultiplier,
    /// User keybindings, each `"<trigger>=text:<value>"` — a TOML-array-friendly
    /// spelling of ghostty's repeatable `keybind = <trigger>=<action>` config
    /// key. Only the `text:` action subset is supported (see
    /// [`crate::keybind`]); the trigger is `+`-joined modifiers + a key name,
    /// and the `text:` value uses ghostty's escape sequences (`\x1b`, `\r`, `\e`,
    /// `\\`, …). The maintainer's real binding is
    /// `"shift+enter=text:\\x1b\\r"`. Unknown actions/keys are logged and
    /// skipped, never fatal. Parsed into a [`crate::keybind::KeybindTable`] at
    /// startup (`crate::app::Controller::new`).
    #[serde(default)]
    pub keybind: Vec<String>,
    /// The opacity (opposite of transparency) of an *unfocused* split pane, in
    /// `[0.15, 1.0]` (values outside are clamped by [`Config::unfocused_split_opacity`],
    /// matching upstream `Config.zig:4684`). Default 0.7. A value of 1 disables
    /// dimming. Only affects panes in a multi-pane tab; the focused pane and
    /// single-pane tabs never dim. Mirrors ghostty's `unfocused-split-opacity`
    /// (`Config.zig:1071`).
    #[serde(rename = "unfocused-split-opacity")]
    pub unfocused_split_opacity: f64,
    /// The color to dim an unfocused split toward, as `#RRGGBB`/`RRGGBB` or an
    /// X11 color name (parsed via [`qwertty_term_vt::color::Rgb::parse`]). When unset,
    /// defaults to the terminal background (upstream `unfocused-split-fill ??
    /// background`, `Config.zig:1080`). An unparseable value is ignored (falls
    /// back to background).
    #[serde(rename = "unfocused-split-fill")]
    pub unfocused_split_fill: Option<String>,
    /// Which screen edge the quick-terminal dropdown animates from:
    /// `top`/`bottom`/`left`/`right`/`center` (upstream `quick-terminal-position`,
    /// `Config.zig:2624`, default `top`). Unknown values fall back to the
    /// default (see [`Config::quick_terminal_position`]).
    #[serde(rename = "quick-terminal-position")]
    pub quick_terminal_position: Option<String>,
    /// The quick-terminal size, `<primary>[,<secondary>]` where each axis is a
    /// `N%` percentage or `Npx` pixel value (upstream `quick-terminal-size`,
    /// `Config.zig:2647`). Unset axes take the position's default (see
    /// [`crate::quickterm::Size`]).
    #[serde(rename = "quick-terminal-size")]
    pub quick_terminal_size: Option<String>,
    /// The quick-terminal slide animation duration in seconds (upstream
    /// `quick-terminal-animation-duration`, `Config.zig:2720`, default 0.2).
    #[serde(rename = "quick-terminal-animation-duration")]
    pub quick_terminal_animation_duration: f64,
    /// Whether the quick terminal auto-hides when it loses focus (upstream
    /// `quick-terminal-autohide`, `Config.zig:2730`, default `true` on macOS).
    #[serde(rename = "quick-terminal-autohide")]
    pub quick_terminal_autohide: bool,
    /// Which bell features fire on a terminal BEL, as a comma-separated flag
    /// list (`system`/`audio`/`attention`/`title`/`border`, each optionally
    /// `no-`-prefixed; `true`/`false` for all/none). Upstream `bell-features`
    /// (`Config.zig:3121`), default `attention` + `title`. Parsed by
    /// [`Config::bell_features`].
    #[serde(rename = "bell-features")]
    pub bell_features: Option<String>,
}

/// The default `unfocused-split-opacity` (upstream `Config.zig:1071`).
pub const DEFAULT_UNFOCUSED_SPLIT_OPACITY: f64 = 0.7;

/// The default `quick-terminal-animation-duration` in seconds (upstream
/// `Config.zig:2720`).
pub const DEFAULT_QUICK_TERMINAL_ANIMATION_DURATION: f64 = 0.2;

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: None,
            copy_on_select: false,
            font_size: None,
            font_family: None,
            mouse_scroll_multiplier: MouseScrollMultiplier::default(),
            keybind: Vec::new(),
            unfocused_split_opacity: DEFAULT_UNFOCUSED_SPLIT_OPACITY,
            unfocused_split_fill: None,
            quick_terminal_position: None,
            quick_terminal_size: None,
            quick_terminal_animation_duration: DEFAULT_QUICK_TERMINAL_ANIMATION_DURATION,
            // macOS default is `true` (upstream `Config.zig:2730`).
            quick_terminal_autohide: true,
            bell_features: None,
        }
    }
}

impl Config {
    /// The `unfocused-split-opacity`, clamped to `[0.15, 1.0]` exactly as
    /// upstream does (`Config.zig:4684`:
    /// `@min(1.0, @max(0.15, unfocused-split-opacity))`).
    pub fn unfocused_split_opacity(&self) -> f64 {
        self.unfocused_split_opacity.clamp(0.15, 1.0)
    }

    /// The parsed `unfocused-split-fill` color, or `None` to use the terminal
    /// background. An unparseable value logs and falls back to `None`.
    pub fn unfocused_split_fill(&self) -> Option<qwertty_term_vt::color::Rgb> {
        let raw = self.unfocused_split_fill.as_deref()?;
        match qwertty_term_vt::color::Rgb::parse(raw) {
            Ok(rgb) => Some(rgb),
            Err(_) => {
                eprintln!("ignoring invalid unfocused-split-fill: {raw:?}");
                None
            }
        }
    }

    /// The quick-terminal drop position, defaulting to `top` when unset or the
    /// value is unrecognized (upstream `quick-terminal-position`).
    pub fn quick_terminal_position(&self) -> crate::quickterm::Position {
        self.quick_terminal_position
            .as_deref()
            .and_then(crate::quickterm::Position::parse)
            .unwrap_or_default()
    }

    /// The parsed quick-terminal size (all-defaults when unset).
    pub fn quick_terminal_size(&self) -> crate::quickterm::Size {
        self.quick_terminal_size
            .as_deref()
            .map(crate::quickterm::Size::parse)
            .unwrap_or_default()
    }

    /// The parsed `bell-features` (upstream defaults `attention` + `title`
    /// when unset).
    pub fn bell_features(&self) -> crate::bell::BellFeatures {
        self.bell_features
            .as_deref()
            .map(crate::bell::BellFeatures::parse)
            .unwrap_or_default()
    }
}

/// The `[mouse-scroll-multiplier]` config table. Field defaults match
/// upstream `Config.MouseScrollMultiplier` (precision 1.0, discrete 3.0).
#[derive(Debug, Clone, Copy, Deserialize, PartialEq)]
#[serde(default)]
pub struct MouseScrollMultiplier {
    pub precision: f64,
    pub discrete: f64,
}

impl Default for MouseScrollMultiplier {
    fn default() -> Self {
        MouseScrollMultiplier {
            precision: 1.0,
            discrete: 3.0,
        }
    }
}

const EXAMPLE_CONFIG: &str = r#"# qwertty-term config
#
# This file is created automatically on first run. Uncomment and edit any of
# the lines below; unknown keys are ignored.

# Theme name, looked up in ~/.config/qwertty-term/themes/, then the legacy
# ~/.config/ghostty/themes/ directory, then the shared ghostty themes directory
# (or an absolute path to a theme file).
# theme = "GruvboxDarkHard"

# Copy the mouse selection to the clipboard as soon as the drag finishes.
# copy-on-select = false

# Terminal font size in points.
# font-size = 14.0

# Substring to prefer when picking among discovered terminal fonts.
# font-family = "JetBrainsMono Nerd Font Mono"

# Wheel-scroll multipliers. `precision` scales trackpad (pixel) deltas;
# `discrete` scales mouse-wheel ticks (rows per detent).
# [mouse-scroll-multiplier]
# precision = 1.0
# discrete = 3.0

# Keybindings. Each entry is "<trigger>=text:<value>": a `+`-joined chord
# (modifiers shift/ctrl/alt/cmd + a key name like enter/tab/escape/space, a
# letter, a digit, f1-f12, or an arrow) that sends literal <value> bytes to the
# focused pane, BEFORE the normal key encoder. Only the `text:` action is
# supported. The value uses ghostty's escapes: \x1b (ESC), \r, \n, \t, \e (ESC),
# \\ (backslash). Unknown triggers/actions are ignored with a warning.
# Example (send ESC+CR on Shift+Enter — many TUIs read this as "soft newline"):
# keybind = ["shift+enter=text:\\x1b\\r"]

# Opacity of an unfocused split pane (multi-pane tabs only). 1.0 disables the
# dimming; values are clamped to [0.15, 1.0]. Default 0.7.
# unfocused-split-opacity = 0.7

# Color unfocused splits are dimmed toward (an X11 color name or RRGGBB hex).
# Defaults to the terminal background when unset.
# unfocused-split-fill = "black"

# Quick terminal (dropdown). Position is the screen edge it slides from:
# top (default), bottom, left, right, or center. Size is "<primary>[,<secondary>]"
# where each axis is a percentage (20%) or pixels (300px); unset axes use the
# position default. Animation duration is in seconds; autohide drops the window
# when it loses focus.
# quick-terminal-position = "top"
# quick-terminal-size = "25%"
# quick-terminal-animation-duration = 0.2
# quick-terminal-autohide = true

# Which bell features fire on a terminal BEL: a comma-separated list of
# system / audio / attention / title / border, each optionally "no-"-prefixed
# to disable (or "true"/"false" for all/none). Default: attention + title.
# bell-features = "system, attention, title"
"#;

/// Load the config, creating the file with a commented example if it does not
/// exist. Returns [`Config::default`] if `$HOME` (and `$QWERTTY_TERM_CONFIG_DIR`)
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
    fn default_config_is_empty() {
        let config = Config::default();
        assert!(!config.copy_on_select);
        assert_eq!(config.theme, None);
        assert_eq!(config.font_size, None);
        assert_eq!(config.font_family, None);
        assert_eq!(
            config.mouse_scroll_multiplier,
            MouseScrollMultiplier::default()
        );
        assert_eq!(config.mouse_scroll_multiplier.precision, 1.0);
        assert_eq!(config.mouse_scroll_multiplier.discrete, 3.0);
        assert!(config.keybind.is_empty());
        // Unfocused-split dimming: upstream defaults (opacity 0.7, no fill).
        assert_eq!(config.unfocused_split_opacity, 0.7);
        assert_eq!(config.unfocused_split_opacity(), 0.7);
        assert_eq!(config.unfocused_split_fill, None);
        assert_eq!(config.unfocused_split_fill(), None);
        // Quick terminal: upstream defaults (top, all-default size, 0.2s, autohide).
        assert_eq!(
            config.quick_terminal_position(),
            crate::quickterm::Position::Top
        );
        assert_eq!(
            config.quick_terminal_size(),
            crate::quickterm::Size::default()
        );
        assert_eq!(config.quick_terminal_animation_duration, 0.2);
        assert!(config.quick_terminal_autohide);
        // Bell: upstream default is attention + title.
        assert_eq!(config.bell_features(), crate::bell::BellFeatures::default());
        assert!(config.bell_features().attention && config.bell_features().title);
    }

    #[test]
    fn parses_bell_features_over_defaults() {
        let config = parse("bell-features = \"system, no-title\"\n").unwrap();
        let f = config.bell_features();
        assert!(f.system && f.attention && !f.title);
    }

    #[test]
    fn parses_quick_terminal_keys() {
        let toml = "\
            quick-terminal-position = \"bottom\"\n\
            quick-terminal-size = \"50%,500px\"\n\
            quick-terminal-animation-duration = 0.35\n\
            quick-terminal-autohide = false\n";
        let config = parse(toml).unwrap();
        assert_eq!(
            config.quick_terminal_position(),
            crate::quickterm::Position::Bottom
        );
        assert_eq!(
            config.quick_terminal_size(),
            crate::quickterm::Size {
                primary: Some(crate::quickterm::Dim::Percentage(50.0)),
                secondary: Some(crate::quickterm::Dim::Pixels(500)),
            }
        );
        assert_eq!(config.quick_terminal_animation_duration, 0.35);
        assert!(!config.quick_terminal_autohide);
    }

    #[test]
    fn quick_terminal_unknown_position_falls_back_to_top() {
        let config = parse("quick-terminal-position = \"sideways\"\n").unwrap();
        assert_eq!(
            config.quick_terminal_position(),
            crate::quickterm::Position::Top
        );
    }

    #[test]
    fn parses_unfocused_split_keys() {
        let toml = "unfocused-split-opacity = 0.5\nunfocused-split-fill = \"#112233\"\n";
        let config = parse(toml).unwrap();
        assert_eq!(config.unfocused_split_opacity, 0.5);
        assert_eq!(config.unfocused_split_opacity(), 0.5);
        assert_eq!(
            config.unfocused_split_fill(),
            Some(qwertty_term_vt::color::Rgb::new(0x11, 0x22, 0x33))
        );
    }

    #[test]
    fn unfocused_split_opacity_clamps_out_of_range() {
        // Below 0.15 clamps up; above 1.0 clamps down (upstream Config.zig:4684).
        let low = parse("unfocused-split-opacity = 0.0\n").unwrap();
        assert_eq!(low.unfocused_split_opacity(), 0.15);
        let high = parse("unfocused-split-opacity = 2.0\n").unwrap();
        assert_eq!(high.unfocused_split_opacity(), 1.0);
        // A negative value clamps to the floor too.
        let neg = parse("unfocused-split-opacity = -1.0\n").unwrap();
        assert_eq!(neg.unfocused_split_opacity(), 0.15);
    }

    #[test]
    fn unfocused_split_fill_accepts_x11_name_and_ignores_garbage() {
        let named = parse("unfocused-split-fill = \"black\"\n").unwrap();
        assert_eq!(
            named.unfocused_split_fill(),
            Some(qwertty_term_vt::color::Rgb::new(0, 0, 0))
        );
        // An unparseable value falls back to None (background).
        let bad = parse("unfocused-split-fill = \"not-a-color-zzz\"\n").unwrap();
        assert_eq!(bad.unfocused_split_fill(), None);
    }

    #[test]
    fn parses_keybind_array_including_maintainer_binding() {
        // TOML-friendly array spelling; the maintainer's real binding needs the
        // backslashes escaped in the TOML string, so `\\x1b\\r` in the file is
        // the two-token `\x1b\r` value the keybind parser then unescapes.
        let toml = r#"keybind = ["shift+enter=text:\\x1b\\r", "ctrl+a=text:\\e[H"]"#;
        let config = parse(toml).unwrap();
        assert_eq!(
            config.keybind,
            vec![
                "shift+enter=text:\\x1b\\r".to_string(),
                "ctrl+a=text:\\e[H".to_string()
            ]
        );
        // End-to-end: the array parses into a live table with the right bytes.
        let table = crate::keybind::KeybindTable::parse(&config.keybind);
        assert_eq!(table.len(), 2);
        assert_eq!(
            table.resolve(
                qwertty_term_input::key::Key::Enter,
                crate::tabkeys::TabMods {
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(&b"\x1b\r"[..])
        );
    }

    #[test]
    fn parses_mouse_scroll_multiplier_table() {
        let toml = "[mouse-scroll-multiplier]\nprecision = 1.5\ndiscrete = 2.5\n";
        let config = parse(toml).unwrap();
        assert_eq!(config.mouse_scroll_multiplier.precision, 1.5);
        assert_eq!(config.mouse_scroll_multiplier.discrete, 2.5);
    }

    #[test]
    fn mouse_scroll_multiplier_partial_table_keeps_defaults() {
        let config = parse("[mouse-scroll-multiplier]\ndiscrete = 5.0\n").unwrap();
        assert_eq!(config.mouse_scroll_multiplier.precision, 1.0);
        assert_eq!(config.mouse_scroll_multiplier.discrete, 5.0);
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
