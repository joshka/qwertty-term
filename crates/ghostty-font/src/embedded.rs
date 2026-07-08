//! Fonts embedded with `ghostty-font`. These are only actually embedded in
//! the binary if they are referenced by the code, so fonts used only for
//! tests will not result in the final binary being larger.
//!
//! Port of Ghostty's `src/font/embedded.zig` (commit `2da015cd6`). Ghostty
//! embeds a variable-weight JetBrains Mono (regular + italic) built by its
//! Zig build script from JetBrains' font-tools source (`build.zig.zon`'s
//! `jetbrains_mono_variable` / `jetbrains_mono_variable_italic` fetches),
//! plus a `nerd_fonts_symbols_only` font (`SymbolsNerdFontMono-Regular.ttf`
//! from the upstream Nerd Fonts release), neither of which are present as
//! static files in the reference checkout's `res/` directory (both are
//! fetched build dependencies). This crate vendors the *exact byte-identical*
//! upstream files instead (see `res/MANIFEST.sha256` for provenance hashes):
//!
//! - [`JETBRAINS_MONO_VARIABLE`] is upstream's `variable`: the variable-weight
//!   (`wght` axis) JetBrains Mono, used as the default regular/bold fallback
//!   font (bold is the same file with `wght=700` set as a variation once
//!   variable-axis instancing is wired; until then it is loaded at its
//!   default instance, `wght=400`).
//! - [`JETBRAINS_MONO_VARIABLE_ITALIC`] is upstream's `variable_italic`: the
//!   italic companion, same axis.
//! - [`SYMBOLS_NERD_FONT_MONO`] is upstream's `symbols_nerd_font`: the
//!   symbols-only Nerd Fonts patched build (byte-identical to the release
//!   asset upstream fetches), used as the explicit nerd-symbols fallback
//!   slot ahead of system discovery.
//! - [`EMOJI_TEXT`] takes the place of `emoji_text`, byte-identical to
//!   ghostty's own `res/NotoEmoji-Regular.ttf`.
//!
//! Be careful to ensure that any fonts embedded here are licensed for
//! redistribution and include their license as necessary; see `res/OFL.txt`
//! (JetBrains Mono) and `res/NerdFonts-LICENSE.txt` (Nerd Fonts patcher/
//! symbols font, MIT).

/// Default fallback font: JetBrains Mono, variable weight (`wght` axis),
/// upstream's `embedded.variable`.
pub const JETBRAINS_MONO_VARIABLE: &[u8] = include_bytes!("../res/JetBrainsMono-Variable.ttf");

/// Default fallback font, italic: JetBrains Mono Italic, variable weight,
/// upstream's `embedded.variable_italic`.
pub const JETBRAINS_MONO_VARIABLE_ITALIC: &[u8] =
    include_bytes!("../res/JetBrainsMono-Italic-Variable.ttf");

/// Symbols-only Nerd Fonts build, upstream's `embedded.symbols_nerd_font`:
/// the explicit nerd-symbols fallback slot (PUA glyphs: powerline, devicons,
/// etc.), added ahead of system discovery.
pub const SYMBOLS_NERD_FONT_MONO: &[u8] = include_bytes!("../res/SymbolsNerdFontMono-Regular.ttf");

/// Text-presentation emoji font, for testing emoji glyph handling without
/// the much larger color emoji font.
pub const EMOJI_TEXT: &[u8] = include_bytes!("../res/NotoEmoji-Regular.ttf");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_parse() {
        for (name, bytes) in [
            ("JETBRAINS_MONO_VARIABLE", JETBRAINS_MONO_VARIABLE),
            (
                "JETBRAINS_MONO_VARIABLE_ITALIC",
                JETBRAINS_MONO_VARIABLE_ITALIC,
            ),
            ("SYMBOLS_NERD_FONT_MONO", SYMBOLS_NERD_FONT_MONO),
            ("EMOJI_TEXT", EMOJI_TEXT),
        ] {
            ttf_parser::Face::parse(bytes, 0)
                .unwrap_or_else(|e| panic!("{name} failed to parse: {e:?}"));
        }
    }

    /// The primary default font is a true variable font: it must carry an
    /// `fvar` table with a `wght` axis (upstream relies on this to synthesize
    /// bold via a `wght=700` variation instance rather than a separate file).
    #[test]
    fn jetbrains_mono_variable_has_wght_axis() {
        let face =
            ttf_parser::Face::parse(JETBRAINS_MONO_VARIABLE, 0).expect("parse variable font");
        let axes: Vec<_> = face
            .variation_axes()
            .into_iter()
            .map(|a| a.tag.to_bytes())
            .collect();
        assert!(
            axes.contains(b"wght"),
            "expected a wght variation axis, found {axes:?}"
        );
    }

    // A byte-for-byte drift guard against `res/font-manifest.sha256` lives in
    // `tests/font_manifest.rs` (a self-contained SHA-256, no new crate
    // dependency, following the same pattern as
    // `ghostty-termio/tests/shell_integration_scripts.rs`).
}
