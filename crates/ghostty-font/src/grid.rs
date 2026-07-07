//! Glyph render cache + atlas upload (SharedGrid-reduced).
//!
//! Reduced port of Ghostty's `src/font/SharedGrid.zig` (commit `2da015cd6`),
//! without the locking (single-threaded, per the plan). See
//! `docs/analysis/font-shaping.md` for the upstream two-cache flow
//! (`getIndex` cache + `renderGlyph` cache) and the atlas upload path.
//!
//! The `Grid` owns a single grayscale [`Atlas`] and two caches:
//!
//! - a codepoint → [`FontIndex`] cache (analog of `SharedGrid.getIndex`), and
//! - a `(FontIndex, glyph_id)` → [`CachedGlyph`] render cache (analog of
//!   `SharedGrid.renderGlyph`).
//!
//! On a render miss it rasterizes (face glyph via CoreText F5, or a sprite via
//! `ghostty-sprite`), reserves an atlas region, uploads the bitmap, and caches
//! the atlas coordinates + placement offsets — the "returning atlas coords"
//! contract the R4 cell engine consumes. On `AtlasFull` it grows the atlas to
//! 2× and retries, mirroring `SharedGrid.renderGlyph`'s escalation.

use std::collections::HashMap;

use crate::atlas::{Atlas, Format, Region};
use crate::collection::{FontIndex, Style};
use crate::metrics::Metrics;
use crate::resolver::CodepointResolver;

/// A cached, atlas-resident glyph: its atlas region plus placement offsets.
///
/// Mirrors upstream `font.Glyph` (`Glyph.zig`): `atlas_x`/`atlas_y` are the
/// top-left of the region in the atlas texture; `offset_x` is the left bearing
/// (cell-left to ink-left); `offset_y` is the top bearing (cell-bottom to
/// ink-top, baseline-relative, +Y up). A fully blank glyph (space) has a
/// zero-size region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedGlyph {
    pub atlas_x: u32,
    pub atlas_y: u32,
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
}

/// Errors from rendering a glyph into the grid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The atlas is full and could not be grown to fit the glyph.
    AtlasFull,
    /// The face could not rasterize the glyph.
    Rasterize(crate::coretext::Error),
    /// The codepoint was routed to the sprite subsystem but it declined to
    /// render it (should not happen for a codepoint that `has_codepoint`).
    SpriteMissing,
    /// A non-sprite index with no backing face (empty style slot).
    NoFace,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AtlasFull => write!(f, "atlas is full and could not grow"),
            Error::Rasterize(e) => write!(f, "rasterize failed: {e}"),
            Error::SpriteMissing => write!(f, "sprite subsystem declined a sprite codepoint"),
            Error::NoFace => write!(f, "font index has no backing face"),
        }
    }
}

impl std::error::Error for Error {}

/// Key for the glyph render cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    index: FontIndex,
    glyph: u32,
}

/// The reduced shared font grid: resolver + grayscale atlas + caches.
pub struct Grid {
    resolver: CodepointResolver,
    metrics: Metrics,
    atlas: Atlas,
    /// Codepoint → resolved font index (caches negatives too, as upstream).
    index_cache: HashMap<u32, Option<FontIndex>>,
    /// (index, glyph) → atlas-resident glyph.
    glyph_cache: HashMap<GlyphKey, CachedGlyph>,
    /// Sprite metrics derived once from `metrics`.
    sprite_metrics: ghostty_sprite::Metrics,
}

/// Initial atlas size (matches a reasonable first-pixels default; grows on
/// demand). Includes the atlas's permanent 1px border.
const INITIAL_ATLAS_SIZE: u32 = 512;

impl Grid {
    /// Build a grid over `resolver` with cell `metrics`, allocating a fresh
    /// grayscale atlas.
    pub fn new(resolver: CodepointResolver, metrics: Metrics) -> Result<Grid, Error> {
        let atlas =
            Atlas::new(INITIAL_ATLAS_SIZE, Format::Grayscale).map_err(|_| Error::AtlasFull)?;
        let sprite_metrics = sprite_metrics_from(&metrics);
        Ok(Grid {
            resolver,
            metrics,
            atlas,
            index_cache: HashMap::new(),
            glyph_cache: HashMap::new(),
            sprite_metrics,
        })
    }

    /// The cell metrics this grid was built with.
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// The underlying atlas (for GPU upload / inspection).
    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    /// The resolver (for shaping, which needs the primary face).
    pub fn resolver(&self) -> &CodepointResolver {
        &self.resolver
    }

    /// Resolve `cp` to a font index, caching the result (analog of
    /// `SharedGrid.getIndex`, reduced to a single style).
    pub fn get_index(&mut self, cp: u32) -> Option<FontIndex> {
        if let Some(v) = self.index_cache.get(&cp) {
            return *v;
        }
        let v = self.resolver.get_index(cp, Style::Regular);
        self.index_cache.insert(cp, v);
        v
    }

    /// Render (and cache) the glyph at `glyph_index` for a face `index`,
    /// returning its atlas-resident placement (analog of
    /// `SharedGrid.renderGlyph`).
    pub fn render_glyph(
        &mut self,
        index: FontIndex,
        glyph_index: u32,
    ) -> Result<CachedGlyph, Error> {
        let key = GlyphKey {
            index,
            glyph: glyph_index,
        };
        if let Some(g) = self.glyph_cache.get(&key) {
            return Ok(*g);
        }

        let glyph = match index {
            FontIndex::Sprite => self.render_sprite(glyph_index)?,
            FontIndex::Face { .. } => self.render_face_glyph(index, glyph_index)?,
        };
        self.glyph_cache.insert(key, glyph);
        Ok(glyph)
    }

    /// Convenience: resolve a codepoint and render its glyph in one call.
    ///
    /// For a face codepoint this looks up the face glyph id via the face's cmap
    /// (the reduced path used when there is no shaper output to consume, e.g.
    /// sprite codepoints and single-codepoint verification). For sprite
    /// codepoints the "glyph id" is the codepoint itself (codepoint == glyph
    /// for special fonts, as upstream).
    pub fn render_codepoint(&mut self, cp: u32) -> Result<Option<CachedGlyph>, Error> {
        let Some(index) = self.get_index(cp) else {
            return Ok(None);
        };
        let glyph_index = match index {
            // Special fonts: codepoint IS the glyph id (upstream
            // `harfbuzz.zig:132` "codepoint == glyph_index").
            FontIndex::Sprite => cp,
            FontIndex::Face { .. } => {
                let face = self
                    .resolver
                    .collection()
                    .get_face(index)
                    .ok_or(Error::NoFace)?;
                let Some(ch) = char::from_u32(cp) else {
                    return Ok(None);
                };
                match face.glyph_index(ch) {
                    Some(g) => g,
                    None => return Ok(None),
                }
            }
        };
        Ok(Some(self.render_glyph(index, glyph_index)?))
    }

    /// Rasterize a face glyph and upload it, growing the atlas on `AtlasFull`.
    fn render_face_glyph(
        &mut self,
        index: FontIndex,
        glyph_index: u32,
    ) -> Result<CachedGlyph, Error> {
        let face = self
            .resolver
            .collection()
            .get_face(index)
            .ok_or(Error::NoFace)?;
        let bmp = face.rasterize(glyph_index).map_err(Error::Rasterize)?;

        // Blank glyph (space, control): a zero-size region with no ink.
        if bmp.width == 0 || bmp.height == 0 {
            return Ok(CachedGlyph {
                atlas_x: 0,
                atlas_y: 0,
                width: 0,
                height: 0,
                offset_x: bmp.bearing_x,
                offset_y: bmp.bearing_y,
            });
        }

        let region = self.reserve_growing(bmp.width, bmp.height)?;
        self.atlas.set(region, &bmp.data);
        Ok(CachedGlyph {
            atlas_x: region.x,
            atlas_y: region.y,
            width: bmp.width,
            height: bmp.height,
            offset_x: bmp.bearing_x,
            offset_y: bmp.bearing_y,
        })
    }

    /// Rasterize a sprite glyph (`cp` == the codepoint) and upload it.
    fn render_sprite(&mut self, cp: u32) -> Result<CachedGlyph, Error> {
        let glyph = ghostty_sprite::render(cp, &self.sprite_metrics).ok_or(Error::SpriteMissing)?;

        if glyph.width == 0 || glyph.height == 0 {
            return Ok(CachedGlyph {
                atlas_x: 0,
                atlas_y: 0,
                width: 0,
                height: 0,
                offset_x: glyph.offset_x,
                offset_y: glyph.offset_y,
            });
        }

        let region = self.reserve_growing(glyph.width, glyph.height)?;
        self.atlas.set(region, &glyph.alpha);
        Ok(CachedGlyph {
            atlas_x: region.x,
            atlas_y: region.y,
            width: glyph.width,
            height: glyph.height,
            offset_x: glyph.offset_x,
            offset_y: glyph.offset_y,
        })
    }

    /// Reserve a region, growing the atlas to 2× and retrying once on
    /// `AtlasFull` (mirrors `SharedGrid.renderGlyph`'s grow-and-retry).
    fn reserve_growing(&mut self, width: u32, height: u32) -> Result<Region, Error> {
        match self.atlas.reserve(width, height) {
            Ok(r) => Ok(r),
            Err(crate::atlas::Error::AtlasFull) => {
                let new_size = self.atlas.size().saturating_mul(2);
                self.atlas.grow(new_size).map_err(|_| Error::AtlasFull)?;
                self.atlas
                    .reserve(width, height)
                    .map_err(|_| Error::AtlasFull)
            }
            Err(_) => Err(Error::AtlasFull),
        }
    }
}

/// Derive `ghostty_sprite::Metrics` from font [`Metrics`].
///
/// Maps the font crate's cell metrics onto the sprite crate's flat metrics
/// struct. `overline_position` differs in sign convention (font: from top, can
/// be negative; sprite: same), and box thickness drives seam alignment for all
/// box/branch/powerline glyphs.
fn sprite_metrics_from(m: &Metrics) -> ghostty_sprite::Metrics {
    ghostty_sprite::Metrics {
        cell_width: m.cell_width,
        cell_height: m.cell_height,
        cell_baseline: m.cell_baseline,
        underline_position: m.underline_position,
        underline_thickness: m.underline_thickness,
        strikethrough_position: m.strikethrough_position,
        strikethrough_thickness: m.strikethrough_thickness,
        overline_position: m.overline_position,
        overline_thickness: m.overline_thickness,
        box_thickness: m.box_thickness,
        cursor_thickness: m.cursor_thickness,
        cursor_height: m.cursor_height,
    }
}
