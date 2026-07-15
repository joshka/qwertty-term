//! Offset-based open-addressing hash map. Port of `src/terminal/hash_map.zig`.
//!
//! A fork of the Zig-stdlib `HashMapUnmanaged` (as of 0.12) tuned for the page
//! representation: it stores its key/value arrays as **offsets** rather than
//! pointers so the whole backing block can be relocated, it never grows (fixed
//! capacity, `OutOfMemory` past it), and it publishes `layout_for_capacity` so
//! the page can pack it into a shared allocation.
//!
//! Layout (a single buffer): `[Header][metadata: u8 × cap][keys][values]`. The
//! stored handle is one `Offset(Metadata)` pointing at the metadata array; the
//! `Header` sits at `metadata - size_of::<Header>()`, and the key/value offsets
//! inside the header are **relative to the metadata pointer**, not the page
//! base (`hash_map.zig:139-169, 296-308`).
//!
//! Capacity is always a power of two; `slot = hash & (cap - 1)`; each metadata
//! byte is a 7-bit fingerprint (top hash bits) plus a used bit. Removal uses
//! backward-shift deletion (Knuth vol. 3, section 6.4, algorithm R): rather
//! than leaving a tombstone, it restores the table to the state it would be in
//! had the removed key never been inserted, so probe chains stay canonical at
//! all times and a free slot is only ever the all-zero byte. A fixed-capacity
//! map cannot outgrow tombstone buildup the way an allocating map does, so
//! tombstones would require either unbounded probe lengths or periodic in-place
//! rebuilds; backward-shift avoids both by construction.
//!
//! Pointer stability: insertion never moves existing entries, but removal may
//! move *other* entries within a probe cluster. Any key or value pointer
//! previously returned by the map must be considered invalidated by any
//! removal (no caller holds an entry pointer across a removal).
//!
//! The hash comes from the [`MapKey`] trait (a stable SplitMix64 mix, standing
//! in for Zig's `autoHash`/Wyhash — see `hash.rs`). Exact hash values are
//! internal: tables are rebuilt through this same code path on both sides and
//! clones are byte copies, so no cross-implementation constraint exists.

use super::hash::MapKey;
use super::size::{Offset, OffsetBuf};

/// Error returned when the fixed-capacity map has no room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutOfMemory;

/// Capacity/size counter type. Port of `HashMapUnmanaged.Size` (`u32`).
pub type Size = u32;

/// A slot's metadata byte: a 7-bit fingerprint plus a used bit.
/// Port of `hash_map.zig` `Metadata` (`packed struct` sized 1 byte).
#[derive(Clone, Copy)]
#[repr(transparent)]
struct Metadata(u8);

impl Metadata {
    const FP_MASK: u8 = 0b0111_1111;
    const USED_BIT: u8 = 0b1000_0000;

    const FREE: u8 = 0; // fingerprint 0, used 0 — the all-zero byte

    #[inline]
    fn is_used(self) -> bool {
        self.0 & Self::USED_BIT != 0
    }

    #[inline]
    fn is_free(self) -> bool {
        // A free slot is always the all-zero byte: `fill` sets the used bit and
        // backward-shift removal zeroes the whole byte. Comparing the full byte
        // (rather than testing just the used bit) lets the optimizer fuse this
        // with the fingerprint comparison in probe loops into single-byte
        // compares. Port of `Metadata.isFree`.
        self.0 == 0
    }

    #[inline]
    fn fingerprint(self) -> u8 {
        self.0 & Self::FP_MASK
    }

    /// Top 7 bits of the hash. Port of `Metadata.takeFingerprint`.
    #[inline]
    fn take_fingerprint(hash: u64) -> u8 {
        (hash >> (64 - 7)) as u8 & Self::FP_MASK
    }

    #[inline]
    fn fill(&mut self, fp: u8) {
        self.0 = Self::USED_BIT | (fp & Self::FP_MASK);
    }
}

/// The header stored just before the metadata array. Port of `hash_map.zig`
/// `Header`. The `keys`/`values` offsets are relative to the metadata pointer.
#[repr(C)]
struct Header<K, V> {
    values: Offset<V>,
    keys: Offset<K>,
    capacity: Size,
    size: Size,
}

/// The memory layout for the backing buffer at a given capacity. Port of
/// `hash_map.zig` `Layout`.
#[derive(Debug, Clone, Copy)]
pub struct MapLayout {
    /// Total buffer size required (aligned to `base_align`).
    pub total_size: usize,
    /// Offset of the keys array, relative to the metadata pointer.
    keys_start: usize,
    /// Offset of the values array, relative to the metadata pointer.
    vals_start: usize,
    /// The (power-of-two) capacity this layout was computed for.
    capacity: Size,
}

const fn align_forward(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

/// The handle stored inside page memory: just the offset of the metadata array.
/// Port of `hash_map.zig` `OffsetHashMap` (`metadata: Offset(Metadata)`).
///
/// Reconstruct a usable [`Map`] view against the page base with [`OffsetHashMap::map`].
pub struct OffsetHashMap<K, V> {
    metadata: Offset<u8>,
    _marker: std::marker::PhantomData<fn() -> (K, V)>,
}

impl<K, V> Copy for OffsetHashMap<K, V> {}
impl<K, V> Clone for OffsetHashMap<K, V> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<K, V> Default for OffsetHashMap<K, V> {
    fn default() -> Self {
        Self {
            metadata: Offset::default(),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<K: MapKey, V: Copy> OffsetHashMap<K, V> {
    /// Required alignment for the backing buffer's true base.
    pub fn base_align() -> usize {
        align_of::<Header<K, V>>()
            .max(if size_of::<K>() == 0 {
                1
            } else {
                align_of::<K>()
            })
            .max(if size_of::<V>() == 0 {
                1
            } else {
                align_of::<V>()
            })
    }

    /// Returns the buffer layout for a given capacity (rounded to a power of
    /// two by the caller). Port of `layoutForCapacity`.
    pub fn layout(capacity: Size) -> MapLayout {
        assert!(capacity == 0 || capacity.is_power_of_two());
        let cap = capacity as usize;

        let meta_start = size_of::<Header<K, V>>();
        let meta_end = meta_start + cap * size_of::<Metadata>();
        let key_align = if size_of::<K>() == 0 {
            1
        } else {
            align_of::<K>()
        };
        let val_align = if size_of::<V>() == 0 {
            1
        } else {
            align_of::<V>()
        };
        let keys_start = align_forward(meta_end, key_align);
        let keys_end = keys_start + cap * size_of::<K>();
        let vals_start = align_forward(keys_end, val_align);
        let vals_end = vals_start + cap * size_of::<V>();
        let total_size = align_forward(vals_end, Self::base_align());

        // Offsets stored in the header are from the metadata pointer.
        MapLayout {
            total_size,
            keys_start: keys_start - meta_start,
            vals_start: vals_start - meta_start,
            capacity,
        }
    }

    /// Initialize a new map into `buf` with the given layout, zeroing metadata.
    ///
    /// # Safety
    ///
    /// `buf.start()` must be `base_align()`-aligned and point at `l.total_size`
    /// writable bytes exclusively owned by this map.
    pub unsafe fn init(buf: OffsetBuf, l: &MapLayout) -> Self {
        // The metadata array begins one Header past the buffer start. All
        // header offsets are relative to that metadata pointer.
        let meta_buf = unsafe { buf.rebase(size_of::<Header<K, V>>()) };
        let metadata: Offset<u8> = buf.member(size_of::<Header<K, V>>());

        // SAFETY: buffer valid per caller contract; header sits just before
        // the metadata array, offsets relative to metadata.
        unsafe {
            let hdr = meta_buf
                .start()
                .sub(size_of::<Header<K, V>>())
                .cast::<Header<K, V>>();
            (*hdr).capacity = l.capacity;
            (*hdr).size = 0;
            (*hdr).keys = Offset::new(l.keys_start as u32);
            (*hdr).values = Offset::new(l.vals_start as u32);

            // Zero the metadata (all-free).
            let meta_ptr = metadata.ptr(buf.base());
            std::slice::from_raw_parts_mut(meta_ptr, l.capacity as usize).fill(0);
        }

        Self {
            metadata,
            _marker: std::marker::PhantomData,
        }
    }

    /// Reconstruct a usable map view against the page base.
    ///
    /// # Safety
    ///
    /// `base` must be the true base this map was initialized against, with the
    /// map's regions valid and (for mutation) not aliased elsewhere.
    pub unsafe fn map(self, base: *mut u8) -> Map<K, V> {
        // SAFETY: metadata offset in bounds per caller contract.
        let metadata = unsafe { self.metadata.ptr(base) }.cast::<Metadata>();
        Map {
            metadata,
            _marker: std::marker::PhantomData,
        }
    }
}

/// A live, pointer-based view of an [`OffsetHashMap`] against a page base.
/// Port of `hash_map.zig` `Unmanaged`. Holds raw pointers; create with
/// [`OffsetHashMap::map`] and drop before any relocation of page memory.
pub struct Map<K, V> {
    metadata: *mut Metadata,
    _marker: std::marker::PhantomData<fn() -> (K, V)>,
}

impl<K, V> Copy for Map<K, V> {}
impl<K, V> Clone for Map<K, V> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<K: MapKey, V: Copy> Map<K, V> {
    #[inline]
    unsafe fn header(&self) -> *mut Header<K, V> {
        // SAFETY: header sits immediately before the metadata array.
        unsafe { self.metadata.cast::<Header<K, V>>().sub(1) }
    }

    #[inline]
    unsafe fn keys(&self) -> *mut K {
        // SAFETY: keys offset is relative to the metadata pointer.
        unsafe {
            let hdr = &*self.header();
            hdr.keys.ptr(self.metadata.cast::<u8>())
        }
    }

    #[inline]
    unsafe fn values(&self) -> *mut V {
        // SAFETY: values offset is relative to the metadata pointer.
        unsafe {
            let hdr = &*self.header();
            hdr.values.ptr(self.metadata.cast::<u8>())
        }
    }

    #[inline]
    pub fn capacity(&self) -> Size {
        // SAFETY: header always present for a live map.
        unsafe { (*self.header()).capacity }
    }

    #[inline]
    pub fn count(&self) -> Size {
        // SAFETY: header always present for a live map.
        unsafe { (*self.header()).size }
    }

    #[inline]
    unsafe fn meta_at(&self, idx: usize) -> *mut Metadata {
        // SAFETY: idx < capacity per caller.
        unsafe { self.metadata.add(idx) }
    }

    /// Find the index containing `key`, or `None`. Port of `getIndex`.
    unsafe fn get_index(&self, key: &K) -> Option<usize> {
        // SAFETY: live map per caller contract.
        unsafe {
            if (*self.header()).size == 0 {
                return None;
            }
            let cap = self.capacity() as usize;
            let mask = (cap - 1) as u64;
            let hash = key.hash64();
            let fp = Metadata::take_fingerprint(hash);
            let mut limit = cap;
            let mut idx = (hash & mask) as usize;

            loop {
                let m = *self.meta_at(idx);
                if m.is_free() || limit == 0 {
                    break;
                }
                if m.is_used() && m.fingerprint() == fp {
                    let test_key = &*self.keys().add(idx);
                    if *test_key == *key {
                        return Some(idx);
                    }
                }
                limit -= 1;
                idx = ((idx as u64 + 1) & mask) as usize;
            }
            None
        }
    }

    /// Get a copy of the value for `key`, if present. Port of `get`.
    ///
    /// # Safety
    ///
    /// The map's regions must be valid for reads (see [`OffsetHashMap::map`]).
    pub unsafe fn get(&self, key: &K) -> Option<V> {
        // SAFETY: per caller contract.
        unsafe { self.get_index(key).map(|idx| *self.values().add(idx)) }
    }

    /// Get a mutable pointer to the value for `key`, if present. Port of `getPtr`.
    ///
    /// # Safety
    ///
    /// Exclusive access to the map for the returned pointer's lifetime.
    pub unsafe fn get_ptr(&self, key: &K) -> Option<*mut V> {
        // SAFETY: per caller contract.
        unsafe { self.get_index(key).map(|idx| self.values().add(idx)) }
    }

    /// Get pointers to the key and value slots for `key`, if present. Port of
    /// `getEntry`.
    ///
    /// # Safety
    ///
    /// Exclusive access for the returned pointers' lifetime.
    pub unsafe fn get_entry(&self, key: &K) -> Option<Entry<K, V>> {
        // SAFETY: per caller contract.
        unsafe {
            self.get_index(key).map(|idx| Entry {
                key_ptr: self.keys().add(idx),
                value_ptr: self.values().add(idx),
            })
        }
    }

    /// Insert `key` -> `value`, asserting the key is absent and capacity
    /// exists. Port of `putAssumeCapacityNoClobber` + `putNoClobber` fused: the
    /// page code only ever inserts fresh cell-offset keys with room reserved.
    ///
    /// # Safety
    ///
    /// Exclusive access; the key must not already be present and a free slot
    /// must exist.
    pub unsafe fn put_assume_capacity_no_clobber(&mut self, key: K, value: V) {
        // SAFETY: per caller contract.
        unsafe {
            // A free slot must exist for the probe below to terminate; with
            // backward-shift deletion (no tombstones) that holds iff the map
            // is not completely full. (Side-effect-free — release-lane safe.)
            debug_assert!((*self.header()).size < self.capacity());

            let cap = self.capacity() as usize;
            let mask = (cap - 1) as u64;
            let hash = key.hash64();
            let mut idx = (hash & mask) as usize;
            while (*self.meta_at(idx)).is_used() {
                idx = ((idx as u64 + 1) & mask) as usize;
            }
            (*self.meta_at(idx)).fill(Metadata::take_fingerprint(hash));
            self.keys().add(idx).write(key);
            self.values().add(idx).write(value);
            (*self.header()).size += 1;
        }
    }

    /// True if `key` is present. Port of `contains`.
    ///
    /// # Safety
    ///
    /// The map's regions must be valid for reads.
    pub unsafe fn contains(&self, key: &K) -> bool {
        // SAFETY: per caller contract.
        unsafe { self.get_index(key).is_some() }
    }

    /// Reserve or find a slot for `key`. On a fresh slot the value is
    /// uninitialized (caller must write). Port of `getOrPutAssumeCapacity` plus
    /// the `growIfNeeded` capacity check from `getOrPutContextAdapted`.
    ///
    /// # Safety
    ///
    /// Exclusive access to the map.
    pub unsafe fn get_or_put(&mut self, key: K) -> Result<GetOrPut<V>, OutOfMemory> {
        // SAFETY: per caller contract.
        unsafe {
            // growIfNeeded(1): if full and key absent, error.
            let available = self.capacity() - (*self.header()).size;
            if available == 0 {
                if let Some(idx) = self.get_index(&key) {
                    return Ok(GetOrPut {
                        value_ptr: self.values().add(idx),
                        found_existing: true,
                    });
                }
                return Err(OutOfMemory);
            }

            let cap = self.capacity() as usize;
            let mask = (cap - 1) as u64;
            let hash = key.hash64();
            let fp = Metadata::take_fingerprint(hash);
            let mut limit = cap;
            let mut idx = (hash & mask) as usize;

            loop {
                let m = *self.meta_at(idx);
                if m.is_free() || limit == 0 {
                    break;
                }
                if m.is_used() && m.fingerprint() == fp {
                    let test_key = &*self.keys().add(idx);
                    if *test_key == key {
                        return Ok(GetOrPut {
                            value_ptr: self.values().add(idx),
                            found_existing: true,
                        });
                    }
                }
                limit -= 1;
                idx = ((idx as u64 + 1) & mask) as usize;
            }

            // The available check above guaranteed room for one new entry, and
            // backward-shift deletion leaves no tombstones, so the probe must
            // have ended at a free slot. Anything else would silently overwrite
            // a live entry. (No side effects in the assert — release-lane safe.)
            debug_assert!((*self.meta_at(idx)).is_free());

            (*self.meta_at(idx)).fill(fp);
            self.keys().add(idx).write(key);
            (*self.header()).size += 1;

            Ok(GetOrPut {
                value_ptr: self.values().add(idx),
                found_existing: false,
            })
        }
    }

    /// Insert or update `key` -> `value`. Port of `put`.
    ///
    /// # Safety
    ///
    /// Exclusive access to the map.
    pub unsafe fn put(&mut self, key: K, value: V) -> Result<(), OutOfMemory> {
        // SAFETY: per caller contract.
        unsafe {
            let gop = self.get_or_put(key)?;
            gop.value_ptr.write(value);
            Ok(())
        }
    }

    /// Insert `key` -> `value`, returning the previous value if any. Port of
    /// `fetchPut`.
    ///
    /// # Safety
    ///
    /// Exclusive access to the map.
    pub unsafe fn fetch_put(&mut self, key: K, value: V) -> Result<Option<V>, OutOfMemory> {
        // SAFETY: per caller contract.
        unsafe {
            let gop = self.get_or_put(key)?;
            let prev = if gop.found_existing {
                Some(*gop.value_ptr)
            } else {
                None
            };
            gop.value_ptr.write(value);
            Ok(prev)
        }
    }

    /// Remove the entry at `idx` using backward-shift deletion (Knuth vol. 3,
    /// section 6.4, algorithm R): rather than marking the slot with a
    /// tombstone, restore the table to the state it would be in had the removed
    /// key never been inserted. Any entry whose probe sequence passes over the
    /// hole is moved into it, which moves the hole further along the cluster,
    /// until the cluster ends at a free slot. Port of `removeByIndexContext`.
    ///
    /// # Safety
    ///
    /// `idx` must address a currently-used slot and the map must have exclusive
    /// access. Moves other entries — invalidates any outstanding key/value ptr.
    unsafe fn remove_by_index(&mut self, idx: usize) {
        // SAFETY: idx valid per caller.
        unsafe {
            let cap = self.capacity() as usize;
            let mask = cap - 1;

            // A completely full table has no free slot to terminate the scan,
            // so bound it to one full cycle. That is sufficient: the hole only
            // ever moves forward to slots the scan has already visited, so each
            // entry is considered exactly once.
            let mut hole = idx;
            let mut j = idx;
            let mut limit = cap - 1;
            while limit != 0 {
                j = (j + 1) & mask;
                if (*self.meta_at(j)).is_free() {
                    break;
                }

                // The entry at `j` may move into the hole only if the hole lies
                // on its probe path, i.e. cyclically within [home, j).
                // Otherwise the move would place it before its home slot and
                // lookups could no longer find it.
                let home = (self.keys().add(j).read().hash64() as usize) & mask;
                if (hole.wrapping_sub(home) & mask) < (j.wrapping_sub(home) & mask) {
                    *self.meta_at(hole) = *self.meta_at(j);
                    self.keys().add(hole).write(self.keys().add(j).read());
                    self.values().add(hole).write(self.values().add(j).read());
                    hole = j;
                }

                limit -= 1;
            }

            (*self.meta_at(hole)).0 = Metadata::FREE;
            (*self.header()).size -= 1;
        }
    }

    /// Remove `key`, returning true if it was present. Port of `remove`.
    ///
    /// # Safety
    ///
    /// Exclusive access to the map.
    pub unsafe fn remove(&mut self, key: &K) -> bool {
        // SAFETY: per caller contract.
        unsafe {
            if let Some(idx) = self.get_index(key) {
                self.remove_by_index(idx);
                true
            } else {
                false
            }
        }
    }

    /// Remove `key`, returning the previous value if present. Port of
    /// `fetchRemove`.
    ///
    /// # Safety
    ///
    /// Exclusive access to the map.
    pub unsafe fn fetch_remove(&mut self, key: &K) -> Option<V> {
        // SAFETY: per caller contract.
        unsafe {
            if let Some(idx) = self.get_index(key) {
                let val = *self.values().add(idx);
                self.remove_by_index(idx);
                Some(val)
            } else {
                None
            }
        }
    }

    /// Remove the entry addressed by a key pointer previously obtained from
    /// this map. Port of `removeByPtr`.
    ///
    /// # Safety
    ///
    /// `key_ptr` must be a valid key pointer into this map's keys array, and
    /// the map must have exclusive access.
    pub unsafe fn remove_by_ptr(&mut self, key_ptr: *mut K) {
        // SAFETY: per caller contract.
        unsafe {
            let idx = if size_of::<K>() > 0 {
                (key_ptr.addr() - self.keys().addr()) / size_of::<K>()
            } else {
                0
            };
            self.remove_by_index(idx);
        }
    }

    /// Reset to empty, retaining capacity. Port of `clearRetainingCapacity`.
    ///
    /// # Safety
    ///
    /// Exclusive access to the map.
    pub unsafe fn clear_retaining_capacity(&mut self) {
        // SAFETY: per caller contract.
        unsafe {
            let cap = self.capacity() as usize;
            std::ptr::write_bytes(self.metadata, 0, cap);
            (*self.header()).size = 0;
        }
    }

    /// Ensure `additional` unused slots are available. Port of
    /// `ensureUnusedCapacity`.
    ///
    /// # Safety
    ///
    /// The map's header must be valid.
    pub unsafe fn ensure_unused_capacity(&self, additional: Size) -> Result<(), OutOfMemory> {
        // SAFETY: per caller contract.
        unsafe {
            let available = self.capacity() - (*self.header()).size;
            if additional > available {
                Err(OutOfMemory)
            } else {
                Ok(())
            }
        }
    }

    /// Iterate over the used entries as (key, value) copies. Port of `iterator`.
    ///
    /// # Safety
    ///
    /// The map's regions must be valid for reads.
    pub unsafe fn iter(&self) -> MapIter<K, V> {
        MapIter {
            metadata: self.metadata,
            keys: unsafe { self.keys() },
            values: unsafe { self.values() },
            idx: 0,
            cap: self.capacity() as usize,
            size: self.count(),
        }
    }
}

/// Result of [`Map::get_or_put`]: a pointer to the slot's value plus whether
/// the key was already present.
pub struct GetOrPut<V> {
    pub value_ptr: *mut V,
    pub found_existing: bool,
}

/// Pointers to a present entry's key and value slots. Port of `hash_map.zig`
/// `Entry`.
pub struct Entry<K, V> {
    pub key_ptr: *mut K,
    pub value_ptr: *mut V,
}

/// Iterator over a map's used entries, yielding (key, value) copies.
pub struct MapIter<K, V> {
    metadata: *mut Metadata,
    keys: *mut K,
    values: *mut V,
    idx: usize,
    cap: usize,
    size: Size,
}

impl<K: Copy, V: Copy> Iterator for MapIter<K, V> {
    type Item = (K, V);

    fn next(&mut self) -> Option<(K, V)> {
        if self.size == 0 {
            return None;
        }
        while self.idx < self.cap {
            let i = self.idx;
            self.idx += 1;
            // SAFETY: i < cap; metadata/keys/values valid for the map lifetime.
            unsafe {
                if (*self.metadata.add(i)).is_used() {
                    return Some((*self.keys.add(i), *self.values.add(i)));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page::size::OffsetBuf;

    /// Backing store for a test map: an aligned byte buffer that outlives the map.
    struct TestMap<K, V> {
        _backing: Vec<u8>,
        off: OffsetHashMap<K, V>,
        base: *mut u8,
    }

    impl<K: MapKey, V: Copy> TestMap<K, V> {
        fn new(cap: Size) -> Self {
            let layout = OffsetHashMap::<K, V>::layout(cap);
            let align = OffsetHashMap::<K, V>::base_align();
            let mut backing = vec![0u8; layout.total_size + align];
            let pad = backing.as_ptr().align_offset(align);
            // SAFETY: buffer big enough; aligned base; exclusively ours.
            let base = unsafe { backing.as_mut_ptr().add(pad) };
            let off = unsafe { OffsetHashMap::<K, V>::init(OffsetBuf::new(base), &layout) };
            TestMap {
                _backing: backing,
                off,
                base,
            }
        }

        fn map(&self) -> Map<K, V> {
            // SAFETY: base is the true base this map was initialized against.
            unsafe { self.off.map(self.base) }
        }
    }

    // Port of hash_map.zig "HashMap basic usage" / "OffsetHashMap basic usage".
    #[test]
    fn basic_usage() {
        let tm = TestMap::<u32, u32>::new(16);
        let mut map = tm.map();
        let count = 5u32;
        let mut total = 0u32;
        // SAFETY: exclusive access.
        unsafe {
            for i in 0..count {
                map.put(i, i).unwrap();
                total += i;
            }
            let mut sum = 0u32;
            for (k, _v) in map.iter() {
                sum += k;
            }
            assert_eq!(sum, total);
            let mut sum2 = 0u32;
            for i in 0..count {
                assert_eq!(map.get(&i), Some(i));
                sum2 += map.get(&i).unwrap();
            }
            assert_eq!(sum2, total);
        }
    }

    // Port of "OffsetHashMap remake map": a second view sees the same data.
    #[test]
    fn remake_map() {
        let tm = TestMap::<u32, u32>::new(16);
        // SAFETY: exclusive access, sequential views.
        unsafe {
            tm.map().put(5, 5).unwrap();
            assert_eq!(tm.map().get(&5), Some(5));
        }
    }

    // Port of "HashMap put" (insert then overwrite).
    #[test]
    fn put_overwrite() {
        let tm = TestMap::<u32, u32>::new(32);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            for i in 0..16 {
                map.put(i, i).unwrap();
            }
            for i in 0..16 {
                assert_eq!(map.get(&i), Some(i));
            }
            for i in 0..16 {
                map.put(i, i * 16 + 1).unwrap();
            }
            for i in 0..16 {
                assert_eq!(map.get(&i), Some(i * 16 + 1));
            }
        }
    }

    // Port of "HashMap put full load".
    #[test]
    fn put_full_load() {
        let cap = 16u32;
        let tm = TestMap::<usize, usize>::new(cap);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            for i in 0..cap as usize {
                map.put(i, i).unwrap();
            }
            for i in 0..cap as usize {
                assert_eq!(map.get(&i), Some(i));
            }
            assert_eq!(map.put(cap as usize, cap as usize), Err(OutOfMemory));
        }
    }

    // Port of "HashMap remove".
    #[test]
    fn remove() {
        let tm = TestMap::<u32, u32>::new(32);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            for i in 0..16 {
                map.put(i, i).unwrap();
            }
            for i in 0..16 {
                if i % 3 == 0 {
                    map.remove(&i);
                }
            }
            assert_eq!(map.count(), 10);
            for (k, v) in map.iter() {
                assert_eq!(k, v);
                assert_ne!(k % 3, 0);
            }
            for i in 0..16 {
                if i % 3 == 0 {
                    assert!(!map.contains(&i));
                } else {
                    assert_eq!(map.get(&i), Some(i));
                }
            }
        }
    }

    // Port of "HashMap reverse removes".
    #[test]
    fn reverse_removes() {
        let tm = TestMap::<u32, u32>::new(32);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            for i in 0..16 {
                map.put(i, i).unwrap();
            }
            let mut i = 16u32;
            while i > 0 {
                map.remove(&(i - 1));
                assert!(!map.contains(&(i - 1)));
                for j in 0..i - 1 {
                    assert_eq!(map.get(&j), Some(j));
                }
                i -= 1;
            }
            assert_eq!(map.count(), 0);
        }
    }

    // Port of "HashMap fetchPut / fetchRemove" (basic hash map usage subset).
    #[test]
    fn fetch_put_remove() {
        let tm = TestMap::<i32, i32>::new(32);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            assert_eq!(map.fetch_put(1, 11).unwrap(), None);
            assert_eq!(map.fetch_put(1, 22).unwrap(), Some(11));
            assert_eq!(map.get(&1), Some(22));
            assert_eq!(map.fetch_remove(&1), Some(22));
            assert_eq!(map.fetch_remove(&1), None);
            assert!(!map.remove(&1));
            assert_eq!(map.get(&1), None);
        }
    }

    // Port of "HashMap clearRetainingCapacity".
    #[test]
    fn clear_retaining_capacity() {
        let tm = TestMap::<u32, u32>::new(16);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            map.put(1, 1).unwrap();
            assert_eq!(map.get(&1), Some(1));
            assert_eq!(map.count(), 1);
            let cap = map.capacity();
            map.clear_retaining_capacity();
            assert_eq!(map.count(), 0);
            assert_eq!(map.capacity(), cap);
            assert!(!map.contains(&1));
        }
    }

    // Port of "HashMap ensureUnusedCapacity with removals" (repeat put/remove
    // must not exhaust the map; backward-shift frees the slot each time).
    #[test]
    fn removal_recycling() {
        let tm = TestMap::<i32, i32>::new(32);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            for i in 0..100 {
                map.ensure_unused_capacity(1).unwrap();
                map.put(i, i).unwrap();
                map.remove(&i);
            }
            assert_eq!(map.count(), 0);
        }
    }

    /// Verify the canonical placement invariant that backward-shift deletion
    /// maintains: every used entry is reachable from its home slot without
    /// crossing a free slot. This is exactly the property lookups depend on.
    /// Port of the Zig test helper `expectCanonical`.
    ///
    /// # Safety
    ///
    /// The map's regions must be valid for reads.
    unsafe fn assert_canonical<K: MapKey, V: Copy>(map: &Map<K, V>) {
        // SAFETY: per caller contract; in-module access to the private view.
        unsafe {
            let cap = map.capacity() as usize;
            let mask = cap - 1;
            let mut used = 0usize;
            for idx in 0..cap {
                if !(*map.meta_at(idx)).is_used() {
                    continue;
                }
                used += 1;
                let home = (map.keys().add(idx).read().hash64() as usize) & mask;
                let mut probe = home;
                while probe != idx {
                    assert!(
                        (*map.meta_at(probe)).is_used(),
                        "free slot in probe chain of idx {idx} (home {home})"
                    );
                    probe = (probe + 1) & mask;
                }
            }
            assert_eq!(map.count() as usize, used);
        }
    }

    /// A key type whose hash forces every value to the same home slot, so
    /// clusters wrap around the index mask. Exercises the cyclic arithmetic in
    /// backward-shift deletion. Port of the "colliding clusters" Zig context.
    #[derive(Clone, Copy, PartialEq, Eq)]
    struct Collide(u32);
    impl MapKey for Collide {
        // Home slot `14 & mask`; fingerprint `14 >> 57 == 0` — all identical,
        // forcing full key comparisons along the probe chain.
        fn hash64(&self) -> u64 {
            14
        }
    }

    // Port of "HashMap removal keeps colliding clusters findable".
    #[test]
    fn removal_colliding_clusters_findable() {
        let tm = TestMap::<Collide, u32>::new(16);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            // Fill half the table: the cluster spans the wraparound point.
            for i in 0..8u32 {
                map.put_assume_capacity_no_clobber(Collide(i), i);
            }
            let mut removed = 0u32;
            for key in [3u32, 0, 7, 4, 1, 6, 2, 5] {
                assert!(map.remove(&Collide(key)));
                removed += 1;
                for i in 0..8u32 {
                    if map.contains(&Collide(i)) {
                        assert_eq!(map.get(&Collide(i)), Some(i));
                    }
                }
                assert_eq!(map.count(), 8 - removed);
                assert_canonical(&map);
            }
        }
    }

    // Port of "HashMap removal from a completely full table": a 100% load
    // factor allows filling every raw slot, so removal cannot rely on a free
    // slot to terminate its cluster scan.
    #[test]
    fn removal_from_full_table() {
        let cap = 64u32;
        let tm = TestMap::<u32, u32>::new(cap);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            for i in 0..cap {
                map.put_assume_capacity_no_clobber(i, i);
            }
            assert_eq!(map.count(), cap);

            let mut expected = cap;
            for i in 0..cap {
                if i % 2 != 0 {
                    continue;
                }
                assert!(map.remove(&i));
                expected -= 1;
                assert_eq!(map.count(), expected);
            }
            for i in 0..cap {
                if i % 2 == 0 {
                    assert_eq!(map.get(&i), None);
                } else {
                    assert_eq!(map.get(&i), Some(i));
                }
            }
            assert_canonical(&map);
        }
    }

    // Port of "HashMap random operations against an oracle": random hits,
    // misses and re-insertions at every load factor from empty to full,
    // compared against a std HashMap oracle plus the canonical invariant.
    #[test]
    fn random_operations_against_oracle() {
        use std::collections::HashMap;
        let cap = 64u32;
        let tm = TestMap::<u32, u32>::new(cap);
        let mut map = tm.map();
        let mut oracle: HashMap<u32, u32> = HashMap::new();

        // Deterministic splitmix64 stream (no Math.random).
        let mut state = 0xdead_beefu64;
        let mut next = move || {
            state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
            crate::page::hash::splitmix64(state)
        };
        // A small key space forces frequent hits, misses and re-insertions.
        let key_space = cap + cap / 2;

        // SAFETY: exclusive access.
        unsafe {
            for _ in 0..20_000 {
                let key = (next() as u32) % key_space;
                match next() % 4 {
                    0 | 1 => {
                        let value = next() as u32;
                        match map.put(key, value) {
                            Ok(()) => {
                                oracle.insert(key, value);
                            }
                            Err(OutOfMemory) => {
                                // Map full: put only fails on an absent key.
                                assert!(!oracle.contains_key(&key));
                                assert_eq!(map.count(), map.capacity());
                            }
                        }
                    }
                    2 => {
                        assert_eq!(oracle.remove(&key).is_some(), map.remove(&key));
                    }
                    _ => {
                        assert_eq!(oracle.get(&key).copied(), map.get(&key));
                    }
                }
                assert_eq!(oracle.len() as u32, map.count());
            }

            for (&k, &v) in &oracle {
                assert_eq!(map.get(&k), Some(v));
            }
            assert_canonical(&map);
        }
    }

    // Port of "HashMap removeByPtr".
    #[test]
    fn remove_by_ptr() {
        let tm = TestMap::<i32, u64>::new(64);
        let mut map = tm.map();
        // SAFETY: exclusive access.
        unsafe {
            for i in 0..10 {
                map.put(i, 0).unwrap();
            }
            assert_eq!(map.count(), 10);
            for i in 0..10 {
                let entry = map.get_entry(&i).unwrap();
                map.remove_by_ptr(entry.key_ptr);
            }
            assert_eq!(map.count(), 0);
        }
    }

    // Port of "HashMap put and remove loop in random order".
    #[test]
    fn put_remove_random_order() {
        let tm = TestMap::<u32, u32>::new(64);
        let mut map = tm.map();
        let mut keys: Vec<u32> = (0..32).collect();
        // Deterministic splitmix64 shuffle.
        let mut state = 0u64;
        let mut rng = move || {
            state = state.wrapping_add(0x9e3779b97f4a7c15);
            crate::page::hash::splitmix64(state)
        };
        // SAFETY: exclusive access.
        unsafe {
            for _ in 0..100 {
                // Fisher-Yates shuffle.
                for i in (1..keys.len()).rev() {
                    let j = (rng() as usize) % (i + 1);
                    keys.swap(i, j);
                }
                for &k in &keys {
                    map.put(k, k).unwrap();
                }
                assert_eq!(map.count(), 32);
                for &k in &keys {
                    map.remove(&k);
                }
                assert_eq!(map.count(), 0);
            }
        }
    }

    // Port of "layoutForCapacity no overflow for large capacity".
    #[test]
    fn layout_no_overflow_large_capacity() {
        let large_cap: Size = 1 << 30;
        let layout = OffsetHashMap::<u64, u64>::layout(large_cap);
        let min_expected = large_cap as usize * (size_of::<u64>() + size_of::<u64>());
        assert!(layout.total_size >= min_expected);
        assert!(layout.keys_start > 0);
        assert!(layout.vals_start > layout.keys_start);
    }
}
