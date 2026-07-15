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
//! Scope note (R6 slices 1–2): resolves both pin-anchored and virtual (`U=1`)
//! placements to window-relative grid coordinates, positioned in absolute
//! [`Tag::Screen`] rows against the window `scrollback_offset` rows up from the
//! bottom (slice 2: images track scrollback, partially-scrolled images get a
//! negative `grid_row` and are clipped by the GPU rasterizer, fully-off ones are
//! culled). The three z-order buckets remain R6 slice 4.

use std::borrow::Cow;
use std::sync::Arc;

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
    /// Viewport-relative top-left grid cell. `grid_row` is **signed**: a
    /// placement whose top has scrolled above the visible window has a negative
    /// row (its top quad edge is off-screen and the GPU rasterizer clips it),
    /// while `grid_col` is scroll-invariant. Fully-off placements are culled by
    /// the resolver, so a resolved placement always overlaps the window.
    pub grid_col: u32,
    pub grid_row: i32,
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

/// The visible window in absolute [`Tag::Screen`] row coordinates: rows
/// `[top_y, top_y + rows - 1]`, `rows` rows ending `scrollback_offset` rows up
/// from the bottom — exactly the window [`Screen::snapshot_window`] renders. All
/// placement positions are expressed relative to `top_y`, so a placement scrolled
/// above the window gets a negative `grid_row` and one scrolled below is culled.
struct Window {
    /// Absolute screen row of the top visible row (`window_top`).
    top_y: u32,
    /// Absolute screen row of the bottom visible row.
    bot_y: u32,
}

impl Window {
    fn compute(pages: &PageList, geo: &TerminalGeometry, scrollback_offset: usize) -> Window {
        let rows = usize::from(geo.rows);
        let total_rows = pages.total_rows();
        let scrollback_len = total_rows.saturating_sub(rows);
        let offset = scrollback_offset.min(scrollback_len);
        // Mirrors `Screen::snapshot_window`'s `window_top`.
        let top_y = total_rows.saturating_sub(offset + rows) as u32;
        let bot_y = top_y + (rows.saturating_sub(1)) as u32;
        Window { top_y, bot_y }
    }

    /// Map an image occupying `grid_rows` cells starting at absolute screen row
    /// `img_top_y` to a window-relative signed row, or `None` if it's entirely
    /// outside the visible window.
    fn place_row(&self, img_top_y: u32, grid_rows: u32) -> Option<i32> {
        if grid_rows == 0 {
            return None;
        }
        let img_bot_y = img_top_y + (grid_rows - 1);
        // Cull entirely-below (top past window bottom) or entirely-above
        // (bottom before window top).
        if img_top_y > self.bot_y || img_bot_y < self.top_y {
            return None;
        }
        Some(i64::from(img_top_y) as i32 - i64::from(self.top_y) as i32)
    }
}

/// Resolve every visible placement in `storage` to a [`RenderImagePlacement`],
/// positioned relative to the window `scrollback_offset` rows up from the bottom
/// (`0` = the live active area — see [`Window`]). `pages` maps pins to absolute
/// screen rows, `geo` gives cell geometry. Returns placements in arbitrary order
/// (the renderer sorts by z when the buckets land in R6 slice 4).
///
/// Placements scrolled fully out of the window are culled; ones partially above
/// it get a negative `grid_row` and are clipped by the GPU rasterizer. Virtual
/// (`U=1`) placements walk the same window's placeholder cells (port of
/// `prepKittyVirtualPlacement`).
#[must_use]
pub fn resolve_placements(
    storage: &ImageStorage,
    pages: &PageList,
    geo: &TerminalGeometry,
    scrollback_offset: usize,
) -> Vec<RenderImagePlacement> {
    let win = Window::compute(pages, geo, scrollback_offset);
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

        // Absolute screen position of the placement's top-left cell (works for
        // pins scrolled anywhere in history, unlike the viewport frame).
        let Some(point) = pages.point_from_pin(Tag::Screen, pin) else {
            continue;
        };

        let (dest_width, dest_height) = placement.pixel_size(image, geo);
        if dest_width == 0 || dest_height == 0 {
            continue;
        }
        let (_, grid_rows) = placement.grid_size(image, geo);
        let Some(grid_row) = win.place_row(point.coord.y, grid_rows) else {
            continue;
        };

        let (source_x, source_width) =
            clamp_source(placement.source_x, placement.source_width, image.width);
        let (source_y, source_height) =
            clamp_source(placement.source_y, placement.source_height, image.height);

        out.push(RenderImagePlacement {
            image_id,
            z: placement.z,
            grid_col: u32::from(point.coord.x),
            grid_row,
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
        resolve_virtual(storage, pages, geo, &win, scrollback_offset, &mut out);
    }

    out
}

/// One image the terminal holds, resolved for a renderer: identity +
/// `generation` (the re-upload key) + decoded, `Arc`-shared RGBA pixels. Carried
/// through the snapshot boundary so the live-app render path can draw images
/// without a live `&Terminal` (R6 slice 5).
///
/// The RGBA is copied once (from the stored format) when the window is captured;
/// making [`Image`]'s stored data an `Arc<[u8]>` to share it copy-free is a
/// follow-up optimization (#19).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotKittyImage {
    pub id: u32,
    pub generation: u64,
    pub width: u32,
    pub height: u32,
    /// Decoded, tightly-packed RGBA (`width * height * 4` bytes).
    pub rgba: Arc<[u8]>,
}

/// Resolve everything a renderer needs to draw kitty images for the window
/// `scrollback_offset` rows up from the bottom: the visible placements
/// ([`resolve_placements`]), the decoded RGBA of every image the terminal holds,
/// and the live-image-id set for GPU texture eviction (R6 slices 1–3, threaded
/// through the snapshot in slice 5). Returns empty vecs when kitty storage is
/// disabled. Images are keyed by id; only images with a resolved placement need
/// drawing, but all live images are returned so their textures aren't evicted.
#[must_use]
pub fn resolve_window(
    storage: &ImageStorage,
    pages: &PageList,
    geo: &TerminalGeometry,
    scrollback_offset: usize,
) -> (Vec<RenderImagePlacement>, Vec<SnapshotKittyImage>, Vec<u32>) {
    if !storage.enabled() {
        return (Vec::new(), Vec::new(), Vec::new());
    }
    let live_ids: Vec<u32> = storage.images.keys().copied().collect();
    let placements = resolve_placements(storage, pages, geo, scrollback_offset);

    // Decode only the images actually referenced by a visible placement — the
    // ones whose textures the renderer will upload this frame.
    let mut images: Vec<SnapshotKittyImage> = Vec::new();
    for p in &placements {
        if images.iter().any(|i| i.id == p.image_id) {
            continue;
        }
        if let Some(img) = storage.image_by_id(p.image_id) {
            images.push(SnapshotKittyImage {
                id: img.id,
                generation: img.generation,
                width: img.width,
                height: img.height,
                rgba: Arc::from(image_rgba(img).into_owned()),
            });
        }
    }
    (placements, images, live_ids)
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

/// Resolve virtual (`U=1`) placements by walking the visible window's
/// placeholder cells. Port of `prepKittyVirtualPlacement` (`image.zig:465-521`):
/// each run of placeholder cells resolves against its stored placement to a
/// [`unicode::RenderPlacement`], mapped to window-relative grid coordinates. The
/// walked pin range is the same window [`Screen::snapshot_window`] renders, so
/// virtual placements track scrollback the same way the pin-anchored path does.
fn resolve_virtual(
    storage: &ImageStorage,
    pages: &PageList,
    geo: &TerminalGeometry,
    win: &Window,
    scrollback_offset: usize,
    out: &mut Vec<RenderImagePlacement>,
) {
    // Guard the divisors first (a live `Terminal` never has zero cols/rows —
    // construction underflows `cols - 1` before that — but `geo` is a plain
    // param, so guard explicitly rather than relying on the caller). Then guard
    // the quotient: a sub-cell-sized viewport yields a zero cell and nothing to
    // place.
    if geo.cols == 0 || geo.rows == 0 {
        return;
    }
    let cell_width = geo.width_px / u32::from(geo.cols);
    let cell_height = geo.height_px / u32::from(geo.rows);
    if cell_width == 0 || cell_height == 0 {
        return;
    }

    // Window pin range (same construction as `Screen::snapshot_window`): the
    // bottom is the screen's bottom-right pinned `offset` rows up; the top is
    // that pinned another `rows - 1` up.
    let rows = usize::from(geo.rows);
    let total_rows = pages.total_rows();
    let offset = scrollback_offset.min(total_rows.saturating_sub(rows));
    let Some(bottom_right) = pages.get_bottom_right(Tag::Screen) else {
        return;
    };
    // SAFETY: `bottom_right` addresses a live page; `up` only walks `prev`
    // pointers within the same live page list.
    let Some(win_bottom) = (unsafe { bottom_right.up(offset) }) else {
        return;
    };
    // SAFETY: as above — `win_bottom` is a live pin derived by walking `prev`.
    let Some(win_top) = (unsafe { win_bottom.up(rows.saturating_sub(1)) }) else {
        return;
    };

    // SAFETY: `win_top`/`win_bottom` are live pins into `pages`; the iterator
    // only reads cells within the owned page chain for the lifetime of this call.
    let mut iter = unsafe { unicode::placement_iterator(win_top, Some(win_bottom)) };
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
        let Some(point) = pages.point_from_pin(Tag::Screen, rp.top_left) else {
            continue;
        };
        // The run's top-left is inside the walked window by construction, so
        // `place_row` (with the single anchoring cell) always resolves.
        let Some(grid_row) = win.place_row(point.coord.y, 1) else {
            continue;
        };

        out.push(RenderImagePlacement {
            image_id: placement.image_id,
            // Upstream draws virtual placements below text (z = -1).
            z: -1,
            grid_col: u32::from(point.coord.x),
            grid_row,
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
        let placements = resolve_placements(&screen.kitty_images, &screen.pages, &geo_of(&t), 0);
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
        let placements = resolve_placements(&screen.kitty_images, &screen.pages, &geo_of(&t), 0);
        assert_eq!(placements.len(), 1);
        assert_eq!(
            (placements[0].dest_width, placements[0].dest_height),
            (32, 24)
        );
    }

    /// R6 slice 2: an image scrolled into history is culled in the live view,
    /// clips its top (negative `grid_row`) when the window edge falls inside it,
    /// sits at row 0 when scrolled fully back, and is culled once fully above.
    #[test]
    fn scrolled_placement_clips_top_and_culls() {
        use crate::stream::{Stream, TerminalHandler};

        let mut t = Terminal::new(Options {
            cols: 10,
            rows: 4,
            ..Default::default()
        });
        t.width_px = 80; // 8px cells
        t.height_px = 32; // 4 rows × 8px

        // Display a 3-cell-tall image (c=3,r=3) at home (screen rows 0..=2), then
        // push it into scrollback with plenty of newlines.
        let rgba = [3u8, 3, 3, 255].repeat(4); // 2×2 RGBA
        let payload = base64::engine::general_purpose::STANDARD.encode(&rgba);
        let mut stream = Stream::new(TerminalHandler::new(t));
        stream.feed(b"\x1b[H");
        stream.feed(format!("\x1b_Ga=T,f=32,t=d,i=1,s=2,v=2,c=3,r=3;{payload}\x1b\\").as_bytes());
        for _ in 0..12 {
            stream.feed(b"scroll\r\n");
        }
        let t = stream.handler.terminal;

        let geo = geo_of(&t);
        let screen = t.screen();
        let rows = usize::from(t.rows);
        let total = screen.pages.total_rows();
        let scrollback_len = total - rows;
        assert!(scrollback_len >= 3, "need enough scrollback for the test");
        let res = |off: usize| resolve_placements(&screen.kitty_images, &screen.pages, &geo, off);

        // Live view: the image is far above the window → culled.
        assert!(
            res(0).is_empty(),
            "scrolled-away image is culled in the live view"
        );

        // Scrolled fully to the top: window top row is screen row 0, so the image
        // sits at grid_row 0, fully visible.
        let top = res(scrollback_len);
        assert_eq!(top.len(), 1);
        assert_eq!(top[0].grid_row, 0);
        assert_eq!(top[0].grid_col, 0);

        // One row down: the window top falls on the image's second row, so its
        // first row is clipped above the window → grid_row = -1 (rows 1..=2 show).
        let mid = res(scrollback_len - 1);
        assert_eq!(mid.len(), 1);
        assert_eq!(mid[0].grid_row, -1);

        // Three rows down: the whole image (rows 0..=2) is above the window → culled.
        assert!(
            res(scrollback_len - 3).is_empty(),
            "image entirely above the window is culled"
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
        let placements = resolve_placements(&screen.kitty_images, &screen.pages, &geo_of(&t), 0);
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
