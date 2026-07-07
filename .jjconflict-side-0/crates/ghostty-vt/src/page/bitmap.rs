//! Offset-based bitmap chunk allocator. Port of `src/terminal/bitmap_allocator.zig`.
//!
//! A relatively naive bitmap allocator that uses memory offsets against a
//! fixed backing buffer so that the backing buffer can be moved without
//! updating pointers. `CHUNK` is the minimum distributed unit of memory in
//! bytes and must be a power of two.
//!
//! Layout deviation from upstream: ghostty sizes the chunk slab as
//! `aligned_cap * chunk_size` (`bitmap_allocator.zig:222`), which multiplies a
//! byte count by the chunk size — over-reserving by ~chunk_size for large
//! capacities and (for capacities under 64 chunks) leaving the bitmap
//! advertising chunks beyond a correctly-sized slab. This port sizes the slab
//! as `aligned_chunk_count * CHUNK` so the slab exactly covers every bit in
//! the bitmap: identical allocator behavior (the bitmap governs all
//! decisions), strictly safe bounds. See `docs/analysis/page-memory.md`.

use super::size::{Offset, OffsetBuf, OffsetSlice};

/// Error returned when the backing buffer has no room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfMemory;

const fn align_forward(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

#[derive(Debug, Clone, Copy)]
pub struct BitmapLayout {
    pub total_size: usize,
    pub bitmap_count: usize,
    pub bitmap_start: usize,
    pub chunks_start: usize,
}

/// Bitmap allocator over `CHUNK`-byte chunks. One bit per chunk; 1 = free.
#[derive(Debug, Clone, Copy)]
pub struct BitmapAllocator<const CHUNK: usize> {
    /// The bitmap of available chunks (1 = free).
    bitmap: Offset<u64>,
    bitmap_count: usize,
    /// The contiguous buffer of chunks.
    chunks: Offset<u8>,
}

impl<const CHUNK: usize> BitmapAllocator<CHUNK> {
    pub const BASE_ALIGN: usize = align_of::<u64>();
    pub const BITMAP_BIT_SIZE: usize = 64;

    const _POWER_OF_TWO: () = assert!(CHUNK.is_power_of_two());

    /// Get the layout for the given capacity in bytes. The capacity is
    /// rounded up to the nearest chunk size and bitmap size so everything is
    /// perfectly divisible.
    pub fn layout(cap: usize) -> BitmapLayout {
        let aligned_cap = align_forward(cap, CHUNK);
        // 1 bitmap word per 64 chunks; round the chunk count up so bitmaps
        // are always full words.
        let chunk_count = aligned_cap / CHUNK;
        let aligned_chunk_count = align_forward(chunk_count, 64);
        let bitmap_count = aligned_chunk_count / 64;

        let bitmap_start = 0;
        let bitmap_end = size_of::<u64>() * bitmap_count;
        let chunks_start = bitmap_end;
        // Deviation from upstream: exact bitmap coverage (see module docs).
        let chunks_end = chunks_start + aligned_chunk_count * CHUNK;

        BitmapLayout {
            total_size: chunks_end,
            bitmap_count,
            bitmap_start,
            chunks_start,
        }
    }

    /// Returns the number of bytes required to allocate `n` elements of type
    /// `T`, accounting for chunk-size alignment.
    pub fn bytes_required<T>(n: usize) -> usize {
        align_forward(size_of::<T>() * n, CHUNK)
    }

    /// Initialize the allocator with the given buffer and layout, marking all
    /// chunks free.
    ///
    /// # Safety
    ///
    /// `buf.start()` must be `BASE_ALIGN`-aligned and point at least
    /// `l.total_size` writable bytes that are exclusively owned by this
    /// allocator (no other structure may address them).
    pub unsafe fn init(buf: OffsetBuf, l: &BitmapLayout) -> Self {
        // SAFETY: buffer valid per the caller contract.
        unsafe {
            debug_assert!(buf.start().addr().is_multiple_of(Self::BASE_ALIGN));
            let bitmap: Offset<u64> = buf.member(l.bitmap_start);
            // Initialize bitmaps to all 1s: all chunks free.
            let bitmap_ptr = bitmap.ptr(buf.base());
            std::slice::from_raw_parts_mut(bitmap_ptr, l.bitmap_count).fill(u64::MAX);

            Self {
                bitmap,
                bitmap_count: l.bitmap_count,
                chunks: buf.member(l.chunks_start),
            }
        }
    }

    /// Allocate `n` elements of type `T`, returning the offset slice.
    ///
    /// A zero-length request returns an empty slice without touching the
    /// bitmap (upstream asserts `n > 0`; the page code never frees or
    /// dereferences empty slices' storage, so this is a safe extension used
    /// for empty hyperlink URIs/IDs).
    ///
    /// # Safety
    ///
    /// `base` must be the true base this allocator was initialized against,
    /// with its bitmap/chunk regions valid for reads and writes and not
    /// concurrently accessed.
    pub unsafe fn alloc<T>(
        &mut self,
        base: *mut u8,
        n: usize,
    ) -> Result<OffsetSlice<T>, OutOfMemory> {
        // Alignment is not handled generally; all page types divide CHUNK.
        debug_assert!(CHUNK.is_multiple_of(align_of::<T>()));
        if n == 0 {
            return Ok(OffsetSlice::default());
        }

        let byte_count = size_of::<T>().checked_mul(n).ok_or(OutOfMemory)?;
        let chunk_count = byte_count.div_ceil(CHUNK);

        // SAFETY: bitmap region valid per the caller contract.
        let bitmaps =
            unsafe { std::slice::from_raw_parts_mut(self.bitmap.ptr(base), self.bitmap_count) };
        let idx = find_free_chunks(bitmaps, chunk_count).ok_or(OutOfMemory)?;

        Ok(OffsetSlice {
            offset: Offset::new(self.chunks.get() + (idx * CHUNK) as u32),
            len: n,
        })
    }

    /// Free a previously allocated slice.
    ///
    /// # Safety
    ///
    /// Same base contract as [`BitmapAllocator::alloc`]; `slice` must be
    /// exactly a value previously returned by `alloc` on this allocator (same
    /// offset and length) that has not already been freed.
    pub unsafe fn free<T>(&mut self, base: *mut u8, slice: OffsetSlice<T>) {
        if slice.len == 0 {
            return;
        }

        let bytes = size_of::<T>() * slice.len;
        let aligned_len = align_forward(bytes, CHUNK);
        let chunk_count = aligned_len / CHUNK;
        let chunk_idx = (slice.offset.get() - self.chunks.get()) as usize / CHUNK;

        // SAFETY: bitmap region valid per the caller contract.
        let bitmaps =
            unsafe { std::slice::from_raw_parts_mut(self.bitmap.ptr(base), self.bitmap_count) };

        // Current bitmap word and number of chunks left to mark free.
        let mut i = chunk_idx / 64;
        let mut rem = chunk_count;

        // Mark bits in the starting bitmap.
        {
            let bit = chunk_idx % 64;
            let bits = rem.min(64 - bit);
            bitmaps[i] |= (u64::MAX >> (64 - bits as u32)) << bit as u32;
            rem -= bits;
        }

        // Mark any full bitmap words.
        i += 1;
        while rem > 64 {
            bitmaps[i] = u64::MAX;
            rem -= 64;
            i += 1;
        }

        // Mark bits at the start of the last word.
        if rem > 0 {
            bitmaps[i] |= u64::MAX >> (64 - rem as u32);
        }
    }

    /// Total capacity in bytes.
    pub fn capacity_bytes(&self) -> usize {
        self.bitmap_count * Self::BITMAP_BIT_SIZE * CHUNK
    }

    /// Number of bytes currently in use.
    ///
    /// # Safety
    ///
    /// Same base contract as [`BitmapAllocator::alloc`] (reads only).
    pub unsafe fn used_bytes(&self, base: *const u8) -> usize {
        // SAFETY: bitmap region valid per the caller contract.
        let bitmaps =
            unsafe { std::slice::from_raw_parts(self.bitmap.ptr_const(base), self.bitmap_count) };
        let free_chunks: usize = bitmaps.iter().map(|b| b.count_ones() as usize).sum();
        let total_chunks = self.bitmap_count * Self::BITMAP_BIT_SIZE;
        (total_chunks - free_chunks) * CHUNK
    }

    /// For testing only: whether every chunk of `slice` is marked allocated.
    ///
    /// # Safety
    ///
    /// Same contract as [`BitmapAllocator::free`] minus the not-freed rule.
    #[cfg(test)]
    unsafe fn is_allocated<T>(&self, base: *const u8, slice: OffsetSlice<T>) -> bool {
        let bytes = size_of::<T>() * slice.len;
        let aligned_len = align_forward(bytes, CHUNK);
        let chunk_count = aligned_len / CHUNK;
        let chunk_idx = (slice.offset.get() - self.chunks.get()) as usize / CHUNK;

        // SAFETY: bitmap region valid per the caller contract.
        let bitmaps =
            unsafe { std::slice::from_raw_parts(self.bitmap.ptr_const(base), self.bitmap_count) };
        for i in chunk_idx..chunk_idx + chunk_count {
            if bitmaps[i / 64] & (1u64 << (i % 64)) != 0 {
                return false;
            }
        }
        true
    }
}

/// Find `n` sequential free chunks in the given bitmaps and return the index
/// of the first chunk, marking them used. Port of `bitmap_allocator.zig:238`.
fn find_free_chunks(bitmaps: &mut [u64], n: usize) -> Option<usize> {
    // Large runs (> 64 chunks) require special handling.
    if n > 64 {
        let mut i = 0;
        'search: while i < bitmaps.len() {
            // Number of chunks available at the end of this bitmap word.
            let prefix = (!bitmaps[i]).leading_zeros() as usize;
            if prefix == 0 {
                i += 1;
                continue;
            }

            let start_bitmap = i;
            let start_bit = 64 - prefix;

            // Remaining sequential free chunks we need to find.
            let mut rem = n - prefix;

            i += 1;
            while rem > 64 {
                // Ran out of bitmaps; no sufficiently large gap.
                if i >= bitmaps.len() {
                    return None;
                }
                // Word has content: retry starting at this word.
                if bitmaps[i] != u64::MAX {
                    continue 'search;
                }
                rem -= 64;
                i += 1;
            }

            // Bounds guard not present upstream (which would index out of
            // bounds when the run ends exactly at the last word boundary);
            // "no space" is the correct answer.
            if i >= bitmaps.len() {
                return None;
            }

            // Not enough free chunks at the start of this word: retry here.
            if ((!bitmaps[i]).trailing_zeros() as usize) < rem {
                continue 'search;
            }

            let suffix = (n - prefix) % 64;

            // Found! Mark everything between start and end as used.
            bitmaps[start_bitmap] ^= (u64::MAX >> start_bit as u32) << start_bit as u32;
            let full_bitmaps = (n - prefix - suffix) / 64;
            for bitmap in &mut bitmaps[start_bitmap + 1..][..full_bitmaps] {
                *bitmap = 0;
            }
            if suffix > 0 {
                bitmaps[i] ^= u64::MAX >> (64 - suffix as u32);
            }

            return Some(start_bitmap * 64 + start_bit);
        }

        return None;
    }

    debug_assert!((1..=64).contains(&n));
    for (idx, bitmap) in bitmaps.iter_mut().enumerate() {
        // Shift-and the bitmap against itself to find `n` sequential 1s.
        let mut shifted: u64 = *bitmap;
        for i in 1..n {
            shifted &= *bitmap >> i as u32;
        }
        if shifted == 0 {
            continue;
        }

        // Trailing zeros = first bit index with at least n sequential 1s.
        let bit = shifted.trailing_zeros();

        // Mark as used.
        let mask = (u64::MAX >> (64 - n as u32)) << bit;
        *bitmap ^= mask;

        return Some(idx * 64 + bit as usize);
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aligned_buf(len: usize) -> Vec<u64> {
        vec![0u64; len.div_ceil(8)]
    }

    // Port of "findFreeChunks single found".
    #[test]
    fn find_free_chunks_single_found() {
        let mut bitmaps =
            [0b10000000_00000000_00000000_00000000_00000000_00000000_00001110_00000000u64];
        let idx = find_free_chunks(&mut bitmaps, 2).unwrap();
        assert_eq!(idx, 9);
        assert_eq!(
            bitmaps[0],
            0b10000000_00000000_00000000_00000000_00000000_00000000_00001000_00000000
        );
    }

    // Port of "findFreeChunks single not found".
    #[test]
    fn find_free_chunks_single_not_found() {
        let mut bitmaps =
            [0b10000111_00000000_00000000_00000000_00000000_00000000_00000000_00000000u64];
        assert_eq!(find_free_chunks(&mut bitmaps, 4), None);
    }

    // Port of "findFreeChunks multiple found".
    #[test]
    fn find_free_chunks_multiple_found() {
        let mut bitmaps = [
            0b10000111_00000000_00000000_00000000_00000000_00000000_00000000_01110000u64,
            0b10000000_00111110_00000000_00000000_00000000_00000000_00111110_00000000u64,
        ];
        let idx = find_free_chunks(&mut bitmaps, 4).unwrap();
        assert_eq!(idx, 73);
        assert_eq!(
            bitmaps[1],
            0b10000000_00111110_00000000_00000000_00000000_00000000_00100000_00000000
        );
    }

    // Port of "findFreeChunks exactly 64 chunks".
    #[test]
    fn find_free_chunks_exactly_64() {
        let mut bitmaps = [u64::MAX];
        let idx = find_free_chunks(&mut bitmaps, 64).unwrap();
        assert_eq!(bitmaps[0], 0);
        assert_eq!(idx, 0);
    }

    // Port of "findFreeChunks larger than 64 chunks".
    #[test]
    fn find_free_chunks_larger_than_64() {
        let mut bitmaps = [u64::MAX, u64::MAX];
        let idx = find_free_chunks(&mut bitmaps, 65).unwrap();
        assert_eq!(bitmaps[0], 0);
        assert_eq!(
            bitmaps[1],
            0b11111111_11111111_11111111_11111111_11111111_11111111_11111111_11111110
        );
        assert_eq!(idx, 0);
    }

    // Port of "findFreeChunks larger than 64 chunks not at beginning".
    #[test]
    fn find_free_chunks_larger_than_64_not_at_beginning() {
        let mut bitmaps = [
            0b11111111_00000000_00000000_00000000_00000000_00000000_00000000_00000000u64,
            u64::MAX,
            u64::MAX,
        ];
        let idx = find_free_chunks(&mut bitmaps, 65).unwrap();
        assert_eq!(bitmaps[0], 0);
        assert_eq!(
            bitmaps[1],
            0b11111110_00000000_00000000_00000000_00000000_00000000_00000000_00000000
        );
        assert_eq!(bitmaps[2], u64::MAX);
        assert_eq!(idx, 56);
    }

    // Port of "findFreeChunks larger than 64 chunks exact".
    #[test]
    fn find_free_chunks_larger_than_64_exact() {
        let mut bitmaps = [u64::MAX, u64::MAX];
        let idx = find_free_chunks(&mut bitmaps, 128).unwrap();
        assert_eq!(bitmaps[0], 0);
        assert_eq!(bitmaps[1], 0);
        assert_eq!(idx, 0);
    }

    // Port of "BitmapAllocator layout".
    #[test]
    fn layout() {
        let layout = BitmapAllocator::<4>::layout(64 * 4);
        assert_eq!(layout.bitmap_count, 1);
    }

    // Test helper: init an allocator over a fresh buffer.
    fn with_alloc<const CHUNK: usize>(
        cap: usize,
        f: impl FnOnce(&mut BitmapAllocator<CHUNK>, *mut u8),
    ) {
        let layout = BitmapAllocator::<CHUNK>::layout(cap);
        let mut backing = aligned_buf(layout.total_size);
        let base = backing.as_mut_ptr().cast::<u8>();
        // SAFETY: backing is u64-aligned and total_size long; exclusively ours.
        let mut bm = unsafe { BitmapAllocator::<CHUNK>::init(OffsetBuf::new(base), &layout) };
        f(&mut bm, base);
    }

    // Port of "BitmapAllocator alloc sequentially".
    #[test]
    fn alloc_sequentially() {
        with_alloc::<4>(64, |bm, base| unsafe {
            let ptr = bm.alloc::<u8>(base, 1).unwrap();
            ptr.slice_mut(base)[0] = b'A';

            let ptr2 = bm.alloc::<u8>(base, 1).unwrap();
            assert_ne!(ptr.offset, ptr2.offset);
            // Should grab the next chunk.
            assert_eq!(ptr.offset.get() + 4, ptr2.offset.get());

            // Free ptr and the next allocation should be back.
            bm.free(base, ptr);
            let ptr3 = bm.alloc::<u8>(base, 1).unwrap();
            assert_eq!(ptr.offset, ptr3.offset);
        });
    }

    // Port of "BitmapAllocator alloc non-byte" (u21 -> u32).
    #[test]
    fn alloc_non_byte() {
        with_alloc::<4>(128, |bm, base| unsafe {
            let ptr = bm.alloc::<u32>(base, 1).unwrap();
            ptr.slice_mut(base)[0] = b'A' as u32;

            let ptr2 = bm.alloc::<u32>(base, 1).unwrap();
            assert_ne!(ptr.offset, ptr2.offset);
            assert_eq!(ptr.offset.get() + 4, ptr2.offset.get());

            bm.free(base, ptr);
            let ptr3 = bm.alloc::<u32>(base, 1).unwrap();
            assert_eq!(ptr.offset, ptr3.offset);
        });
    }

    // Port of "BitmapAllocator alloc non-byte multi-chunk".
    #[test]
    fn alloc_non_byte_multi_chunk() {
        with_alloc::<16>(128, |bm, base| unsafe {
            let ptr = bm.alloc::<u32>(base, 6).unwrap();
            assert_eq!(ptr.len, 6);
            for v in ptr.slice_mut(base) {
                *v = b'A' as u32;
            }

            let ptr2 = bm.alloc::<u32>(base, 1).unwrap();
            assert_ne!(ptr.offset, ptr2.offset);
            assert_eq!(ptr.offset.get() + (4 * 4 * 2), ptr2.offset.get());

            bm.free(base, ptr);
            let ptr3 = bm.alloc::<u32>(base, 1).unwrap();
            assert_eq!(ptr.offset, ptr3.offset);
        });
    }

    // Port of "BitmapAllocator alloc large".
    #[test]
    fn alloc_large() {
        with_alloc::<2>(256, |bm, base| unsafe {
            let ptr = bm.alloc::<u8>(base, 129).unwrap();
            ptr.slice_mut(base)[0] = b'A';
            bm.free(base, ptr);
        });
    }

    fn bitmaps<'a, const CHUNK: usize>(bm: &BitmapAllocator<CHUNK>, base: *mut u8) -> &'a [u64] {
        // SAFETY: test-only view of the bitmap region.
        unsafe { std::slice::from_raw_parts(bm.bitmap.ptr_const(base), bm.bitmap_count) }
    }

    // Port of "BitmapAllocator alloc and free one bitmap".
    #[test]
    fn alloc_and_free_one_bitmap() {
        with_alloc::<1>(64 * 3, |bm, base| unsafe {
            let slice = bm.alloc::<u8>(base, 64).unwrap();
            assert_eq!(slice.len, 64);
            slice.slice_mut(base).fill(0x11);
            assert!(slice.slice(base).iter().all(|&b| b == 0x11));

            assert!(bm.is_allocated(base, slice));
            bm.free(base, slice);
            assert!(!bm.is_allocated(base, slice));

            assert_eq!(bitmaps(bm, base), &[u64::MAX; 3]);
        });
    }

    // Port of "BitmapAllocator alloc and free half bitmap".
    #[test]
    fn alloc_and_free_half_bitmap() {
        with_alloc::<1>(64 * 3, |bm, base| unsafe {
            let slice = bm.alloc::<u8>(base, 32).unwrap();
            assert_eq!(slice.len, 32);
            slice.slice_mut(base).fill(0x11);

            assert!(bm.is_allocated(base, slice));
            bm.free(base, slice);
            assert!(!bm.is_allocated(base, slice));

            assert_eq!(bitmaps(bm, base), &[u64::MAX; 3]);
        });
    }

    // Port of "BitmapAllocator alloc and free two half bitmaps".
    #[test]
    fn alloc_and_free_two_half_bitmaps() {
        with_alloc::<1>(64 * 3, |bm, base| unsafe {
            let slice = bm.alloc::<u8>(base, 32).unwrap();
            slice.slice_mut(base).fill(0x11);

            let slice2 = bm.alloc::<u8>(base, 32).unwrap();
            slice2.slice_mut(base).fill(0x22);
            assert!(slice2.slice(base).iter().all(|&b| b == 0x22));
            assert!(slice.slice(base).iter().all(|&b| b == 0x11));

            assert!(bm.is_allocated(base, slice2));
            bm.free(base, slice2);
            assert!(!bm.is_allocated(base, slice2));
            assert!(bm.is_allocated(base, slice));
            bm.free(base, slice);
            assert!(!bm.is_allocated(base, slice));

            assert_eq!(bitmaps(bm, base), &[u64::MAX; 3]);
        });
    }

    // Port of "BitmapAllocator alloc and free 1.5 bitmaps".
    #[test]
    fn alloc_and_free_1_5_bitmaps() {
        with_alloc::<1>(64 * 3, |bm, base| unsafe {
            let slice = bm.alloc::<u8>(base, 96).unwrap();
            assert_eq!(slice.len, 96);
            slice.slice_mut(base).fill(0x11);

            assert!(bm.is_allocated(base, slice));
            bm.free(base, slice);
            assert!(!bm.is_allocated(base, slice));

            assert_eq!(bitmaps(bm, base), &[u64::MAX; 3]);
        });
    }

    // Port of "BitmapAllocator alloc and free two 1.5 bitmaps".
    #[test]
    fn alloc_and_free_two_1_5_bitmaps() {
        with_alloc::<1>(64 * 3, |bm, base| unsafe {
            let slice = bm.alloc::<u8>(base, 96).unwrap();
            slice.slice_mut(base).fill(0x11);
            let slice2 = bm.alloc::<u8>(base, 96).unwrap();
            slice2.slice_mut(base).fill(0x22);
            assert!(slice2.slice(base).iter().all(|&b| b == 0x22));
            assert!(slice.slice(base).iter().all(|&b| b == 0x11));

            assert!(bm.is_allocated(base, slice2));
            bm.free(base, slice2);
            assert!(!bm.is_allocated(base, slice2));
            assert!(bm.is_allocated(base, slice));
            bm.free(base, slice);
            assert!(!bm.is_allocated(base, slice));

            assert_eq!(bitmaps(bm, base), &[u64::MAX; 3]);
        });
    }

    // Port of "BitmapAllocator alloc and free 1.5 bitmaps offset by 0.75".
    #[test]
    fn alloc_and_free_1_5_bitmaps_offset_0_75() {
        with_alloc::<1>(64 * 3, |bm, base| unsafe {
            let slice = bm.alloc::<u8>(base, 48).unwrap();
            slice.slice_mut(base).fill(0x11);

            // 1.5-bitmap allocation spanning 0.75..2.25 (3 different words).
            let slice2 = bm.alloc::<u8>(base, 96).unwrap();
            slice2.slice_mut(base).fill(0x22);
            assert!(slice2.slice(base).iter().all(|&b| b == 0x22));
            assert!(slice.slice(base).iter().all(|&b| b == 0x11));

            assert!(bm.is_allocated(base, slice2));
            bm.free(base, slice2);
            assert!(!bm.is_allocated(base, slice2));
            assert!(bm.is_allocated(base, slice));
            bm.free(base, slice);
            assert!(!bm.is_allocated(base, slice));

            assert_eq!(bitmaps(bm, base), &[u64::MAX; 3]);
        });
    }

    // Port of "BitmapAllocator alloc and free three 0.75 bitmaps".
    #[test]
    fn alloc_and_free_three_0_75_bitmaps() {
        with_alloc::<1>(64 * 3, |bm, base| unsafe {
            let slice = bm.alloc::<u8>(base, 48).unwrap();
            slice.slice_mut(base).fill(0x11);
            let slice2 = bm.alloc::<u8>(base, 48).unwrap();
            slice2.slice_mut(base).fill(0x22);
            let slice3 = bm.alloc::<u8>(base, 48).unwrap();
            slice3.slice_mut(base).fill(0x33);
            assert!(slice3.slice(base).iter().all(|&b| b == 0x33));
            assert!(slice2.slice(base).iter().all(|&b| b == 0x22));
            assert!(slice.slice(base).iter().all(|&b| b == 0x11));

            assert!(bm.is_allocated(base, slice2));
            bm.free(base, slice2);
            assert!(!bm.is_allocated(base, slice2));
            assert!(bm.is_allocated(base, slice));
            bm.free(base, slice);
            assert!(!bm.is_allocated(base, slice));
            assert!(bm.is_allocated(base, slice3));
            bm.free(base, slice3);
            assert!(!bm.is_allocated(base, slice3));

            assert_eq!(bitmaps(bm, base), &[u64::MAX; 3]);
        });
    }

    // Port of "BitmapAllocator alloc and free two 1.5 bitmaps offset 0.75".
    #[test]
    fn alloc_and_free_two_1_5_bitmaps_offset_0_75() {
        with_alloc::<1>(64 * 4, |bm, base| unsafe {
            let slice = bm.alloc::<u8>(base, 48).unwrap();
            slice.slice_mut(base).fill(0x11);
            let slice2 = bm.alloc::<u8>(base, 96).unwrap();
            slice2.slice_mut(base).fill(0x22);
            let slice3 = bm.alloc::<u8>(base, 96).unwrap();
            slice3.slice_mut(base).fill(0x33);
            assert!(slice3.slice(base).iter().all(|&b| b == 0x33));
            assert!(slice2.slice(base).iter().all(|&b| b == 0x22));
            assert!(slice.slice(base).iter().all(|&b| b == 0x11));

            assert!(bm.is_allocated(base, slice2));
            bm.free(base, slice2);
            assert!(!bm.is_allocated(base, slice2));
            assert!(bm.is_allocated(base, slice));
            bm.free(base, slice);
            assert!(!bm.is_allocated(base, slice));
            assert!(bm.is_allocated(base, slice3));
            bm.free(base, slice3);
            assert!(!bm.is_allocated(base, slice3));

            assert_eq!(bitmaps(bm, base), &[u64::MAX; 4]);
        });
    }

    // Port of "BitmapAllocator bytesRequired".
    #[test]
    fn bytes_required() {
        // Chunk size of 16 bytes (like grapheme_chunk in page).
        {
            type A = BitmapAllocator<16>;
            assert_eq!(A::bytes_required::<u8>(1), 16);
            assert_eq!(A::bytes_required::<u8>(16), 16);
            assert_eq!(A::bytes_required::<u8>(17), 32);
            // u21 -> u32 (4 bytes each)
            assert_eq!(A::bytes_required::<u32>(1), 16);
            assert_eq!(A::bytes_required::<u32>(4), 16);
            assert_eq!(A::bytes_required::<u32>(5), 32);
            assert_eq!(A::bytes_required::<u32>(6), 32);
        }
        // Chunk size of 4 bytes.
        {
            type A = BitmapAllocator<4>;
            assert_eq!(A::bytes_required::<u8>(1), 4);
            assert_eq!(A::bytes_required::<u8>(4), 4);
            assert_eq!(A::bytes_required::<u8>(5), 8);
            assert_eq!(A::bytes_required::<u32>(1), 4);
            assert_eq!(A::bytes_required::<u32>(2), 8);
        }
        // Chunk size of 32 bytes (like string_chunk in page).
        {
            type A = BitmapAllocator<32>;
            assert_eq!(A::bytes_required::<u8>(1), 32);
            assert_eq!(A::bytes_required::<u8>(32), 32);
            assert_eq!(A::bytes_required::<u8>(33), 64);
        }
    }

    // Targeted addition (beyond the Zig suite): random alloc/free churn with
    // a shadow model, exercising fragmentation and multi-word runs.
    #[test]
    fn alloc_free_churn() {
        with_alloc::<4>(64 * 4 * 4, |bm, base| {
            // Simple deterministic PRNG (splitmix64).
            let mut state = 0x12345678u64;
            let mut rng = move || {
                state = state.wrapping_add(0x9e3779b97f4a7c15);
                crate::page::hash::splitmix64(state)
            };

            let mut live: Vec<(OffsetSlice<u8>, u8)> = Vec::new();
            for round in 0..2000u64 {
                let r = rng();
                if r % 3 != 0 || live.is_empty() {
                    let n = (r % 100 + 1) as usize;
                    // SAFETY: base valid for this allocator.
                    match unsafe { bm.alloc::<u8>(base, n) } {
                        Ok(slice) => {
                            let tag = (round % 251) as u8;
                            // No live allocation may overlap the new one.
                            let start = slice.offset.get();
                            let end = start + slice.len as u32;
                            for (other, _) in &live {
                                let ostart = other.offset.get();
                                let oend = ostart
                                    + BitmapAllocator::<4>::bytes_required::<u8>(other.len) as u32;
                                assert!(end <= ostart || oend <= start);
                            }
                            // SAFETY: freshly allocated, in bounds.
                            unsafe { slice.slice_mut(base).fill(tag) };
                            live.push((slice, tag));
                        }
                        Err(OutOfMemory) => {
                            // Free half of the live set and continue.
                            for _ in 0..live.len() / 2 + 1 {
                                let (slice, tag) = live.swap_remove((rng() as usize) % live.len());
                                // SAFETY: previously allocated, unfreed.
                                unsafe {
                                    assert!(slice.slice(base).iter().all(|&b| b == tag));
                                    bm.free(base, slice);
                                }
                            }
                        }
                    }
                } else {
                    let (slice, tag) = live.swap_remove((rng() as usize) % live.len());
                    // SAFETY: previously allocated, unfreed.
                    unsafe {
                        assert!(slice.slice(base).iter().all(|&b| b == tag));
                        bm.free(base, slice);
                    }
                }
            }
            // Drain and verify the allocator returns to fully free.
            for (slice, tag) in live.drain(..) {
                // SAFETY: previously allocated, unfreed.
                unsafe {
                    assert!(slice.slice(base).iter().all(|&b| b == tag));
                    bm.free(base, slice);
                }
            }
            // SAFETY: base valid.
            assert_eq!(unsafe { bm.used_bytes(base) }, 0);
        });
    }
}
