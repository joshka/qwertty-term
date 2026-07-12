//! Renderer-facing placement resolution: turn the stored, pin-anchored kitty
//! [`ImageStorage`] placements into flat, `Pin`-free, viewport-relative draw
//! data a GPU renderer can consume directly.
//!
//! Port of the *placement-build* half of Ghostty's `src/renderer/image.zig`
//! (`prepKittyPlacement` + `prepKittyVirtualPlacement`, commit `2da015cd6`).
//! Upstream does this build inside the renderer, which there has direct access
//! to the terminal under the draw mutex. This port splits the renderer onto its
//! own thread behind a captured snapshot, so the resolution — which must
//! dereference tracked `*mut Pin`s and walk the [`PageList`] — lives here in
//! `qwertty-term-vt`, where the page chain is owned and the deref is sound. The
//! result ([`RenderImagePlacement`]) carries no `Pin`, so it can safely cross
//! the crate/thread boundary into the renderer.
//!
//! Scope note (R6 slice 1): this resolves both pin-anchored and virtual (`U=1`)
//! placements to viewport coordinates, skipping any placement whose top-left is
//! outside the current viewport (basic culling via
//! [`PageList::point_from_pin`]). Pixel-accurate top/bottom clipping of
//! partially-scrolled images and the three z-order buckets are R6 slices 2/4.

use std::borrow::Cow;

use crate::kitty::TerminalGeometry;
use crate::kitty::command::Format;
use crate::kitty::image::Image;
use crate::kitty::storage::{ImageStorage, Location};
use crate::kitty::unicode;
use crate::pagelist::PageList;
use crate::point::Tag;

/// One resolved kitty placement, ready to draw: viewport-relative grid
/// position, per-cell pixel offset, source rectangle within the image, and
/// destination pixel size. Flat and `Pin`-free (see the module docs).
///
/// Field meanings mirror upstream `renderer.image.Placement` and the GPU-side
/// `Image` vertex struct: the renderer places the image starting at grid cell
/// (`grid_col`, `grid_row`), offset by (`cell_offset_x`, `cell_offset_y`)
/// pixels, sampling the source rectangle and scaling it to
/// (`dest_width`, `dest_height`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderImagePlacement {
    /// The image this placement draws (key into [`ImageStorage::image_by_id`]).
    pub image_id: u32,
    /// Z-index: `< 0` draws below text, `>= 0` above (bucketing is R6 slice 4).
    pub z: i32,
    /// Viewport-relative top-left grid cell (0-indexed from the visible top).
    pub grid_col: u32,
    pub grid_row: u32,
    /// Pixel offset from the top-left of the grid cell.
    pub cell_offset_x: u32,
    pub cell_offset_y: u32,
    /// Source rectangle within the image, in pixels (already clamped to the
    /// image bounds; `0`-width/height source fields resolve to "the rest of
    /// the image").
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    /// Final on-screen size of the image in pixels.
    pub dest_width: u32,
    pub dest_height: u32,
}

/// Resolve every currently-visible placement in `storage` to a
/// [`RenderImagePlacement`], using `pages` for pin→viewport mapping and `geo`
/// for cell geometry. Returns placements in arbitrary order (the renderer sorts
/// by z when the z-order buckets land in R6 slice 4).
///
/// A placement is skipped when its image is missing or its top-left cell is
/// scrolled out of the viewport. Virtual (`U=1`) placements are resolved by
/// walking the viewport's placeholder cells (port of `prepKittyVirtualPlacement`).
#[must_use]
pub fn resolve_placements(
    storage: &ImageStorage,
    pages: &PageList,
    geo: &TerminalGeometry,
) -> Vec<RenderImagePlacement> {
    let mut out = Vec::new();
    let mut has_virtual = false;

    for (key, placement) in &storage.placements {
        let pin = match placement.location {
            // SAFETY: the pin is tracked by `pages` (untracked only on the
            // placement's `deinit`), so its node and page chain are live for
            // the duration of this borrow.
            Location::Pin(p) => unsafe { *p },
            Location::Virtual => {
                has_virtual = true;
                continue;
            }
        };

        let image_id = key.image_id;
        let Some(image) = storage.image_by_id(image_id) else {
            continue;
        };

        // Basic viewport cull: skip placements whose top-left is not in the
        // visible window (pixel-accurate partial clipping is R6 slice 2).
        let Some(point) = pages.point_from_pin(Tag::Viewport, pin) else {
            continue;
        };

        let (dest_width, dest_height) = placement.pixel_size(image, geo);
        if dest_width == 0 || dest_height == 0 {
            continue;
        }

        let (source_x, source_width) =
            clamp_source(placement.source_x, placement.source_width, image.width);
        let (source_y, source_height) =
            clamp_source(placement.source_y, placement.source_height, image.height);

        out.push(RenderImagePlacement {
            image_id,
            z: placement.z,
            grid_col: u32::from(point.coord.x),
            grid_row: point.coord.y,
            cell_offset_x: placement.x_offset,
            cell_offset_y: placement.y_offset,
            source_x,
            source_y,
            source_width,
            source_height,
            dest_width,
            dest_height,
        });
    }

    if has_virtual {
        resolve_virtual(storage, pages, geo, &mut out);
    }

    out
}

/// The image's pixels as tightly-packed RGBA (`width * height * 4` bytes),
/// converting from the stored pixel format if necessary. Port of the swizzle
/// step in upstream `renderer.image.Image.convert` (`image.zig:853-872`): the
/// GPU image texture is always RGBA, so gray/gray-alpha/rgb sources are
/// expanded. An already-`Rgba` image borrows its data with no copy.
///
/// A post-`complete` [`Image`] is never `Png` (decode replaces it with `Rgba`);
/// the `Png` arm is defensive and treated as already-RGBA.
#[must_use]
pub fn image_rgba(image: &Image) -> Cow<'_, [u8]> {
    match image.format {
        Format::Rgba | Format::Png => Cow::Borrowed(&image.data),
        Format::Rgb => Cow::Owned(expand(&image.data, 3, |px, out| {
            out.extend_from_slice(px);
            out.push(255);
        })),
        Format::Gray => Cow::Owned(expand(&image.data, 1, |px, out| {
            out.extend_from_slice(&[px[0], px[0], px[0], 255]);
        })),
        Format::GrayAlpha => Cow::Owned(expand(&image.data, 2, |px, out| {
            out.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
        })),
    }
}

/// Expand `data` from `src_bpp`-byte pixels to RGBA via `write`, ignoring any
/// trailing partial pixel (a malformed transfer would have been rejected on
/// `complete`, but truncating defends against a bad slice length here).
fn expand(data: &[u8], src_bpp: usize, write: impl Fn(&[u8], &mut Vec<u8>)) -> Vec<u8> {
    let pixels = data.len() / src_bpp;
    let mut out = Vec::with_capacity(pixels * 4);
    for i in 0..pixels {
        write(&data[i * src_bpp..i * src_bpp + src_bpp], &mut out);
    }
    out
}

/// Clamp a source-rect origin+extent against the image extent along one axis,
/// resolving a `0` extent to "the rest of the image". Port of the source-rect
/// clamping in upstream `prepKittyPlacement` (`image.zig:432-441`).
fn clamp_source(origin: u32, extent: u32, image_extent: u32) -> (u32, u32) {
    let origin = origin.min(image_extent);
    let remaining = image_extent - origin;
    let extent = if extent == 0 {
        remaining
    } else {
        extent.min(remaining)
    };
    (origin, extent)
}

/// Resolve virtual (`U=1`) placements by walking the viewport's placeholder
/// cells. Port of `prepKittyVirtualPlacement` (`image.zig:465-521`): each run of
/// placeholder cells resolves against its stored placement to a
/// [`unicode::RenderPlacement`], which we then map to viewport grid coordinates.
fn resolve_virtual(
    storage: &ImageStorage,
    pages: &PageList,
    geo: &TerminalGeometry,
    out: &mut Vec<RenderImagePlacement>,
) {
    let cell_width = geo.width_px / u32::from(geo.cols);
    let cell_height = geo.height_px / u32::from(geo.rows);
    if cell_width == 0 || cell_height == 0 {
        return;
    }

    let top = pages.get_top_left(Tag::Viewport);
    // SAFETY: `top` is a live viewport pin; `down_overflow_clamped` walks at
    // most `rows - 1` rows down the owned page chain, clamping at the end.
    let bottom = unsafe { top.down_overflow_clamped(usize::from(geo.rows).saturating_sub(1)) };

    // SAFETY: `top`/`bottom` are live pins into `pages`; the iterator only reads
    // cells within the owned page chain for the lifetime of this call.
    let mut iter = unsafe { unicode::placement_iterator(top, Some(bottom)) };
    // SAFETY (each `next`): the pins the iterator walks stay live for this call.
    while let Some(placement) = unsafe { iter.next() } {
        let Some(image) = storage.image_by_id(placement.image_id) else {
            continue;
        };
        let Ok(rp) = placement.render_placement(storage, image, cell_width, cell_height) else {
            continue;
        };
        if rp.dest_width == 0 || rp.dest_height == 0 {
            continue;
        }
        let Some(point) = pages.point_from_pin(Tag::Viewport, rp.top_left) else {
            continue;
        };

        out.push(RenderImagePlacement {
            image_id: placement.image_id,
            // Upstream draws virtual placements below text (z = -1).
            z: -1,
            grid_col: u32::from(point.coord.x),
            grid_row: point.coord.y,
            cell_offset_x: rp.offset_x,
            cell_offset_y: rp.offset_y,
            source_x: rp.source_x,
            source_y: rp.source_y,
            source_width: rp.source_width,
            source_height: rp.source_height,
            dest_width: rp.dest_width,
            dest_height: rp.dest_height,
        });
    }
}

#[cfg(test)]
mod tests {
    use base64::Engine as _;

    use super::*;
    use crate::kitty::command;
    use crate::kitty::exec::execute;
    use crate::terminal::{Options, Terminal};

    fn geo_of(t: &Terminal) -> TerminalGeometry {
        TerminalGeometry {
            cols: t.cols,
            rows: t.rows,
            width_px: t.width_px,
            height_px: t.height_px,
        }
    }

    fn transmit_and_display(t: &mut Terminal, id: u32, w: u32, h: u32, extra: &str) {
        // f=32 (RGBA) direct transmit-and-display of a `w*h*4`-byte solid image.
        let rgba = [7u8, 8, 9, 255].repeat((w * h) as usize);
        let payload = base64::engine::general_purpose::STANDARD.encode(&rgba);
        let s = format!("a=T,f=32,t=d,i={id},s={w},v={h}{extra};{payload}");
        let cmd = command::Parser::parse_string(s.as_bytes()).expect("parse");
        execute(t, &cmd).expect("display response");
    }

    #[test]
    fn resolves_native_pin_placement_at_cursor() {
        let mut t = Terminal::new(Options {
            cols: 10,
            rows: 4,
            ..Default::default()
        });
        t.width_px = 80;
        t.height_px = 64;

        // Cursor starts at home (0,0); native-size (no c/r) 3×2 image.
        transmit_and_display(&mut t, 1, 3, 2, "");

        let screen = t.screen();
        let placements = resolve_placements(&screen.kitty_images, &screen.pages, &geo_of(&t));
        assert_eq!(placements.len(), 1);
        let p = placements[0];
        assert_eq!(p.image_id, 1);
        assert_eq!((p.grid_col, p.grid_row), (0, 0));
        // Native size (columns/rows unset): dest == image size.
        assert_eq!((p.dest_width, p.dest_height), (3, 2));
        // Full-image source (0 fields resolve to the whole image).
        assert_eq!(
            (p.source_x, p.source_y, p.source_width, p.source_height),
            (0, 0, 3, 2)
        );
    }

    #[test]
    fn resolves_scaled_placement_dest_size() {
        let mut t = Terminal::new(Options {
            cols: 10,
            rows: 6,
            ..Default::default()
        });
        // 8px cells (80/10, 48/6).
        t.width_px = 80;
        t.height_px = 48;

        // 2×2 image scaled to c=4,r=3 cells → dest = (4*8, 3*8) = (32, 24).
        transmit_and_display(&mut t, 1, 2, 2, ",c=4,r=3");

        let screen = t.screen();
        let placements = resolve_placements(&screen.kitty_images, &screen.pages, &geo_of(&t));
        assert_eq!(placements.len(), 1);
        assert_eq!(
            (placements[0].dest_width, placements[0].dest_height),
            (32, 24)
        );
    }

    #[test]
    fn no_placements_when_storage_empty() {
        let t = Terminal::new(Options {
            cols: 8,
            rows: 4,
            ..Default::default()
        });
        let screen = t.screen();
        let placements = resolve_placements(&screen.kitty_images, &screen.pages, &geo_of(&t));
        assert!(placements.is_empty());
    }

    #[test]
    fn image_rgba_expands_rgb_to_rgba() {
        // A 1×2 RGB image → 2 pixels, each gaining an opaque alpha byte.
        let mut t = Terminal::new(Options {
            cols: 4,
            rows: 4,
            ..Default::default()
        });
        let rgb = [10u8, 20, 30, 40, 50, 60]; // 2 px RGB
        let payload = base64::engine::general_purpose::STANDARD.encode(rgb);
        let s = format!("a=t,f=24,t=d,i=1,s=1,v=2;{payload}");
        let cmd = command::Parser::parse_string(s.as_bytes()).expect("parse");
        execute(&mut t, &cmd).expect("transmit response");

        let img = t.screen().kitty_images.image_by_id(1).expect("image");
        assert_eq!(
            &*image_rgba(img),
            &[10, 20, 30, 255, 40, 50, 60, 255],
            "RGB should expand to opaque RGBA"
        );
    }
}
