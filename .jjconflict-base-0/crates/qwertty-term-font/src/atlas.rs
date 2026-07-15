//! A texture atlas (<https://en.wikipedia.org/wiki/Texture_atlas>).
//!
//! Port of Ghostty's `src/font/Atlas.zig` (commit `2da015cd6`). The
//! implementation is based on "A Thousand Ways to Pack the Bin - A Practical
//! Approach to Two-Dimensional Rectangle Bin Packing" by Jukka Jylänki. This
//! specific implementation is based heavily on Nicolas P. Rougier's
//! freetype-gl project as well as Jukka's C++ implementation:
//! <https://github.com/juj/RectangleBinPack>.
//!
//! See `docs/analysis/font-foundations.md` for the decision to port this
//! rather than adopt `etagere`.
//!
//! Limitations that are easy to fix, but weren't needed upstream:
//!
//! * Written data must be packed, no support for custom strides.
//! * Texture is always a square, no ability to set width != height. Note
//!   that regions written INTO the atlas do not have to be square, only the
//!   full atlas texture itself.

use std::sync::atomic::{AtomicUsize, Ordering};

/// The format of the texture data being written into the [`Atlas`]. This
/// must be uniform for all textures in the Atlas. If you have some textures
/// with different formats, you must use multiple atlases or convert the
/// textures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// 1 byte per pixel grayscale.
    Grayscale,
    /// 3 bytes per pixel BGR.
    Bgr,
    /// 4 bytes per pixel BGRA.
    Bgra,
}

impl Format {
    pub fn depth(self) -> u32 {
        match self {
            Format::Grayscale => 1,
            Format::Bgr => 3,
            Format::Bgra => 4,
        }
    }
}

/// A skyline node: a horizontal segment of the free-space profile, recording
/// the topmost occupied `y` for the x-range `[x, x + width)`.
#[derive(Debug, Clone, Copy)]
struct Node {
    x: u32,
    y: u32,
    width: u32,
}

/// Errors produced by [`Atlas`] operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Atlas cannot fit the desired region. You must enlarge the atlas.
    AtlasFull,
    /// The requested growth would overflow available memory.
    OutOfMemory,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::AtlasFull => write!(f, "atlas is full"),
            Error::OutOfMemory => write!(f, "allocation failure"),
        }
    }
}

impl std::error::Error for Error {}

/// A region within the texture atlas. These can be acquired using
/// [`Atlas::reserve`]. A region reservation is required to write data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Region {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// Number of nodes to preallocate in the list on init.
const NODE_PREALLOC: usize = 64;

/// A texture atlas: owned CPU-side pixel storage plus a skyline bin-packer
/// for allocating rectangular regions within it.
///
/// `modified` bumps on every write (`set`, `set_from_larger`, `grow`,
/// `clear`); a renderer polls this to decide whether texture data must be
/// re-uploaded to the GPU. `resized` bumps only on `grow`; a renderer polls
/// this separately to decide whether the GPU texture itself must be
/// reallocated (vs. an in-place partial upload). Both counters are exposed
/// as plain `usize` reads/increments (not compare-and-swap protocols) since
/// they only need to be observed, not synchronized as a lock.
pub struct Atlas {
    /// Raw texture data, always `size * size * format.depth()` bytes.
    data: Vec<u8>,

    /// Width and height of the atlas texture. The current implementation is
    /// always square so this is both the width and the height.
    size: u32,

    /// The nodes (rectangles) of available space.
    nodes: Vec<Node>,

    /// The format of the texture data being written into the Atlas.
    format: Format,

    modified: AtomicUsize,
    resized: AtomicUsize,
}

impl Atlas {
    pub fn new(size: u32, format: Format) -> Result<Atlas, Error> {
        let byte_len = atlas_byte_len(size, format)?;

        let mut data = Vec::new();
        data.try_reserve_exact(byte_len)
            .map_err(|_| Error::OutOfMemory)?;
        data.resize(byte_len, 0);

        let mut nodes = Vec::new();
        nodes
            .try_reserve_exact(NODE_PREALLOC)
            .map_err(|_| Error::OutOfMemory)?;

        let mut result = Atlas {
            data,
            size,
            nodes,
            format,
            modified: AtomicUsize::new(0),
            resized: AtomicUsize::new(0),
        };

        // This sets up our initial state.
        result.clear();

        Ok(result)
    }

    pub fn size(&self) -> u32 {
        self.size
    }

    pub fn format(&self) -> Format {
        self.format
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn modified(&self) -> usize {
        self.modified.load(Ordering::Relaxed)
    }

    pub fn resized(&self) -> usize {
        self.resized.load(Ordering::Relaxed)
    }

    /// Reserve a region within the atlas with the given width and height.
    ///
    /// May allocate to add a new rectangle into the internal list of
    /// rectangles. This will not automatically enlarge the texture if it is
    /// full.
    pub fn reserve(&mut self, width: u32, height: u32) -> Result<Region, Error> {
        // x, y are populated within `best_idx` below.
        let mut region = Region {
            x: 0,
            y: 0,
            width,
            height,
        };

        // If our width/height are 0, then we return the region as-is. This
        // may seem like an error case but it simplifies downstream callers
        // who might be trying to write empty data.
        if width == 0 && height == 0 {
            return Ok(region);
        }

        // Find the location in our nodes list to insert the new node for
        // this region.
        let best_idx = {
            let mut best_height = u32::MAX;
            let mut best_width = best_height;
            let mut chosen: Option<usize> = None;

            for i in 0..self.nodes.len() {
                // Check if our region fits within this node.
                let y = match self.fit(i, width, height) {
                    Some(y) => y,
                    None => continue,
                };

                let node = self.nodes[i];
                if (y + height) < best_height
                    || ((y + height) == best_height && node.width > 0 && node.width < best_width)
                {
                    chosen = Some(i);
                    best_width = node.width;
                    best_height = y + height;
                    region.x = node.x;
                    region.y = y;
                }
            }

            chosen.ok_or(Error::AtlasFull)?
        };

        // Insert our new node for this rectangle at the exact best index.
        self.nodes.try_reserve(1).map_err(|_| Error::OutOfMemory)?;
        self.nodes.insert(
            best_idx,
            Node {
                x: region.x,
                y: region.y + height,
                width,
            },
        );

        // Optimize our rectangles.
        let i = best_idx + 1;
        while i < self.nodes.len() {
            let prev = self.nodes[i - 1];
            let node = &mut self.nodes[i];
            if node.x < (prev.x + prev.width) {
                let shrink = prev.x + prev.width - node.x;
                node.x += shrink;
                node.width = node.width.saturating_sub(shrink);
                if node.width == 0 {
                    self.nodes.remove(i);
                    continue;
                }
            }

            break;
        }
        self.merge();

        Ok(region)
    }

    /// Attempts to fit a rectangle of `width x height` into the node at
    /// `idx`. The return value is the `y` within the texture where the
    /// rectangle can be placed. The `x` is the same as the node.
    fn fit(&self, idx: usize, width: u32, height: u32) -> Option<u32> {
        // If the added width exceeds our texture size, it doesn't fit.
        let node = self.nodes[idx];
        if (node.x + width) > (self.size - 1) {
            return None;
        }

        // Go node by node looking for space that can fit our width.
        let mut y = node.y;
        let mut i = idx;
        let mut width_left = width;
        while width_left > 0 {
            let n = self.nodes[i];
            if n.y > y {
                y = n.y;
            }

            // If the added height exceeds our texture size, it doesn't fit.
            if (y + height) > (self.size - 1) {
                return None;
            }

            width_left = width_left.saturating_sub(n.width);
            i += 1;
        }

        Some(y)
    }

    /// Merge adjacent nodes with the same `y` value.
    fn merge(&mut self) {
        let mut i = 0;
        while i + 1 < self.nodes.len() {
            let next = self.nodes[i + 1];
            if self.nodes[i].y == next.y {
                self.nodes[i].width += next.width;
                self.nodes.remove(i + 1);
                continue;
            }

            i += 1;
        }
    }

    /// Set the data associated with a reserved region. The data is expected
    /// to fit exactly within the region. The data must be formatted with the
    /// proper bytes-per-pixel configured on init.
    pub fn set(&mut self, reg: Region, data: &[u8]) {
        assert!(reg.x < (self.size - 1));
        assert!((reg.x + reg.width) <= (self.size - 1));
        assert!(reg.y < (self.size - 1));
        assert!((reg.y + reg.height) <= (self.size - 1));

        let depth = self.format.depth();
        for i in 0..reg.height {
            let tex_offset = (((reg.y + i) * self.size + reg.x) * depth) as usize;
            let data_offset = (i * reg.width * depth) as usize;
            let len = (reg.width * depth) as usize;
            self.data[tex_offset..tex_offset + len]
                .copy_from_slice(&data[data_offset..data_offset + len]);
        }

        self.modified.fetch_add(1, Ordering::Relaxed);
    }

    /// Like [`Atlas::set`] but allows specifying a width for the source data
    /// and an offset x and y, so that a section of a larger buffer may be
    /// copied in to the atlas.
    pub fn set_from_larger(
        &mut self,
        reg: Region,
        src: &[u8],
        src_width: u32,
        src_x: u32,
        src_y: u32,
    ) {
        assert!(reg.x < (self.size - 1));
        assert!((reg.x + reg.width) <= (self.size - 1));
        assert!(reg.y < (self.size - 1));
        assert!((reg.y + reg.height) <= (self.size - 1));

        let depth = self.format.depth();
        for i in 0..reg.height {
            let tex_offset = (((reg.y + i) * self.size + reg.x) * depth) as usize;
            let src_offset = (((src_y + i) * src_width + src_x) * depth) as usize;
            let len = (reg.width * depth) as usize;
            self.data[tex_offset..tex_offset + len]
                .copy_from_slice(&src[src_offset..src_offset + len]);
        }

        self.modified.fetch_add(1, Ordering::Relaxed);
    }

    /// Grow the texture to the new size, preserving all previously written
    /// data.
    ///
    /// The only fallible step (allocating the new backing buffer) happens
    /// before any mutation of `self`, mirroring the Zig source's
    /// `errdefer comptime unreachable` marker ("infallible past the
    /// allocation point"): if this returns `Err`, `self` is guaranteed
    /// unchanged.
    pub fn grow(&mut self, size_new: u32) -> Result<(), Error> {
        assert!(size_new >= self.size);
        if size_new == self.size {
            return Ok(());
        }

        // We reserve space ahead of time for the new node, so that we won't
        // have to handle any errors after allocating our new data.
        self.nodes.try_reserve(1).map_err(|_| Error::OutOfMemory)?;

        let byte_len = atlas_byte_len(size_new, self.format)?;
        let mut data_new = Vec::new();
        data_new
            .try_reserve_exact(byte_len)
            .map_err(|_| Error::OutOfMemory)?;
        data_new.resize(byte_len, 0);

        // Everything below is infallible: `self` is only mutated once we
        // know both allocations above succeeded.
        let data_old = std::mem::replace(&mut self.data, data_new);
        let size_old = self.size;
        self.size = size_new;

        // Copy the old data over, skipping the first and last border rows
        // (we don't bother skipping the border column so we can avoid
        // strides).
        let depth = self.format.depth() as usize;
        self.set(
            Region {
                x: 0,
                y: 1,
                width: size_old,
                height: size_old - 2,
            },
            &data_old[size_old as usize * depth..],
        );

        // Add the new rectangle for our added right-hand space.
        self.nodes.push(Node {
            x: size_old - 1,
            y: 1,
            width: size_new - size_old,
        });

        // We are both modified and resized.
        self.modified.fetch_add(1, Ordering::Relaxed);
        self.resized.fetch_add(1, Ordering::Relaxed);

        Ok(())
    }

    /// Empty the atlas. This doesn't reclaim any previously allocated
    /// memory.
    pub fn clear(&mut self) {
        self.modified.fetch_add(1, Ordering::Relaxed);
        self.data.fill(0);
        self.nodes.clear();

        // Add our initial rectangle. This is the size of the full texture
        // and is the initial rectangle we fit our regions in. We keep a
        // 1px border to avoid artifacting when sampling the texture.
        self.nodes.push(Node {
            x: 1,
            y: 1,
            width: self.size - 2,
        });
    }
}

fn atlas_byte_len(size: u32, format: Format) -> Result<usize, Error> {
    (size as usize)
        .checked_mul(size as usize)
        .and_then(|v| v.checked_mul(format.depth() as usize))
        .ok_or(Error::OutOfMemory)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_fit() {
        // +2 for 1px border
        let mut atlas = Atlas::new(34, Format::Grayscale).unwrap();

        let modified = atlas.modified();
        atlas.reserve(32, 32).unwrap();
        assert_eq!(modified, atlas.modified());
        assert_eq!(atlas.reserve(1, 1), Err(Error::AtlasFull));
    }

    #[test]
    fn doesnt_fit() {
        let mut atlas = Atlas::new(32, Format::Grayscale).unwrap();

        // doesn't fit due to border
        assert_eq!(atlas.reserve(32, 32), Err(Error::AtlasFull));
    }

    #[test]
    fn fit_multiple() {
        let mut atlas = Atlas::new(32, Format::Grayscale).unwrap();

        atlas.reserve(15, 30).unwrap();
        atlas.reserve(15, 30).unwrap();
        assert_eq!(atlas.reserve(1, 1), Err(Error::AtlasFull));
    }

    #[test]
    fn writing_data() {
        let mut atlas = Atlas::new(32, Format::Grayscale).unwrap();

        let reg = atlas.reserve(2, 2).unwrap();
        let old = atlas.modified();
        atlas.set(reg, &[1, 2, 3, 4]);
        let new = atlas.modified();
        assert!(new > old);

        // 33 because of the 1px border and so on
        assert_eq!(atlas.data()[33], 1);
        assert_eq!(atlas.data()[34], 2);
        assert_eq!(atlas.data()[65], 3);
        assert_eq!(atlas.data()[66], 4);
    }

    #[test]
    fn writing_data_from_a_larger_source() {
        let mut atlas = Atlas::new(32, Format::Grayscale).unwrap();

        let reg = atlas.reserve(2, 2).unwrap();
        let old = atlas.modified();
        #[rustfmt::skip]
        atlas.set_from_larger(reg, &[
            8, 8, 8, 8, 8,
            8, 8, 1, 2, 8,
            8, 8, 3, 4, 8,
            8, 8, 8, 8, 8,
        ], 5, 2, 1);
        let new = atlas.modified();
        assert!(new > old);

        // 33 because of the 1px border and so on
        assert_eq!(atlas.data()[33], 1);
        assert_eq!(atlas.data()[34], 2);
        assert_eq!(atlas.data()[65], 3);
        assert_eq!(atlas.data()[66], 4);

        // None of the `8`s from the source data outside of the specified
        // region should have made it on to the atlas.
        assert!(!atlas.data().contains(&8));
    }

    #[test]
    fn grow() {
        // +2 for 1px border
        let mut atlas = Atlas::new(4, Format::Grayscale).unwrap();

        let reg = atlas.reserve(2, 2).unwrap();
        assert_eq!(atlas.reserve(1, 1), Err(Error::AtlasFull));

        // Write some data so we can verify that growing doesn't mess it up
        atlas.set(reg, &[1, 2, 3, 4]);
        assert_eq!(atlas.data()[5], 1);
        assert_eq!(atlas.data()[6], 2);
        assert_eq!(atlas.data()[9], 3);
        assert_eq!(atlas.data()[10], 4);

        // Expand by exactly 1 should fit our new 1x1 block.
        let old_modified = atlas.modified();
        let old_resized = atlas.resized();
        atlas.grow(atlas.size() + 1).unwrap();
        let new_modified = atlas.modified();
        let new_resized = atlas.resized();
        assert!(new_modified > old_modified);
        assert!(new_resized > old_resized);
        atlas.reserve(1, 1).unwrap();

        // Ensure our data is still set. Note the offsets change due to
        // size.
        let size = atlas.size() as usize;
        assert_eq!(atlas.data()[size + 1], 1);
        assert_eq!(atlas.data()[size + 2], 2);
        assert_eq!(atlas.data()[size * 2 + 1], 3);
        assert_eq!(atlas.data()[size * 2 + 2], 4);
    }

    #[test]
    fn writing_bgr_data() {
        let mut atlas = Atlas::new(32, Format::Bgr).unwrap();

        // This is BGR so its 3 bpp
        let reg = atlas.reserve(1, 2).unwrap();
        #[rustfmt::skip]
        atlas.set(reg, &[
            1, 2, 3,
            4, 5, 6,
        ]);

        // 33 because of the 1px border and so on
        let depth = atlas.format().depth() as usize;
        assert_eq!(atlas.data()[33 * depth], 1);
        assert_eq!(atlas.data()[33 * depth + 1], 2);
        assert_eq!(atlas.data()[33 * depth + 2], 3);
        assert_eq!(atlas.data()[65 * depth], 4);
        assert_eq!(atlas.data()[65 * depth + 1], 5);
        assert_eq!(atlas.data()[65 * depth + 2], 6);
    }

    #[test]
    fn grow_bgr() {
        // Atlas is 4x4 so its a 1px border meaning we only have 2x2
        // available.
        let mut atlas = Atlas::new(4, Format::Bgr).unwrap();

        // Get our 2x2, which should be ALL our usable space.
        let reg = atlas.reserve(2, 2).unwrap();
        assert_eq!(atlas.reserve(1, 1), Err(Error::AtlasFull));

        // This is BGR so its 3 bpp
        #[rustfmt::skip]
        atlas.set(reg, &[
            10, 11, 12, // (0, 0) (x, y) from top-left
            13, 14, 15, // (1, 0)
            20, 21, 22, // (0, 1)
            23, 24, 25, // (1, 1)
        ]);

        // Our top left skips the first row (size * depth) and the first
        // column (depth) for the 1px border.
        let depth = atlas.format().depth() as usize;
        let mut tl = (atlas.size() as usize * depth) + depth;
        assert_eq!(atlas.data()[tl], 10);
        assert_eq!(atlas.data()[tl + 1], 11);
        assert_eq!(atlas.data()[tl + 2], 12);
        assert_eq!(atlas.data()[tl + 3], 13);
        assert_eq!(atlas.data()[tl + 4], 14);
        assert_eq!(atlas.data()[tl + 5], 15);
        assert_eq!(atlas.data()[tl + 6], 0); // border

        tl += atlas.size() as usize * depth; // next row
        assert_eq!(atlas.data()[tl], 20);
        assert_eq!(atlas.data()[tl + 1], 21);
        assert_eq!(atlas.data()[tl + 2], 22);
        assert_eq!(atlas.data()[tl + 3], 23);
        assert_eq!(atlas.data()[tl + 4], 24);
        assert_eq!(atlas.data()[tl + 5], 25);
        assert_eq!(atlas.data()[tl + 6], 0); // border

        // Expand by exactly 1 should fit our new 1x1 block.
        atlas.grow(atlas.size() + 1).unwrap();

        // Data should be in same place accounting for the new size.
        let mut tl = (atlas.size() as usize * depth) + depth;
        assert_eq!(atlas.data()[tl], 10);
        assert_eq!(atlas.data()[tl + 1], 11);
        assert_eq!(atlas.data()[tl + 2], 12);
        assert_eq!(atlas.data()[tl + 3], 13);
        assert_eq!(atlas.data()[tl + 4], 14);
        assert_eq!(atlas.data()[tl + 5], 15);
        assert_eq!(atlas.data()[tl + 6], 0); // border

        tl += atlas.size() as usize * depth; // next row
        assert_eq!(atlas.data()[tl], 20);
        assert_eq!(atlas.data()[tl + 1], 21);
        assert_eq!(atlas.data()[tl + 2], 22);
        assert_eq!(atlas.data()[tl + 3], 23);
        assert_eq!(atlas.data()[tl + 4], 24);
        assert_eq!(atlas.data()[tl + 5], 25);
        assert_eq!(atlas.data()[tl + 6], 0); // border

        // Should fit the new blocks around the edges.
        atlas.reserve(1, 3).unwrap();
        atlas.reserve(2, 1).unwrap();
        assert_eq!(atlas.reserve(1, 1), Err(Error::AtlasFull));
    }

    /// Analog of the Zig `"grow OOM"` fault-injection test. Rather than a
    /// `FixedBufferAllocator` sized to the exact byte count (Zig's fault
    /// injection technique, no direct Rust ecosystem equivalent), we assert
    /// the same atomicity guarantee directly: a `grow` that would overflow
    /// `usize` (and thus fail its `try_reserve_exact`) leaves the atlas
    /// completely unchanged.
    #[test]
    fn grow_oom_leaves_atlas_unchanged() {
        // BGRA (depth 4) is used here rather than grayscale (depth 1)
        // specifically so that `u32::MAX * u32::MAX * depth` deterministically
        // overflows a 64-bit `usize` at the `checked_mul` step in
        // `atlas_byte_len` -- for depth 1, `u32::MAX * u32::MAX` alone still
        // fits in a 64-bit `usize`, so hitting OOM would require an actual
        // multi-exabyte allocation attempt instead of a deterministic
        // overflow check.
        let mut atlas = Atlas::new(4, Format::Bgra).unwrap();

        let reg = atlas.reserve(2, 2).unwrap();
        assert_eq!(atlas.reserve(1, 1), Err(Error::AtlasFull));

        // Write some data so we can verify that attempted growing doesn't
        // mess it up. 2x2 pixels at 4 bytes/pixel = 16 bytes.
        #[rustfmt::skip]
        let data = [
            1, 2, 3, 4,
            5, 6, 7, 8,
            9, 10, 11, 12,
            13, 14, 15, 16,
        ];
        atlas.set(reg, &data);
        let depth = atlas.format().depth() as usize;
        assert_eq!(atlas.data()[5 * depth], 1);
        assert_eq!(atlas.data()[5 * depth + 1], 2);

        let old_modified = atlas.modified();
        let old_resized = atlas.resized();

        // A size whose byte length (size * size * depth) overflows usize is
        // guaranteed to fail the byte-length computation deterministically,
        // before any allocation attempt (`u32::MAX * u32::MAX * 4` exceeds
        // `usize::MAX` even on 64-bit).
        assert_eq!(atlas.grow(u32::MAX), Err(Error::OutOfMemory));

        let new_modified = atlas.modified();
        let new_resized = atlas.resized();
        assert_eq!(old_modified, new_modified);
        assert_eq!(old_resized, new_resized);

        // Ensure our data is still set.
        assert_eq!(atlas.data()[5 * depth], 1);
        assert_eq!(atlas.data()[5 * depth + 1], 2);
    }

    /// Analog of the Zig `"init error"` fault-injection test: since Rust's
    /// `Atlas::new` uses `try_reserve`/`try_reserve_exact` for its
    /// allocations (rather than the infallible-by-default `Vec::with_capacity`),
    /// a size that overflows the byte-length computation deterministically
    /// exercises the same early-return-without-partial-construction path
    /// the Zig fault-injection harness targets.
    #[test]
    fn init_error_on_overflow() {
        // See `grow_oom_leaves_atlas_unchanged` for why BGRA (depth 4) is
        // needed to make `u32::MAX * u32::MAX * depth` deterministically
        // overflow `usize` rather than requiring a real huge allocation
        // attempt.
        assert!(matches!(
            Atlas::new(u32::MAX, Format::Bgra),
            Err(Error::OutOfMemory)
        ));
    }

    /// Analog of the Zig `"reserve error"` test's spirit (verifying no
    /// partial/corrupt state after a failed operation): `reserve` only
    /// mutates `self.nodes` after its `try_reserve` call succeeds, so a
    /// reservation that cannot fit (a real, reachable error in the ported
    /// API, unlike a forced allocator fault) leaves the atlas's node list
    /// unchanged.
    #[test]
    fn reserve_error_leaves_atlas_unchanged() {
        let mut atlas = Atlas::new(32, Format::Grayscale).unwrap();
        let reg = atlas.reserve(2, 2).unwrap();
        atlas.set(reg, &[1, 2, 3, 4]);

        let before = atlas.data().to_vec();
        let modified_before = atlas.modified();

        // 31x31 does not fit alongside the already-reserved 2x2 in a 32-atlas
        // usable area of 30x30; this is guaranteed AtlasFull.
        assert_eq!(atlas.reserve(31, 31), Err(Error::AtlasFull));

        assert_eq!(atlas.data(), before.as_slice());
        assert_eq!(atlas.modified(), modified_before);
    }
}
