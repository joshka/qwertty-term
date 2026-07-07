//! Kitty graphics image storage & placement tracking (port of `graphics_storage.zig`, commit
//! `2da015cd6`).
//!
//! [`ImageStorage`] is the per-screen model: a map of transmitted [`Image`]s, a map of
//! [`Placement`]s (screen positions where images are drawn), byte-limit eviction, a
//! `dirty`/`generation` change-tracking pair, and the big `delete`-command dispatch.
//!
//! # Terminal decoupling
//!
//! Upstream reaches into a live `Terminal` for cell/pixel geometry, the active screen's
//! [`PageList`], and the cursor position. This port has no `Terminal` type yet (a sibling
//! chunk owns it), so the geometry-dependent surface is decoupled:
//!
//! - Geometry (`pixel_size`/`grid_size`/`rect`) takes a POD [`TerminalGeometry`](super::TerminalGeometry).
//! - [`ImageStorage::delete`] takes `&mut PageList`, a [`TerminalGeometry`](super::TerminalGeometry),
//!   and an explicit cursor `(x, y)` in active coordinates — exactly the pieces the Zig `delete`
//!   reads out of `Terminal`.
//! - Placement pins are tracked in the [`PageList`]; [`Placement::deinit`] untracks them via
//!   [`PageList::untrack_pin`]. See the pin-lifecycle note below.
//!
//! # Pin lifecycle
//!
//! A [`Location::Pin`] holds a `*mut Pin` returned by [`PageList::track_pin`]. The [`PageList`]
//! owns the allocation and keeps the pin coordinates current across scroll/resize; the
//! placement holds only the raw pointer. Ownership is 1:1 — each placement owns exactly one
//! tracked pin and is responsible for untracking it exactly once. Every code path that removes
//! a placement from the map (`delete`, `deinit`, `clear_placements`) first calls
//! [`Placement::deinit`], which untracks the pin. Eviction (`evict_image`) is the sole
//! exception, matching upstream: it drops placement entries *without* untracking, because
//! `evictImage` in Zig likewise `removeByPtr`s without a `deinit` (the pins are cleaned up when
//! the screen/pagelist itself is torn down). Virtual placements own no pin and untrack nothing.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use super::image::{Image, Rect};
use super::{TerminalGeometry, command};
use crate::page::size::CellCountInt;
use crate::pagelist::{PageList, Pin};
use crate::point::Point;

/// Process-global counter backing all generation stamps (see [`ImageStorage::generation`] and
/// [`Image::generation`]). Global rather than per-storage so stamps are unique across every
/// storage in the process: two mutation events never produce the same value, even across
/// separate screens (main vs. alt), storage resets, or separate terminals. This lets consumers
/// use a generation value alone as a cache key without ambiguity. Port of the
/// `generation_counter` in `graphics_storage.zig`.
static GENERATION_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Returns the next generation stamp. Stamps are unique and strictly monotonically increasing
/// process-wide, starting at 1 (0 is reserved to mean "never stamped"). Port of
/// `nextGeneration`.
pub fn next_generation() -> u64 {
    GENERATION_COUNTER.fetch_add(1, Ordering::Relaxed) + 1
}

/// Failure adding an image to storage: it (alone or after eviction) does not fit the byte
/// limit. Port of the `error.OutOfMemory` that `addImage` returns in these cases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddImageError {
    /// The image does not fit the byte limit and eviction could not free enough space.
    OutOfMemory,
}

impl std::fmt::Display for AddImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for AddImageError {}

/// Default per-storage byte limit (320MB). Port of `total_limit`'s default.
pub const DEFAULT_TOTAL_LIMIT: usize = 320 * 1000 * 1000;

/// The initial auto-assigned image ID. Starts mid-way through the u32 range to avoid collisions
/// with buggy programs. Port of `next_image_id`'s default.
pub const INITIAL_IMAGE_ID: u32 = 2147483647;

/// A placement is uniquely identified by its image ID and placement ID. A `p=0` transmit gets
/// an auto-incremented **internal** id (so multiple placements can exist for one image); a
/// `p>0` transmit uses an **external** id (one placement per `(image_id, p)`). Port of
/// `ImageStorage.PlacementKey`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlacementKey {
    pub image_id: u32,
    pub placement_id: PlacementId,
}

/// The placement-id half of a [`PlacementKey`]. Port of the `placement_id` packed struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlacementId {
    pub tag: PlacementTag,
    pub id: u32,
}

/// Whether a placement id was auto-assigned (`p=0`) or client-chosen (`p>0`). Port of the
/// `enum(u1) { internal, external }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlacementTag {
    Internal,
    External,
}

/// Where a placement is drawn. Port of `Placement.Location`.
///
/// `Pin` holds a `*mut Pin` tracked in the [`PageList`] (see the pin-lifecycle note on the
/// module). This is the one place the storage model leaks a `ghostty-vt` type; see the analysis
/// doc's extraction notes.
#[derive(Debug, Clone, Copy)]
pub enum Location {
    /// Exactly placed on a screen pin (a `*mut Pin` tracked by the [`PageList`]).
    Pin(*mut Pin),
    /// Virtual placement (`U=1`) for unicode placeholders; no rect.
    Virtual,
}

/// A placement of an image on the screen. Port of `ImageStorage.Placement`.
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    /// Where this placement should be drawn.
    pub location: Location,

    /// Offset of the x/y from the top-left of the cell.
    pub x_offset: u32,
    pub y_offset: u32,

    /// Source rectangle to pull from the image.
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,

    /// The columns/rows this image occupies.
    pub columns: u32,
    pub rows: u32,

    /// The z-index for this placement.
    pub z: i32,
}

impl Placement {
    /// A placement at `location` with all other fields defaulted. Convenience mirroring the Zig
    /// struct-literal `.{ .location = ... }` used throughout the tests.
    pub fn new(location: Location) -> Placement {
        Placement {
            location,
            x_offset: 0,
            y_offset: 0,
            source_x: 0,
            source_y: 0,
            source_width: 0,
            source_height: 0,
            columns: 0,
            rows: 0,
            z: 0,
        }
    }

    /// Untrack the placement's pin, if any. Port of `Placement.deinit`.
    ///
    /// Must be called exactly once per placement before it is dropped from the map (except on
    /// eviction, which matches upstream by not untracking — see the module pin-lifecycle note).
    pub fn deinit(&self, pages: &mut PageList) {
        match self.location {
            Location::Pin(p) => pages.untrack_pin(p),
            Location::Virtual => {}
        }
    }

    /// The size of this placement's image in pixels, honoring source rect, cols/rows, and
    /// aspect ratio. Port of `Placement.pixelSize`.
    pub fn pixel_size(&self, image: &Image, geo: &TerminalGeometry) -> (u32, u32) {
        // Height / width of the image in px.
        let width = if self.source_width > 0 {
            self.source_width
        } else {
            image.width
        };
        let height = if self.source_height > 0 {
            self.source_height
        } else {
            image.height
        };

        // No specified cols/rows: native size, no re-scaling.
        if self.columns == 0 && self.rows == 0 {
            return (width, height);
        }

        // Cell size (assumes width divides evenly by cols, height by rows).
        let cell_width: u32 = geo.width_px / geo.cols as u32;
        let cell_height: u32 = geo.height_px / geo.rows as u32;

        let width_f64 = width as f64;
        let height_f64 = height as f64;

        // Both cols AND rows specified: compute directly, no aspect adjustment.
        if self.columns > 0 && self.rows > 0 {
            return (cell_width * self.columns, cell_height * self.rows);
        }

        // Only columns: derive height from the aspect ratio.
        if self.columns > 0 {
            let aspect = height_f64 / width_f64;
            let calc_width = cell_width * self.columns;
            let calc_height = (calc_width as f64 * aspect).round() as u32;
            return (calc_width, calc_height);
        }

        // Otherwise only rows: derive width from the aspect ratio.
        let aspect = width_f64 / height_f64;
        let calc_height = cell_height * self.rows;
        let calc_width = (calc_height as f64 * aspect).round() as u32;
        (calc_width, calc_height)
    }

    /// The size in grid cells this placement takes up. Port of `Placement.gridSize`.
    pub fn grid_size(&self, image: &Image, geo: &TerminalGeometry) -> (u32, u32) {
        // Trivial if both specified.
        if self.columns > 0 && self.rows > 0 {
            return (self.columns, self.rows);
        }

        // Otherwise compute pixel size, divide by cell size, round up. `div_ceil` on a zero
        // cell size returns 0 (upstream's `divCeil` errors, caught to 0).
        let (px_w, px_h) = self.pixel_size(image, geo);
        let cell_w = geo.width_px / geo.cols as u32;
        let cell_h = geo.height_px / geo.rows as u32;
        let cols = if cell_w == 0 {
            0
        } else {
            (px_w + self.x_offset).div_ceil(cell_w)
        };
        let rows = if cell_h == 0 {
            0
        } else {
            (px_h + self.y_offset).div_ceil(cell_h)
        };
        (cols, rows)
    }

    /// The rectangle (in grid cells) this placement occupies, or `None` for a virtual
    /// placement. Port of `Placement.rect`.
    ///
    /// # Safety
    /// The pin's node (and the page chain) must be live.
    unsafe fn rect(&self, image: &Image, geo: &TerminalGeometry) -> Option<Rect> {
        let (grid_cols, grid_rows) = self.grid_size(image, geo);
        let pin = match self.location {
            Location::Pin(p) => unsafe { *p },
            Location::Virtual => return None,
        };

        let mut br = unsafe { pin.down_overflow_clamped(grid_rows as usize - 1) };
        br.x = std::cmp::min(
            // Subtract one: the x value is already one width. If the image is width "1"
            // then we add zero to X because X itself is width 1.
            pin.x() + (grid_cols as CellCountInt - 1),
            geo.cols - 1,
        );

        Some(Rect {
            top_left: pin,
            bottom_right: br,
        })
    }
}

/// An image storage, associated with a terminal screen (main or alt). Holds all transmitted
/// images and their placements. Port of `graphics_storage.ImageStorage`.
pub struct ImageStorage {
    /// Set when placements or images change **and** on scroll/resize (geometry). Purely
    /// informational for the renderer, which clears it. Invariant: always set when `generation`
    /// changes (a geometry-only event sets `dirty` without bumping `generation`).
    pub dirty: bool,

    /// Generation stamp of the last **content** mutation (transmit/replace/placement/delete).
    /// Zero means never mutated. NOT bumped by geometry events, so an unchanged generation means
    /// identical content. Written only via [`ImageStorage::mark_mutated`].
    pub generation: u64,

    /// Next auto-assigned image ID.
    pub next_image_id: u32,

    /// Next auto-assigned internal placement ID (`p=0`).
    pub next_internal_placement_id: u32,

    /// The known images, keyed by image id.
    pub images: HashMap<u32, Image>,

    /// The placements for loaded images.
    pub placements: HashMap<PlacementKey, Placement>,

    /// The allowed transmission mediums for image loading.
    pub image_limits: super::image::Limits,

    /// Total loaded image bytes and the eviction limit. `enabled()` is `total_limit != 0`.
    pub total_bytes: usize,
    pub total_limit: usize,
}

impl Default for ImageStorage {
    fn default() -> Self {
        ImageStorage {
            dirty: false,
            generation: 0,
            next_image_id: INITIAL_IMAGE_ID,
            next_internal_placement_id: 0,
            images: HashMap::new(),
            placements: HashMap::new(),
            image_limits: super::image::Limits::DIRECT,
            total_bytes: 0,
            total_limit: DEFAULT_TOTAL_LIMIT,
        }
    }
}

impl ImageStorage {
    /// A fresh storage with all defaults. Port of the `.{}` struct-literal init used in tests.
    pub fn new() -> ImageStorage {
        ImageStorage::default()
    }

    /// Untrack all placement pins and clear both maps. Port of `ImageStorage.deinit` (minus the
    /// `loading` field, which lives with the exec layer). `pages` is the active screen's list.
    pub fn deinit(&mut self, pages: &mut PageList) {
        self.clear_placements(pages);
        self.placements.clear();
        self.images.clear();
    }

    /// Kitty image protocol is enabled if the limit is non-zero. Port of `enabled`.
    pub fn enabled(&self) -> bool {
        self.total_limit != 0
    }

    /// Record a content mutation: mark dirty and assign a fresh generation stamp. Must be called
    /// by anything that changes the set of images/placements (or image contents). Do NOT call
    /// for geometry-only events. Port of `markMutated`.
    fn mark_mutated(&mut self) {
        self.dirty = true;
        self.generation = next_generation();
    }

    /// Set the total byte limit. Lowering below `total_bytes` evicts; `limit == 0` fully resets
    /// the storage (disabling the protocol) preserving `image_limits`. Port of `setLimit`.
    pub fn set_limit(&mut self, pages: &mut PageList, limit: usize) {
        // Special case: disabling quickly deletes all.
        if limit == 0 {
            let image_limits = self.image_limits;
            self.deinit(pages);
            *self = ImageStorage {
                image_limits,
                ..ImageStorage::default()
            };
            self.mark_mutated();
        }

        // Lowering the limit: evict if necessary.
        if limit < self.total_bytes {
            let req_bytes = self.total_bytes - limit;
            self.evict_image(req_bytes);
        }

        self.total_limit = limit;
    }

    /// Add an already-loaded image, freeing any existing image with the same id. Errors if the
    /// image alone exceeds the limit or eviction cannot free enough space. Port of `addImage`.
    pub fn add_image(&mut self, img: Image) -> Result<(), AddImageError> {
        // If the image itself is over the limit, error immediately.
        if img.data.len() > self.total_limit {
            return Err(AddImageError::OutOfMemory);
        }

        // If this would put us over the limit, evict.
        let total_bytes = self.total_bytes + img.data.len();
        if total_bytes > self.total_limit {
            let req_bytes = total_bytes - self.total_limit;
            if !self.evict_image(req_bytes) {
                return Err(AddImageError::OutOfMemory);
            }
        }

        // Free an existing same-id image, adjusting the byte total.
        if let Some(existing) = self.images.get(&img.id) {
            self.total_bytes -= existing.data.len();
        }

        let data_len = img.data.len();
        let id = img.id;
        self.images.insert(id, img);
        self.total_bytes += data_len;

        // Stamp the stored image with a fresh generation so every add/replace is uniquely
        // detectable (even a same-dimensions retransmit).
        self.mark_mutated();
        let generation = self.generation;
        if let Some(stored) = self.images.get_mut(&id) {
            stored.generation = generation;
        }
        Ok(())
    }

    /// Add a placement for an image (which the caller must have verified exists). A `p=0`
    /// placement id becomes an auto-incremented internal id; `p>0` is an external id. Port of
    /// `addPlacement`.
    pub fn add_placement(&mut self, image_id: u32, placement_id: u32, p: Placement) {
        debug_assert!(self.images.contains_key(&image_id));

        let key = PlacementKey {
            image_id,
            placement_id: if placement_id == 0 {
                let id = self.next_internal_placement_id;
                self.next_internal_placement_id = self.next_internal_placement_id.wrapping_add(1);
                PlacementId {
                    tag: PlacementTag::Internal,
                    id,
                }
            } else {
                PlacementId {
                    tag: PlacementTag::External,
                    id: placement_id,
                }
            },
        };

        self.placements.insert(key, p);
        self.mark_mutated();
    }

    /// Untrack every placement pin and empty the placement map without deallocating capacity.
    /// Port of `clearPlacements`.
    fn clear_placements(&mut self, pages: &mut PageList) {
        for p in self.placements.values() {
            p.deinit(pages);
        }
        self.placements.clear();
    }

    /// Get an image by its ID. Port of `imageById`.
    pub fn image_by_id(&self, image_id: u32) -> Option<&Image> {
        self.images.get(&image_id)
    }

    /// Get an image by its number, returning the newest-generation match. Port of
    /// `imageByNumber`.
    pub fn image_by_number(&self, image_number: u32) -> Option<&Image> {
        let mut newest: Option<&Image> = None;
        for img in self.images.values() {
            if img.number == image_number
                && (newest.is_none() || img.generation > newest.unwrap().generation)
            {
                newest = Some(img);
            }
        }
        newest
    }

    /// Delete placements and/or images per a [`command::Delete`] command. `pages` is the active
    /// screen's list, `geo` the current geometry, and `cursor` the active-coords cursor position
    /// (used by [`command::Delete::IntersectCursor`]). Port of `ImageStorage.delete`.
    ///
    /// Only marks a mutation if something actually changed — a delete-all runs on every screen
    /// clear (`ESC [ 2 J`), and empty clears must not dirty the state or bump the generation.
    pub fn delete(
        &mut self,
        pages: &mut PageList,
        geo: &TerminalGeometry,
        cursor: (CellCountInt, CellCountInt),
        cmd: command::Delete,
    ) {
        let placements_before = self.placements.len();
        let images_before = self.images.len();

        match cmd {
            command::Delete::All(delete_images) => {
                let keys: Vec<PlacementKey> = self
                    .placements
                    .iter()
                    .filter(|(_, p)| !matches!(p.location, Location::Virtual))
                    .map(|(k, _)| *k)
                    .collect();
                for key in keys {
                    if let Some(p) = self.placements.remove(&key) {
                        p.deinit(pages);
                        if delete_images {
                            self.delete_if_unused(key.image_id);
                        }
                    }
                }

                if delete_images {
                    let image_ids: Vec<u32> = self.images.keys().copied().collect();
                    for id in image_ids {
                        self.delete_if_unused(id);
                    }
                }
            }

            command::Delete::Id {
                delete,
                image_id,
                placement_id,
            } => self.delete_by_id(pages, image_id, placement_id, delete),

            command::Delete::Newest {
                delete,
                image_number,
                placement_id,
            } => {
                if let Some(img) = self.image_by_number(image_number) {
                    let id = img.id;
                    self.delete_by_id(pages, id, placement_id, delete);
                }
            }

            command::Delete::IntersectCursor(delete_images) => {
                self.delete_intersecting(
                    pages,
                    geo,
                    Point::active(cursor.0, cursor.1 as u32),
                    delete_images,
                    |_| true,
                );
            }

            command::Delete::IntersectCell { delete, x, y } => {
                if x == 0 || y == 0 {
                    return;
                }
                let (Ok(px), Ok(py)) =
                    (CellCountInt::try_from(x - 1), CellCountInt::try_from(y - 1))
                else {
                    return;
                };
                self.delete_intersecting(pages, geo, Point::active(px, py as u32), delete, |_| {
                    true
                });
            }

            command::Delete::IntersectCellZ { delete, x, y, z } => {
                if x == 0 || y == 0 {
                    return;
                }
                let (Ok(px), Ok(py)) =
                    (CellCountInt::try_from(x - 1), CellCountInt::try_from(y - 1))
                else {
                    return;
                };
                self.delete_intersecting(
                    pages,
                    geo,
                    Point::active(px, py as u32),
                    delete,
                    |p: &Placement| p.z == z,
                );
            }

            command::Delete::Column { delete, x } => {
                if x == 0 {
                    return;
                }
                let x = (x - 1) as CellCountInt;
                let keys: Vec<PlacementKey> = self.placements.keys().copied().collect();
                for key in keys {
                    let Some(img) = self.images.get(&key.image_id).cloned() else {
                        continue;
                    };
                    let Some(placement) = self.placements.get(&key).copied() else {
                        continue;
                    };
                    let Some(rect) = (unsafe { placement.rect(&img, geo) }) else {
                        continue;
                    };
                    if rect.top_left.x() <= x
                        && rect.bottom_right.x() >= x
                        && let Some(p) = self.placements.remove(&key)
                    {
                        p.deinit(pages);
                        if delete {
                            self.delete_if_unused(img.id);
                        }
                    }
                }
            }

            command::Delete::Row { delete, y } => {
                if y == 0 {
                    return;
                }
                // y is in active coords; convert to a pin to compare by page offsets.
                let Some(ay) = CellCountInt::try_from(y - 1).ok() else {
                    return;
                };
                let Some(target_pin) = pages.pin(Point::active(0, ay as u32)) else {
                    return;
                };
                let keys: Vec<PlacementKey> = self.placements.keys().copied().collect();
                for key in keys {
                    let Some(img) = self.images.get(&key.image_id).cloned() else {
                        continue;
                    };
                    let Some(placement) = self.placements.get(&key).copied() else {
                        continue;
                    };
                    let Some(rect) = (unsafe { placement.rect(&img, geo) }) else {
                        continue;
                    };
                    // Copy the target pin to at least the top-left x for the comparison.
                    let mut target_pin_copy = target_pin;
                    target_pin_copy.x = rect.top_left.x();
                    if unsafe { target_pin_copy.is_between(rect.top_left, rect.bottom_right) }
                        && let Some(p) = self.placements.remove(&key)
                    {
                        p.deinit(pages);
                        if delete {
                            self.delete_if_unused(img.id);
                        }
                    }
                }
            }

            command::Delete::Z { delete, z } => {
                let keys: Vec<PlacementKey> = self
                    .placements
                    .iter()
                    // Virtual placeholders cannot delete by z (per spec).
                    .filter(|(_, p)| !matches!(p.location, Location::Virtual))
                    .filter(|(_, p)| p.z == z)
                    .map(|(k, _)| *k)
                    .collect();
                for key in keys {
                    if let Some(p) = self.placements.remove(&key) {
                        p.deinit(pages);
                        if delete {
                            self.delete_if_unused(key.image_id);
                        }
                    }
                }
            }

            command::Delete::Range {
                delete,
                first,
                last,
            } => {
                if first == 0 || last == 0 {
                    return;
                }
                if first > last {
                    return;
                }
                let keys: Vec<PlacementKey> = self.placements.keys().copied().collect();
                for key in keys {
                    // NOTE: upstream uses `>= first OR <= last` (`||`), preserved verbatim.
                    if (key.image_id >= first || key.image_id <= last)
                        && let Some(p) = self.placements.remove(&key)
                    {
                        p.deinit(pages);
                        if delete {
                            self.delete_if_unused(key.image_id);
                        }
                    }
                }
            }

            // Animation frames aren't supported yet, so they're "successfully" deleted.
            command::Delete::AnimationFrames(_) => {}
        }

        if self.placements.len() != placements_before || self.images.len() != images_before {
            self.mark_mutated();
        }
    }

    fn delete_by_id(
        &mut self,
        pages: &mut PageList,
        image_id: u32,
        placement_id: u32,
        delete_unused: bool,
    ) {
        if placement_id == 0 {
            // Delete all placements with this image ID.
            let keys: Vec<PlacementKey> = self
                .placements
                .keys()
                .copied()
                .filter(|k| k.image_id == image_id)
                .collect();
            for key in keys {
                if let Some(p) = self.placements.remove(&key) {
                    p.deinit(pages);
                }
            }
        } else {
            let key = PlacementKey {
                image_id,
                placement_id: PlacementId {
                    tag: PlacementTag::External,
                    id: placement_id,
                },
            };
            if let Some(p) = self.placements.remove(&key) {
                p.deinit(pages);
            }
        }

        if delete_unused {
            self.delete_if_unused(image_id);
        }
    }

    /// Delete an image if no placement references it. Port of `deleteIfUnused`.
    fn delete_if_unused(&mut self, image_id: u32) {
        let used = self.placements.keys().any(|k| k.image_id == image_id);
        if used {
            return;
        }
        if let Some(img) = self.images.remove(&image_id) {
            self.total_bytes -= img.data.len();
        }
    }

    /// Delete all placements whose rect intersects `p`, subject to `filter`. Port of
    /// `deleteIntersecting`.
    fn delete_intersecting(
        &mut self,
        pages: &mut PageList,
        geo: &TerminalGeometry,
        p: Point,
        delete_unused: bool,
        filter: impl Fn(&Placement) -> bool,
    ) {
        let Some(target_pin) = pages.pin(p) else {
            return;
        };

        let keys: Vec<PlacementKey> = self.placements.keys().copied().collect();
        for key in keys {
            let Some(img) = self.images.get(&key.image_id).cloned() else {
                continue;
            };
            let Some(placement) = self.placements.get(&key).copied() else {
                continue;
            };
            let Some(rect) = (unsafe { placement.rect(&img, geo) }) else {
                continue;
            };
            if unsafe { target_pin.is_between(rect.top_left, rect.bottom_right) } {
                if !filter(&placement) {
                    continue;
                }
                if let Some(rp) = self.placements.remove(&key) {
                    rp.deinit(pages);
                    if delete_unused {
                        self.delete_if_unused(img.id);
                    }
                }
            }
        }
    }

    /// Evict images to free `req` bytes, prioritizing unused images, then oldest generation,
    /// tie-broken by id. Returns whether enough was freed. Marks a mutation if anything was
    /// evicted. Port of `evictImage`.
    ///
    /// Unlike the delete family, eviction drops placement entries **without** untracking their
    /// pins — matching upstream `evictImage`, which `removeByPtr`s placements without a `deinit`.
    fn evict_image(&mut self, req: usize) -> bool {
        debug_assert!(req <= self.total_limit);

        #[derive(Clone, Copy)]
        struct Candidate {
            id: u32,
            generation: u64,
            used: bool,
        }

        let mut candidates: Vec<Candidate> = self
            .images
            .values()
            .map(|img| Candidate {
                id: img.id,
                generation: img.generation,
                used: self.placements.keys().any(|k| k.image_id == img.id),
            })
            .collect();

        // Sort best-to-evict first: unused before used; then oldest generation; tie-break by id.
        candidates.sort_unstable_by(|lhs, rhs| {
            if lhs.used == rhs.used {
                if lhs.generation == rhs.generation {
                    lhs.id.cmp(&rhs.id)
                } else {
                    lhs.generation.cmp(&rhs.generation)
                }
            } else {
                // If lhs is not used, it's the better candidate (sorts first).
                if !lhs.used {
                    std::cmp::Ordering::Less
                } else {
                    std::cmp::Ordering::Greater
                }
            }
        });

        let mut any_evicted = false;
        let mut evicted: usize = 0;
        let mut freed_enough = false;
        for c in candidates {
            // Drop all placements for this image (no pin untrack; see doc comment).
            let keys: Vec<PlacementKey> = self
                .placements
                .keys()
                .copied()
                .filter(|k| k.image_id == c.id)
                .collect();
            for key in keys {
                self.placements.remove(&key);
                any_evicted = true;
            }

            if let Some(img) = self.images.remove(&c.id) {
                evicted += img.data.len();
                self.total_bytes -= img.data.len();
                any_evicted = true;
                if evicted > req {
                    freed_enough = true;
                    break;
                }
            }
        }

        if any_evicted {
            self.mark_mutated();
        }
        freed_enough
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pagelist::PageList;
    use crate::point::Point;

    /// Track a pin at active coords `(x, y)`. Port of the `trackPin(t, coord)` test helper,
    /// which does `pages.trackPin(pages.pin(.{ .active = coord }).?)`. Drives against a
    /// `PageList` directly since no `Terminal` type exists in this chunk's base.
    fn track_pin(pages: &mut PageList, x: CellCountInt, y: CellCountInt) -> *mut Pin {
        let pin = pages.pin(Point::active(x, y as u32)).unwrap();
        pages.track_pin(pin)
    }

    /// A pinned placement at `(x, y)`.
    fn pin_placement(pages: &mut PageList, x: CellCountInt, y: CellCountInt) -> Placement {
        Placement::new(Location::Pin(track_pin(pages, x, y)))
    }

    /// An image with the given id and dimensions (other fields defaulted).
    fn image(id: u32, width: u32, height: u32) -> Image {
        Image {
            id,
            width,
            height,
            ..Image::default()
        }
    }

    /// Geometry with a 1px cell (cols == width_px, rows == height_px). Mirrors the Zig tests'
    /// `t.width_px = 100; t.height_px = 100` on a 100x100 grid.
    fn geo(
        cols: CellCountInt,
        rows: CellCountInt,
        width_px: u32,
        height_px: u32,
    ) -> TerminalGeometry {
        TerminalGeometry::new(cols, rows, width_px, height_px)
    }

    #[test]
    fn add_placement_with_zero_placement_id() {
        let mut pages = PageList::init(100, 100, None);
        let mut s = ImageStorage::new();

        s.add_image(image(1, 50, 50)).unwrap();
        s.add_image(image(2, 25, 25)).unwrap();
        let p1 = pin_placement(&mut pages, 25, 25);
        s.add_placement(1, 0, p1);
        let p2 = pin_placement(&mut pages, 25, 25);
        s.add_placement(1, 0, p2);

        assert_eq!(s.placements.len(), 2);
        assert_eq!(s.images.len(), 2);

        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::Internal,
                id: 0
            },
        }));
        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::Internal,
                id: 1
            },
        }));

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_all_placements_and_images() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p2);

        s.dirty = false;
        s.delete(&mut pages, &g, (0, 0), command::Delete::All(true));
        assert!(s.dirty);
        assert_eq!(s.images.len(), 0);
        assert_eq!(s.placements.len(), 0);
        assert_eq!(pages.count_tracked_pins(), tracked);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_all_placements_and_images_preserves_limit() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();
        s.total_limit = 5000;

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p2);

        s.dirty = false;
        s.delete(&mut pages, &g, (0, 0), command::Delete::All(true));
        assert!(s.dirty);
        assert_eq!(s.images.len(), 0);
        assert_eq!(s.placements.len(), 0);
        assert_eq!(s.total_limit, 5000);
        assert_eq!(pages.count_tracked_pins(), tracked);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_all_placements() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p2);

        s.dirty = false;
        s.delete(&mut pages, &g, (0, 0), command::Delete::All(false));
        assert!(s.dirty);
        assert_eq!(s.placements.len(), 0);
        assert_eq!(s.images.len(), 3);
        assert_eq!(pages.count_tracked_pins(), tracked);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_all_placements_by_image_id() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Id {
                delete: false,
                image_id: 2,
                placement_id: 0,
            },
        );
        assert!(s.dirty);
        assert_eq!(s.placements.len(), 1);
        assert_eq!(s.images.len(), 3);
        assert_eq!(pages.count_tracked_pins(), tracked + 1);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_all_placements_by_image_id_and_unused_images() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Id {
                delete: true,
                image_id: 2,
                placement_id: 0,
            },
        );
        assert!(s.dirty);
        assert_eq!(s.placements.len(), 1);
        assert_eq!(s.images.len(), 2);
        assert_eq!(pages.count_tracked_pins(), tracked + 1);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_placement_by_specific_id() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 2, p2);
        let p3 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p3);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Id {
                delete: true,
                image_id: 1,
                placement_id: 2,
            },
        );
        assert!(s.dirty);
        assert_eq!(s.placements.len(), 2);
        assert_eq!(s.images.len(), 3);
        assert_eq!(pages.count_tracked_pins(), tracked + 2);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_intersecting_cursor() {
        let mut pages = PageList::init(100, 100, None);
        let g = geo(100, 100, 100, 100);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 50, 50)).unwrap();
        s.add_image(image(2, 25, 25)).unwrap();
        let p1 = pin_placement(&mut pages, 0, 0);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 25, 25);
        s.add_placement(1, 2, p2);

        // cursor at (12, 12)
        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (12, 12),
            command::Delete::IntersectCursor(false),
        );
        assert!(s.dirty);
        assert_eq!(s.placements.len(), 1);
        assert_eq!(s.images.len(), 2);
        assert_eq!(pages.count_tracked_pins(), tracked + 1);

        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::External,
                id: 2
            },
        }));

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_intersecting_cursor_plus_unused() {
        let mut pages = PageList::init(100, 100, None);
        let g = geo(100, 100, 100, 100);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 50, 50)).unwrap();
        s.add_image(image(2, 25, 25)).unwrap();
        let p1 = pin_placement(&mut pages, 0, 0);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 25, 25);
        s.add_placement(1, 2, p2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (12, 12),
            command::Delete::IntersectCursor(true),
        );
        assert!(s.dirty);
        assert_eq!(s.placements.len(), 1);
        assert_eq!(s.images.len(), 2);
        assert_eq!(pages.count_tracked_pins(), tracked + 1);

        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::External,
                id: 2
            },
        }));

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_intersecting_cursor_hits_multiple() {
        let mut pages = PageList::init(100, 100, None);
        let g = geo(100, 100, 100, 100);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 50, 50)).unwrap();
        s.add_image(image(2, 25, 25)).unwrap();
        let p1 = pin_placement(&mut pages, 0, 0);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 25, 25);
        s.add_placement(1, 2, p2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (26, 26),
            command::Delete::IntersectCursor(true),
        );
        assert!(s.dirty);
        assert_eq!(s.placements.len(), 0);
        assert_eq!(s.images.len(), 1);
        assert_eq!(pages.count_tracked_pins(), tracked);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_by_column() {
        let mut pages = PageList::init(100, 100, None);
        let g = geo(100, 100, 100, 100);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 50, 50)).unwrap();
        s.add_image(image(2, 25, 25)).unwrap();
        let p1 = pin_placement(&mut pages, 0, 0);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 25, 25);
        s.add_placement(1, 2, p2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Column {
                delete: false,
                x: 60,
            },
        );
        assert!(s.dirty);
        assert_eq!(s.placements.len(), 1);
        assert_eq!(s.images.len(), 2);
        assert_eq!(pages.count_tracked_pins(), tracked + 1);

        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::External,
                id: 1
            },
        }));

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_by_column_1x1() {
        let mut pages = PageList::init(100, 100, None);
        let g = geo(100, 100, 100, 100);
        let mut s = ImageStorage::new();

        s.add_image(image(1, 1, 1)).unwrap();
        let p1 = pin_placement(&mut pages, 0, 0);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 0);
        s.add_placement(1, 2, p2);
        let p3 = pin_placement(&mut pages, 2, 0);
        s.add_placement(1, 3, p3);

        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Column {
                delete: false,
                x: 2,
            },
        );
        assert_eq!(s.placements.len(), 2);
        assert_eq!(s.images.len(), 1);

        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::External,
                id: 1
            },
        }));
        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::External,
                id: 3
            },
        }));

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_by_row() {
        let mut pages = PageList::init(100, 100, None);
        let g = geo(100, 100, 100, 100);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 50, 50)).unwrap();
        s.add_image(image(2, 25, 25)).unwrap();
        let p1 = pin_placement(&mut pages, 0, 0);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 25, 25);
        s.add_placement(1, 2, p2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Row {
                delete: false,
                y: 60,
            },
        );
        assert!(s.dirty);
        assert_eq!(s.placements.len(), 1);
        assert_eq!(s.images.len(), 2);
        assert_eq!(pages.count_tracked_pins(), tracked + 1);

        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::External,
                id: 1
            },
        }));

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_by_row_1x1() {
        let mut pages = PageList::init(100, 100, None);
        let g = geo(100, 100, 100, 100);
        let mut s = ImageStorage::new();

        s.add_image(image(1, 1, 1)).unwrap();
        let p1 = pin_placement(&mut pages, 0, 0);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 0, 1);
        s.add_placement(1, 2, p2);
        let p3 = pin_placement(&mut pages, 0, 2);
        s.add_placement(1, 3, p3);

        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Row {
                delete: false,
                y: 2,
            },
        );
        assert_eq!(s.placements.len(), 2);
        assert_eq!(s.images.len(), 1);

        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::External,
                id: 1
            },
        }));
        assert!(s.placements.contains_key(&PlacementKey {
            image_id: 1,
            placement_id: PlacementId {
                tag: PlacementTag::External,
                id: 3
            },
        }));

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_images_by_range_1() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p2);
        assert_eq!(s.images.len(), 3);
        assert_eq!(s.placements.len(), 2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Range {
                delete: false,
                first: 1,
                last: 2,
            },
        );
        assert!(s.dirty);
        assert_eq!(s.images.len(), 3);
        assert_eq!(s.placements.len(), 0);
        assert_eq!(pages.count_tracked_pins(), tracked);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_images_by_range_2() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p2);
        assert_eq!(s.images.len(), 3);
        assert_eq!(s.placements.len(), 2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Range {
                delete: true,
                first: 1,
                last: 2,
            },
        );
        assert!(s.dirty);
        assert_eq!(s.images.len(), 1);
        assert_eq!(s.placements.len(), 0);
        assert_eq!(pages.count_tracked_pins(), tracked);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_images_by_range_3() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p2);
        assert_eq!(s.images.len(), 3);
        assert_eq!(s.placements.len(), 2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Range {
                delete: false,
                first: 1,
                last: 1,
            },
        );
        assert!(s.dirty);
        assert_eq!(s.images.len(), 3);
        assert_eq!(s.placements.len(), 0);
        assert_eq!(pages.count_tracked_pins(), tracked);

        s.deinit(&mut pages);
    }

    #[test]
    fn delete_images_by_range_4() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let tracked = pages.count_tracked_pins();
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        s.add_image(image(2, 0, 0)).unwrap();
        s.add_image(image(3, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let p2 = pin_placement(&mut pages, 1, 1);
        s.add_placement(2, 1, p2);
        assert_eq!(s.images.len(), 3);
        assert_eq!(s.placements.len(), 2);

        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Range {
                delete: true,
                first: 1,
                last: 1,
            },
        );
        assert!(s.dirty);
        assert_eq!(s.images.len(), 1);
        assert_eq!(s.placements.len(), 0);
        assert_eq!(pages.count_tracked_pins(), tracked);

        s.deinit(&mut pages);
    }

    #[test]
    fn aspect_ratio_calculation_when_only_columns_or_rows_specified() {
        // 100x100 grid, 1000px wide (10 px/col), 2000px tall (20 px/row).
        let g = geo(100, 100, 1000, 2000);

        // Case 1: only columns specified.
        {
            let img = image(1, 16, 9);
            let placement = Placement {
                columns: 10,
                rows: 0,
                ..Placement::new(Location::Virtual)
            };
            // 10 cols * 10px = 100px width. 100 * (9/16) = 56.25 -> 56.
            let (w, h) = placement.pixel_size(&img, &g);
            assert_eq!(w, 100);
            assert_eq!(h, 56);
        }

        // Case 2: only rows specified.
        {
            let img = image(2, 16, 9);
            let placement = Placement {
                columns: 0,
                rows: 5,
                ..Placement::new(Location::Virtual)
            };
            // 5 rows * 20px = 100px height. 100 * (16/9) = 177.77... -> 178.
            let (w, h) = placement.pixel_size(&img, &g);
            assert_eq!(w, 178);
            assert_eq!(h, 100);
        }
    }

    #[test]
    fn generation_stamps_on_image_add_and_replace() {
        let mut pages = PageList::init(3, 3, None);
        let mut s = ImageStorage::new();

        // Fresh storage has generation zero.
        assert_eq!(s.generation, 0);

        s.add_image(image(1, 1, 1)).unwrap();
        let gen1 = s.generation;
        assert!(gen1 > 0);
        assert_eq!(s.image_by_id(1).unwrap().generation, gen1);

        // A second image gets a strictly greater stamp.
        s.add_image(image(2, 1, 1)).unwrap();
        let gen2 = s.generation;
        assert!(gen2 > gen1);
        assert_eq!(s.image_by_id(2).unwrap().generation, gen2);

        // Retransmit same id (identical dims) gets a fresh stamp.
        s.add_image(image(1, 1, 1)).unwrap();
        let gen3 = s.generation;
        assert!(gen3 > gen2);
        assert_eq!(s.image_by_id(1).unwrap().generation, gen3);

        // Image 2 kept its stamp.
        assert_eq!(s.image_by_id(2).unwrap().generation, gen2);

        s.deinit(&mut pages);
    }

    #[test]
    fn generation_bumps_on_placement_and_delete() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let mut s = ImageStorage::new();

        s.add_image(image(1, 0, 0)).unwrap();
        let gen_add = s.generation;

        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let gen_place = s.generation;
        assert!(gen_place > gen_add);

        // Reads don't change the generation.
        let _ = s.image_by_id(1);
        let _ = s.image_by_number(1);
        assert_eq!(s.generation, gen_place);

        s.delete(&mut pages, &g, (0, 0), command::Delete::All(true));
        assert!(s.generation > gen_place);

        s.deinit(&mut pages);
    }

    #[test]
    fn generation_bumps_when_set_limit_evicts_or_disables() {
        let mut pages = PageList::init(3, 3, None);
        let mut s = ImageStorage::new();

        let mut img = image(1, 1, 1);
        img.data = b"1234".to_vec();
        s.add_image(img).unwrap();
        let gen_add = s.generation;

        // Lowering the limit evicts the image and must mark a mutation.
        s.dirty = false;
        s.set_limit(&mut pages, 1);
        assert!(s.dirty);
        assert!(s.generation > gen_add);
        assert_eq!(s.images.len(), 0);
        let gen_evict = s.generation;

        // Disabling (limit=0) resets the storage and must mark a mutation.
        s.dirty = false;
        s.set_limit(&mut pages, 0);
        assert!(s.dirty);
        assert!(s.generation > gen_evict);

        s.deinit(&mut pages);
    }

    #[test]
    fn image_by_number_returns_most_recently_transmitted() {
        let mut pages = PageList::init(3, 3, None);
        let mut s = ImageStorage::new();

        // Two images sharing a number: the newest transmission wins.
        let mut i1 = image(1, 0, 0);
        i1.number = 7;
        s.add_image(i1).unwrap();
        let mut i2 = image(2, 0, 0);
        i2.number = 7;
        s.add_image(i2).unwrap();
        assert_eq!(s.image_by_number(7).unwrap().id, 2);

        // Retransmit the first: it becomes the newest.
        let mut i1b = image(1, 0, 0);
        i1b.number = 7;
        s.add_image(i1b).unwrap();
        assert_eq!(s.image_by_number(7).unwrap().id, 1);

        s.deinit(&mut pages);
    }

    #[test]
    fn next_generation_is_unique_and_monotonic() {
        let a = next_generation();
        let b = next_generation();
        assert!(b > a);
        assert!(a > 0);
    }

    #[test]
    fn no_op_delete_does_not_mark_a_mutation() {
        let mut pages = PageList::init(3, 3, None);
        let g = geo(3, 3, 0, 0);
        let mut s = ImageStorage::new();

        // A delete-all on empty storage must not dirty or bump the generation.
        s.delete(&mut pages, &g, (0, 0), command::Delete::All(true));
        assert!(!s.dirty);
        assert_eq!(s.generation, 0);

        // Same for a delete that matches nothing.
        s.add_image(image(1, 0, 0)).unwrap();
        let p1 = pin_placement(&mut pages, 1, 1);
        s.add_placement(1, 1, p1);
        let gen_prev = s.generation;
        s.dirty = false;
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Id {
                delete: false,
                image_id: 42,
                placement_id: 0,
            },
        );
        assert!(!s.dirty);
        assert_eq!(s.generation, gen_prev);

        // But a delete that removes something does mark a mutation.
        s.delete(
            &mut pages,
            &g,
            (0, 0),
            command::Delete::Id {
                delete: false,
                image_id: 1,
                placement_id: 0,
            },
        );
        assert!(s.dirty);
        assert!(s.generation > gen_prev);

        s.deinit(&mut pages);
    }
}
