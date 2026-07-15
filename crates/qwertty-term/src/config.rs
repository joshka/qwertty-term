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
//! Load order (later overrides earlier): the default-location files —
//! `~/.config/qwertty-term/config.toml`, then (macOS) `~/Library/Application
//! Support/qwertty-term/config.toml` — each expanded with its `config-file`
//! includes, then CLI `--key=value` overrides. `$QWERTTY_TERM_CONFIG_DIR`, if set,
//! collapses the default list to a single `<dir>/config.toml`. A commented
//! example is written on first run when no file exists anywhere. Parsing is
//! lenient — unknown keys are ignored and a malformed file falls back to defaults
//! rather than failing startup.

use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::OnceLock;
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
    // Per-metric nudges applied to the computed cell metrics
    // (`adjust-*`, upstream `Config.zig:429`+). Each value is a `MetricModifier`:
    // a bare integer of pixels (`2`, `-1`) or a percentage delta (`10%`, `-5%`;
    // "20%" = 20% larger, not 20% of). Parsed into a
    // [`qwertty_term_font::metrics::ModifierSet`] by [`Config::metric_modifiers`]
    // and applied to the font `Metrics` at grid-build time. An unparseable value
    // is logged and skipped. See `docs/analysis/font-foundations.md` §modifiers.
    #[serde(rename = "adjust-cell-width")]
    pub adjust_cell_width: Option<String>,
    #[serde(rename = "adjust-cell-height")]
    pub adjust_cell_height: Option<String>,
    #[serde(rename = "adjust-font-baseline")]
    pub adjust_font_baseline: Option<String>,
    #[serde(rename = "adjust-underline-position")]
    pub adjust_underline_position: Option<String>,
    #[serde(rename = "adjust-underline-thickness")]
    pub adjust_underline_thickness: Option<String>,
    #[serde(rename = "adjust-strikethrough-position")]
    pub adjust_strikethrough_position: Option<String>,
    #[serde(rename = "adjust-strikethrough-thickness")]
    pub adjust_strikethrough_thickness: Option<String>,
    #[serde(rename = "adjust-overline-position")]
    pub adjust_overline_position: Option<String>,
    #[serde(rename = "adjust-overline-thickness")]
    pub adjust_overline_thickness: Option<String>,
    #[serde(rename = "adjust-cursor-thickness")]
    pub adjust_cursor_thickness: Option<String>,
    #[serde(rename = "adjust-cursor-height")]
    pub adjust_cursor_height: Option<String>,
    #[serde(rename = "adjust-box-thickness")]
    pub adjust_box_thickness: Option<String>,
    #[serde(rename = "adjust-icon-height")]
    pub adjust_icon_height: Option<String>,
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
    /// The text-cursor color, as `#RRGGBB`/`RRGGBB` or an X11 color name (parsed
    /// via [`qwertty_term_vt::color::Rgb::parse`]). Overrides the theme's
    /// `cursor-color`; when unset the theme value (or the built-in default) is
    /// used. Mirrors upstream `cursor-color` (`Config.zig:600`). A running
    /// program's OSC 12 still overrides this at runtime. An unparseable value is
    /// ignored (falls back to the theme/default). See [`Config::cursor_color`].
    #[serde(rename = "cursor-color")]
    pub cursor_color: Option<String>,
    /// Default terminal background color, as `#RRGGBB`/`RRGGBB` or an X11 color
    /// name. Overrides the theme's `background`; a running program's OSC 11 still
    /// overrides it at runtime. Upstream `background` (`Config.zig:585`). An
    /// unparseable value is ignored. See [`Config::background`].
    pub background: Option<String>,
    /// Default terminal foreground (text) color, same format as `background`.
    /// Overrides the theme's `foreground`; OSC 10 still wins at runtime. Upstream
    /// `foreground` (`Config.zig:580`). See [`Config::foreground`].
    pub foreground: Option<String>,
    /// Selection highlight background, same format as `background`. Overrides the
    /// theme's `selection-background`. When both selection colors resolve
    /// (config or theme) the selection uses them; otherwise it inverts the cell.
    /// Upstream `selection-background` (`Config.zig:625`).
    #[serde(rename = "selection-background")]
    pub selection_background: Option<String>,
    /// Selection highlight foreground (text) color, same format as `background`.
    /// Overrides the theme's `selection-foreground`. Upstream
    /// `selection-foreground` (`Config.zig:620`).
    #[serde(rename = "selection-foreground")]
    pub selection_foreground: Option<String>,
    /// Per-index palette color overrides, each `"N=<color>"` where `N` is a
    /// palette index `0..=255` and `<color>` is `#RRGGBB`/`RRGGBB`/an X11 name.
    /// A TOML-array spelling of upstream's repeatable `palette = N=color`
    /// (`Config.zig:560`). Applied on top of the theme's palette; a running
    /// program's OSC 4 still overrides at runtime. Bad entries are logged and
    /// skipped. See [`Config::palette_overrides`].
    #[serde(default)]
    pub palette: Vec<String>,
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
    /// Focus the pane the mouse moves over, without clicking (upstream
    /// `focus-follows-mouse`, `Config.zig:2348`, default false).
    #[serde(rename = "focus-follows-mouse")]
    pub focus_follows_mouse: bool,
    /// What a middle-click does: `primary-paste` (default) / `ignore` (upstream
    /// `middle-click-action`, `Config.zig:2442`). Parsed by
    /// [`Config::middle_click_action`].
    #[serde(rename = "middle-click-action")]
    pub middle_click_action: Option<String>,
    /// Whether the terminal program may capture the shift modifier during mouse
    /// reporting (`false`/`true`/`always`/`never`, default `false`) — upstream
    /// `mouse-shift-capture` (`Config.zig:964`). Parsed by
    /// [`Config::mouse_shift_capture`].
    #[serde(rename = "mouse-shift-capture")]
    pub mouse_shift_capture: Option<String>,
    /// Characters that mark word boundaries during double/triple-click word
    /// selection — each character in the string becomes a boundary (the null
    /// char U+0000 is always one). Unset uses the built-in default set (upstream
    /// `selection-word-chars`, `Config.zig:762`). Parsed by
    /// [`Config::selection_word_chars_codepoints`].
    #[serde(rename = "selection-word-chars")]
    pub selection_word_chars: Option<String>,
    /// The double/triple-click detection window in milliseconds; `0` (the
    /// default) uses the OS click interval (falling back to 500 ms) — upstream
    /// `click-repeat-interval`, `Config.zig:2448`. Read via
    /// [`Config::click_repeat_interval`].
    #[serde(rename = "click-repeat-interval")]
    pub click_repeat_interval: u32,
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
    /// Clear the text selection after an explicit copy (the `copy_to_clipboard`
    /// action / Cmd-C / menu / context-menu Copy). Does **not** apply to
    /// `copy-on-select` (upstream `selection-clear-on-copy`, `Config.zig:736`,
    /// default false).
    #[serde(rename = "selection-clear-on-copy")]
    pub selection_clear_on_copy: bool,
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
    /// Whether macOS restores windows across quit/relaunch: `default` (follow
    /// the system setting) / `never` / `always` (upstream `window-save-state`,
    /// `Config.zig:2229`). Parsed by [`Config::window_save_state`].
    #[serde(rename = "window-save-state")]
    pub window_save_state: Option<String>,
    /// When to show the `cols ⨯ rows` resize overlay: `always` / `never` /
    /// `after-first` (default, upstream `resize-overlay`, `Config.zig:2292`).
    /// Parsed by [`Config::resize_overlay`].
    #[serde(rename = "resize-overlay")]
    pub resize_overlay: Option<String>,
    /// Where the resize overlay sits: `center` (default) / `top-left` /
    /// `top-center` / `top-right` / `bottom-left` / `bottom-center` /
    /// `bottom-right` (upstream `resize-overlay-position`, `Config.zig:2306`).
    /// Parsed by [`Config::resize_overlay_position`].
    #[serde(rename = "resize-overlay-position")]
    pub resize_overlay_position: Option<String>,
    /// How long the resize overlay stays after the last resize, in milliseconds
    /// (upstream `resize-overlay-duration`, `Config.zig:2340`, default 750ms).
    #[serde(rename = "resize-overlay-duration")]
    pub resize_overlay_duration: f64,
    /// Whether to quit the app after the last window/surface closes (upstream
    /// `quit-after-last-window-closed`, `Config.zig:2509`, default **false** on
    /// macOS — the standard "app stays running with no windows" behavior).
    #[serde(rename = "quit-after-last-window-closed")]
    pub quit_after_last_window_closed: bool,
    /// A fixed window/tab title that overrides the program-set title. When set
    /// (and non-empty), the window title is forced to this value and OSC 0/2
    /// title changes from the running program are ignored; a blank value resets
    /// to normal (program-driven) titling. Upstream `title` (`Config.zig:1484`).
    /// See [`Config::forced_title`].
    pub title: Option<String>,
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
    /// The window subtitle: `false` (default, no subtitle) or `working-directory`
    /// (the focused surface's cwd). Upstream `window-subtitle`
    /// (`Config.zig:2109`, enum `WindowSubtitle` at `Config.zig:5284`) ships this
    /// on GTK only; we provide the same config surface natively on macOS via
    /// `NSWindow.subtitle`. Parsed by [`Config::window_subtitle`].
    #[serde(rename = "window-subtitle")]
    pub window_subtitle: Option<String>,
    /// Where a new tab opens relative to the current one: `current` (default,
    /// immediately after the active tab) or `end` (after the last tab in the
    /// group). Upstream `window-new-tab-position` (`Config.zig:2242`, enum
    /// `WindowNewTabPosition` at `Config.zig:9181`; applied in
    /// `TerminalController.swift:456`). Parsed by
    /// [`Config::window_new_tab_position`].
    #[serde(rename = "window-new-tab-position")]
    pub window_new_tab_position: Option<String>,
    /// The tab bar visibility policy: `auto` (default, the macOS convention —
    /// visible only at 2+ tabs), `always` (visible even with one tab), or `never`
    /// (native tabbing disabled; new tabs open as windows). Upstream
    /// `window-show-tab-bar` (`Config.zig:2265`, enum `WindowShowTabBar` at
    /// `Config.zig:9193`) is a GTK feature; we map the same enum onto macOS's
    /// `NSWindowTabbingMode`. Parsed by [`Config::window_show_tab_bar`].
    #[serde(rename = "window-show-tab-bar")]
    pub window_show_tab_bar: Option<String>,
    /// Resize the window in whole-cell increments of the focused surface's cell
    /// size (`NSWindow.contentResizeIncrements`) instead of pixel increments.
    /// Upstream `window-step-resize` (`Config.zig:2234`, default `false`;
    /// applied in `BaseTerminalController.swift:884`).
    #[serde(rename = "window-step-resize")]
    pub window_step_resize: bool,
    /// Whether the window casts a drop shadow (`NSWindow.hasShadow`). Upstream
    /// `macos-window-shadow` (`Config.zig:3335`, default `true`; applied in
    /// `TerminalWindow.swift:476`).
    #[serde(rename = "macos-window-shadow")]
    pub macos_window_shadow: bool,
    /// The traffic-light window buttons: `visible` (default) or `hidden` (hide
    /// close/miniaturize/zoom). Upstream `macos-window-buttons`
    /// (`Config.zig:3218`, enum `MacWindowButtons` at `Config.zig:8946`; applied
    /// in `TerminalWindow.swift:129`). Parsed by [`Config::macos_window_buttons`].
    #[serde(rename = "macos-window-buttons")]
    pub macos_window_buttons: Option<String>,
    /// The window appearance theme: `auto` (default; on macOS, light/dark by the
    /// terminal background luminosity), `system`, `light`, or `dark`. `ghostty`
    /// (config-colored titlebar) is a Linux-only upstream mode; on macOS it
    /// falls back to `system`. Upstream `window-theme` (`Config.zig:2129`, enum
    /// `WindowTheme` at `Config.zig:8931`; macOS appearance mapping in
    /// `NSAppearance+Extension.swift`). Parsed by [`Config::window_theme`].
    #[serde(rename = "window-theme")]
    pub window_theme: Option<String>,
    /// Enable answering the window-title report query (`CSI 21 t`). Default
    /// **false** — the reply can leak sensitive information and, with a
    /// maliciously crafted title, enable code execution, so upstream gates it
    /// off (upstream `title-report`, `Config.zig:2389`, applied in
    /// `Surface.zig:983`). The engine defaults this on for libghostty-vt
    /// parity, so the app must set it explicitly (see
    /// [`crate::engine::Engine::set_title_reporting`]).
    #[serde(rename = "title-report")]
    pub title_report: bool,
    /// The answerback string sent when the terminal receives `ENQ` (`0x05`)
    /// from the running program. Empty (the default) sends nothing. Upstream
    /// `enquiry-response` (`Config.zig:3735`).
    #[serde(rename = "enquiry-response")]
    pub enquiry_response: Option<String>,
    /// The reply format for OSC 4/10/11 color queries: `none` (no reply),
    /// `8-bit` (unscaled `rr/gg/bb`), or `16-bit` (scaled `rrrr/gggg/bbbb`,
    /// the default). Upstream `osc-color-report-format` (`Config.zig:2919`,
    /// enum `OSCColorReportFormat` at `Config.zig:8924`). Parsed by
    /// [`Config::osc_color_report_format`].
    #[serde(rename = "osc-color-report-format")]
    pub osc_color_report_format: Option<String>,
    /// The maximum bytes of image data (e.g. Kitty graphics) per terminal
    /// screen, `u32` (max 4 GiB). Default 320 MB; `0` disables image protocols.
    /// Applied per screen, so the effective per-surface limit is double.
    /// Upstream `image-storage-limit` (`Config.zig:2398`).
    #[serde(rename = "image-storage-limit")]
    pub image_storage_limit: u32,
    /// The maximum bytes of scrollback retained per terminal surface. Default
    /// 10 MB (upstream `scrollback-limit`, `Config.zig:1387`). When the limit
    /// is reached the oldest lines are pruned. Applied at surface construction;
    /// a reload only affects new surfaces (matching upstream).
    #[serde(rename = "scrollback-limit")]
    pub scrollback_limit: usize,
    /// Allow the "KAM" mode (ANSI mode 2, `disable_keyboard`) to suppress
    /// keyboard input at the running program's request. Default **false**
    /// (upstream `vt-kam-allowed`, `Config.zig:2927`; the gate lives in
    /// `Surface.zig:2699`). Rarely wanted; leave off unless you know you need
    /// KAM.
    #[serde(rename = "vt-kam-allowed")]
    pub vt_kam_allowed: bool,
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
            adjust_cell_width: None,
            adjust_cell_height: None,
            adjust_font_baseline: None,
            adjust_underline_position: None,
            adjust_underline_thickness: None,
            adjust_strikethrough_position: None,
            adjust_strikethrough_thickness: None,
            adjust_overline_position: None,
            adjust_overline_thickness: None,
            adjust_cursor_thickness: None,
            adjust_cursor_height: None,
            adjust_box_thickness: None,
            adjust_icon_height: None,
            mouse_scroll_multiplier: MouseScrollMultiplier::default(),
            keybind: Vec::new(),
            unfocused_split_opacity: DEFAULT_UNFOCUSED_SPLIT_OPACITY,
            unfocused_split_fill: None,
            cursor_color: None,
            background: None,
            foreground: None,
            selection_background: None,
            selection_foreground: None,
            palette: Vec::new(),
            quick_terminal_position: None,
            quick_terminal_size: None,
            quick_terminal_animation_duration: DEFAULT_QUICK_TERMINAL_ANIMATION_DURATION,
            // macOS default is `true` (upstream `Config.zig:2730`).
            quick_terminal_autohide: true,
            bell_features: None,
            right_click_action: None,
            mouse_hide_while_typing: false,
            focus_follows_mouse: false,
            middle_click_action: None,
            mouse_shift_capture: None,
            // Unset → the built-in word-boundary set; click interval → OS default.
            selection_word_chars: None,
            click_repeat_interval: 0,
            // All clipboard-hardening keys default on (upstream defaults).
            clipboard_paste_protection: true,
            clipboard_paste_bracketed_safe: true,
            clipboard_trim_trailing_spaces: true,
            selection_clear_on_typing: true,
            selection_clear_on_copy: false,
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
            window_save_state: None,
            // Resize overlay: after-first / center / 750ms (upstream defaults).
            resize_overlay: None,
            resize_overlay_position: None,
            resize_overlay_duration: crate::resize_overlay::DEFAULT_DURATION_MS,
            // macOS default: stay running after the last window closes
            // (upstream `Config.zig:2509` → false on macOS).
            quit_after_last_window_closed: false,
            title: None,
            window_width: 0,
            window_height: 0,
            window_position_x: None,
            window_position_y: None,
            window_subtitle: None,
            window_new_tab_position: None,
            window_show_tab_bar: None,
            window_step_resize: false,
            // macOS windows cast a shadow by default (upstream `Config.zig:3335`).
            macos_window_shadow: true,
            macos_window_buttons: None,
            window_theme: None,
            // Title reporting off by default — the reply is a security risk
            // (upstream `title-report` false, `Config.zig:2389`).
            title_report: false,
            // No ENQ answerback unless configured (upstream `Config.zig:3735`).
            enquiry_response: None,
            // OSC color queries reply 16-bit by default (upstream
            // `Config.zig:2919`).
            osc_color_report_format: None,
            // 320 MB of image storage per screen (upstream `Config.zig:2398`).
            image_storage_limit: 320 * 1000 * 1000,
            // 10 MB of scrollback per surface (upstream `Config.zig:1387`).
            scrollback_limit: 10_000_000,
            // KAM keyboard-disable is off by default (upstream `Config.zig:2927`).
            vt_kam_allowed: false,
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

    /// The configured text-cursor color (`cursor-color`), or `None` when unset or
    /// unparseable (logged and skipped, so a bad value never breaks startup).
    pub fn cursor_color(&self) -> Option<qwertty_term_vt::color::Rgb> {
        parse_color(self.cursor_color.as_deref(), "cursor-color")
    }

    /// The configured default `background` color, or `None` when unset/invalid.
    pub fn background(&self) -> Option<qwertty_term_vt::color::Rgb> {
        parse_color(self.background.as_deref(), "background")
    }

    /// The configured default `foreground` color, or `None` when unset/invalid.
    pub fn foreground(&self) -> Option<qwertty_term_vt::color::Rgb> {
        parse_color(self.foreground.as_deref(), "foreground")
    }

    /// The configured `selection-background`, or `None` when unset/invalid.
    pub fn selection_background(&self) -> Option<qwertty_term_vt::color::Rgb> {
        parse_color(self.selection_background.as_deref(), "selection-background")
    }

    /// The configured `selection-foreground`, or `None` when unset/invalid.
    pub fn selection_foreground(&self) -> Option<qwertty_term_vt::color::Rgb> {
        parse_color(self.selection_foreground.as_deref(), "selection-foreground")
    }

    /// The parsed `palette` overrides as `(index, color)` pairs. Each entry is
    /// `"N=<color>"`; entries with a bad index (`0..=255`) or unparseable color
    /// are logged and skipped (so one typo never breaks the palette).
    pub fn palette_overrides(&self) -> Vec<(u8, qwertty_term_vt::color::Rgb)> {
        self.palette
            .iter()
            .filter_map(|entry| {
                let Some((idx, color)) = entry.split_once('=') else {
                    eprintln!("ignoring invalid palette entry (want N=color): {entry:?}");
                    return None;
                };
                let Ok(idx) = idx.trim().parse::<u8>() else {
                    eprintln!("ignoring palette entry with bad index (0-255): {entry:?}");
                    return None;
                };
                let rgb = parse_color(Some(color.trim()), "palette color")?;
                Some((idx, rgb))
            })
            .collect()
    }

    /// Build the font-metric modifier set from the `adjust-*` keys, mapping each
    /// to its [`Key`](qwertty_term_font::metrics::Key) exactly as upstream does
    /// (`SharedGridSet.zig`). Each present value is parsed via
    /// [`Modifier::parse`](qwertty_term_font::metrics::Modifier::parse); an
    /// unparseable value is logged and skipped (house rule) so a typo never
    /// breaks font loading. Applied to the computed `Metrics` at grid-build time.
    pub fn metric_modifiers(&self) -> qwertty_term_font::metrics::ModifierSet {
        use qwertty_term_font::metrics::{Key, Modifier, ModifierSet};

        let mut set = ModifierSet::new();
        let mut put = |key: Key, raw: &Option<String>| {
            let Some(raw) = raw.as_deref() else { return };
            match Modifier::parse(raw) {
                Ok(modifier) => {
                    set.insert(key, modifier);
                }
                Err(_) => eprintln!("ignoring invalid adjust-* value {raw:?} for {key:?}"),
            }
        };

        put(Key::CellWidth, &self.adjust_cell_width);
        put(Key::CellHeight, &self.adjust_cell_height);
        put(Key::CellBaseline, &self.adjust_font_baseline);
        put(Key::UnderlinePosition, &self.adjust_underline_position);
        put(Key::UnderlineThickness, &self.adjust_underline_thickness);
        put(
            Key::StrikethroughPosition,
            &self.adjust_strikethrough_position,
        );
        put(
            Key::StrikethroughThickness,
            &self.adjust_strikethrough_thickness,
        );
        put(Key::OverlinePosition, &self.adjust_overline_position);
        put(Key::OverlineThickness, &self.adjust_overline_thickness);
        put(Key::CursorThickness, &self.adjust_cursor_thickness);
        put(Key::CursorHeight, &self.adjust_cursor_height);
        put(Key::BoxThickness, &self.adjust_box_thickness);
        put(Key::IconHeight, &self.adjust_icon_height);
        set
    }

    /// The fixed window title override (`title`), or `None` when unset or blank.
    /// A blank value resets to program-driven titling (upstream semantics), so
    /// it is treated the same as unset.
    pub fn forced_title(&self) -> Option<&str> {
        self.title.as_deref().filter(|t| !t.is_empty())
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

    /// The parsed `osc-color-report-format` (default `Bit16`). Maps to the
    /// engine's [`OscColorReportFormat`](qwertty_term_vt::stream::OscColorReportFormat);
    /// an unknown value falls back to the default.
    pub fn osc_color_report_format(&self) -> qwertty_term_vt::stream::OscColorReportFormat {
        use qwertty_term_vt::stream::OscColorReportFormat;
        match self
            .osc_color_report_format
            .as_deref()
            .map(|s| s.trim().to_ascii_lowercase())
            .as_deref()
        {
            Some("none") => OscColorReportFormat::None,
            Some("8-bit") => OscColorReportFormat::Bit8,
            _ => OscColorReportFormat::Bit16,
        }
    }

    /// The `enquiry-response` answerback bytes (empty when unset).
    pub fn enquiry_response_bytes(&self) -> &[u8] {
        self.enquiry_response
            .as_deref()
            .map(str::as_bytes)
            .unwrap_or(&[])
    }

    /// The parsed `window-save-state` mode (default `Default`).
    pub fn window_save_state(&self) -> WindowSaveState {
        self.window_save_state
            .as_deref()
            .map(WindowSaveState::parse)
            .unwrap_or_default()
    }

    /// The parsed `resize-overlay` mode (default `AfterFirst`).
    pub fn resize_overlay(&self) -> crate::resize_overlay::ResizeOverlayMode {
        self.resize_overlay
            .as_deref()
            .map(crate::resize_overlay::ResizeOverlayMode::parse)
            .unwrap_or_default()
    }

    /// The parsed `resize-overlay-position` (default `Center`).
    pub fn resize_overlay_position(&self) -> crate::resize_overlay::ResizeOverlayPosition {
        self.resize_overlay_position
            .as_deref()
            .map(crate::resize_overlay::ResizeOverlayPosition::parse)
            .unwrap_or_default()
    }

    /// The `resize-overlay-duration` as a `Duration` (default 750ms).
    pub fn resize_overlay_duration(&self) -> std::time::Duration {
        crate::resize_overlay::duration_from_ms(self.resize_overlay_duration)
    }

    /// The configured `selection-word-chars` as boundary codepoints, or `None`
    /// when unset (the caller then uses the built-in default set). Each
    /// character of the string is one boundary codepoint, and the null char
    /// (U+0000) is always prepended — matching upstream `SelectionWordChars`
    /// (`Config.zig:6112`). Multi-codepoint graphemes aren't meaningful here
    /// (word boundaries are per-cell), so each `char` maps to one codepoint.
    pub fn selection_word_chars_codepoints(&self) -> Option<Vec<u32>> {
        let s = self.selection_word_chars.as_ref()?;
        let mut codepoints = Vec::with_capacity(s.chars().count() + 1);
        codepoints.push(0); // null is always a boundary
        codepoints.extend(s.chars().map(|c| c as u32));
        Some(codepoints)
    }

    /// The `click-repeat-interval` (double/triple-click window) as a `Duration`,
    /// or `None` when `0` — the caller then uses the OS click interval. Upstream
    /// `click-repeat-interval` (`Config.zig:2448`, `0` = OS default).
    pub fn click_repeat_interval(&self) -> Option<std::time::Duration> {
        (self.click_repeat_interval != 0)
            .then(|| std::time::Duration::from_millis(self.click_repeat_interval as u64))
    }

    /// The parsed `mouse-shift-capture` policy (default `false`).
    pub fn mouse_shift_capture(&self) -> MouseShiftCapture {
        self.mouse_shift_capture
            .as_deref()
            .map(MouseShiftCapture::parse)
            .unwrap_or_default()
    }

    /// The parsed `right-click-action` (defaults to `context-menu`).
    pub fn right_click_action(&self) -> crate::context_menu::RightClickAction {
        self.right_click_action
            .as_deref()
            .map(crate::context_menu::RightClickAction::parse)
            .unwrap_or_default()
    }

    /// The parsed `middle-click-action` (default `PrimaryPaste`).
    pub fn middle_click_action(&self) -> MiddleClickAction {
        self.middle_click_action
            .as_deref()
            .map(MiddleClickAction::parse)
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

    /// The parsed `window-subtitle` policy (default `Disabled`).
    pub fn window_subtitle(&self) -> WindowSubtitle {
        self.window_subtitle
            .as_deref()
            .map(WindowSubtitle::parse)
            .unwrap_or_default()
    }

    /// The parsed `window-new-tab-position` (default `Current`).
    pub fn window_new_tab_position(&self) -> WindowNewTabPosition {
        self.window_new_tab_position
            .as_deref()
            .map(WindowNewTabPosition::parse)
            .unwrap_or_default()
    }

    /// The parsed `window-show-tab-bar` policy (default `Auto`).
    pub fn window_show_tab_bar(&self) -> WindowShowTabBar {
        self.window_show_tab_bar
            .as_deref()
            .map(WindowShowTabBar::parse)
            .unwrap_or_default()
    }

    /// The parsed `macos-window-buttons` policy (default `Visible`).
    pub fn macos_window_buttons(&self) -> MacWindowButtons {
        self.macos_window_buttons
            .as_deref()
            .map(MacWindowButtons::parse)
            .unwrap_or_default()
    }

    /// The parsed `window-theme` (default `Auto`).
    pub fn window_theme(&self) -> WindowTheme {
        self.window_theme
            .as_deref()
            .map(WindowTheme::parse)
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

/// The window subtitle policy (`window-subtitle`, upstream `WindowSubtitle`,
/// `Config.zig:5284`, default `false`). Upstream ships this on GTK only; we map
/// the same enum onto `NSWindow.subtitle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowSubtitle {
    /// No subtitle. The default.
    #[default]
    Disabled,
    /// Show the focused surface's working directory as the subtitle.
    WorkingDirectory,
}

impl WindowSubtitle {
    /// Parse the config value (`false` / `working-directory`); unknown values
    /// fall back to `false` (disabled).
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "working-directory" => Self::WorkingDirectory,
            _ => Self::Disabled,
        }
    }
}

/// Where a new tab opens relative to the current one (`window-new-tab-position`,
/// upstream `WindowNewTabPosition`, `Config.zig:9181`, default `current`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowNewTabPosition {
    /// Insert the new tab immediately after the active tab. The default.
    #[default]
    Current,
    /// Insert the new tab after the last tab in the group.
    End,
}

impl WindowNewTabPosition {
    /// Parse the config value (`current` / `end`); unknown values fall back to
    /// `current` (upstream's default).
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "end" => Self::End,
            _ => Self::Current,
        }
    }
}

/// The tab bar visibility policy (`window-show-tab-bar`, upstream
/// `WindowShowTabBar`, `Config.zig:9193`, default `auto`). Upstream is a GTK
/// feature; we map the enum onto macOS's `NSWindowTabbingMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowShowTabBar {
    /// The macOS convention: the tab bar appears only at 2+ tabs
    /// (`NSWindowTabbingMode::Automatic`). The default.
    #[default]
    Auto,
    /// Always show the tab bar, even with a single tab
    /// (`NSWindowTabbingMode::Preferred`).
    Always,
    /// Never show the tab bar; native tabbing is disabled so new tabs open as
    /// windows (`NSWindowTabbingMode::Disallowed`).
    Never,
}

impl WindowShowTabBar {
    /// Parse the config value (`auto` / `always` / `never`); unknown values fall
    /// back to `auto` (upstream's default).
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "always" => Self::Always,
            "never" => Self::Never,
            _ => Self::Auto,
        }
    }
}

/// The window appearance theme (`window-theme`, upstream `WindowTheme`,
/// `Config.zig:8931`, default `auto`). On macOS this maps to an
/// `NSAppearance` (`NSAppearance+Extension.swift`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowTheme {
    /// Light or dark by the terminal background luminosity (macOS). The default.
    #[default]
    Auto,
    /// Follow the system appearance (no override).
    System,
    /// Force the light (aqua) appearance.
    Light,
    /// Force the dark (darkAqua) appearance.
    Dark,
    /// Config-colored titlebar — a Linux-only upstream mode; on macOS this
    /// behaves like `System` (no appearance override).
    Ghostty,
}

impl WindowTheme {
    /// Parse the config value (`auto`/`system`/`light`/`dark`/`ghostty`);
    /// unknown values fall back to `auto` (upstream's default).
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "system" => Self::System,
            "light" => Self::Light,
            "dark" => Self::Dark,
            "ghostty" => Self::Ghostty,
            _ => Self::Auto,
        }
    }
}

/// The traffic-light window-button policy (`macos-window-buttons`, upstream
/// `MacWindowButtons`, `Config.zig:8946`, default `visible`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MacWindowButtons {
    /// Show the close/miniaturize/zoom buttons. The default.
    #[default]
    Visible,
    /// Hide all three standard window buttons.
    Hidden,
}

impl MacWindowButtons {
    /// Parse the config value (`visible` / `hidden`); unknown values fall back to
    /// `visible` (upstream's default).
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "hidden" => Self::Hidden,
            _ => Self::Visible,
        }
    }
}

/// Whether macOS restores windows across quit/relaunch (`window-save-state`,
/// upstream `WindowSaveState`, `Config.zig:9174`, default `default`). Maps to
/// the `NSQuitAlwaysKeepsWindows` user default + per-window `isRestorable`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowSaveState {
    /// Follow the system "Close windows when quitting an app" setting.
    #[default]
    Default,
    /// Never restore windows.
    Never,
    /// Always restore windows.
    Always,
}

impl WindowSaveState {
    /// Parse the config value; unknown values fall back to `default`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "never" => Self::Never,
            "always" => Self::Always,
            _ => Self::Default,
        }
    }
}

/// What a middle-click does (`middle-click-action`, upstream `MiddleClickAction`,
/// `Config.zig:8610`, default `primary-paste`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MiddleClickAction {
    /// Paste the current selection into the pane (the X11-style "primary" paste).
    #[default]
    PrimaryPaste,
    /// Do nothing.
    Ignore,
}

impl MiddleClickAction {
    /// Parse the config value; unknown values fall back to `primary-paste`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "ignore" => Self::Ignore,
            _ => Self::PrimaryPaste,
        }
    }
}

/// Whether the terminal program may "capture" the shift modifier while mouse
/// reporting is active — i.e. whether shift is passed through to the program's
/// mouse report rather than overriding reporting to let the user select text
/// (`mouse-shift-capture`, upstream `MouseShiftCapture`, `Config.zig:964`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseShiftCapture {
    /// Shift is not captured by default, but the program may request it via
    /// XTSHIFTESCAPE (`CSI > 1 s`). Upstream default.
    #[default]
    False,
    /// Shift is captured by default, but the program may release it via
    /// XTSHIFTESCAPE (`CSI > 0 s`).
    True,
    /// Shift is always captured, ignoring any program request.
    Always,
    /// Shift is never captured, ignoring any program request (shift always
    /// overrides reporting for selection).
    Never,
}

impl MouseShiftCapture {
    /// Parse the config value; unknown values fall back to `false`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "true" => Self::True,
            "always" => Self::Always,
            "never" => Self::Never,
            _ => Self::False,
        }
    }

    /// Whether shift is captured by the program given this config and the
    /// program's runtime XTSHIFTESCAPE request `flag`. Port of upstream
    /// `Surface.mouseShiftCapture` (`Surface.zig:3712`): `never`/`always` ignore
    /// the program; otherwise the program's explicit request wins, falling back
    /// to the config default when it hasn't set one (`Null`).
    pub fn captures(self, flag: qwertty_term_vt::terminal::MouseShiftCapture) -> bool {
        use qwertty_term_vt::terminal::MouseShiftCapture as Flag;
        match self {
            Self::Never => false,
            Self::Always => true,
            Self::False | Self::True => match flag {
                Flag::True => true,
                Flag::False => false,
                Flag::Null => self == Self::True,
            },
        }
    }
}

const EXAMPLE_CONFIG: &str = r##"# qwertty-term config
#
# This file is created automatically on first run. Uncomment and edit any of
# the lines below; unknown keys are ignored.

# Theme name, looked up in ~/.config/qwertty-term/themes/, then the legacy
# ~/.config/ghostty/themes/ directory, then the shared ghostty themes directory
# (or an absolute path to a theme file).
# theme = "GruvboxDarkHard"

# Text-cursor color (#RRGGBB, RRGGBB, or an X11 color name). Overrides the
# theme's cursor color; a running program's OSC 12 still wins at runtime.
# cursor-color = "#ff8800"

# Default background/foreground and selection colors (same color format as
# cursor-color). Each overrides the theme; the program's OSC 10/11 still win.
# background = "#101010"
# foreground = "#e0e0e0"
# selection-background = "#334455"
# selection-foreground = "#ffffff"

# Per-index palette overrides ("N=color" for index 0-255), applied on top of the
# theme; the program's OSC 4 still wins at runtime.
# palette = ["0=#1e1e2e", "1=#f38ba8"]

# Copy the mouse selection to the clipboard as soon as the drag finishes.
# copy-on-select = false

# Terminal font size in points.
# font-size = 14.0

# Substring to prefer when picking among discovered terminal fonts.
# font-family = "JetBrainsMono Nerd Font Mono"

# Per-metric cell nudges. Each value is a MetricModifier: an integer of pixels
# ("2", "-1") or a percentage delta ("10%", "-5%"). Keys: adjust-cell-width,
# -cell-height, -font-baseline, -underline-position, -underline-thickness,
# -strikethrough-position, -strikethrough-thickness, -overline-position,
# -overline-thickness, -cursor-thickness, -cursor-height, -box-thickness,
# -icon-height.
# adjust-cell-height = "10%"

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
# focus-follows-mouse focuses the pane the mouse moves over (default false).
# middle-click-action is "primary-paste" (paste the selection) or "ignore".
# focus-follows-mouse = false
# middle-click-action = "primary-paste"

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

# Whether macOS restores windows across quit/relaunch: "default" (follow the
# system "Close windows when quitting an app" setting), "never", or "always".
# window-save-state = "default"

# The cols x rows resize overlay shown during a live window resize:
# resize-overlay = "always" / "never" / "after-first" (default; not on the
# initial size). Position is center (default) or a corner/edge; duration is how
# long it lingers after the last resize, in milliseconds.
# resize-overlay = "after-first"
# resize-overlay-position = "center"
# resize-overlay-duration = 750

# Fixed window/tab title. When set, forces the title and ignores the program's
# OSC 0/2 title changes; a blank value ("") keeps normal program-driven titling.
# title = "work"

# Window state. quit-after-last-window-closed keeps the standard macOS behavior
# of staying running with no windows when false (default); set true to quit.
# window-width/height are the initial size in cells (0 = default). Both
# window-position-x/y (pixels from the screen's top-left) must be set to apply.
# quit-after-last-window-closed = false
# window-width = 0
# window-height = 0
# window-position-x = 100
# window-position-y = 50

# VT protocol toggles. title-report answers CSI 21 t window-title queries
# (default false — a security risk). enquiry-response is the answerback sent on
# ENQ (0x05); empty = silent. osc-color-report-format is the OSC 4/10/11 color
# reply form: none / 8-bit / 16-bit (default). image-storage-limit is the bytes
# of image data (Kitty graphics) per screen (default 320MB; 0 disables images).
# scrollback-limit is the bytes of scrollback per surface (default 10MB; only
# affects new surfaces). vt-kam-allowed lets ANSI mode 2 (KAM) disable keyboard
# input at the program's request (default false).
# title-report = false
# enquiry-response = ""
# osc-color-report-format = "16-bit"
# image-storage-limit = 320000000
# scrollback-limit = 10000000
# vt-kam-allowed = false
"##;

/// CLI `--key=value` overrides captured once at startup and replayed on every
/// [`load`] — including live reloads — so a flag like `--font-size=20` survives a
/// config reload instead of silently reverting to the file value.
static CLI_OVERRIDES: OnceLock<Vec<String>> = OnceLock::new();

/// Load the config: merge every default-location file that exists, then apply the
/// CLI overrides captured by [`load_with_cli`]. Creates a commented template on
/// first run. Returns [`Config::default`] on total failure (no `$HOME`, unreadable
/// files, un-deserializable merge).
pub fn load() -> Config {
    load_merged(CLI_OVERRIDES.get().map(Vec::as_slice).unwrap_or(&[]))
}

/// Like [`load`], but first captures `overrides` (raw `--key=value` args) so they
/// apply now *and* on every subsequent [`load`] (reload). Call once, at startup;
/// later calls are ignored (the first set of overrides sticks).
pub fn load_with_cli(overrides: Vec<String>) -> Config {
    let _ = CLI_OVERRIDES.set(overrides);
    load()
}

/// The default config-file locations, **lowest priority first** (later files
/// override earlier ones, mirroring upstream `loadDefaultFiles`,
/// `docs/analysis/config-core.md` §2):
///
/// 1. XDG: `~/.config/qwertty-term/config.toml`
/// 2. macOS only: `~/Library/Application Support/qwertty-term/config.toml`
///
/// `$QWERTTY_TERM_CONFIG_DIR`, if set, replaces the whole list with a single
/// explicit `<dir>/config.toml` (so tests and power users get one hermetic file).
fn default_config_paths() -> Vec<PathBuf> {
    if let Some(dir) = env::var_os("QWERTTY_TERM_CONFIG_DIR") {
        return vec![PathBuf::from(dir).join("config.toml")];
    }
    let Some(home) = env::var_os("HOME") else {
        return Vec::new();
    };
    let home = PathBuf::from(home);
    #[allow(unused_mut)]
    let mut paths = vec![
        home.join(".config")
            .join("qwertty-term")
            .join("config.toml"),
    ];
    #[cfg(target_os = "macos")]
    paths.push(
        home.join("Library")
            .join("Application Support")
            .join("qwertty-term")
            .join("config.toml"),
    );
    paths
}

/// Merge all existing default-location files (each with its includes) in priority
/// order, then apply `overrides`, into one [`Config`].
fn load_merged(overrides: &[String]) -> Config {
    let paths = default_config_paths();
    let existing: Vec<PathBuf> = paths.iter().filter(|p| p.exists()).cloned().collect();

    // First run: drop a commented template at the primary (XDG) location so the
    // user has something to edit. Only when *no* location has a file.
    if existing.is_empty()
        && let Some(primary) = paths.first()
    {
        create_default_config(primary);
    }

    let mut merged = toml::Table::new();
    for path in &existing {
        load_file_into(&mut merged, path);
    }
    apply_cli_overrides(&mut merged, overrides);

    toml::Value::Table(merged).try_into().unwrap_or_else(|err| {
        eprintln!("failed to load merged config: {err}");
        Config::default()
    })
}

/// Load one config file plus its `config-file` includes, merging into `merged`.
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
fn load_file_into(merged: &mut toml::Table, root: &Path) {
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
        merge_tables(merged, table);
    }
}

/// Test-only thin wrapper: load a single file (plus includes) into a fresh
/// [`Config`], bypassing the default-location search and CLI overrides.
#[cfg(test)]
fn load_from(root: &Path) -> Config {
    let mut merged = toml::Table::new();
    load_file_into(&mut merged, root);
    toml::Value::Table(merged)
        .try_into()
        .unwrap_or_else(|_| Config::default())
}

/// Keys whose value must be a *list* even from a single CLI flag, so a
/// `--keybind=…` appends to (rather than replaces) file-provided values under
/// [`merge_tables`]. Currently only `keybind` is repeatable.
const CLI_ARRAY_KEYS: &[&str] = &["keybind"];

/// Apply `--key=value` overrides on top of the file-merged `merged` table.
///
/// Each override is applied **incrementally and validated**: an override that
/// would make the config fail to deserialize is warned and dropped, so one bad
/// flag can never discard the whole (already-valid) file config. Scalars last-win
/// (CLI beats files); repeatable keys ([`CLI_ARRAY_KEYS`]) append. Values are
/// coerced by trying the raw text as a TOML literal (numbers/bools/quoted
/// strings) and falling back to a bare string, so `--font-size=16` becomes an
/// integer while `--theme=Nord` becomes a string.
fn apply_cli_overrides(merged: &mut toml::Table, args: &[String]) {
    for arg in args {
        let Some((key, raw)) = arg.strip_prefix("--").and_then(|b| b.split_once('=')) else {
            eprintln!("ignoring CLI arg (expected --key=value): {arg}");
            continue;
        };
        let key = key.trim();
        if key.is_empty() {
            continue;
        }
        let value = if CLI_ARRAY_KEYS.contains(&key) {
            toml::Value::Array(vec![toml::Value::String(raw.to_string())])
        } else {
            parse_toml_scalar(raw)
        };

        let mut candidate = merged.clone();
        let mut one = toml::Table::new();
        one.insert(key.to_string(), value);
        merge_tables(&mut candidate, one);

        if toml::Value::Table(candidate.clone())
            .try_into::<Config>()
            .is_ok()
        {
            *merged = candidate;
        } else {
            eprintln!("ignoring invalid CLI override: {arg}");
        }
    }
}

/// Coerce a raw CLI value string into a TOML scalar: parse it as a TOML literal
/// (int/float/bool/quoted string) if it is one, else keep it as a bare string.
fn parse_toml_scalar(raw: &str) -> toml::Value {
    if let Ok(table) = format!("v = {raw}").parse::<toml::Table>()
        && let Some(value) = table.get("v")
    {
        return value.clone();
    }
    toml::Value::String(raw.to_string())
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

/// Parse a color config value (`#RRGGBB`/`RRGGBB`/X11 name) via
/// [`qwertty_term_vt::color::Rgb::parse`], logging and skipping an unparseable
/// value (so a typo'd color never breaks startup). `key` names the setting for
/// the warning. Returns `None` for an unset (`None`) or invalid value.
fn parse_color(raw: Option<&str>, key: &str) -> Option<qwertty_term_vt::color::Rgb> {
    let raw = raw?;
    match qwertty_term_vt::color::Rgb::parse(raw) {
        Ok(rgb) => Some(rgb),
        Err(_) => {
            eprintln!("ignoring invalid {key}: {raw:?}");
            None
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
        // selection-clear-on-copy is off by default (upstream); parses when set.
        assert!(!config.selection_clear_on_copy);
        assert!(
            parse("selection-clear-on-copy = true\n")
                .unwrap()
                .selection_clear_on_copy
        );
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
        // Window-save-state defaults to `default` (follow the system setting).
        assert_eq!(
            config.window_save_state(),
            crate::config::WindowSaveState::Default
        );
        // Resize overlay defaults: after-first, center, 750ms.
        assert_eq!(
            config.resize_overlay(),
            crate::resize_overlay::ResizeOverlayMode::AfterFirst
        );
        assert_eq!(
            config.resize_overlay_position(),
            crate::resize_overlay::ResizeOverlayPosition::Center
        );
        assert_eq!(
            config.resize_overlay_duration(),
            std::time::Duration::from_millis(750)
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
    fn parses_resize_overlay_keys() {
        let config = parse(
            "resize-overlay = \"always\"\n\
             resize-overlay-position = \"top-right\"\n\
             resize-overlay-duration = 300\n",
        )
        .unwrap();
        assert_eq!(
            config.resize_overlay(),
            crate::resize_overlay::ResizeOverlayMode::Always
        );
        assert_eq!(
            config.resize_overlay_position(),
            crate::resize_overlay::ResizeOverlayPosition::TopRight
        );
        assert_eq!(
            config.resize_overlay_duration(),
            std::time::Duration::from_millis(300)
        );
    }

    #[test]
    fn parses_selection_gesture_keys() {
        // Unset → no override (caller uses the built-in default set / OS interval).
        let default = parse("").unwrap();
        assert_eq!(default.selection_word_chars_codepoints(), None);
        assert_eq!(default.click_repeat_interval(), None);

        // selection-word-chars: null is always prepended, then each char.
        let config = parse(
            "selection-word-chars = \" /.\"\n\
             click-repeat-interval = 250\n",
        )
        .unwrap();
        assert_eq!(
            config.selection_word_chars_codepoints(),
            Some(vec![0, ' ' as u32, '/' as u32, '.' as u32])
        );
        assert_eq!(
            config.click_repeat_interval(),
            Some(std::time::Duration::from_millis(250))
        );

        // An empty string still yields the null-only boundary set.
        let empty = parse("selection-word-chars = \"\"\n").unwrap();
        assert_eq!(empty.selection_word_chars_codepoints(), Some(vec![0]));

        // A multi-byte UTF-8 boundary char maps to its codepoint.
        let unicode = parse("selection-word-chars = \"│\"\n").unwrap();
        assert_eq!(
            unicode.selection_word_chars_codepoints(),
            Some(vec![0, '│' as u32])
        );
    }

    #[test]
    fn parses_mouse_shift_capture_key() {
        use crate::config::MouseShiftCapture;
        use qwertty_term_vt::terminal::MouseShiftCapture as Flag;

        assert_eq!(
            parse("").unwrap().mouse_shift_capture(),
            MouseShiftCapture::False
        );
        assert_eq!(
            parse("mouse-shift-capture = \"always\"\n")
                .unwrap()
                .mouse_shift_capture(),
            MouseShiftCapture::Always
        );
        assert_eq!(
            parse("mouse-shift-capture = \"true\"\n")
                .unwrap()
                .mouse_shift_capture(),
            MouseShiftCapture::True
        );

        // `captures()` combines the config with the program's XTSHIFTESCAPE flag.
        // never/always ignore the program flag entirely.
        assert!(!MouseShiftCapture::Never.captures(Flag::True));
        assert!(MouseShiftCapture::Always.captures(Flag::False));
        // false/true: the program's explicit request wins, else the config default.
        assert!(MouseShiftCapture::False.captures(Flag::True));
        assert!(!MouseShiftCapture::True.captures(Flag::False));
        assert!(!MouseShiftCapture::False.captures(Flag::Null));
        assert!(MouseShiftCapture::True.captures(Flag::Null));
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
    fn vt_toggle_defaults_match_upstream() {
        use qwertty_term_vt::stream::OscColorReportFormat;
        let c = parse("").unwrap();
        // Upstream defaults verified at ghostty `2da015cd6`:
        assert!(!c.title_report); // Config.zig:2389
        assert_eq!(c.enquiry_response_bytes(), b""); // Config.zig:3735
        assert_eq!(c.osc_color_report_format(), OscColorReportFormat::Bit16); // Config.zig:2919
        assert_eq!(c.image_storage_limit, 320 * 1000 * 1000); // Config.zig:2398
        assert_eq!(c.scrollback_limit, 10_000_000); // Config.zig:1387
        assert!(!c.vt_kam_allowed); // Config.zig:2927
    }

    #[test]
    fn parses_vt_toggle_keys() {
        use qwertty_term_vt::stream::OscColorReportFormat;
        assert!(parse("title-report = true\n").unwrap().title_report);
        assert_eq!(
            parse("enquiry-response = \"PONG\"\n")
                .unwrap()
                .enquiry_response_bytes(),
            b"PONG"
        );
        assert_eq!(
            parse("osc-color-report-format = \"none\"\n")
                .unwrap()
                .osc_color_report_format(),
            OscColorReportFormat::None
        );
        assert_eq!(
            parse("osc-color-report-format = \"8-bit\"\n")
                .unwrap()
                .osc_color_report_format(),
            OscColorReportFormat::Bit8
        );
        assert_eq!(
            parse("osc-color-report-format = \"16-bit\"\n")
                .unwrap()
                .osc_color_report_format(),
            OscColorReportFormat::Bit16
        );
        // Unknown value falls back to the 16-bit default.
        assert_eq!(
            parse("osc-color-report-format = \"nonsense\"\n")
                .unwrap()
                .osc_color_report_format(),
            OscColorReportFormat::Bit16
        );
        assert_eq!(
            parse("image-storage-limit = 1048576\n")
                .unwrap()
                .image_storage_limit,
            1_048_576
        );
        assert_eq!(
            parse("scrollback-limit = 500000\n")
                .unwrap()
                .scrollback_limit,
            500_000
        );
        assert!(parse("vt-kam-allowed = true\n").unwrap().vt_kam_allowed);
    }

    #[test]
    fn parses_window_save_state_key() {
        use crate::config::WindowSaveState;
        assert_eq!(
            parse("window-save-state = \"never\"\n")
                .unwrap()
                .window_save_state(),
            WindowSaveState::Never
        );
        assert_eq!(
            parse("window-save-state = \"always\"\n")
                .unwrap()
                .window_save_state(),
            WindowSaveState::Always
        );
        // Absent + unknown → default.
        assert_eq!(
            parse("").unwrap().window_save_state(),
            WindowSaveState::Default
        );
    }

    #[test]
    fn parses_mouse_keys() {
        let config = parse(
            "right-click-action = \"paste\"\nmouse-hide-while-typing = true\n\
             focus-follows-mouse = true\nmiddle-click-action = \"ignore\"\n",
        )
        .unwrap();
        assert_eq!(
            config.right_click_action(),
            crate::context_menu::RightClickAction::Paste
        );
        assert!(config.mouse_hide_while_typing);
        assert!(config.focus_follows_mouse);
        assert_eq!(
            config.middle_click_action(),
            crate::config::MiddleClickAction::Ignore
        );
        // Defaults: focus-follows-mouse off, middle-click primary-paste.
        let d = parse("").unwrap();
        assert!(!d.focus_follows_mouse);
        assert_eq!(
            d.middle_click_action(),
            crate::config::MiddleClickAction::PrimaryPaste
        );
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
    fn example_config_template_parses_to_defaults() {
        // The commented first-run template must always be valid TOML that parses
        // to the defaults (every setting commented out). Guards template edits —
        // e.g. a `"#…"` value that would need the `r##"…"##` raw-string delimiter.
        let config = parse(EXAMPLE_CONFIG).expect("EXAMPLE_CONFIG must parse");
        assert_eq!(config, Config::default());
    }

    #[test]
    fn cursor_color_parses_hex_and_x11_and_ignores_garbage() {
        let default = Config::default();
        assert_eq!(default.cursor_color, None);
        assert_eq!(default.cursor_color(), None);

        let hex = parse("cursor-color = \"#ff8800\"\n").unwrap();
        assert_eq!(
            hex.cursor_color(),
            Some(qwertty_term_vt::color::Rgb::new(0xff, 0x88, 0x00))
        );
        let named = parse("cursor-color = \"red\"\n").unwrap();
        assert_eq!(
            named.cursor_color(),
            Some(qwertty_term_vt::color::Rgb::new(0xff, 0, 0))
        );
        // An unparseable value falls back to None (theme/default cursor color).
        let bad = parse("cursor-color = \"not-a-color-zzz\"\n").unwrap();
        assert_eq!(bad.cursor_color(), None);
    }

    #[test]
    fn color_overrides_parse_and_default_to_none() {
        use qwertty_term_vt::color::Rgb;
        let d = Config::default();
        assert_eq!(d.background(), None);
        assert_eq!(d.foreground(), None);
        assert_eq!(d.selection_background(), None);
        assert_eq!(d.selection_foreground(), None);

        let c = parse(
            "background = \"#101010\"\n\
             foreground = \"white\"\n\
             selection-background = \"#334455\"\n\
             selection-foreground = \"not-a-color\"\n",
        )
        .unwrap();
        assert_eq!(c.background(), Some(Rgb::new(0x10, 0x10, 0x10)));
        assert_eq!(c.foreground(), Some(Rgb::new(0xff, 0xff, 0xff)));
        assert_eq!(c.selection_background(), Some(Rgb::new(0x33, 0x44, 0x55)));
        // Garbage → None (skipped).
        assert_eq!(c.selection_foreground(), None);
    }

    #[test]
    fn palette_overrides_parse_pairs_and_skip_garbage() {
        use qwertty_term_vt::color::Rgb;
        assert!(Config::default().palette_overrides().is_empty());

        let c = parse(
            "palette = [\"0=#1e1e2e\", \"1=red\", \"bad-entry\", \"999=#000000\", \"2=not-a-color\"]",
        )
        .unwrap();
        // Only the two well-formed entries survive (bad separator, out-of-range
        // index, and unparseable color are all skipped).
        assert_eq!(
            c.palette_overrides(),
            vec![(0, Rgb::new(0x1e, 0x1e, 0x2e)), (1, Rgb::new(0xff, 0, 0))]
        );
    }

    #[test]
    fn forced_title_is_none_when_unset_or_blank() {
        assert_eq!(Config::default().forced_title(), None);
        assert_eq!(
            parse("title = \"Build\"\n").unwrap().forced_title(),
            Some("Build")
        );
        // A blank value resets to program-driven titling (treated as unset).
        assert_eq!(parse("title = \"\"\n").unwrap().forced_title(), None);
    }

    #[test]
    fn metric_modifiers_maps_adjust_keys_and_skips_garbage() {
        use qwertty_term_font::metrics::{Key, Modifier};

        // Default config → no modifiers.
        assert!(Config::default().metric_modifiers().is_empty());

        let config = parse(
            "adjust-cell-width = \"2\"\n\
             adjust-cell-height = \"10%\"\n\
             adjust-font-baseline = \"-1\"\n\
             adjust-cursor-thickness = \"3\"\n\
             adjust-box-thickness = \"not-a-number\"\n",
        )
        .unwrap();
        let mods = config.metric_modifiers();

        // Mapped to the right Key with the right parsed Modifier.
        assert_eq!(mods.get(&Key::CellWidth), Some(&Modifier::Absolute(2)));
        assert_eq!(mods.get(&Key::CellHeight), Some(&Modifier::Percent(1.1)));
        // `adjust-font-baseline` targets the cell_baseline metric.
        assert_eq!(mods.get(&Key::CellBaseline), Some(&Modifier::Absolute(-1)));
        assert_eq!(
            mods.get(&Key::CursorThickness),
            Some(&Modifier::Absolute(3))
        );
        // The garbage value is skipped, not fatal — its key is absent.
        assert_eq!(mods.get(&Key::BoxThickness), None);
        // Only the four valid keys made it in.
        assert_eq!(mods.len(), 4);
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

    // ---- two-location merge ----

    #[test]
    fn load_file_into_merges_locations_in_priority_order() {
        // Simulate the XDG (low) then App Support (high) files: the second file
        // overrides the shared scalar and its keybind appends.
        let dir = scratch_dir("locations");
        fs::write(
            dir.join("low.toml"),
            "theme = \"Low\"\nfont-size = 12\nkeybind = [\"a=text:x\"]\n",
        )
        .unwrap();
        fs::write(
            dir.join("high.toml"),
            "theme = \"High\"\nkeybind = [\"b=text:y\"]\n",
        )
        .unwrap();

        let mut merged = toml::Table::new();
        load_file_into(&mut merged, &dir.join("low.toml"));
        load_file_into(&mut merged, &dir.join("high.toml"));
        let config: Config = toml::Value::Table(merged).try_into().unwrap();

        assert_eq!(config.theme.as_deref(), Some("High")); // high wins
        assert_eq!(config.font_size, Some(12.0)); // low-only key survives
        assert_eq!(config.keybind, vec!["a=text:x", "b=text:y"]); // appended
    }

    // ---- CLI overrides ----

    fn merged_with_overrides(base: &str, args: &[&str]) -> Config {
        let mut merged: toml::Table = base.parse().unwrap();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        apply_cli_overrides(&mut merged, &args);
        toml::Value::Table(merged).try_into().unwrap()
    }

    #[test]
    fn cli_override_beats_file_and_infers_types() {
        let config = merged_with_overrides(
            "theme = \"File\"\nfont-size = 10\n",
            &[
                "--theme=Cli",           // bare string
                "--font-size=16",        // number
                "--copy-on-select=true", // bool
            ],
        );
        assert_eq!(config.theme.as_deref(), Some("Cli"));
        assert_eq!(config.font_size, Some(16.0));
        assert!(config.copy_on_select);
    }

    #[test]
    fn cli_keybind_appends_to_file_values() {
        let config = merged_with_overrides(
            "keybind = [\"a=text:x\"]\n",
            &["--keybind=b=text:y", "--keybind=c=text:z"],
        );
        assert_eq!(config.keybind, vec!["a=text:x", "b=text:y", "c=text:z"]);
    }

    #[test]
    fn cli_string_value_with_spaces_is_quoted() {
        let config = merged_with_overrides("", &["--font-family=FiraCode Nerd Font Mono"]);
        assert_eq!(
            config.font_family.as_deref(),
            Some("FiraCode Nerd Font Mono")
        );
    }

    #[test]
    fn invalid_cli_override_is_dropped_not_fatal() {
        // `--font-size=huge` can't become an f32 → that one override is dropped,
        // but the valid file value and the valid `--theme` override both survive.
        let config =
            merged_with_overrides("font-size = 14\n", &["--font-size=huge", "--theme=Kept"]);
        assert_eq!(config.font_size, Some(14.0)); // bad override ignored
        assert_eq!(config.theme.as_deref(), Some("Kept")); // good one applied
    }

    #[test]
    fn malformed_cli_arg_is_ignored() {
        // Missing `=` and missing `--` are both skipped without affecting output.
        let config = merged_with_overrides("theme = \"X\"\n", &["--no-value", "theme=Y"]);
        assert_eq!(config.theme.as_deref(), Some("X"));
    }
}
