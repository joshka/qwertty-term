//! Font grid construction for the renderer (macOS only).
//!
//! Builds a `qwertty-term-font` [`Grid`] (shaper + glyph→atlas cache + sprite
//! dispatch) over a CoreText [`Face`], at a given pixel size, honoring the
//! config `font-family` (falling back to the embedded JetBrains Mono). Returns
//! the grid together with its cell metrics — the cell width/height the render
//! [`Engine`](qwertty_term_renderer::engine::Engine) needs and the grid-geometry math
//! uses to map a pixel viewport to cols×rows.
//!
//! Mirrors how `crates/qwertty-term-renderer/tests/first_pixels.rs` builds its grid.

#![cfg(target_os = "macos")]

use qwertty_term_font::coretext::Face;
use qwertty_term_font::grid::Grid;
use qwertty_term_font::{CodepointResolver, Collection, Metrics};

/// Failure building a [`FontGrid`]: either loading the face or constructing the
/// grid (atlas) failed.
#[derive(Debug)]
pub enum FontError {
    /// The CoreText face could not be loaded.
    Face(qwertty_term_font::coretext::Error),
    /// The glyph grid / atlas could not be constructed.
    Grid(qwertty_term_font::grid::Error),
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
    // Whether the primary face actually resolved to the configured family (vs.
    // `load_by_name` silently falling back to the embedded JetBrains Mono on a
    // miss). Only a real match takes the named-family styled-completion path;
    // otherwise the embedded default chain is the correct behavior.
    let configured = family.filter(|n| !n.is_empty());
    let face = match configured {
        Some(name) => Face::load_by_name(name, size_px),
        None => Face::load_embedded(size_px),
    }
    .map_err(FontError::Face)?;
    let metrics = Metrics::calc(face.face_metrics());
    let (cell_width, cell_height) = (metrics.cell_width, metrics.cell_height);

    // A configured family whose primary really resolved gets its *own* styled
    // members (real discovered bold/italic first, then upstream's synthetic
    // ladder), with the embedded default chain behind them — see
    // `Collection::new_with_family_styles`. A miss (embedded fallback) or the
    // no-family default uses the embedded default chain directly
    // (`new_with_default_fallbacks`). Both mirror `SharedGridSet`'s two-phase
    // construction; the embedded fonts are vendored drift-tested assets, so a
    // load failure signals a corrupted binary, not a recoverable condition.
    let resolved_matches = configured.is_some_and(|name| {
        let got = face.family_name().to_lowercase();
        let want = name.to_lowercase();
        got.contains(&want) || want.contains(&got)
    });
    let collection = if let Some(name) = configured.filter(|_| resolved_matches) {
        Collection::new_with_family_styles(face, name, size_px).map_err(FontError::Face)?
    } else {
        Collection::new_with_default_fallbacks(face, size_px).map_err(FontError::Face)?
    };
    let resolver = CodepointResolver::new(collection);
    let grid = Grid::new(resolver, metrics).map_err(FontError::Grid)?;
    Ok(FontGrid {
        grid,
        cell_width,
        cell_height,
    })
}
