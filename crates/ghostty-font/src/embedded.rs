//! Fonts embedded with `ghostty-font`. These are only actually embedded in
//! the binary if they are referenced by the code, so fonts used only for
//! tests will not result in the final binary being larger.
//!
//! Port of Ghostty's `src/font/embedded.zig` (commit `2da015cd6`). Ghostty
//! embeds a variable-weight JetBrains Mono built by its Zig build script from
//! a font-tools source, plus a `nerd_fonts_symbols_only` font, neither of
//! which are present as static files in the reference checkout's `res/`
//! directory (both are fetched build dependencies declared in
//! `build.zig.zon`). This port instead embeds the static files that *are*
//! present under `res/`, which cover the same roles:
//!
//! - [`JETBRAINS_MONO`] takes the place of ghostty's `variable`/`regular`
//!   default fallback font (static, non-variable build of the same
//!   typeface).
//! - [`JETBRAINS_MONO_NERD`] takes the place of `symbols_nerd_font`: it is
//!   JetBrains Mono patched with Nerd Fonts' symbol glyphs, so it covers the
//!   same "nerd symbols" need (a symbols-only-patched build was not present
//!   locally to copy verbatim).
//! - [`EMOJI_TEXT`] takes the place of `emoji_text`, byte-identical to
//!   ghostty's own `res/NotoEmoji-Regular.ttf`.
//!
//! Be careful to ensure that any fonts embedded here are licensed for
//! redistribution and include their license as necessary; see `res/OFL.txt`.

/// Default fallback font: JetBrains Mono (static regular weight).
pub const JETBRAINS_MONO: &[u8] = include_bytes!("../res/JetBrainsMonoNoNF-Regular.ttf");

/// JetBrains Mono patched with Nerd Fonts symbols.
pub const JETBRAINS_MONO_NERD: &[u8] = include_bytes!("../res/JetBrainsMonoNerdFont-Regular.ttf");

/// Text-presentation emoji font, for testing emoji glyph handling without
/// the much larger color emoji font.
pub const EMOJI_TEXT: &[u8] = include_bytes!("../res/NotoEmoji-Regular.ttf");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_fonts_parse() {
        for (name, bytes) in [
            ("JETBRAINS_MONO", JETBRAINS_MONO),
            ("JETBRAINS_MONO_NERD", JETBRAINS_MONO_NERD),
            ("EMOJI_TEXT", EMOJI_TEXT),
        ] {
            ttf_parser::Face::parse(bytes, 0)
                .unwrap_or_else(|e| panic!("{name} failed to parse: {e:?}"));
        }
    }
}
