use std::{
    env, fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use eframe::egui::{Context, FontData, FontDefinitions, FontFamily, FontId};
use ttf_parser::Face;

const DEFAULT_FONT_SIZE: f32 = 14.0;
const TERMINAL_FAMILY: &str = "qwertty-term-terminal";
const TERMINAL_FONT_PREFIX: &str = "qwertty-term-terminal-font";

#[derive(Clone, Debug)]
pub(crate) struct TerminalFont {
    family: FontFamily,
    size: f32,
    diagnostics: Vec<FontDiagnostic>,
}

impl TerminalFont {
    pub(crate) fn id(&self) -> FontId {
        FontId::new(self.size, self.family.clone())
    }

    pub(crate) fn size(&self) -> f32 {
        self.size
    }

    pub(crate) fn set_size(&mut self, size: f32) {
        self.size = size.clamp(6.0, 48.0);
    }

    pub(crate) fn diagnostics(&self) -> &[FontDiagnostic] {
        &self.diagnostics
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct FontDiagnostic {
    pub(crate) path: PathBuf,
    pub(crate) coverage: GlyphCoverage,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct GlyphCoverage {
    pub(crate) powerline: usize,
    pub(crate) devicons: usize,
}

impl GlyphCoverage {
    fn inspect(bytes: &[u8]) -> Option<Self> {
        let face = Face::parse(bytes, 0).ok()?;
        Some(Self {
            powerline: POWERLINE_GLYPHS
                .iter()
                .filter(|ch| face.glyph_index(**ch).is_some())
                .count(),
            devicons: DEVICON_GLYPHS
                .iter()
                .filter(|ch| face.glyph_index(**ch).is_some())
                .count(),
        })
    }

    pub(crate) fn summary(self) -> String {
        format!(
            "Powerline {}/{}, devicons {}/{}",
            self.powerline,
            POWERLINE_GLYPHS.len(),
            self.devicons,
            DEVICON_GLYPHS.len()
        )
    }
}

pub(crate) fn font_report_lines() -> Vec<String> {
    let diagnostics = terminal_font_diagnostics();
    if diagnostics.is_empty() {
        return vec!["No local Nerd Font files found.".to_string()];
    }
    diagnostics.iter().map(font_report_line).collect()
}

pub(crate) fn glyph_probe_text() -> String {
    format!(
        "Powerline: {}\r\nDevicons: {}",
        POWERLINE_GLYPHS.iter().collect::<String>(),
        DEVICON_GLYPHS.iter().collect::<String>()
    )
}

/// Configure the terminal font. `preferred_family` (typically the config's
/// `font-family` key) moves any discovered font whose file name contains it
/// (case-insensitively) to the front of the candidate list, ahead of the
/// existing size/style-based ordering. This is a name-substring preference,
/// not full font-family/style matching — the underlying discovery is still
/// "local Nerd Font files found on disk" (see `discover_nerd_fonts`), so an
/// unmatched or unset preference falls back to today's ordering unchanged.
pub(crate) fn configure_with_family(
    ctx: &Context,
    saved_size: Option<f32>,
    preferred_family: Option<&str>,
) -> TerminalFont {
    let size = configured_font_size(saved_size);
    let mut font_paths = terminal_font_paths();
    prefer_family(&mut font_paths, preferred_family);
    if font_paths.is_empty() {
        return TerminalFont {
            family: FontFamily::Monospace,
            size,
            diagnostics: Vec::new(),
        };
    }

    let diagnostics = install_terminal_fonts(ctx, &font_paths);
    if !diagnostics.is_empty() {
        TerminalFont {
            family: FontFamily::Name(TERMINAL_FAMILY.into()),
            size,
            diagnostics,
        }
    } else {
        TerminalFont {
            family: FontFamily::Monospace,
            size,
            diagnostics: Vec::new(),
        }
    }
}

/// Move the first discovered font path matching `preferred_family` (a
/// case-insensitive file-name substring) to the front, preserving the
/// relative order of everything else. No-op if `preferred_family` is `None`
/// or matches nothing.
fn prefer_family(font_paths: &mut Vec<PathBuf>, preferred_family: Option<&str>) {
    let Some(preferred) = preferred_family else {
        return;
    };
    let preferred = preferred.to_ascii_lowercase();
    let Some(match_index) = font_paths.iter().position(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.to_ascii_lowercase().contains(&preferred))
    }) else {
        return;
    };
    let matched = font_paths.remove(match_index);
    font_paths.insert(0, matched);
}

const POWERLINE_GLYPHS: [char; 4] = ['\u{e0b0}', '\u{e0b1}', '\u{e0b2}', '\u{e0b3}'];
const DEVICON_GLYPHS: [char; 6] = [
    '\u{e5ff}', // folder
    '\u{e700}', // rust
    '\u{e711}', // javascript
    '\u{f17c}', // linux
    '\u{f179}', // apple
    '\u{f121}', // code
];

fn configured_font_size(saved_size: Option<f32>) -> f32 {
    env::var("QWERTTY_TERM_FONT_SIZE")
        .ok()
        .and_then(|size| size.parse::<f32>().ok())
        .filter(|size| (6.0..=48.0).contains(size))
        .or(saved_size)
        .unwrap_or(DEFAULT_FONT_SIZE)
}

fn configured_font_path() -> Option<PathBuf> {
    env::var_os("QWERTTY_TERM_FONT_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_file())
}

fn terminal_font_paths() -> Vec<PathBuf> {
    ordered_font_paths(configured_font_path(), discover_nerd_fonts())
}

fn terminal_font_diagnostics() -> Vec<FontDiagnostic> {
    inspect_font_paths(&terminal_font_paths())
}

fn ordered_font_paths(configured: Option<PathBuf>, discovered: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(path) = configured {
        paths.push(path);
    }
    for path in discovered {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths.truncate(8);
    paths
}

fn install_terminal_fonts(ctx: &Context, paths: &[PathBuf]) -> Vec<FontDiagnostic> {
    let mut fonts = FontDefinitions::default();
    let fallback = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();
    let terminal_family = FontFamily::Name(Arc::from(TERMINAL_FAMILY));

    let mut terminal_fonts = Vec::new();
    let mut diagnostics = Vec::new();
    for path in paths {
        let font_name = format!("{TERMINAL_FONT_PREFIX}-{}", terminal_fonts.len());
        match fs::read(path) {
            Ok(bytes) => {
                let diagnostic = inspect_font_bytes(path, &bytes);
                fonts
                    .font_data
                    .insert(font_name.clone(), Arc::new(FontData::from_owned(bytes)));
                terminal_fonts.push(font_name);
                diagnostics.push(diagnostic);
            }
            Err(err) => {
                eprintln!("failed to read terminal font {}: {err}", path.display());
            }
        }
    }
    if terminal_fonts.is_empty() {
        return Vec::new();
    }

    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .splice(0..0, terminal_fonts.clone());

    terminal_fonts.extend(fallback);
    fonts.families.insert(terminal_family, terminal_fonts);
    ctx.set_fonts(fonts);
    diagnostics
}

fn inspect_font_paths(paths: &[PathBuf]) -> Vec<FontDiagnostic> {
    paths
        .iter()
        .filter_map(|path| {
            fs::read(path)
                .map(|bytes| inspect_font_bytes(path, &bytes))
                .map_err(|err| eprintln!("failed to read terminal font {}: {err}", path.display()))
                .ok()
        })
        .collect()
}

fn inspect_font_bytes(path: &Path, bytes: &[u8]) -> FontDiagnostic {
    FontDiagnostic {
        path: path.to_path_buf(),
        coverage: GlyphCoverage::inspect(bytes).unwrap_or_default(),
    }
}

fn font_report_line(diagnostic: &FontDiagnostic) -> String {
    format!(
        "{}: {}",
        diagnostic.path.display(),
        diagnostic.coverage.summary()
    )
}

fn discover_nerd_fonts() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for directory in font_directories() {
        collect_nerd_fonts(&directory, &mut candidates);
    }
    candidates.sort_by_key(|path| (font_score(path), path.to_string_lossy().to_string()));
    candidates
}

fn font_directories() -> Vec<PathBuf> {
    let mut directories = Vec::new();
    if let Some(home) = env::var_os("HOME") {
        directories.push(PathBuf::from(home).join("Library/Fonts"));
    }
    directories.push(PathBuf::from("/Library/Fonts"));
    directories.push(PathBuf::from("/System/Library/Fonts"));
    directories
}

fn collect_nerd_fonts(directory: &Path, candidates: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_nerd_fonts(&path, candidates);
        } else if is_nerd_font_file(&path) {
            candidates.push(path);
        }
    }
}

fn is_nerd_font_file(path: &Path) -> bool {
    let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    let lower = file_name.to_ascii_lowercase();
    let is_font = lower.ends_with(".ttf") || lower.ends_with(".otf");
    is_font && lower.contains("nerdfont")
}

fn font_score(path: &Path) -> u8 {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mut score = 20;
    if file_name.contains("mono") {
        score -= 8;
    }
    if file_name.contains("regular") {
        score -= 6;
    }
    if file_name.contains("jetbrains") {
        score -= 3;
    }
    if file_name.contains("propo") {
        score += 8;
    }
    if file_name.contains("italic")
        || file_name.contains("bold")
        || file_name.contains("thin")
        || file_name.contains("light")
    {
        score += 5;
    }
    score
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_nerd_font_files() {
        assert!(is_nerd_font_file(Path::new(
            "/Users/me/Library/Fonts/JetBrainsMonoNerdFontMono-Regular.ttf"
        )));
        assert!(is_nerd_font_file(Path::new(
            "/Users/me/Library/Fonts/SymbolsNerdFont-Regular.otf"
        )));
        assert!(!is_nerd_font_file(Path::new(
            "/Users/me/Library/Fonts/PlainMono-Regular.ttf"
        )));
    }

    #[test]
    fn prefers_mono_regular_nerd_fonts() {
        let good = font_score(Path::new("JetBrainsMonoNerdFontMono-Regular.ttf"));
        let proportional = font_score(Path::new("JetBrainsMonoNerdFontPropo-Regular.ttf"));
        let italic = font_score(Path::new("JetBrainsMonoNerdFontMono-Italic.ttf"));

        assert!(good < proportional);
        assert!(good < italic);
    }

    #[test]
    fn configured_font_path_is_first_fallbacks_are_deduplicated() {
        let configured = PathBuf::from("/fonts/ConfiguredNerdFontMono-Regular.ttf");
        let discovered = vec![
            PathBuf::from("/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
            configured.clone(),
            PathBuf::from("/fonts/SymbolsNerdFont-Regular.otf"),
        ];

        let ordered = ordered_font_paths(Some(configured.clone()), discovered);

        assert_eq!(
            ordered,
            vec![
                configured,
                PathBuf::from("/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
                PathBuf::from("/fonts/SymbolsNerdFont-Regular.otf"),
            ]
        );
    }

    #[test]
    fn font_fallback_list_is_bounded() {
        let discovered = (0..12)
            .map(|idx| PathBuf::from(format!("/fonts/Test{idx}NerdFont-Regular.ttf")))
            .collect();

        let ordered = ordered_font_paths(None, discovered);

        assert_eq!(ordered.len(), 8);
    }

    #[test]
    fn invalid_font_bytes_have_no_glyph_coverage() {
        assert_eq!(GlyphCoverage::inspect(b"not a font"), None);
    }

    #[test]
    fn glyph_coverage_summary_names_probe_groups() {
        let coverage = GlyphCoverage {
            powerline: 3,
            devicons: 2,
        };

        assert_eq!(coverage.summary(), "Powerline 3/4, devicons 2/6");
    }

    #[test]
    fn font_report_line_includes_path_and_coverage() {
        let diagnostic = FontDiagnostic {
            path: PathBuf::from("/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
            coverage: GlyphCoverage {
                powerline: 4,
                devicons: 5,
            },
        };

        assert_eq!(
            font_report_line(&diagnostic),
            "/fonts/JetBrainsMonoNerdFontMono-Regular.ttf: Powerline 4/4, devicons 5/6"
        );
    }

    #[test]
    fn inspect_font_paths_skips_unreadable_files() {
        let diagnostics = inspect_font_paths(&[PathBuf::from("/definitely/missing/font.ttf")]);

        assert!(diagnostics.is_empty());
    }

    #[test]
    fn prefer_family_moves_matching_font_to_front() {
        let mut paths = vec![
            PathBuf::from("/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
            PathBuf::from("/fonts/FiraCodeNerdFontMono-Regular.ttf"),
        ];

        prefer_family(&mut paths, Some("firacode"));

        assert_eq!(
            paths,
            vec![
                PathBuf::from("/fonts/FiraCodeNerdFontMono-Regular.ttf"),
                PathBuf::from("/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
            ]
        );
    }

    #[test]
    fn prefer_family_is_noop_when_unset_or_unmatched() {
        let original = vec![
            PathBuf::from("/fonts/JetBrainsMonoNerdFontMono-Regular.ttf"),
            PathBuf::from("/fonts/FiraCodeNerdFontMono-Regular.ttf"),
        ];

        let mut none_pref = original.clone();
        prefer_family(&mut none_pref, None);
        assert_eq!(none_pref, original);

        let mut no_match = original.clone();
        prefer_family(&mut no_match, Some("does-not-exist"));
        assert_eq!(no_match, original);
    }
}
