//! Font grid construction for the renderer (macOS only).
//!
//! Builds a `ghostty-font` [`Grid`] (shaper + glyph→atlas cache + sprite
//! dispatch) over a CoreText [`Face`], at a given pixel size, honoring the
//! config `font-family` (falling back to the embedded JetBrains Mono). Returns
//! the grid together with its cell metrics — the cell width/height the render
//! [`Engine`](ghostty_renderer::engine::Engine) needs and the grid-geometry math
//! uses to map a pixel viewport to cols×rows.
//!
//! Mirrors how `crates/ghostty-renderer/tests/first_pixels.rs` builds its grid.

#![cfg(target_os = "macos")]

use ghostty_font::coretext::Face;
use ghostty_font::grid::Grid;
use ghostty_font::{CodepointResolver, Collection, Metrics};

/// Failure building a [`FontGrid`]: either loading the face or constructing the
/// grid (atlas) failed.
#[derive(Debug)]
pub enum FontError {
    /// The CoreText face could not be loaded.
    Face(ghostty_font::coretext::Error),
    /// The glyph grid / atlas could not be constructed.
    Grid(ghostty_font::grid::Error),
}

impl std::fmt::Display for FontError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FontError::Face(e) => write!(f, "font face load failed: {e:?}"),
            FontError::Grid(e) => write!(f, "font grid build failed: {e:?}"),
        }
    }
}

impl std::error::Error for FontError {}

/// A built font grid plus its cell metrics.
pub struct FontGrid {
    /// The shaping + atlas grid the render engine drives.
    pub grid: Grid,
    /// Cell width in device pixels.
    pub cell_width: u32,
    /// Cell height in device pixels.
    pub cell_height: u32,
}

/// Build a [`FontGrid`] at `size_px` pixels. If `family` is `Some`, load that
/// CoreText family (falling back to the embedded face when it doesn't resolve);
/// otherwise use the embedded JetBrains Mono.
///
/// `size_px` should already account for the display's backing scale
/// (`contentsScale`) so glyphs are rasterized at native resolution.
pub fn build(family: Option<&str>, size_px: f64) -> Result<FontGrid, FontError> {
    let face = match family {
        Some(name) if !name.is_empty() => Face::load_by_name(name, size_px),
        _ => Face::load_embedded(size_px),
    }
    .map_err(FontError::Face)?;
    let metrics = Metrics::calc(face.face_metrics());
    let (cell_width, cell_height) = (metrics.cell_width, metrics.cell_height);
    // Explicit nerd-symbols fallback slot ahead of discovery, mirroring
    // upstream's `SharedGridSet` default-chain construction (see
    // `Collection::new_with_default_fallbacks`). The embedded nerd-symbols
    // font is a vendored, drift-tested asset (`embedded::SYMBOLS_NERD_FONT_MONO`
    // / `tests/font_manifest.rs`), so a load failure here would indicate a
    // corrupted binary rather than a recoverable runtime condition.
    let collection =
        Collection::new_with_default_fallbacks(face, size_px).map_err(FontError::Face)?;
    let resolver = CodepointResolver::new(collection);
    let grid = Grid::new(resolver, metrics).map_err(FontError::Grid)?;
    Ok(FontGrid {
        grid,
        cell_width,
        cell_height,
    })
}
