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

use std::collections::{HashSet, VecDeque};
use std::path::Path;
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
    /// skipped, never fatal. Parsed into the ported `Binding.zig`
    /// [`Set`](qwertty_term_input::binding::Set) at startup
    /// (`crate::app::Controller::new`, via `crate::keybind::build_set`).
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
    /// What a right-click does: `context-menu` (default) / `paste` / `copy` /
    /// `copy-or-paste` / `ignore` (upstream `right-click-action`,
    /// `Config.zig:2432`). Parsed by [`Config::right_click_action`].
    #[serde(rename = "right-click-action")]
    pub right_click_action: Option<String>,
    /// Hide the mouse cursor while typing, revealing it on the next mouse move
    /// (upstream `mouse-hide-while-typing`, `Config.zig:921`, default false).
    #[serde(rename = "mouse-hide-while-typing")]
    pub mouse_hide_while_typing: bool,
    /// Require confirmation before pasting text that appears unsafe (contains a
    /// newline or a bracketed-paste end sequence) — the copy/paste-attack guard
    /// (upstream `clipboard-paste-protection`, `Config.zig:2372`, default true).
    #[serde(rename = "clipboard-paste-protection")]
    pub clipboard_paste_protection: bool,
    /// Trust bracketed pastes (framed by the running program) as safe (upstream
    /// `clipboard-paste-bracketed-safe`, `Config.zig:2378`, default true).
    #[serde(rename = "clipboard-paste-bracketed-safe")]
    pub clipboard_paste_bracketed_safe: bool,
    /// Trim trailing whitespace from copied lines that have other content
    /// (upstream `clipboard-trim-trailing-spaces`, `Config.zig:2367`, default
    /// true).
    #[serde(rename = "clipboard-trim-trailing-spaces")]
    pub clipboard_trim_trailing_spaces: bool,
    /// Clear the text selection when the user types (upstream
    /// `selection-clear-on-typing`, `Config.zig:724`, default true).
    #[serde(rename = "selection-clear-on-typing")]
    pub selection_clear_on_typing: bool,
    /// Allow applications to post desktop notifications via OSC 9 / OSC 777
    /// (upstream `desktop-notifications`, `Config.zig:3690`, default true).
    /// Gated in the core drain (matching upstream `Surface.zig:1080`): when
    /// false, OSC 9/777 notifications are dropped before delivery. Note real
    /// macOS delivery needs a signed app bundle (see ADR 0003); unbundled the
    /// app falls back to a dock attention request.
    #[serde(rename = "desktop-notifications")]
    pub desktop_notifications: bool,
    /// When to notify on an OSC 133 command completion: `never` (default) /
    /// `unfocused` / `always` (upstream `notify-on-command-finish`,
    /// `Config.zig:1217`). Requires shell integration (OSC 133). Parsed by
    /// [`Config::notify_on_command_finish`].
    #[serde(rename = "notify-on-command-finish")]
    pub notify_on_command_finish: Option<String>,
    /// Which effects fire on command finish, as a comma list of `bell`/`notify`
    /// (each `no-`-prefixable; `true`/`false`/`none` for all/none). Upstream
    /// `notify-on-command-finish-action` (`Config.zig:1231`), default `bell`
    /// on + `notify` off. Parsed by [`Config::notify_on_command_finish_action`].
    #[serde(rename = "notify-on-command-finish-action")]
    pub notify_on_command_finish_action: Option<String>,
    /// Minimum command duration (seconds) before a finish notifies (upstream
    /// `notify-on-command-finish-after`, `Config.zig:1268`, default 5s). A
    /// reduced form of upstream's `Duration` string — plain seconds, matching
    /// the other duration keys in this config.
    #[serde(rename = "notify-on-command-finish-after")]
    pub notify_on_command_finish_after: f64,
    /// Whether to show the in-surface OSC 9;4 progress bar (upstream
    /// `progress-style`, `Config.zig:3697`, default true). When false, progress
    /// reports are ignored (no bar). A reduced form of upstream's style enum —
    /// a plain on/off toggle.
    #[serde(rename = "progress-style")]
    pub progress_style: bool,
    /// When to confirm before closing a surface with a running process:
    /// `false` / `true` (default) / `always` (upstream `confirm-close-surface`,
    /// `Config.zig:2498`). "Running" is decided by shell-integration prompt
    /// state (OSC 133); with no shell integration it errs toward confirming.
    /// Parsed by [`Config::confirm_close_surface`].
    #[serde(rename = "confirm-close-surface")]
    pub confirm_close_surface: Option<String>,
    /// Whether to quit the app after the last window/surface closes (upstream
    /// `quit-after-last-window-closed`, `Config.zig:2509`, default **false** on
    /// macOS — the standard "app stays running with no windows" behavior).
    #[serde(rename = "quit-after-last-window-closed")]
    pub quit_after_last_window_closed: bool,
    /// Initial window width in **cells** (grid columns). `0` (default) = the
    /// app's default size. Upstream `window-width` (`Config.zig:2171`); only
    /// affects the first window.
    #[serde(rename = "window-width")]
    pub window_width: u32,
    /// Initial window height in **cells** (grid rows). `0` (default) = the
    /// app's default size. Upstream `window-height` (`Config.zig:2170`).
    #[serde(rename = "window-height")]
    pub window_height: u32,
    /// Initial window x position in pixels from the visible screen's top-left.
    /// Both x and y must be set to take effect (upstream `window-position-x`,
    /// `Config.zig:2196`).
    #[serde(rename = "window-position-x")]
    pub window_position_x: Option<i32>,
    /// Initial window y position in pixels from the visible screen's top-left
    /// (upstream `window-position-y`, `Config.zig:2197`).
    #[serde(rename = "window-position-y")]
    pub window_position_y: Option<i32>,
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
            right_click_action: None,
            mouse_hide_while_typing: false,
            // All clipboard-hardening keys default on (upstream defaults).
            clipboard_paste_protection: true,
            clipboard_paste_bracketed_safe: true,
            clipboard_trim_trailing_spaces: true,
            selection_clear_on_typing: true,
            // Applications may post OSC 9/777 desktop notifications by default
            // (upstream `Config.zig:3690`).
            desktop_notifications: true,
            // Command-finish notifications are off by default (upstream `never`).
            notify_on_command_finish: None,
            notify_on_command_finish_action: None,
            notify_on_command_finish_after: 5.0,
            // The OSC 9;4 progress bar is shown by default (upstream).
            progress_style: true,
            // Confirm before closing a surface with a running process (upstream
            // default `true`).
            confirm_close_surface: None,
            // macOS default: stay running after the last window closes
            // (upstream `Config.zig:2509` → false on macOS).
            quit_after_last_window_closed: false,
            window_width: 0,
            window_height: 0,
            window_position_x: None,
            window_position_y: None,
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

    /// The parsed `notify-on-command-finish` mode (default `Never`).
    pub fn notify_on_command_finish(&self) -> crate::notify::NotifyOnCommandFinish {
        self.notify_on_command_finish
            .as_deref()
            .map(crate::notify::NotifyOnCommandFinish::parse)
            .unwrap_or_default()
    }

    /// The parsed `notify-on-command-finish-action` (default `bell` on).
    pub fn notify_on_command_finish_action(&self) -> crate::notify::CommandFinishAction {
        self.notify_on_command_finish_action
            .as_deref()
            .map(crate::notify::CommandFinishAction::parse)
            .unwrap_or_default()
    }

    /// The `notify-on-command-finish-after` threshold as a `Duration`
    /// (non-negative; default 5s).
    pub fn notify_on_command_finish_after(&self) -> std::time::Duration {
        std::time::Duration::from_secs_f64(self.notify_on_command_finish_after.max(0.0))
    }

    /// The parsed `confirm-close-surface` mode (default `OnRunning`).
    pub fn confirm_close_surface(&self) -> ConfirmCloseSurface {
        self.confirm_close_surface
            .as_deref()
            .map(ConfirmCloseSurface::parse)
            .unwrap_or_default()
    }

    /// The parsed `right-click-action` (defaults to `context-menu`).
    pub fn right_click_action(&self) -> crate::context_menu::RightClickAction {
        self.right_click_action
            .as_deref()
            .map(crate::context_menu::RightClickAction::parse)
            .unwrap_or_default()
    }

    /// The paste-protection settings (`clipboard-paste-protection` +
    /// `clipboard-paste-bracketed-safe`).
    pub fn paste_protection(&self) -> crate::paste::PasteProtection {
        crate::paste::PasteProtection {
            enabled: self.clipboard_paste_protection,
            bracketed_safe: self.clipboard_paste_bracketed_safe,
        }
    }

    /// The configured initial window size in `(cols, rows)`, or `None` to use
    /// the app default. Only returns a size when *both* dimensions are set
    /// (a lone dimension is ignored, matching upstream's all-or-nothing
    /// geometry). Clamped to the upstream minimum of 10×4.
    pub fn initial_window_cells(&self) -> Option<(u32, u32)> {
        if self.window_width == 0 || self.window_height == 0 {
            return None;
        }
        Some((self.window_width.max(10), self.window_height.max(4)))
    }

    /// The configured initial window position `(x, y)` in pixels, or `None`.
    /// Both `window-position-x` and `-y` must be set (upstream ignores a lone
    /// value).
    pub fn initial_window_position(&self) -> Option<(i32, i32)> {
        match (self.window_position_x, self.window_position_y) {
            (Some(x), Some(y)) => Some((x, y)),
            _ => None,
        }
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

/// When to confirm before closing a surface/tab/window with a running process
/// (`confirm-close-surface`, upstream `ConfirmCloseSurface`, `Config.zig:5242`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConfirmCloseSurface {
    /// Never confirm — close immediately.
    Never,
    /// Confirm only when a process is running (the cursor is not at a shell
    /// prompt). The default.
    #[default]
    OnRunning,
    /// Always confirm, even at a prompt.
    Always,
}

impl ConfirmCloseSurface {
    /// Parse the config value (`false`/`true`/`always`, matching upstream's
    /// tri-state enum spelled with the bool-looking words). Unknown values fall
    /// back to the default `OnRunning` (`true`).
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "false" | "never" => Self::Never,
            "always" => Self::Always,
            _ => Self::OnRunning,
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

# What a right-click does: context-menu (default), paste, copy, copy-or-paste,
# or ignore. And whether to hide the mouse cursor while typing (shown again on
# the next mouse move).
# right-click-action = "context-menu"
# mouse-hide-while-typing = false

# Clipboard hardening (all default true): confirm before pasting unsafe
# (multiline) text; trust bracketed pastes as safe; trim trailing whitespace
# from copied lines; clear the selection when you start typing.
# clipboard-paste-protection = true
# clipboard-paste-bracketed-safe = true
# clipboard-trim-trailing-spaces = true
# selection-clear-on-typing = true

# Allow apps to post desktop notifications via OSC 9 / OSC 777 (default true).
# Real macOS notifications require a signed app bundle (see ADR 0003); when
# unbundled the app falls back to a dock attention request.
# desktop-notifications = true

# Notify when a shell command finishes (needs OSC 133 shell integration):
# never (default) / unfocused / always. The action is a comma list of bell /
# notify (default bell), and only commands running at least
# notify-on-command-finish-after seconds notify.
# notify-on-command-finish = "never"
# notify-on-command-finish-action = "bell"
# notify-on-command-finish-after = 5

# Show the in-surface OSC 9;4 progress bar (default true; set false to ignore
# progress reports).
# progress-style = true

# Confirm before closing a surface/tab/window that has a running process:
# "false" (never), "true" (default; only when a command is running per shell
# integration), or "always". Without shell integration this errs toward
# confirming.
# confirm-close-surface = "true"

# Window state. quit-after-last-window-closed keeps the standard macOS behavior
# of staying running with no windows when false (default); set true to quit.
# window-width/height are the initial size in cells (0 = default). Both
# window-position-x/y (pixels from the screen's top-left) must be set to apply.
# quit-after-last-window-closed = false
# window-width = 0
# window-height = 0
# window-position-x = 100
# window-position-y = 50
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

    load_from(&path)
}

/// Load a config file plus its `config-file` includes, merged into one [`Config`].
///
/// Include semantics mirror upstream (`docs/analysis/config-core.md` §3): the
/// `config-file` directive (a string or array of strings) is processed
/// **deferred**, breadth-first, *after* the file that declares it — so an include
/// overrides keys set in its parent. Paths resolve relative to the *including*
/// file's directory (`~/` expanded), a `?` prefix marks an include optional
/// (missing is silently skipped), and a **cycle** (a file already loaded) is
/// skipped with a warning. The merge is last-wins for scalars and **appends** for
/// arrays (so `keybind` accumulates across files). Any per-file read/parse error
/// is warned and skipped — a bad include never fails startup.
fn load_from(root: &Path) -> Config {
    let mut merged = toml::Table::new();
    let mut queue: VecDeque<(PathBuf, bool)> = VecDeque::new();
    queue.push_back((root.to_path_buf(), false));
    let mut seen: HashSet<PathBuf> = HashSet::new();

    while let Some((path, optional)) = queue.pop_front() {
        // Cycle / diamond dedup on the canonical path.
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        if !seen.insert(canonical) {
            eprintln!("config-file: cycle detected, skipping {}", path.display());
            continue;
        }
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(err) => {
                if !optional {
                    eprintln!("config-file: cannot read {}: {err}", path.display());
                }
                continue;
            }
        };
        let mut table: toml::Table = match contents.parse() {
            Ok(table) => table,
            Err(err) => {
                eprintln!("failed to parse {}: {err}", path.display());
                continue;
            }
        };
        // Enqueue this file's includes (relative to its own dir), removing the
        // directive so it never reaches `Config` deserialization.
        let dir = path.parent().unwrap_or_else(|| Path::new("."));
        for spec in take_includes(&mut table) {
            let (rel, opt) = parse_include_spec(&spec);
            queue.push_back((resolve_include(dir, rel), opt));
        }
        merge_tables(&mut merged, table);
    }

    toml::Value::Table(merged).try_into().unwrap_or_else(|err| {
        eprintln!("failed to load merged config: {err}");
        Config::default()
    })
}

/// Remove the `config-file` include directive from `table`, returning the listed
/// paths (accepts a single string or an array of strings).
fn take_includes(table: &mut toml::Table) -> Vec<String> {
    match table.remove("config-file") {
        Some(toml::Value::String(s)) => vec![s],
        Some(toml::Value::Array(arr)) => arr
            .into_iter()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
        _ => Vec::new(),
    }
}

/// A leading `?` marks an include optional (missing → silently skipped).
fn parse_include_spec(spec: &str) -> (&str, bool) {
    match spec.strip_prefix('?') {
        Some(rest) => (rest, true),
        None => (spec, false),
    }
}

/// Resolve an include path: `~/` expansion, then (if still relative) against the
/// including file's directory.
fn resolve_include(dir: &Path, rel: &str) -> PathBuf {
    let expanded = match rel.strip_prefix("~/") {
        Some(rest) => match env::var_os("HOME") {
            Some(home) => PathBuf::from(home).join(rest),
            None => PathBuf::from(rel),
        },
        None => PathBuf::from(rel),
    };
    if expanded.is_absolute() {
        expanded
    } else {
        dir.join(expanded)
    }
}

/// Merge `other` into `base`: recurse into sub-tables, **append** arrays (so
/// repeatables like `keybind` accumulate), and otherwise last-wins.
fn merge_tables(base: &mut toml::Table, other: toml::Table) {
    for (key, value) in other {
        match (base.get_mut(&key), value) {
            (Some(toml::Value::Table(bt)), toml::Value::Table(ot)) => merge_tables(bt, ot),
            (Some(toml::Value::Array(ba)), toml::Value::Array(oa)) => ba.extend(oa),
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
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
        // Mouse: right-click shows the context menu by default; no hide-on-type.
        assert_eq!(
            config.right_click_action(),
            crate::context_menu::RightClickAction::ContextMenu
        );
        assert!(!config.mouse_hide_while_typing);
        // Clipboard hardening: all upstream defaults are on.
        assert_eq!(
            config.paste_protection(),
            crate::paste::PasteProtection::default()
        );
        assert!(config.clipboard_trim_trailing_spaces);
        assert!(config.selection_clear_on_typing);
        // Desktop notifications: allowed by default (upstream).
        assert!(config.desktop_notifications);
        // Command-finish notifications: off by default, bell action, 5s.
        assert_eq!(
            config.notify_on_command_finish(),
            crate::notify::NotifyOnCommandFinish::Never
        );
        assert_eq!(
            config.notify_on_command_finish_action(),
            crate::notify::CommandFinishAction::default()
        );
        assert_eq!(
            config.notify_on_command_finish_after(),
            std::time::Duration::from_secs(5)
        );
        // The progress bar is shown by default.
        assert!(config.progress_style);
        // Confirm-close defaults to on-running (upstream `true`).
        assert_eq!(
            config.confirm_close_surface(),
            crate::config::ConfirmCloseSurface::OnRunning
        );
        // Window state: macOS default doesn't quit after last window; no
        // configured geometry.
        assert!(!config.quit_after_last_window_closed);
        assert_eq!(config.initial_window_cells(), None);
        assert_eq!(config.initial_window_position(), None);
    }

    #[test]
    fn parses_window_state_keys() {
        let config = parse(
            "quit-after-last-window-closed = true\n\
             window-width = 120\n\
             window-height = 40\n\
             window-position-x = 100\n\
             window-position-y = 50\n",
        )
        .unwrap();
        assert!(config.quit_after_last_window_closed);
        assert_eq!(config.initial_window_cells(), Some((120, 40)));
        assert_eq!(config.initial_window_position(), Some((100, 50)));
    }

    #[test]
    fn window_geometry_is_all_or_nothing_and_clamped() {
        // A lone width (no height) is ignored.
        let one = parse("window-width = 120\n").unwrap();
        assert_eq!(one.initial_window_cells(), None);
        // A lone position axis is ignored.
        let pos = parse("window-position-x = 100\n").unwrap();
        assert_eq!(pos.initial_window_position(), None);
        // Below the 10×4 minimum clamps up.
        let tiny = parse("window-width = 3\nwindow-height = 1\n").unwrap();
        assert_eq!(tiny.initial_window_cells(), Some((10, 4)));
    }

    #[test]
    fn parses_clipboard_hardening_keys() {
        let config = parse(
            "clipboard-paste-protection = false\n\
             clipboard-paste-bracketed-safe = false\n\
             clipboard-trim-trailing-spaces = false\n\
             selection-clear-on-typing = false\n",
        )
        .unwrap();
        assert!(!config.clipboard_paste_protection);
        assert_eq!(
            config.paste_protection(),
            crate::paste::PasteProtection {
                enabled: false,
                bracketed_safe: false,
            }
        );
        assert!(!config.clipboard_trim_trailing_spaces);
        assert!(!config.selection_clear_on_typing);
    }

    #[test]
    fn parses_desktop_notifications_key() {
        let off = parse("desktop-notifications = false\n").unwrap();
        assert!(!off.desktop_notifications);
        // Absent → upstream default (allowed).
        assert!(parse("").unwrap().desktop_notifications);
    }

    #[test]
    fn parses_notify_on_command_finish_keys() {
        let config = parse(
            "notify-on-command-finish = \"always\"\n\
             notify-on-command-finish-action = \"bell, notify\"\n\
             notify-on-command-finish-after = 2.5\n",
        )
        .unwrap();
        assert_eq!(
            config.notify_on_command_finish(),
            crate::notify::NotifyOnCommandFinish::Always
        );
        assert_eq!(
            config.notify_on_command_finish_action(),
            crate::notify::CommandFinishAction {
                bell: true,
                notify: true
            }
        );
        assert_eq!(
            config.notify_on_command_finish_after(),
            std::time::Duration::from_secs_f64(2.5)
        );
    }

    #[test]
    fn parses_progress_style_key() {
        assert!(!parse("progress-style = false\n").unwrap().progress_style);
        assert!(parse("").unwrap().progress_style);
    }

    #[test]
    fn parses_confirm_close_surface_key() {
        use crate::config::ConfirmCloseSurface;
        assert_eq!(
            parse("confirm-close-surface = \"false\"\n")
                .unwrap()
                .confirm_close_surface(),
            ConfirmCloseSurface::Never
        );
        assert_eq!(
            parse("confirm-close-surface = \"always\"\n")
                .unwrap()
                .confirm_close_surface(),
            ConfirmCloseSurface::Always
        );
        // Absent + unknown → default (on-running).
        assert_eq!(
            parse("").unwrap().confirm_close_surface(),
            ConfirmCloseSurface::OnRunning
        );
    }

    #[test]
    fn parses_mouse_keys() {
        let config =
            parse("right-click-action = \"paste\"\nmouse-hide-while-typing = true\n").unwrap();
        assert_eq!(
            config.right_click_action(),
            crate::context_menu::RightClickAction::Paste
        );
        assert!(config.mouse_hide_while_typing);
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
        // End-to-end: the array parses into a live keybind Set with the right bytes.
        let set = crate::keybind::build_set(&config.keybind);
        assert_eq!(
            crate::keybind::resolve_text_bytes(
                &set,
                qwertty_term_input::key::Key::Enter,
                crate::tabkeys::TabMods {
                    shift: true,
                    ..Default::default()
                }
            ),
            Some(b"\x1b\r".to_vec())
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

    // ---- config-file includes ----

    #[test]
    fn merge_tables_appends_arrays_and_overrides_scalars() {
        let mut base: toml::Table = "theme = \"A\"\nkeybind = [\"a=text:x\"]\n".parse().unwrap();
        let other: toml::Table = "theme = \"B\"\nkeybind = [\"b=text:y\"]\n".parse().unwrap();
        merge_tables(&mut base, other);
        // Scalar: last wins.
        assert_eq!(base["theme"].as_str(), Some("B"));
        // Array: appended.
        let binds: Vec<_> = base["keybind"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(binds, vec!["a=text:x", "b=text:y"]);
    }

    #[test]
    fn parse_include_spec_handles_optional_prefix() {
        assert_eq!(parse_include_spec("foo.toml"), ("foo.toml", false));
        assert_eq!(parse_include_spec("?foo.toml"), ("foo.toml", true));
    }

    /// Make a fresh temp dir for a filesystem test (no external `tempfile` dep).
    fn scratch_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("qwertty_cfg_test_{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn load_from_merges_includes_with_override_and_append() {
        let dir = scratch_dir("includes");
        fs::write(
            dir.join("config.toml"),
            "theme = \"Base\"\nkeybind = [\"a=text:x\"]\nconfig-file = [\"extra.toml\"]\n",
        )
        .unwrap();
        // The include is processed after the parent, so it overrides `theme`…
        fs::write(
            dir.join("extra.toml"),
            "theme = \"Override\"\nkeybind = [\"b=text:y\"]\n",
        )
        .unwrap();

        let config = load_from(&dir.join("config.toml"));
        assert_eq!(config.theme.as_deref(), Some("Override"));
        // …and `keybind` accumulates across both files.
        assert_eq!(config.keybind, vec!["a=text:x", "b=text:y"]);
    }

    #[test]
    fn load_from_breaks_include_cycles() {
        let dir = scratch_dir("cycle");
        fs::write(
            dir.join("config.toml"),
            "theme = \"A\"\nconfig-file = [\"b.toml\"]\n",
        )
        .unwrap();
        fs::write(dir.join("b.toml"), "config-file = [\"config.toml\"]\n").unwrap();
        // Must terminate (cycle detected) and still load the base.
        let config = load_from(&dir.join("config.toml"));
        assert_eq!(config.theme.as_deref(), Some("A"));
    }

    #[test]
    fn load_from_optional_missing_include_is_skipped() {
        let dir = scratch_dir("optional");
        fs::write(
            dir.join("config.toml"),
            "theme = \"A\"\nconfig-file = [\"?does-not-exist.toml\"]\n",
        )
        .unwrap();
        let config = load_from(&dir.join("config.toml"));
        assert_eq!(config.theme.as_deref(), Some("A"));
    }
}
