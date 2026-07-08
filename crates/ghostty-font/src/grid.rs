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
use crate::coretext::PixelFormat;
use crate::metrics::Metrics;
use crate::presentation::Presentation;
use crate::resolver::CodepointResolver;

/// Which atlas a cached glyph lives in — the renderer's texture selector.
///
/// This is the **atlas-selector bit** the color-atlas follow-up chunk consumes:
/// the grid tags each glyph with the atlas it was uploaded to, and the renderer
/// maps this to the frozen `CellText.atlas` field (grayscale vs color). See
/// `docs/analysis/font-discovery.md` §8. Sprites and text (outline) glyphs go
/// to [`AtlasKind::Grayscale`]; color (emoji/BGRA) glyphs go to
/// [`AtlasKind::Color`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AtlasKind {
    /// The grayscale (alpha8) atlas: text outlines, sprites.
    Grayscale,
    /// The color (BGRA) atlas: emoji / color glyphs.
    Color,
}

impl AtlasKind {
    /// The presentation this atlas kind corresponds to (`Grayscale ⇒ text`,
    /// `Color ⇒ emoji`), the analog of upstream `getPresentation`.
    pub fn presentation(self) -> Presentation {
        match self {
            AtlasKind::Grayscale => Presentation::Text,
            AtlasKind::Color => Presentation::Emoji,
        }
    }
}

/// A cached, atlas-resident glyph: its atlas region plus placement offsets.
///
/// Mirrors upstream `font.Glyph` (`Glyph.zig`): `atlas_x`/`atlas_y` are the
/// top-left of the region in the atlas texture; `offset_x` is the left bearing
/// (cell-left to ink-left); `offset_y` is the top bearing (cell-bottom to
/// ink-top, baseline-relative, +Y up). A fully blank glyph (space) has a
/// zero-size region. `atlas` names which texture the region lives in — the
/// renderer's atlas selector (see [`AtlasKind`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachedGlyph {
    pub atlas_x: u32,
    pub atlas_y: u32,
    pub width: u32,
    pub height: u32,
    pub offset_x: i32,
    pub offset_y: i32,
    /// Which atlas this glyph was uploaded to (grayscale vs color).
    pub atlas: AtlasKind,
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

/// Which constraint to apply when rasterizing a glyph. Part of the glyph cache
/// key (upstream keys `renderGlyph` by `opts`, which carries the constraint) so
/// the same glyph rendered natural-size vs constrained caches separately.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ConstraintKind {
    /// No constraint for a text/outline glyph, but color (emoji) glyphs still
    /// get the fixed emoji `.cover` constraint (the historical default).
    DefaultColorEmoji,
    /// The Nerd Fonts per-codepoint constraint for `cp` (Item 3). Applied to PUA
    /// icon codepoints whose `nerd_font_constraints::get_constraint` is `Some`.
    Nerd(u32),
}

/// Key for the glyph render cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    index: FontIndex,
    glyph: u32,
    constraint: ConstraintKind,
}

/// The reduced shared font grid: resolver + grayscale/color atlases + caches.
///
/// F5-full adds the **color (BGRA) atlas** alongside the grayscale one so that
/// discovered color faces (emoji) rasterize and store correctly. `render_glyph`
/// routes each glyph by its rasterized pixel format: text/sprite (alpha8) → the
/// grayscale atlas, color (BGRA) → the color atlas. Each [`CachedGlyph`] carries
/// an [`AtlasKind`] tag so the renderer knows which texture to sample (the
/// atlas-selector seam; see `docs/analysis/font-discovery.md` §8).
pub struct Grid {
    resolver: CodepointResolver,
    metrics: Metrics,
    /// Grayscale (alpha8) atlas: text outlines + sprites.
    atlas: Atlas,
    /// Color (BGRA) atlas: emoji / color glyphs.
    color_atlas: Atlas,
    /// (codepoint, style) → resolved font index (caches negatives too, as
    /// upstream). Keyed by style so a bold and a regular request for the same
    /// codepoint resolve — and cache — independently.
    index_cache: HashMap<(u32, Style), Option<FontIndex>>,
    /// (index, glyph) → atlas-resident glyph.
    glyph_cache: HashMap<GlyphKey, CachedGlyph>,
    /// Sprite metrics derived once from `metrics`.
    sprite_metrics: ghostty_sprite::Metrics,
}

/// Initial atlas size (matches a reasonable first-pixels default; grows on
/// demand). Includes the atlas's permanent 1px border.
const INITIAL_ATLAS_SIZE: u32 = 512;

/// The number of cells a color (emoji) glyph is allowed to occupy when the
/// cell-fit constraint scales it. Emoji are width-2 in the terminal, matching
/// upstream's emoji test (constraint width 2). The `.cover` constraint takes
/// the smaller of the width/height cover factors, so a square emoji still
/// scales to the cell height and centers within the 2-cell span.
const EMOJI_CONSTRAINT_WIDTH: u32 = 2;

impl Grid {
    /// Build a grid over `resolver` with cell `metrics`, allocating fresh
    /// grayscale and color atlases.
    pub fn new(resolver: CodepointResolver, metrics: Metrics) -> Result<Grid, Error> {
        let atlas =
            Atlas::new(INITIAL_ATLAS_SIZE, Format::Grayscale).map_err(|_| Error::AtlasFull)?;
        let color_atlas =
            Atlas::new(INITIAL_ATLAS_SIZE, Format::Bgra).map_err(|_| Error::AtlasFull)?;
        let sprite_metrics = sprite_metrics_from(&metrics);
        Ok(Grid {
            resolver,
            metrics,
            atlas,
            color_atlas,
            index_cache: HashMap::new(),
            glyph_cache: HashMap::new(),
            sprite_metrics,
        })
    }

    /// The cell metrics this grid was built with.
    pub fn metrics(&self) -> &Metrics {
        &self.metrics
    }

    /// The grayscale atlas (text outlines + sprites).
    pub fn atlas(&self) -> &Atlas {
        &self.atlas
    }

    /// The color (BGRA) atlas (emoji / color glyphs).
    pub fn color_atlas(&self) -> &Atlas {
        &self.color_atlas
    }

    /// The resolver (for shaping, which needs the primary face).
    pub fn resolver(&self) -> &CodepointResolver {
        &self.resolver
    }

    /// Resolve a **regular-style** `cp` to a font index, caching the result
    /// (analog of `SharedGrid.getIndex`). Convenience wrapper over
    /// [`Grid::get_index_styled`].
    pub fn get_index(&mut self, cp: u32) -> Option<FontIndex> {
        self.get_index_styled(cp, Style::Regular)
    }

    /// Resolve `cp` under `style` to a font index, caching the result per
    /// `(cp, style)` (analog of `SharedGrid.getIndex`, which keys its index
    /// cache by style). The resolver maps a styled request that has no face to
    /// regular (its steps 1/5/7), so a bold codepoint the bold face lacks still
    /// resolves — but a codepoint the bold face *has* resolves to the bold
    /// slot, giving it a distinct [`FontIndex`] and thus a distinct glyph-cache
    /// entry from regular.
    pub fn get_index_styled(&mut self, cp: u32, style: Style) -> Option<FontIndex> {
        if let Some(v) = self.index_cache.get(&(cp, style)) {
            return *v;
        }
        let v = self.resolver.get_index(cp, style);
        self.index_cache.insert((cp, style), v);
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
        self.render_glyph_with(index, glyph_index, ConstraintKind::DefaultColorEmoji)
    }

    /// Render (and cache) a codepoint's glyph applying the **Nerd Fonts**
    /// per-codepoint constraint (Item 3) if `cp` has one. For a PUA icon
    /// codepoint whose `nerd_font_constraints::get_constraint(cp)` is `Some`,
    /// the glyph is scaled/aligned per the table (so oversized/misaligned
    /// powerline + devicon icons render at the correct cell-fit size); otherwise
    /// this behaves exactly like [`Grid::render_glyph`]. Applies regardless of
    /// which face resolved the codepoint (nerd-patched primary OR nerd-symbols
    /// fallback), matching upstream where the codepoint range — not the font —
    /// gates the constraint (`renderer/generic.zig:3189`).
    pub fn render_glyph_nerd(
        &mut self,
        index: FontIndex,
        glyph_index: u32,
        cp: u32,
    ) -> Result<CachedGlyph, Error> {
        let kind = if crate::nerd_font_constraints::get_constraint(cp).is_some() {
            ConstraintKind::Nerd(cp)
        } else {
            ConstraintKind::DefaultColorEmoji
        };
        self.render_glyph_with(index, glyph_index, kind)
    }

    fn render_glyph_with(
        &mut self,
        index: FontIndex,
        glyph_index: u32,
        constraint: ConstraintKind,
    ) -> Result<CachedGlyph, Error> {
        let key = GlyphKey {
            index,
            glyph: glyph_index,
            constraint,
        };
        if let Some(g) = self.glyph_cache.get(&key) {
            return Ok(*g);
        }

        let glyph = match index {
            FontIndex::Sprite => self.render_sprite(glyph_index)?,
            FontIndex::Face { .. } => self.render_face_glyph(index, glyph_index, constraint)?,
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
        self.render_codepoint_styled(cp, Style::Regular)
    }

    /// Style-aware [`Grid::render_codepoint`]: resolve `cp` under `style` and
    /// render the resolved face's glyph. Used by the renderer's non-shaped
    /// paths (sprite / single-codepoint fallback) so bold/italic single cells
    /// pick the styled face.
    pub fn render_codepoint_styled(
        &mut self,
        cp: u32,
        style: Style,
    ) -> Result<Option<CachedGlyph>, Error> {
        let Some(index) = self.get_index_styled(cp, style) else {
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
        // Apply the Nerd Fonts constraint for PUA icon codepoints (Item 3):
        // this single-codepoint cmap path is how nerd icons resolved from the
        // nerd-symbols fallback (and single-codepoint primary lookups) reach the
        // rasterizer, so it's where the constraint must be gated by `cp`.
        Ok(Some(self.render_glyph_nerd(index, glyph_index, cp)?))
    }

    /// Rasterize a face glyph and upload it, growing the atlas on `AtlasFull`.
    fn render_face_glyph(
        &mut self,
        index: FontIndex,
        glyph_index: u32,
        constraint: ConstraintKind,
    ) -> Result<CachedGlyph, Error> {
        let face = self
            .resolver
            .collection()
            .get_face(index)
            .ok_or(Error::NoFace)?;
        // Choose the constraint (upstream `renderer/generic.zig:3189` +
        // `SharedGrid.renderGlyph`): a Nerd Fonts PUA icon codepoint gets its
        // table constraint; otherwise a color (emoji) glyph gets the fixed
        // emoji `.cover` constraint and text glyphs get none. `constraint_width`
        // is 2 (emoji and multi-cell icons may span two cells; the constraint's
        // own `max_constraint_width` clamps it back to 1 where required — the
        // table sets `max_constraint_width = 1` for single-cell icons).
        let is_color = face.has_color();
        let constraint_arg: Option<(crate::constraint::Constraint, &Metrics, u32)> =
            match constraint {
                ConstraintKind::Nerd(cp) => crate::nerd_font_constraints::get_constraint(cp)
                    .map(|c| (c, &self.metrics, EMOJI_CONSTRAINT_WIDTH)),
                ConstraintKind::DefaultColorEmoji if is_color => Some((
                    crate::constraint::Constraint::EMOJI,
                    &self.metrics,
                    EMOJI_CONSTRAINT_WIDTH,
                )),
                ConstraintKind::DefaultColorEmoji => None,
            };
        let bmp = face
            .rasterize_constrained(glyph_index, constraint_arg)
            .map_err(Error::Rasterize)?;

        // Route by pixel format: BGRA color glyphs → color atlas, alpha8 text
        // glyphs → grayscale atlas.
        let kind = match bmp.format {
            PixelFormat::Alpha8 => AtlasKind::Grayscale,
            PixelFormat::Bgra => AtlasKind::Color,
        };

        // Baseline shift: `Bitmap::bearing_y` is measured from the glyph's
        // **baseline** (the CoreGraphics drawing origin CoreText returns bounds
        // relative to), but a `CachedGlyph::offset_y` must be **cell-bottom
        // relative** — the distance the shader subtracts from `cell_size.y`.
        // Upstream `renderGlyph` (coretext.zig, `2da015cd6`) folds this in
        // before deriving `offset_y`:
        //
        //   // We need to add the baseline position before passing to the
        //   // constrain function since it operates on cell-relative positions,
        //   // not baseline.
        //   .y = rect.origin.y + cell_baseline
        //   ...
        //   const offset_y = px_y + px_height;  // cell-bottom → ink-top
        //
        // Our reduced `rasterize` doesn't take `grid_metrics`, so we apply the
        // integer `cell_baseline` here instead. Because `cell_baseline` is a
        // whole pixel, adding it after `px_y = floor(y)` is arithmetically
        // identical to upstream adding it before the floor (the fractional part
        // and thus the rasterized pixels are unchanged). Sprites are drawn into
        // a full-cell canvas and are already cell-relative, so they do NOT get
        // this term (see `render_sprite`).
        let cell_baseline = self.metrics.cell_baseline as i32;

        // Blank glyph (space, control): a zero-size region with no ink.
        if bmp.width == 0 || bmp.height == 0 {
            return Ok(CachedGlyph {
                atlas_x: 0,
                atlas_y: 0,
                width: 0,
                height: 0,
                offset_x: bmp.bearing_x,
                offset_y: bmp.bearing_y + cell_baseline,
                atlas: kind,
            });
        }

        let region = self.reserve_growing(kind, bmp.width, bmp.height)?;
        self.atlas_mut(kind).set(region, &bmp.data);
        Ok(CachedGlyph {
            atlas_x: region.x,
            atlas_y: region.y,
            width: bmp.width,
            height: bmp.height,
            offset_x: bmp.bearing_x,
            offset_y: bmp.bearing_y + cell_baseline,
            atlas: kind,
        })
    }

    /// Rasterize a sprite glyph (`cp` == the codepoint) and upload it. Sprites
    /// are always grayscale.
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
                atlas: AtlasKind::Grayscale,
            });
        }

        let region = self.reserve_growing(AtlasKind::Grayscale, glyph.width, glyph.height)?;
        self.atlas.set(region, &glyph.alpha);
        Ok(CachedGlyph {
            atlas_x: region.x,
            atlas_y: region.y,
            width: glyph.width,
            height: glyph.height,
            offset_x: glyph.offset_x,
            offset_y: glyph.offset_y,
            atlas: AtlasKind::Grayscale,
        })
    }

    /// The atlas for a given kind (mutable).
    fn atlas_mut(&mut self, kind: AtlasKind) -> &mut Atlas {
        match kind {
            AtlasKind::Grayscale => &mut self.atlas,
            AtlasKind::Color => &mut self.color_atlas,
        }
    }

    /// Reserve a region in the `kind` atlas, growing it to 2× and retrying once
    /// on `AtlasFull` (mirrors `SharedGrid.renderGlyph`'s grow-and-retry).
    fn reserve_growing(
        &mut self,
        kind: AtlasKind,
        width: u32,
        height: u32,
    ) -> Result<Region, Error> {
        let atlas = self.atlas_mut(kind);
        match atlas.reserve(width, height) {
            Ok(r) => Ok(r),
            Err(crate::atlas::Error::AtlasFull) => {
                let new_size = atlas.size().saturating_mul(2);
                atlas.grow(new_size).map_err(|_| Error::AtlasFull)?;
                atlas.reserve(width, height).map_err(|_| Error::AtlasFull)
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

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use crate::coretext::Face;

    fn default_grid(size_px: f64) -> Grid {
        let face = Face::load_embedded(size_px).expect("load embedded");
        let metrics = Metrics::calc(face.face_metrics());
        let collection = crate::collection::Collection::new_with_default_fallbacks(face, size_px)
            .expect("default style table");
        let resolver = CodepointResolver::new(collection);
        Grid::new(resolver, metrics).expect("grid")
    }

    /// GlyphKey collision guard: a bold 'a' and a regular 'a' resolve to
    /// different [`FontIndex`] values (different style lists), so their
    /// `(index, glyph)` cache keys differ and they cache as **separate** atlas
    /// entries — not the same one. Without a style in the key, bold 'a' would
    /// collide with regular 'a' and share (thin) pixels.
    #[test]
    fn bold_and_regular_cache_separately() {
        let mut grid = default_grid(32.0);

        let reg_idx = grid.get_index_styled('a' as u32, Style::Regular).unwrap();
        let bold_idx = grid.get_index_styled('a' as u32, Style::Bold).unwrap();
        assert_ne!(
            reg_idx, bold_idx,
            "bold and regular 'a' must resolve to distinct font indices"
        );

        // Both faces map 'a' to the same glyph id (shared cmap), so the *only*
        // thing separating their cache entries is the style in the FontIndex.
        let reg_gid = grid
            .resolver()
            .collection()
            .get_face(reg_idx)
            .unwrap()
            .glyph_index('a')
            .unwrap();
        let bold_gid = grid
            .resolver()
            .collection()
            .get_face(bold_idx)
            .unwrap()
            .glyph_index('a')
            .unwrap();

        let reg_glyph = grid.render_glyph(reg_idx, reg_gid).expect("render reg a");
        let bold_glyph = grid
            .render_glyph(bold_idx, bold_gid)
            .expect("render bold a");

        // Distinct cache keys => distinct atlas regions (or at least not the
        // identical placement the collision bug would produce).
        assert_eq!(grid.glyph_cache.len(), 2, "two distinct cache entries");
        assert!(
            (reg_glyph.atlas_x, reg_glyph.atlas_y) != (bold_glyph.atlas_x, bold_glyph.atlas_y),
            "bold and regular 'a' should occupy different atlas regions"
        );

        // The bold glyph is heavier: with synthetic-free wght=700 the ink box is
        // at least as wide/tall as regular. (Exact pixels are macOS-rasterizer
        // sensitive; assert the weaker, stable invariant.)
        assert!(
            bold_glyph.width >= reg_glyph.width,
            "bold 'a' ({0}x{1}) should be no narrower than regular ('a' {2}x{3})",
            bold_glyph.width,
            bold_glyph.height,
            reg_glyph.width,
            reg_glyph.height
        );
    }
}
