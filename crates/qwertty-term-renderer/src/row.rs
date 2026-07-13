//! Row background-extension heuristics. Port of `src/renderer/row.zig`
//! (commit `2da015cd6`).
//!
//! Upstream operates on raw page cells/styles (`page.Row`, `[]const
//! page.Cell`, `[]const Style`). This crate instead takes the already
//! resolved [`qwertty_term_vt::snapshot::SnapshotCell`] view, since
//! `SnapshotCell::style` already carries a fully-resolved `CellStyle` (no
//! separate style-table lookup needed) — see `docs/analysis/renderer-r0.md`
//! for the full divergence writeup. `SemanticPrompt` isn't currently
//! surfaced on `SnapshotRow`, so it's taken as an explicit parameter here
//! rather than read off the row (tracked as a deferral in that doc).

use qwertty_term_vt::color::Rgb;
use qwertty_term_vt::page::SemanticPrompt;
use qwertty_term_vt::snapshot::{SnapshotCell, SnapshotColor};

/// Resolve a [`SnapshotColor`] against the palette/default, mirroring
/// upstream's `Style.bg(cell, palette)`. Returns `None` for
/// `SnapshotColor::Default` (no explicit color set — "use the renderer's
/// default"), matching upstream's "no background on the style" case.
fn resolve_bg(color: SnapshotColor, palette: &qwertty_term_vt::color::Palette) -> Option<Rgb> {
    match color {
        SnapshotColor::Default => None,
        SnapshotColor::Palette(i) => Some(palette[i as usize]),
        SnapshotColor::Rgb { r, g, b } => Some(Rgb::new(r, g, b)),
    }
}

/// Returns true if the row of this pin should never have its background
/// color extended for filling padding space in the renderer. This is a set
/// of heuristics that help making our padding look better.
pub fn never_extend_bg(
    semantic_prompt: SemanticPrompt,
    cells: &[SnapshotCell],
    palette: &qwertty_term_vt::color::Palette,
    default_background: Option<Rgb>,
) -> bool {
    // Any semantic prompts should not have their background extended
    // because prompts often contain special formatting (such as
    // powerline) that looks bad when extended.
    match semantic_prompt {
        SemanticPrompt::Prompt | SemanticPrompt::PromptContinuation => return true,
        SemanticPrompt::None => {}
    }

    for cell in cells {
        // Powerline glyphs are a perfect-fit shape; never extend past them.
        // Checked before the background-color check so a powerline glyph
        // that also happens to carry a non-default bg still short-circuits
        // here, matching upstream's per-cell codepoint check ordering.
        if matches!(cell.ch as u32,
            0xE0B0..=0xE0C8 | 0xE0CA | 0xE0CC..=0xE0D2 | 0xE0D4)
        {
            return true;
        }

        // If any cell has a default background color then we don't extend
        // because the default background color probably looks good enough
        // as an extension. A default background is applied if there is no
        // background on the style or the explicitly set background matches
        // our default background.
        let bg = resolve_bg(cell.style.bg, palette);
        match bg {
            None => return true,
            Some(bg) => {
                if Some(bg) == default_background {
                    return true;
                }
            }
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwertty_term_vt::snapshot::{CellStyle, CellWidth};

    fn cell(ch: char, bg: SnapshotColor) -> SnapshotCell {
        SnapshotCell {
            ch,
            combining: Vec::new(),
            width: CellWidth::Narrow,
            style: CellStyle {
                bg,
                ..CellStyle::default()
            },
            link: None,
        }
    }

    #[test]
    fn prompt_rows_never_extend() {
        let palette = qwertty_term_vt::color::DEFAULT;
        let cells = vec![cell('x', SnapshotColor::Rgb { r: 1, g: 2, b: 3 })];
        assert!(never_extend_bg(
            SemanticPrompt::Prompt,
            &cells,
            &palette,
            None
        ));
        assert!(never_extend_bg(
            SemanticPrompt::PromptContinuation,
            &cells,
            &palette,
            None
        ));
    }

    #[test]
    fn default_bg_cell_never_extends() {
        let palette = qwertty_term_vt::color::DEFAULT;
        // No explicit bg (SnapshotColor::Default) resolves to `None`, which
        // always triggers the "never extend" path.
        let cells = vec![cell('x', SnapshotColor::Default)];
        assert!(never_extend_bg(
            SemanticPrompt::None,
            &cells,
            &palette,
            None
        ));

        // An explicit bg that happens to equal the terminal's default
        // background also never extends.
        let default_bg = Rgb::new(10, 20, 30);
        let cells = vec![cell(
            'x',
            SnapshotColor::Rgb {
                r: 10,
                g: 20,
                b: 30,
            },
        )];
        assert!(never_extend_bg(
            SemanticPrompt::None,
            &cells,
            &palette,
            Some(default_bg)
        ));
    }

    #[test]
    fn powerline_glyph_never_extends() {
        let palette = qwertty_term_vt::color::DEFAULT;
        let cells = vec![cell('\u{E0B0}', SnapshotColor::Rgb { r: 1, g: 2, b: 3 })];
        assert!(never_extend_bg(
            SemanticPrompt::None,
            &cells,
            &palette,
            None
        ));
    }

    #[test]
    fn non_default_bg_extends() {
        let palette = qwertty_term_vt::color::DEFAULT;
        let default_bg = Rgb::new(0, 0, 0);
        let cells = vec![cell(
            'x',
            SnapshotColor::Rgb {
                r: 10,
                g: 20,
                b: 30,
            },
        )];
        assert!(!never_extend_bg(
            SemanticPrompt::None,
            &cells,
            &palette,
            Some(default_bg)
        ));
    }
}
