//! Loading and parsing of ghostty theme files, for the `theme` config key.
//!
//! This is a copy of the spike's `theme_file.rs`
//! (`crates/spike/src/window/theme_file.rs`, read-only reference material for
//! R5). The spike's module is `pub(crate)` inside a different crate, so it
//! can't be reused through a path dependency without modifying the spike;
//! this copy keeps the same file format, key set, search order, and lenient-
//! parsing semantics so a user's theme file resolves identically under both
//! binaries. Flagged for a later dedup (see `docs/analysis/renderer-r5.md`'s
//! deferrals) once there is a shared, non-read-only home for it.
//!
//! Ghostty theme files use the same flat `key = value` syntax as the main
//! config, restricted (for a theme) to a small key set surveyed directly from
//! the shipped themes (`Adwaita Dark`, `3024 Night`, `Aardvark Blue`,
//! `Dracula`, `Nord`, ...): `palette = N=#RRGGBB` (repeated, one per index),
//! plus `background`, `foreground`, `cursor-color`, `cursor-text`,
//! `selection-background`, `selection-foreground`. Parsing is lenient:
//! unknown keys are ignored and malformed lines are skipped rather than
//! failing the whole file, so a theme with a key this app doesn't apply yet
//! (or a future ghostty addition) still loads.
//!
//! Search order for a bare theme name (not an absolute path):
//!   1. `~/.config/ghostty/themes/<name>`
//!   2. `$QWERTTY_TERM_THEMES_DIR/<name>` if set, else a hardcoded fallback
//!      shared themes directory (the env var exists so this resolves on
//!      machines without that checkout at that exact path).

use std::{env, fs, path::Path, path::PathBuf};

use qwertty_term_vt::color::{DEFAULT, Rgb, parse_palette_entry};
use qwertty_term_vt::terminal::Colors;

/// The color fields parsed out of a ghostty theme file. `cursor_text` and the
/// `selection_*` fields are parsed but not yet consumed by
/// [`ThemeColors::to_colors`] (the engine's startup [`Colors`] has no
/// selection-color slots) — kept so the theme is fully represented and
/// future selection-rendering work doesn't need to revisit the parser.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ThemeColors {
    pub(crate) palette: [Rgb; 256],
    pub(crate) background: Option<Rgb>,
    pub(crate) foreground: Option<Rgb>,
    pub(crate) cursor_color: Option<Rgb>,
    pub(crate) cursor_text: Option<Rgb>,
    pub(crate) selection_background: Option<Rgb>,
    pub(crate) selection_foreground: Option<Rgb>,
}

impl Default for ThemeColors {
    fn default() -> Self {
        Self {
            palette: DEFAULT,
            background: None,
            foreground: None,
            cursor_color: None,
            cursor_text: None,
            selection_background: None,
            selection_foreground: None,
        }
    }
}

impl ThemeColors {
    /// Parse a theme file's contents (ghostty's `key = value` line format).
    pub(crate) fn parse(contents: &str) -> Self {
        let mut theme = Self::default();
        for line in contents.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();

            if key == "palette" {
                if let Ok(entry) = parse_palette_entry(value) {
                    theme.palette[entry.index as usize] = entry.color;
                }
                continue;
            }

            let Ok(rgb) = Rgb::parse(value) else {
                continue;
            };
            match key {
                "background" => theme.background = Some(rgb),
                "foreground" => theme.foreground = Some(rgb),
                "cursor-color" => theme.cursor_color = Some(rgb),
                "cursor-text" => theme.cursor_text = Some(rgb),
                "selection-background" => theme.selection_background = Some(rgb),
                "selection-foreground" => theme.selection_foreground = Some(rgb),
                _ => {} // unknown key; ignore leniently
            }
        }
        theme
    }

    /// Build the `qwertty-term-vt` startup [`Colors`] this theme implies: the full
    /// 256-entry palette (defaults, with any theme overrides applied) plus
    /// OSC-10/11-equivalent default fg/bg/cursor. `qwertty-term-vt`'s `Colors` /
    /// `DynamicRgb` / `DynamicPalette` types are already public and
    /// constructible from outside the crate, so no `qwertty-term-vt` changes are
    /// needed to seed initial state this way — later OSC 4/10/11 sequences
    /// from the running program still override at runtime through the same
    /// dynamic-color path (see `Terminal::colors`).
    pub(crate) fn to_colors(&self) -> Colors {
        let mut colors = Colors {
            palette: qwertty_term_vt::color::DynamicPalette::new(self.palette),
            ..Colors::default()
        };
        if let Some(fg) = self.foreground {
            colors.foreground.set(fg);
        }
        if let Some(bg) = self.background {
            colors.background.set(bg);
        }
        if let Some(cursor) = self.cursor_color {
            colors.cursor.set(cursor);
        }
        colors
    }
}

/// Resolve a `theme` config value to a theme file path and load+parse it.
///
/// Absolute paths are used as-is. Otherwise, the name is looked up first in
/// `~/.config/ghostty/themes/`, then in the shared ghostty themes directory
/// (overridable via `QWERTTY_TERM_THEMES_DIR` for machines without the
/// hardcoded checkout path). Returns `None` (falling back to `qwertty-term-vt`'s
/// built-in default colors) if the theme can't be found or read; a warning is
/// printed to stderr in that case.
pub(crate) fn load_theme(name: &str) -> Option<ThemeColors> {
    let path = resolve_theme_path(name)?;
    match fs::read_to_string(&path) {
        Ok(contents) => Some(ThemeColors::parse(&contents)),
        Err(err) => {
            eprintln!("failed to read theme {}: {err}", path.display());
            None
        }
    }
}

/// Fallback shared themes directory when `QWERTTY_TERM_THEMES_DIR` is unset —
/// the maintainer's local ghostty checkout. Other machines should set the env
/// var.
const DEFAULT_SHARED_THEMES_DIR: &str = "/Users/joshka/local/ghostty/zig-out/share/ghostty/themes";

fn resolve_theme_path(name: &str) -> Option<PathBuf> {
    let as_path = Path::new(name);
    if as_path.is_absolute() {
        return as_path.is_file().then(|| as_path.to_path_buf());
    }

    for directory in theme_search_dirs() {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    eprintln!(
        "theme '{name}' not found in {}",
        theme_search_dirs()
            .iter()
            .map(|dir| dir.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    );
    None
}

/// The theme directory search order: `~/.config/ghostty/themes/` first
/// (matches upstream ghostty's own user-themes location), then the shared
/// themes directory (env-overridable).
fn theme_search_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = env::var_os("HOME") {
        dirs.push(PathBuf::from(home).join(".config/ghostty/themes"));
    }
    let shared = env::var("QWERTTY_TERM_THEMES_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SHARED_THEMES_DIR));
    dirs.push(shared);
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    // A fixture theme covering the full observed key set, modeled on the real
    // "Aardvark Blue" ghostty theme (values changed to avoid asserting
    // against upstream data that could shift).
    const FIXTURE_THEME: &str = "\
palette = 0=#111111
palette = 1=#222222
palette = 15=#ffffff
background = #102030
foreground = #dddddd
cursor-color = #00ff00
cursor-text = #000000
selection-background = #bfdbfe
selection-foreground = #000000
";

    #[test]
    fn parses_palette_entries_by_index() {
        let theme = ThemeColors::parse(FIXTURE_THEME);
        assert_eq!(theme.palette[0], Rgb::new(0x11, 0x11, 0x11));
        assert_eq!(theme.palette[1], Rgb::new(0x22, 0x22, 0x22));
        assert_eq!(theme.palette[15], Rgb::new(0xff, 0xff, 0xff));
        // Untouched indexes keep the qwertty-term-vt default palette.
        assert_eq!(theme.palette[2], DEFAULT[2]);
    }

    #[test]
    fn parses_named_color_keys() {
        let theme = ThemeColors::parse(FIXTURE_THEME);
        assert_eq!(theme.background, Some(Rgb::new(0x10, 0x20, 0x30)));
        assert_eq!(theme.foreground, Some(Rgb::new(0xdd, 0xdd, 0xdd)));
        assert_eq!(theme.cursor_color, Some(Rgb::new(0x00, 0xff, 0x00)));
        assert_eq!(theme.cursor_text, Some(Rgb::new(0x00, 0x00, 0x00)));
        assert_eq!(theme.selection_background, Some(Rgb::new(0xbf, 0xdb, 0xfe)));
        assert_eq!(theme.selection_foreground, Some(Rgb::new(0x00, 0x00, 0x00)));
    }

    #[test]
    fn ignores_unknown_keys_and_blank_and_comment_lines() {
        let contents = "\
# a comment
some-future-key = #abcabc

background = #101010
";
        let theme = ThemeColors::parse(contents);
        assert_eq!(theme.background, Some(Rgb::new(0x10, 0x10, 0x10)));
    }

    #[test]
    fn skips_malformed_lines_without_panicking() {
        let contents = "\
palette = not-a-valid-entry
background
foreground = #ffffff
";
        let theme = ThemeColors::parse(contents);
        assert_eq!(theme.foreground, Some(Rgb::new(0xff, 0xff, 0xff)));
        // The malformed palette line left index 0 at its default.
        assert_eq!(theme.palette[0], DEFAULT[0]);
    }

    #[test]
    fn default_theme_matches_qwertty_term_vt_defaults() {
        let theme = ThemeColors::default();
        assert_eq!(theme.palette, DEFAULT);
        assert_eq!(theme.background, None);
        assert_eq!(theme.foreground, None);
    }

    #[test]
    fn to_colors_applies_palette_and_default_fg_bg() {
        let theme = ThemeColors::parse(FIXTURE_THEME);
        let colors = theme.to_colors();
        assert_eq!(colors.palette.current[0], Rgb::new(0x11, 0x11, 0x11));
        assert_eq!(colors.foreground.get(), Some(Rgb::new(0xdd, 0xdd, 0xdd)));
        assert_eq!(colors.background.get(), Some(Rgb::new(0x10, 0x20, 0x30)));
        assert_eq!(colors.cursor.get(), Some(Rgb::new(0x00, 0xff, 0x00)));
    }

    #[test]
    fn to_colors_with_no_overrides_leaves_default_fg_bg_unset() {
        let theme = ThemeColors::default();
        let colors = theme.to_colors();
        assert_eq!(colors.foreground.get(), None);
        assert_eq!(colors.background.get(), None);
    }

    #[test]
    fn resolves_absolute_path_as_is() {
        let dir = std::env::temp_dir().join("qwertty-term-app-theme-test-absolute");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("MyTheme");
        fs::write(&path, "background = #010203\n").unwrap();

        let theme = load_theme(path.to_str().unwrap());
        assert_eq!(theme.unwrap().background, Some(Rgb::new(0x01, 0x02, 0x03)));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn missing_theme_returns_none() {
        assert!(resolve_theme_path("definitely-does-not-exist-anywhere").is_none());
    }
}
