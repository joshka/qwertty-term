//! Kitty graphics unicode placeholder (`U=1` virtual placement) support. Port
//! of `src/terminal/kitty/graphics_unicode.zig`, commit `2da015cd6`.
//!
//! A `U=1` display command creates a [`super::Location::Virtual`] placement:
//! no screen rect is tracked; instead the client paints `U+10EEEE` placeholder
//! codepoints into the grid, using the cell's fg color (image id), underline
//! color (placement id), and up to three combining diacritics (row/column
//! index + high image-id byte) to say *which* placement, and which fragment
//! of it, a given cell shows. [`placement_iterator`] walks a row range and
//! reassembles these cells into [`Placement`]s (runs of contiguous,
//! compatible placeholder cells); [`Placement::render_placement`] then
//! resolves a `Placement` plus the stored [`super::Placement`] and
//! [`super::Image`] into a renderer-facing [`RenderPlacement`] — the
//! aspect-ratio-preserving source/dest rect math.
//!
//! The print-path half (recognizing `U+10EEEE` and setting
//! `Row::kitty_virtual_placeholder`) lives in `crate::terminal::print`
//! (`print_cell`); this module is the read side the renderer uses to walk
//! flagged rows back into placements.

use crate::page::Cell as PageCell;
use crate::page::style::Color as StyleColor;
use crate::pagelist::{Direction, Pin};

use super::{Image, ImageStorage, Location};

/// Codepoint for the unicode placeholder character (`U+10EEEE`).
pub const PLACEHOLDER: u32 = 0x10EEEE;

/// Errors from [`Placement::render_placement`]. Port of `graphics_unicode.Placement.Error`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPlacementError {
    /// The requested/derived grid size doesn't fit in a [`crate::page::size::CellCountInt`].
    PlacementGridOutOfBounds,
    /// No matching stored placement (by placement id, or any virtual placement for the
    /// image) was found.
    PlacementMissingPlacement,
}

impl std::fmt::Display for RenderPlacementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for RenderPlacementError {}

/// Returns an iterator over all virtual placements starting at `pin`, in row order
/// (top-to-bottom, left-to-right), stopping at `limit` (inclusive) if given, else running to
/// the end of the page list. Port of `placementIterator`.
///
/// # Safety
/// `pin` (and `limit`, if given) must reference live nodes in the same page list.
pub unsafe fn placement_iterator(pin: Pin, limit: Option<Pin>) -> PlacementIterator {
    let mut row_it = unsafe { pin.row_iterator(Direction::RightDown, limit) };
    let row = unsafe { row_it.next() };
    PlacementIterator { row_it, row }
}

/// Iterator over unicode virtual placements. Port of `graphics_unicode.PlacementIterator`.
pub struct PlacementIterator {
    row_it: crate::pagelist::RowIterator,
    row: Option<Pin>,
}

impl PlacementIterator {
    /// Returns the next virtual placement (a run of 1+ contiguous compatible placeholder
    /// cells), or `None` when exhausted. Port of `PlacementIterator.next`.
    ///
    /// # Safety
    /// Every node reachable from the iterator's starting pin must be live.
    pub unsafe fn next(&mut self) -> Option<Placement> {
        while let Some(row) = self.row {
            // This row flag is set on rows that have the virtual placeholder.
            let has_placeholder = unsafe {
                let (r, _) = row.row_and_cell();
                (*r).kitty_virtual_placeholder()
            };
            if !has_placeholder {
                self.row = unsafe { self.row_it.next() };
                continue;
            }

            // Our current run. A run is always only a single row.
            let mut run: Option<IncompletePlacement> = None;

            // Iterate over the remaining cells and find one with a placeholder.
            // `cells` holds raw pointers into the row starting at `row.x()`;
            // we advance `self.row`'s x in lockstep so that if we return
            // mid-row, the next call resumes from the same cell (mirrors
            // upstream aliasing `row` against `self.row`).
            let cells: *mut [PageCell] = unsafe { row.cells(crate::pagelist::CellSubset::Right) };
            let start_x = row.x();
            // `<*mut [T]>::len` reads the slice pointer's length metadata
            // directly, with no deref (avoids the raw-pointer autoref lint).
            let len = cells.len();
            for i in 0..len {
                // `cur` now points at the top-left pin of the (possibly
                // still-incomplete) placement.
                let mut cur = row;
                cur.x = start_x + i as crate::page::size::CellCountInt;
                self.row = Some(cur);

                let cell: &PageCell = unsafe { &(*cells)[i] };
                if cell.codepoint() != PLACEHOLDER {
                    if let Some(prev) = run {
                        return Some(prev.complete());
                    }
                    continue;
                }

                let curr = unsafe { IncompletePlacement::init(cur, cell) };
                match run {
                    Some(mut prev) => {
                        if !prev.append(&curr) {
                            // Can't append: complete the previous run and
                            // return it. `self.row` already points back at
                            // this same cell, so the next call continues the
                            // new run from here.
                            return Some(prev.complete());
                        }
                        run = Some(prev);
                    }
                    None => {
                        let mut prev = curr;
                        if prev.row.is_none() {
                            prev.row = Some(0);
                        }
                        if prev.col.is_none() {
                            prev.col = Some(0);
                        }
                        run = Some(prev);
                    }
                }
            }

            // Move to the next row no matter what.
            self.row = unsafe { self.row_it.next() };

            if let Some(prev) = run {
                return Some(prev.complete());
            }
        }

        None
    }
}

/// A virtual placement in the terminal. May represent more than one cell if the cells combine
/// into a run. Port of `graphics_unicode.Placement`.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    /// The top-left pin of the placement.
    pub pin: Pin,

    /// The image ID and placement ID for this virtual placement. The image ID is encoded in
    /// the fg color (plus an optional 8-bit high value in the 3rd diacritic). The placement ID
    /// is encoded in the underline color (optionally).
    pub image_id: u32,
    pub placement_id: u32,

    /// Starting row/col index (0-indexed) for the fragment of the image shown in this
    /// placement.
    pub col: u32,
    pub row: u32,

    /// Width/height in cells of this placement.
    pub width: u32,
    pub height: u32,
}

impl Placement {
    /// Convert this virtual placement into a renderer-facing [`RenderPlacement`], resolving
    /// against the stored placement (rows/columns) and image, and honoring aspect-ratio
    /// preserving centering. Port of `Placement.renderPlacement`.
    pub fn render_placement(
        &self,
        storage: &ImageStorage,
        img: &Image,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<RenderPlacement, RenderPlacementError> {
        // Naming convention (matches upstream): `img_*` is the original image,
        // `p_*` is the final placement, `vp_*` is this virtual placement.
        let p_grid = self.grid(storage, img, cell_width, cell_height)?;

        let img_width_f64 = img.width as f64;
        let img_height_f64 = img.height as f64;

        // Fit the source image into the grid size while preserving aspect
        // ratio, centering horizontally/vertically as necessary.
        struct Scale {
            x_offset: f64,
            y_offset: f64,
            x_scale: f64,
            y_scale: f64,
        }
        let p_scale: Scale = {
            let p_cols_px = (p_grid.0 * cell_width) as f64;
            let p_rows_px = (p_grid.1 * cell_height) as f64;
            if img_width_f64 * p_rows_px > img_height_f64 * p_cols_px {
                // Image is wider than the grid: fit width, center height.
                let x_scale = p_cols_px / img_width_f64.max(1.0);
                let y_scale = x_scale;
                let y_offset = (p_rows_px - img_height_f64 * y_scale) / 2.0;
                Scale {
                    x_offset: 0.0,
                    y_offset,
                    x_scale,
                    y_scale,
                }
            } else {
                // Image is taller than the grid: fit height, center width.
                let y_scale = p_rows_px / img_height_f64.max(1.0);
                let x_scale = y_scale;
                let x_offset = (p_cols_px - img_width_f64 * x_scale) / 2.0;
                Scale {
                    x_offset,
                    y_offset: 0.0,
                    x_scale,
                    y_scale,
                }
            }
        };

        // Scale the original image per `p_scale`.
        let img_scaled_x_offset = p_scale.x_offset / p_scale.x_scale;
        let img_scaled_y_offset = p_scale.y_offset / p_scale.y_scale;
        let img_scaled_width = img_width_f64 + (img_scaled_x_offset * 2.0);
        let img_scaled_height = img_height_f64 + (img_scaled_y_offset * 2.0);

        // The source rectangle for the scaled image, in scaled-image space.
        let (mut src_x, mut src_y, mut src_width, mut src_height) = {
            let vp_width = self.width as f64;
            let vp_height = self.height as f64;
            let vp_col = self.col as f64;
            let vp_row = self.row as f64;
            let p_grid_cols = p_grid.0 as f64;
            let p_grid_rows = p_grid.1 as f64;

            let width = img_scaled_width * (vp_width / p_grid_cols);
            let height = img_scaled_height * (vp_height / p_grid_rows);
            let x = img_scaled_width * (vp_col / p_grid_cols);
            let y = img_scaled_height * (vp_row / p_grid_rows);
            (x, y, width, height)
        };

        // The destination rectangle: x/y are offsets from the top-left, per
        // `RenderPlacement`.
        let mut dest_x_offset = 0.0f64;
        let mut dest_y_offset = 0.0f64;
        let mut dest_width = (self.width * cell_width) as f64;
        let mut dest_height = (self.height * cell_height) as f64;

        if src_y < img_scaled_y_offset {
            // Source rect y is within the offset area: the source texture
            // doesn't actually have the offset area blank, so adjust.
            let offset = img_scaled_y_offset - src_y;
            src_height -= offset;
            dest_y_offset = offset;
            dest_height -= offset * p_scale.y_scale;
            src_y = 0.0;

            // If height now exceeds the original, both top and bottom
            // offsets are in play; bring it back down.
            if src_height > img_height_f64 {
                src_height = img_height_f64;
                dest_height = img_height_f64 * p_scale.y_scale;
            }
        } else if src_y + src_height > img_scaled_height - img_scaled_y_offset {
            // Source y is in the bottom offset area: shorten to fit the cell.
            src_y -= img_scaled_y_offset;
            src_height = img_scaled_height - img_scaled_y_offset - src_y;
            src_height -= img_scaled_y_offset;
            dest_height = src_height * p_scale.y_scale;
        } else {
            src_y -= img_scaled_y_offset;
        }

        if src_x < img_scaled_x_offset {
            let offset = img_scaled_x_offset - src_x;
            src_width -= offset;
            dest_x_offset = offset;
            dest_width -= offset * p_scale.x_scale;
            src_x = 0.0;

            if src_width > img_width_f64 {
                src_width = img_width_f64;
                dest_width = img_width_f64 * p_scale.x_scale;
            }
        } else if src_x + src_width > img_scaled_width - img_scaled_x_offset {
            src_x -= img_scaled_x_offset;
            src_width = img_scaled_width - img_scaled_x_offset - src_x;
            src_width -= img_scaled_x_offset;
            dest_width = src_width * p_scale.x_scale;
        } else {
            src_x -= img_scaled_x_offset;
        }

        // If the modified source width/height is <= 0, we're rendering
        // entirely outside the visible image: render nothing.
        if src_width <= 0.0 || src_height <= 0.0 {
            return Ok(RenderPlacement {
                top_left: self.pin,
                offset_x: 0,
                offset_y: 0,
                source_x: 0,
                source_y: 0,
                source_width: 0,
                source_height: 0,
                dest_width: 0,
                dest_height: 0,
            });
        }

        Ok(RenderPlacement {
            top_left: self.pin,
            offset_x: (dest_x_offset * p_scale.x_scale).round() as u32,
            offset_y: (dest_y_offset * p_scale.y_scale).round() as u32,
            source_x: src_x.round() as u32,
            source_y: src_y.round() as u32,
            source_width: src_width.round() as u32,
            source_height: src_height.round() as u32,
            dest_width: dest_width.round() as u32,
            dest_height: dest_height.round() as u32,
        })
    }

    /// The grid size (rows/columns) for this placement: the requested rows/cols on the stored
    /// placement if specified, else a size that fits the whole image at its original size.
    /// Does not preserve aspect ratio — that's `render_placement`'s job. Port of
    /// `graphics_unicode.Placement.grid`.
    fn grid(
        &self,
        storage: &ImageStorage,
        image: &Image,
        cell_width: u32,
        cell_height: u32,
    ) -> Result<(u32, u32), RenderPlacementError> {
        // If a placement ID is specified, look for that exact one. Otherwise
        // find the first virtual placement for this image.
        let placement = if self.placement_id > 0 {
            let key = super::PlacementKey {
                image_id: self.image_id,
                placement_id: super::PlacementId {
                    tag: super::PlacementTag::External,
                    id: self.placement_id,
                },
            };
            *storage
                .placements
                .get(&key)
                .ok_or(RenderPlacementError::PlacementMissingPlacement)?
        } else {
            storage
                .placements
                .iter()
                .find(|(k, p)| {
                    k.image_id == self.image_id && matches!(p.location, Location::Virtual)
                })
                .map(|(_, p)| *p)
                .ok_or(RenderPlacementError::PlacementMissingPlacement)?
        };

        let mut rows = placement.rows;
        let mut columns = placement.columns;
        if rows == 0 {
            rows = image.height.div_ceil(cell_height);
        }
        if columns == 0 {
            columns = image.width.div_ceil(cell_width);
        }

        let rows = crate::page::size::CellCountInt::try_from(rows)
            .map_err(|_| RenderPlacementError::PlacementGridOutOfBounds)?;
        let columns = crate::page::size::CellCountInt::try_from(columns)
            .map_err(|_| RenderPlacementError::PlacementGridOutOfBounds)?;
        Ok((columns as u32, rows as u32))
    }
}

/// A renderer-facing placement: a flat struct positioning a Kitty graphics image on the
/// screen, broken down into the fields a renderer needs (no `Terminal`/`Screen` dependency).
/// Port of `graphics_render.Placement`.
#[derive(Debug, Clone, Copy, Default)]
pub struct RenderPlacement {
    /// The top-left corner of the image, in grid coordinates.
    pub top_left: Pin,

    /// Offset in pixels from the top-left corner of the grid cell.
    pub offset_x: u32,
    pub offset_y: u32,

    /// The source rectangle of the image to render (need not match the destination size; the
    /// renderer scales to fit).
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,

    /// The final width/height of the image in pixels.
    pub dest_width: u32,
    pub dest_height: u32,
}

/// The placement information present in a single cell. "Incomplete" because the spec allows
/// missing diacritics that continue a previous valid placement. Port of
/// `graphics_unicode.IncompletePlacement`.
#[derive(Debug, Clone, Copy)]
struct IncompletePlacement {
    /// The pin of the cell that created this incomplete placement.
    pin: Pin,

    /// Lower 24 bits of the image ID (from the fg color). Always present.
    image_id_low: u32,

    /// Higher 8 bits of the image ID (from the 3rd diacritic). Optional.
    image_id_high: Option<u8>,

    /// Placement ID, optionally specified via the underline color.
    placement_id: Option<u32>,

    /// Row/col index for the image fragment (0-indexed), from diacritics 1/2. Row is first,
    /// col second. Either may be absent, continuing a previous placement.
    row: Option<u32>,
    col: Option<u32>,

    /// The run width so far, in cells.
    width: u32,
}

impl IncompletePlacement {
    /// Parse the incomplete placement information from a row and cell.
    ///
    /// # Safety
    /// `row`/`cell` must be a valid, live pin/cell pair with `cell.codepoint() == PLACEHOLDER`.
    unsafe fn init(row: Pin, cell: &PageCell) -> IncompletePlacement {
        debug_assert_eq!(cell.codepoint(), PLACEHOLDER);

        // SAFETY: per caller.
        let style_id = cell.style_id();
        let page = unsafe { row.page() };
        let style: crate::page::style::Style = if style_id == crate::page::style::DEFAULT_ID {
            crate::page::style::Style::default()
        } else {
            // SAFETY: style_id is the cell's own live style id.
            unsafe { *(*page).style_by_id(style_id) }
        };

        let mut result = IncompletePlacement {
            pin: row,
            image_id_low: color_to_id(style.fg_color),
            image_id_high: None,
            placement_id: {
                let id = color_to_id(style.underline_color);
                if id != 0 { Some(id) } else { None }
            },
            row: None,
            col: None,
            width: 1,
        };

        // Decode all diacritics we can. Invalid diacritics are treated as if
        // absent (matches observed Kitty behavior, not formally specified).
        // SAFETY: cell is a live cell of `page`.
        let cps: &[u32] = unsafe { (*page).lookup_grapheme(cell as *const PageCell) }
            .map(|s| unsafe { &*s })
            .unwrap_or(&[]);
        if !cps.is_empty() {
            result.row = get_index(cps[0]);

            if cps.len() > 1 {
                result.col = get_index(cps[1]);

                // Any additional diacritics are ignored.
                if cps.len() > 2
                    && let Some(high) = get_index(cps[2])
                {
                    result.image_id_high = u8::try_from(high).ok();
                }
            }
        }

        result
    }

    /// Append this incomplete placement to an existing run. Returns `true` if compatible (and
    /// mutates `self` to extend the run); `false` leaves `self` unchanged. Port of
    /// `IncompletePlacement.append`.
    fn append(&mut self, other: &IncompletePlacement) -> bool {
        if !self.can_append(other) {
            return false;
        }
        self.width += 1;
        true
    }

    /// Port of `IncompletePlacement.canAppend` ("converted from Kitty's logic, don't @ me").
    fn can_append(&self, other: &IncompletePlacement) -> bool {
        self.image_id_low == other.image_id_low
            && self.placement_id == other.placement_id
            && (other.row.is_none() || other.row == self.row)
            && (other.col.is_none() || other.col == Some(self.col.unwrap_or(0) + self.width))
            && (other.image_id_high.is_none() || other.image_id_high == self.image_id_high)
    }

    /// Complete this incomplete placement into a full [`Placement`], not continuous with any
    /// previous run. Port of `IncompletePlacement.complete`.
    fn complete(&self) -> Placement {
        Placement {
            pin: self.pin,
            image_id: self.image_id_low | ((self.image_id_high.unwrap_or(0) as u32) << 24),
            placement_id: self.placement_id.unwrap_or(0),
            col: self.col.unwrap_or(0),
            row: self.row.unwrap_or(0),
            width: self.width,
            height: 1,
        }
    }
}

/// Convert a style color to a Kitty image protocol ID: the 24 most significant bits of the
/// color, which works uniformly for palette and RGB colors. Port of
/// `IncompletePlacement.colorToId`.
fn color_to_id(c: StyleColor) -> u32 {
    match c {
        StyleColor::None => 0,
        StyleColor::Palette(v) => v as u32,
        StyleColor::Rgb(rgb) => ((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | (rgb.b as u32),
    }
}

/// Get the row/col index for a diacritic codepoint (0-indexed), or `None` if `cp` isn't one of
/// the recognized row/column diacritics. Port of `getIndex`.
fn get_index(cp: u32) -> Option<u32> {
    let cp: u32 = cp;
    DIACRITICS.binary_search(&cp).ok().map(|i| i as u32)
}

/// The diacritics used with the Kitty graphics protocol Unicode placement feature to specify
/// row/column for placement. The array index determines the value. Port of
/// `graphics_unicode.diacritics`.
///
/// Derived from:
/// <https://sw.kovidgoyal.net/kitty/_downloads/f0a0de9ec8d9ff4456206db8e0814937/rowcolumn-diacritics.txt>
#[rustfmt::skip]
const DIACRITICS: &[u32] = &[
    0x0305, 0x030D, 0x030E, 0x0310, 0x0312, 0x033D, 0x033E, 0x033F,
    0x0346, 0x034A, 0x034B, 0x034C, 0x0350, 0x0351, 0x0352, 0x0357,
    0x035B, 0x0363, 0x0364, 0x0365, 0x0366, 0x0367, 0x0368, 0x0369,
    0x036A, 0x036B, 0x036C, 0x036D, 0x036E, 0x036F, 0x0483, 0x0484,
    0x0485, 0x0486, 0x0487, 0x0592, 0x0593, 0x0594, 0x0595, 0x0597,
    0x0598, 0x0599, 0x059C, 0x059D, 0x059E, 0x059F, 0x05A0, 0x05A1,
    0x05A8, 0x05A9, 0x05AB, 0x05AC, 0x05AF, 0x05C4, 0x0610, 0x0611,
    0x0612, 0x0613, 0x0614, 0x0615, 0x0616, 0x0617, 0x0657, 0x0658,
    0x0659, 0x065A, 0x065B, 0x065D, 0x065E, 0x06D6, 0x06D7, 0x06D8,
    0x06D9, 0x06DA, 0x06DB, 0x06DC, 0x06DF, 0x06E0, 0x06E1, 0x06E2,
    0x06E4, 0x06E7, 0x06E8, 0x06EB, 0x06EC, 0x0730, 0x0732, 0x0733,
    0x0735, 0x0736, 0x073A, 0x073D, 0x073F, 0x0740, 0x0741, 0x0743,
    0x0745, 0x0747, 0x0749, 0x074A, 0x07EB, 0x07EC, 0x07ED, 0x07EE,
    0x07EF, 0x07F0, 0x07F1, 0x07F3, 0x0816, 0x0817, 0x0818, 0x0819,
    0x081B, 0x081C, 0x081D, 0x081E, 0x081F, 0x0820, 0x0821, 0x0822,
    0x0823, 0x0825, 0x0826, 0x0827, 0x0829, 0x082A, 0x082B, 0x082C,
    0x082D, 0x0951, 0x0953, 0x0954, 0x0F82, 0x0F83, 0x0F86, 0x0F87,
    0x135D, 0x135E, 0x135F, 0x17DD, 0x193A, 0x1A17, 0x1A75, 0x1A76,
    0x1A77, 0x1A78, 0x1A79, 0x1A7A, 0x1A7B, 0x1A7C, 0x1B6B, 0x1B6D,
    0x1B6E, 0x1B6F, 0x1B70, 0x1B71, 0x1B72, 0x1B73, 0x1CD0, 0x1CD1,
    0x1CD2, 0x1CDA, 0x1CDB, 0x1CE0, 0x1DC0, 0x1DC1, 0x1DC3, 0x1DC4,
    0x1DC5, 0x1DC6, 0x1DC7, 0x1DC8, 0x1DC9, 0x1DCB, 0x1DCC, 0x1DD1,
    0x1DD2, 0x1DD3, 0x1DD4, 0x1DD5, 0x1DD6, 0x1DD7, 0x1DD8, 0x1DD9,
    0x1DDA, 0x1DDB, 0x1DDC, 0x1DDD, 0x1DDE, 0x1DDF, 0x1DE0, 0x1DE1,
    0x1DE2, 0x1DE3, 0x1DE4, 0x1DE5, 0x1DE6, 0x1DFE, 0x20D0, 0x20D1,
    0x20D4, 0x20D5, 0x20D6, 0x20D7, 0x20DB, 0x20DC, 0x20E1, 0x20E7,
    0x20E9, 0x20F0, 0x2CEF, 0x2CF0, 0x2CF1, 0x2DE0, 0x2DE1, 0x2DE2,
    0x2DE3, 0x2DE4, 0x2DE5, 0x2DE6, 0x2DE7, 0x2DE8, 0x2DE9, 0x2DEA,
    0x2DEB, 0x2DEC, 0x2DED, 0x2DEE, 0x2DEF, 0x2DF0, 0x2DF1, 0x2DF2,
    0x2DF3, 0x2DF4, 0x2DF5, 0x2DF6, 0x2DF7, 0x2DF8, 0x2DF9, 0x2DFA,
    0x2DFB, 0x2DFC, 0x2DFD, 0x2DFE, 0x2DFF, 0xA66F, 0xA67C, 0xA67D,
    0xA6F0, 0xA6F1, 0xA8E0, 0xA8E1, 0xA8E2, 0xA8E3, 0xA8E4, 0xA8E5,
    0xA8E6, 0xA8E7, 0xA8E8, 0xA8E9, 0xA8EA, 0xA8EB, 0xA8EC, 0xA8ED,
    0xA8EE, 0xA8EF, 0xA8F0, 0xA8F1, 0xAAB0, 0xAAB2, 0xAAB3, 0xAAB7,
    0xAAB8, 0xAABE, 0xAABF, 0xAAC1, 0xFE20, 0xFE21, 0xFE22, 0xFE23,
    0xFE24, 0xFE25, 0xFE26, 0x10A0F, 0x10A38, 0x1D185, 0x1D186, 0x1D187,
    0x1D188, 0x1D189, 0x1D1AA, 0x1D1AB, 0x1D1AC, 0x1D1AD, 0x1D242, 0x1D243,
    0x1D244,
];

#[cfg(test)]
mod tests {
    use super::*;

    // Zig: "unicode diacritic sorted".
    #[test]
    fn diacritic_table_is_sorted() {
        assert!(DIACRITICS.windows(2).all(|w| w[0] < w[1]));
    }

    // Zig: "unicode diacritic".
    #[test]
    fn diacritic_spot_checks() {
        assert_eq!(get_index(0x483), Some(30));
        assert_eq!(get_index(0x1d242), Some(294));
    }

    use crate::modes::Mode;
    use crate::point::Tag;
    use crate::terminal::{Options, Terminal};

    fn term(
        cols: crate::page::size::CellCountInt,
        rows: crate::page::size::CellCountInt,
    ) -> Terminal {
        Terminal::new(Options {
            cols,
            rows,
            max_scrollback: 0,
            colors: crate::terminal::Colors::default(),
        })
    }

    /// Collect every placement starting at the viewport's top-left. Test helper mirroring the
    /// `var it = placementIterator(pin, null); it.next()` pattern used throughout upstream.
    fn placements(t: &Terminal) -> Vec<Placement> {
        let pin = t.screen().pages.get_top_left(Tag::Viewport);
        let mut it = unsafe { placement_iterator(pin, None) };
        let mut out = Vec::new();
        while let Some(p) = unsafe { it.next() } {
            out.push(p);
        }
        out
    }

    // Zig: "unicode placement: none".
    #[test]
    fn placement_none() {
        let mut t = term(5, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.print_string("hello\nworld\n1\n2");
        assert!(placements(&t).is_empty());
    }

    // Zig: "unicode placement: single row/col".
    #[test]
    fn placement_single_row_col() {
        let mut t = term(5, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.print_string("\u{10EEEE}\u{0305}\u{0305}");

        let ps = placements(&t);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].image_id, 0);
        assert_eq!(ps[0].placement_id, 0);
        assert_eq!(ps[0].row, 0);
        assert_eq!(ps[0].col, 0);
    }

    // Zig: "unicode placement: continuation break".
    #[test]
    fn placement_continuation_break() {
        let mut t = term(10, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        // Two runs because the column jumps.
        t.print_string("\u{10EEEE}\u{0305}\u{0305}");
        t.print_string("\u{10EEEE}\u{0305}\u{030E}");

        let ps = placements(&t);
        assert_eq!(ps.len(), 2);
        assert_eq!(
            (
                ps[0].image_id,
                ps[0].placement_id,
                ps[0].row,
                ps[0].col,
                ps[0].width
            ),
            (0, 0, 0, 0, 1)
        );
        assert_eq!(
            (
                ps[1].image_id,
                ps[1].placement_id,
                ps[1].row,
                ps[1].col,
                ps[1].width
            ),
            (0, 0, 0, 2, 1)
        );
    }

    // Zig: "unicode placement: continuation with diacritics set".
    #[test]
    fn placement_continuation_with_diacritics_set() {
        let mut t = term(10, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.print_string("\u{10EEEE}\u{0305}\u{0305}");
        t.print_string("\u{10EEEE}\u{0305}\u{030D}");
        t.print_string("\u{10EEEE}\u{0305}\u{030E}");

        let ps = placements(&t);
        assert_eq!(ps.len(), 1);
        assert_eq!(
            (
                ps[0].image_id,
                ps[0].placement_id,
                ps[0].row,
                ps[0].col,
                ps[0].width
            ),
            (0, 0, 0, 0, 3)
        );
    }

    // Zig: "unicode placement: continuation with no col".
    #[test]
    fn placement_continuation_with_no_col() {
        let mut t = term(10, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.print_string("\u{10EEEE}\u{0305}\u{0305}");
        t.print_string("\u{10EEEE}\u{0305}");
        t.print_string("\u{10EEEE}\u{0305}");

        let ps = placements(&t);
        assert_eq!(ps.len(), 1);
        assert_eq!(
            (
                ps[0].image_id,
                ps[0].placement_id,
                ps[0].row,
                ps[0].col,
                ps[0].width
            ),
            (0, 0, 0, 0, 3)
        );
    }

    // Zig: "unicode placement: continuation with no diacritics".
    #[test]
    fn placement_continuation_with_no_diacritics() {
        let mut t = term(10, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.print_string("\u{10EEEE}");
        t.print_string("\u{10EEEE}");
        t.print_string("\u{10EEEE}");

        let ps = placements(&t);
        assert_eq!(ps.len(), 1);
        assert_eq!(
            (
                ps[0].image_id,
                ps[0].placement_id,
                ps[0].row,
                ps[0].col,
                ps[0].width
            ),
            (0, 0, 0, 0, 3)
        );
    }

    // Zig: "unicode placement: run ending".
    #[test]
    fn placement_run_ending() {
        let mut t = term(10, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.print_string("\u{10EEEE}\u{0305}\u{0305}");
        t.print_string("\u{10EEEE}\u{0305}\u{030D}");
        t.print_string("ABC");

        let ps = placements(&t);
        assert_eq!(ps.len(), 1);
        assert_eq!(
            (
                ps[0].image_id,
                ps[0].placement_id,
                ps[0].row,
                ps[0].col,
                ps[0].width
            ),
            (0, 0, 0, 0, 2)
        );
    }

    // Zig: "unicode placement: run starting in the middle".
    #[test]
    fn placement_run_starting_in_the_middle() {
        let mut t = term(10, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.print_string("ABC");
        t.print_string("\u{10EEEE}\u{0305}\u{0305}");
        t.print_string("\u{10EEEE}\u{0305}\u{030D}");

        let ps = placements(&t);
        assert_eq!(ps.len(), 1);
        assert_eq!(
            (
                ps[0].image_id,
                ps[0].placement_id,
                ps[0].row,
                ps[0].col,
                ps[0].width
            ),
            (0, 0, 0, 0, 2)
        );
    }

    // Zig: "unicode placement: specifying image id as palette".
    #[test]
    fn placement_specifying_image_id_as_palette() {
        let mut t = term(5, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.set_attribute(crate::sgr::Attribute::Fg256(42));
        t.print_string("\u{10EEEE}\u{0305}\u{0305}");

        let ps = placements(&t);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].image_id, 42);
        assert_eq!(ps[0].placement_id, 0);
        assert_eq!(ps[0].row, 0);
        assert_eq!(ps[0].col, 0);
    }

    // Zig: "unicode placement: specifying image id with high bits".
    #[test]
    fn placement_specifying_image_id_with_high_bits() {
        let mut t = term(5, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.set_attribute(crate::sgr::Attribute::Fg256(42));
        t.print_string("\u{10EEEE}\u{0305}\u{0305}\u{030E}");

        let ps = placements(&t);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].image_id, 33554474);
        assert_eq!(ps[0].placement_id, 0);
        assert_eq!(ps[0].row, 0);
        assert_eq!(ps[0].col, 0);
    }

    // Zig: "unicode placement: specifying placement id as palette".
    #[test]
    fn placement_specifying_placement_id_as_palette() {
        let mut t = term(5, 5);
        t.modes.set(Mode::GraphemeCluster, true);
        t.set_attribute(crate::sgr::Attribute::Fg256(42));
        t.set_attribute(crate::sgr::Attribute::UnderlineColor256(21));
        t.print_string("\u{10EEEE}\u{0305}\u{0305}");

        let ps = placements(&t);
        assert_eq!(ps.len(), 1);
        assert_eq!(ps[0].image_id, 42);
        assert_eq!(ps[0].placement_id, 21);
        assert_eq!(ps[0].row, 0);
        assert_eq!(ps[0].col, 0);
    }

    // ---- render_placement ---------------------------------------------

    // Zig: "unicode render placement: dog 4x2".
    #[test]
    fn render_placement_dog_4x2() {
        let cell_width = 36;
        let cell_height = 80;
        let mut t = term(100, 100);
        let s = {
            let mut s = ImageStorage::new();
            let image = Image {
                id: 1,
                width: 500,
                height: 306,
                ..Image::default()
            };
            s.add_image(image).unwrap();
            s.add_placement(
                1,
                0,
                crate::kitty::Placement {
                    columns: 4,
                    rows: 2,
                    ..crate::kitty::Placement::new(Location::Virtual)
                },
            );
            s
        };
        let image = Image {
            id: 1,
            width: 500,
            height: 306,
            ..Image::default()
        };
        let pin = unsafe { *t.screen_mut().cursor.page_pin };

        // Row 1
        {
            let p = Placement {
                pin,
                image_id: 1,
                placement_id: 0,
                col: 0,
                row: 0,
                width: 4,
                height: 1,
            };
            let rp = p
                .render_placement(&s, &image, cell_width, cell_height)
                .unwrap();
            assert_eq!(rp.offset_x, 0);
            assert_eq!(rp.offset_y, 36);
            assert_eq!(rp.source_x, 0);
            assert_eq!(rp.source_y, 0);
            assert_eq!(rp.source_width, 500);
            assert_eq!(rp.source_height, 153);
            assert_eq!(rp.dest_width, 144);
            assert_eq!(rp.dest_height, 44);
        }
        // Row 2
        {
            let p = Placement {
                pin,
                image_id: 1,
                placement_id: 0,
                col: 0,
                row: 1,
                width: 4,
                height: 1,
            };
            let rp = p
                .render_placement(&s, &image, cell_width, cell_height)
                .unwrap();
            assert_eq!(rp.offset_x, 0);
            assert_eq!(rp.offset_y, 0);
            assert_eq!(rp.source_x, 0);
            assert_eq!(rp.source_y, 153);
            assert_eq!(rp.source_width, 500);
            assert_eq!(rp.source_height, 153);
            assert_eq!(rp.dest_width, 144);
            assert_eq!(rp.dest_height, 44);
        }
    }

    // Zig: "unicode render placement: dog 2x2 with blank cells".
    #[test]
    fn render_placement_dog_2x2_with_blank_cells() {
        let cell_width = 36;
        let cell_height = 80;
        let mut t = term(100, 100);
        let s = {
            let mut s = ImageStorage::new();
            let image = Image {
                id: 1,
                width: 500,
                height: 306,
                ..Image::default()
            };
            s.add_image(image).unwrap();
            s.add_placement(
                1,
                0,
                crate::kitty::Placement {
                    columns: 2,
                    rows: 2,
                    ..crate::kitty::Placement::new(Location::Virtual)
                },
            );
            s
        };
        let image = Image {
            id: 1,
            width: 500,
            height: 306,
            ..Image::default()
        };
        let pin = unsafe { *t.screen_mut().cursor.page_pin };

        // Row 1
        {
            let p = Placement {
                pin,
                image_id: 1,
                placement_id: 0,
                col: 0,
                row: 0,
                width: 4,
                height: 1,
            };
            let rp = p
                .render_placement(&s, &image, cell_width, cell_height)
                .unwrap();
            assert_eq!(rp.offset_x, 0);
            assert_eq!(rp.offset_y, 58);
            assert_eq!(rp.source_x, 0);
            assert_eq!(rp.source_y, 0);
            assert_eq!(rp.source_width, 500);
            assert_eq!(rp.source_height, 153);
            assert_eq!(rp.dest_width, 72);
            assert_eq!(rp.dest_height, 22);
        }
        // Row 2
        {
            let p = Placement {
                pin,
                image_id: 1,
                placement_id: 0,
                col: 0,
                row: 1,
                width: 4,
                height: 1,
            };
            let rp = p
                .render_placement(&s, &image, cell_width, cell_height)
                .unwrap();
            assert_eq!(rp.offset_x, 0);
            assert_eq!(rp.offset_y, 0);
            assert_eq!(rp.source_x, 0);
            assert_eq!(rp.source_y, 153);
            assert_eq!(rp.source_width, 500);
            assert_eq!(rp.source_height, 153);
            assert_eq!(rp.dest_width, 72);
            assert_eq!(rp.dest_height, 22);
        }
    }

    // Zig: "unicode render placement: dog 1x1".
    #[test]
    fn render_placement_dog_1x1() {
        let cell_width = 36;
        let cell_height = 80;
        let mut t = term(100, 100);
        let s = {
            let mut s = ImageStorage::new();
            let image = Image {
                id: 1,
                width: 500,
                height: 306,
                ..Image::default()
            };
            s.add_image(image).unwrap();
            s.add_placement(
                1,
                0,
                crate::kitty::Placement {
                    columns: 1,
                    rows: 1,
                    ..crate::kitty::Placement::new(Location::Virtual)
                },
            );
            s
        };
        let image = Image {
            id: 1,
            width: 500,
            height: 306,
            ..Image::default()
        };
        let pin = unsafe { *t.screen_mut().cursor.page_pin };

        let p = Placement {
            pin,
            image_id: 1,
            placement_id: 0,
            col: 0,
            row: 0,
            width: 4,
            height: 1,
        };
        let rp = p
            .render_placement(&s, &image, cell_width, cell_height)
            .unwrap();
        assert_eq!(rp.offset_x, 0);
        assert_eq!(rp.offset_y, 29);
        assert_eq!(rp.source_x, 0);
        assert_eq!(rp.source_y, 0);
        assert_eq!(rp.source_width, 500);
        assert_eq!(rp.source_height, 306);
        assert_eq!(rp.dest_width, 36);
        assert_eq!(rp.dest_height, 22);
    }
}
