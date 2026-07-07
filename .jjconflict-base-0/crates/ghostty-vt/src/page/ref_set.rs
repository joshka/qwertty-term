//! Reference-counted, deduplicating set. Port of `src/terminal/ref_counted_set.zig`.
//!
//! Backing store: an open-addressed hash table (`table: Offset<Id>`, Robin Hood
//! with linear probing) mapping value hashes to item IDs, plus a flat array of
//! `items: Offset<Item>` where each `Item` holds the value, its home bucket,
//! its probe-sequence length (PSL), and a reference count.
//!
//! Used for styles and hyperlinks. ID 0 is reserved ("default"/empty); IDs
//! start at 1. Items whose ref count drops to 0 are kept ("dead",
//! resurrectable) until their bucket or ID is needed by another insert.
//!
//! Everything is offset-addressed so the backing block relocates freely. The
//! [`SetContext`] supplies `hash`/`eql`/`deleted`; for hyperlinks the context
//! carries page pointers so hashing can chase offsets, and `eql`'s first
//! argument is always the probe value and the second the resident value — this
//! is what makes cross-page (src vs dst base) lookup work.

use super::size::{Offset, OffsetBuf};

/// Behaviors for a [`RefCountedSet`]. Port of the Zig `Context` duck type.
///
/// `base` is the page base the set's items live against (the destination page
/// for hyperlinks). Any additional state (e.g. a hyperlink source page) is
/// carried inside the implementor.
pub trait SetContext<T> {
    /// Hash a value. For offset-carrying values, chase against `base`.
    fn hash(&self, base: *const u8, value: &T) -> u64;

    /// Compare two values. `a` is always a probe value (may live against a
    /// different base carried by the context) and `b` is always a resident
    /// value living against `base`.
    fn eql(&self, base: *const u8, a: &T, b: &T) -> bool;

    /// Optional deletion callback, invoked when an item is finally deleted (or
    /// when a probe value is discarded because an equal one already exists).
    /// Default is a no-op. `base` is the destination page base.
    #[allow(unused_variables)]
    fn deleted(&self, base: *mut u8, value: &T) {}

    /// Whether this context implements a meaningful [`SetContext::deleted`].
    /// Port of Zig's `@hasDecl(Context, "deleted")` — gates the callback so the
    /// no-op default isn't invoked pointlessly. Override to return `true`.
    fn has_deleted() -> bool {
        false
    }
}

/// Errors from [`RefCountedSet::add`] / [`RefCountedSet::add_with_id`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddError {
    /// No room to add a new item; remove items or grow and reinitialize.
    OutOfMemory,
    /// Many dead low IDs are inaccessible for reuse; caller should rehash.
    NeedsRehash,
}

/// Length of the PSL statistics array. A PSL of 31 is a crafted-hash defense
/// tripwire. Port of `psl_stats: [32]Id`.
const PSL_STATS_LEN: usize = 32;

/// The load factor at which the set reports full. Port of `load_factor`.
pub const LOAD_FACTOR: f64 = 0.8125;

/// Per-item metadata. Port of `Item.Metadata`.
#[derive(Clone, Copy)]
#[repr(C)]
struct Metadata<Id> {
    /// The bucket in the table where this item is referenced. `Id::MAX` means
    /// "not in the table" (the zero-value item's sentinel).
    bucket: Id,
    /// Probe-sequence length between the home bucket and `bucket`.
    psl: Id,
    /// Reference count.
    ref_count: Id,
}

/// A stored item: value plus metadata. Port of `Item`.
///
/// `T` must be `Copy` with a valid all-zero representation (the set memsets the
/// items array to defaults at init and treats zeroed slots as empty).
#[derive(Clone, Copy)]
#[repr(C)]
struct Item<T, Id> {
    value: T,
    meta: Metadata<Id>,
}

/// The memory layout of a set at a given capacity. Port of `Layout`.
#[derive(Debug, Clone, Copy)]
pub struct SetLayout {
    /// Number of item slots (one more than the storable count; ID 0 reserved).
    pub cap: usize,
    /// Table capacity (power of two).
    pub table_cap: usize,
    /// `table_cap - 1`, the probe mask.
    table_mask: usize,
    table_start: usize,
    items_start: usize,
    /// Total backing size in bytes.
    pub total_size: usize,
}

const fn align_forward(v: usize, align: usize) -> usize {
    (v + align - 1) & !(align - 1)
}

/// Trait bound for the ID integer type (u16 for styles and hyperlinks).
pub trait SetId: Copy + Ord {
    const ZERO: Self;
    const ONE: Self;
    const MAX: Self;
    fn from_usize(v: usize) -> Self;
    fn to_usize(self) -> usize;
}

impl SetId for u16 {
    const ZERO: Self = 0;
    const ONE: Self = 1;
    const MAX: Self = u16::MAX;
    #[inline]
    fn from_usize(v: usize) -> Self {
        v as u16
    }
    #[inline]
    fn to_usize(self) -> usize {
        self as usize
    }
}

/// A reference-counted deduplicating set, offset-addressed. Port of
/// `RefCountedSet(T, Id, RefCountInt, Context)`.
///
/// `RefCountInt` is fixed to `Id` here (both styles and hyperlinks use
/// `CellCountInt = u16` for both), matching upstream.
pub struct RefCountedSet<T, Id, C> {
    table: Offset<Id>,
    items: Offset<Item<T, Id>>,
    max_psl: Id,
    psl_stats: [Id; PSL_STATS_LEN],
    living: usize,
    next_id: Id,
    layout: SetLayout,
    context: C,
}

impl<T, Id, C> RefCountedSet<T, Id, C>
where
    T: Copy + Default,
    Id: SetId,
    C: SetContext<T>,
{
    /// The byte size of one stored `Item`, used by the page layout to size the
    /// hyperlink byte budget (`@sizeOf(hyperlink.Set.Item)` upstream).
    pub fn item_size() -> usize {
        size_of::<Item<T, Id>>()
    }

    /// Mutable access to the context (e.g. to rebind hyperlink page pointers
    /// before an operation). Port of the mutable-context threading in
    /// `addContext`/`lookupContext`.
    pub fn context_mut(&mut self) -> &mut C {
        &mut self.context
    }

    /// Minimum table capacity to store `n` items, accounting for the load
    /// factor and reserved ID 0. Port of `capacityForCount`.
    pub fn capacity_for_count(n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        ((n + 1) as f64 / LOAD_FACTOR).ceil() as usize
    }

    /// Required alignment for the backing buffer's true base.
    pub fn base_align() -> usize {
        align_of::<Item<T, Id>>().max(align_of::<Id>())
    }

    /// Compute the layout for a desired table capacity. Port of `Layout.init`,
    /// T-aware (item alignment known).
    pub fn layout(cap: usize) -> SetLayout {
        assert!(cap <= Id::MAX.to_usize() + 1);

        if cap == 0 {
            return SetLayout {
                cap: 0,
                table_cap: 0,
                table_mask: 0,
                table_start: 0,
                items_start: 0,
                total_size: 0,
            };
        }

        let table_cap = cap.next_power_of_two();
        let items_cap = (LOAD_FACTOR * table_cap as f64) as usize;
        let table_mask = table_cap - 1;

        let table_start = 0;
        let table_end = table_start + table_cap * size_of::<Id>();
        let items_start = align_forward(table_end, align_of::<Item<T, Id>>());
        let items_end = items_start + items_cap * size_of::<Item<T, Id>>();
        let total_size = items_end;

        SetLayout {
            cap: items_cap,
            table_cap,
            table_mask,
            table_start,
            items_start,
            total_size,
        }
    }

    /// Initialize a set into `base` with the given layout and context. Zeroes
    /// the table and default-initializes the items array. Port of `init`.
    ///
    /// # Safety
    ///
    /// `base.start()` must be `base_align()`-aligned and point at
    /// `l.total_size` writable bytes exclusively owned by this set.
    pub unsafe fn init(base: OffsetBuf, l: SetLayout, context: C) -> Self {
        let table: Offset<Id> = base.member(l.table_start);
        let items: Offset<Item<T, Id>> = base.member(l.items_start);

        // SAFETY: buffer valid per caller contract.
        unsafe {
            let table_ptr = table.ptr(base.base());
            for i in 0..l.table_cap {
                table_ptr.add(i).write(Id::ZERO);
            }
            let items_ptr = items.ptr(base.base());
            for i in 0..l.cap {
                items_ptr.add(i).write(Item {
                    value: T::default(),
                    meta: Metadata {
                        bucket: Id::MAX,
                        psl: Id::ZERO,
                        ref_count: Id::ZERO,
                    },
                });
            }
        }

        Self {
            table,
            items,
            max_psl: Id::ZERO,
            psl_stats: [Id::ZERO; PSL_STATS_LEN],
            living: 0,
            next_id: Id::ONE,
            layout: l,
            context,
        }
    }

    #[inline]
    unsafe fn table_slice<'a>(&self, base: *mut u8) -> &'a mut [Id] {
        // SAFETY: table region valid per caller contract of the calling method.
        unsafe { std::slice::from_raw_parts_mut(self.table.ptr(base), self.layout.table_cap) }
    }

    #[inline]
    unsafe fn items_slice<'a>(&self, base: *mut u8) -> &'a mut [Item<T, Id>] {
        // SAFETY: items region valid per caller contract of the calling method.
        unsafe { std::slice::from_raw_parts_mut(self.items.ptr(base), self.layout.cap) }
    }

    /// Number of non-dead items. Port of `count`.
    pub fn count(&self) -> usize {
        self.living
    }

    /// Add `value`, deduplicating and incrementing its ref count. Returns the
    /// item's ID. Port of `add`.
    ///
    /// # Safety
    ///
    /// `base` must be the true base this set was initialized against.
    pub unsafe fn add(&mut self, base: *mut u8, value: T) -> Result<Id, AddError> {
        // SAFETY: per caller contract.
        unsafe {
            // Trim dead items off the end of the ID space.
            while self.next_id > Id::ONE
                && self.items_slice(base)[self.next_id.to_usize() - 1]
                    .meta
                    .ref_count
                    == Id::ZERO
            {
                self.next_id = Id::from_usize(self.next_id.to_usize() - 1);
                self.delete_item(base, self.next_id);
            }

            // Already present: bump ref count and return.
            if let Some(id) = self.lookup(base, &value) {
                if C::has_deleted() {
                    self.context.deleted(base, &value);
                }
                self.items_slice(base)[id.to_usize()].meta.ref_count = Id::from_usize(
                    self.items_slice(base)[id.to_usize()]
                        .meta
                        .ref_count
                        .to_usize()
                        + 1,
                );
                return Ok(id);
            }

            // Crafted-hash defense: about to insert with a full PSL tail.
            if self.psl_stats[PSL_STATS_LEN - 1] > Id::ZERO {
                return Err(AddError::OutOfMemory);
            }

            // Need a fresh ID. The threshold is truncated to an integer,
            // matching upstream's `@intFromFloat` — this matters at small
            // capacities: when every allocated ID is living, a rehash
            // reclaims nothing, so we must report OutOfMemory (grow) instead
            // of NeedsRehash or the caller's rehash-retry loop never
            // terminates (e.g. cap 3 with 2 living: 2 < trunc(2.7) is false).
            if self.next_id.to_usize() >= self.layout.cap {
                let rehash_threshold = 0.9;
                if self.living < (self.layout.cap as f64 * rehash_threshold) as usize {
                    return Err(AddError::NeedsRehash);
                }
                return Err(AddError::OutOfMemory);
            }

            let id = self.insert(base, value, self.next_id);
            let idx = id.to_usize();
            self.items_slice(base)[idx].meta.ref_count =
                Id::from_usize(self.items_slice(base)[idx].meta.ref_count.to_usize() + 1);
            debug_assert!(self.items_slice(base)[idx].meta.ref_count == Id::ONE);
            self.living += 1;

            if id == self.next_id {
                self.next_id = Id::from_usize(self.next_id.to_usize() + 1);
            }
            Ok(id)
        }
    }

    /// Add `value`, reusing `id` if possible. Returns `None` if the provided
    /// ID was used, else `Some(actual_id)`. Port of `addWithId`.
    ///
    /// # Safety
    ///
    /// Same base contract as [`RefCountedSet::add`].
    pub unsafe fn add_with_id(
        &mut self,
        base: *mut u8,
        value: T,
        id: Id,
    ) -> Result<Option<Id>, AddError> {
        // SAFETY: per caller contract.
        unsafe {
            debug_assert!(id > Id::ZERO);

            if id < self.next_id {
                let idx = id.to_usize();
                let ref0 = self.items_slice(base)[idx].meta.ref_count == Id::ZERO;
                if ref0 {
                    if self.psl_stats[PSL_STATS_LEN - 1] > Id::ZERO {
                        return Err(AddError::OutOfMemory);
                    }
                    self.delete_item(base, id);
                    let added_id = self.upsert(base, value, id);
                    let aidx = added_id.to_usize();
                    self.items_slice(base)[aidx].meta.ref_count =
                        Id::from_usize(self.items_slice(base)[aidx].meta.ref_count.to_usize() + 1);
                    self.living += 1;
                    return Ok(if added_id == id { None } else { Some(added_id) });
                } else {
                    let resident = self.items_slice(base)[idx].value;
                    if self.context.eql(base, &value, &resident) {
                        if C::has_deleted() {
                            self.context.deleted(base, &value);
                        }
                        self.items_slice(base)[idx].meta.ref_count = Id::from_usize(
                            self.items_slice(base)[idx].meta.ref_count.to_usize() + 1,
                        );
                        return Ok(None);
                    }
                }
            }

            self.add(base, value).map(Some)
        }
    }

    /// Increment an item's ref count. Port of `use`.
    ///
    /// # Safety
    ///
    /// `0 < id < layout.cap`; the item's ref count must be > 0.
    pub unsafe fn use_id(&self, base: *mut u8, id: Id) {
        debug_assert!(id > Id::ZERO && id.to_usize() < self.layout.cap);
        // SAFETY: per caller contract.
        unsafe {
            let item = &mut self.items_slice(base)[id.to_usize()];
            debug_assert!(item.meta.ref_count > Id::ZERO);
            item.meta.ref_count = Id::from_usize(item.meta.ref_count.to_usize() + 1);
        }
    }

    /// Increment an item's ref count by `n`. Port of `useMultiple`.
    ///
    /// # Safety
    ///
    /// Same as [`RefCountedSet::use_id`].
    pub unsafe fn use_multiple(&self, base: *mut u8, id: Id, n: Id) {
        debug_assert!(id > Id::ZERO && id.to_usize() < self.layout.cap);
        // SAFETY: per caller contract.
        unsafe {
            let item = &mut self.items_slice(base)[id.to_usize()];
            debug_assert!(item.meta.ref_count > Id::ZERO);
            item.meta.ref_count = Id::from_usize(item.meta.ref_count.to_usize() + n.to_usize());
        }
    }

    /// Get a pointer to an item's value without changing its ref count. Port of
    /// `get`.
    ///
    /// # Safety
    ///
    /// `0 < id < layout.cap`; the item's ref count must be > 0.
    pub unsafe fn get(&self, base: *mut u8, id: Id) -> *mut T {
        debug_assert!(id > Id::ZERO && id.to_usize() < self.layout.cap);
        // SAFETY: per caller contract.
        unsafe {
            let item = &mut self.items_slice(base)[id.to_usize()];
            debug_assert!(item.meta.ref_count > Id::ZERO);
            &mut item.value as *mut T
        }
    }

    /// Get a copy of an item's ref count. Port of `refCount`.
    ///
    /// # Safety
    ///
    /// `0 < id < layout.cap`.
    pub unsafe fn ref_count(&self, base: *mut u8, id: Id) -> Id {
        debug_assert!(id > Id::ZERO && id.to_usize() < self.layout.cap);
        // SAFETY: per caller contract.
        unsafe { self.items_slice(base)[id.to_usize()].meta.ref_count }
    }

    /// Release one reference to an item. Port of `release`.
    ///
    /// # Safety
    ///
    /// `0 < id < layout.cap`; the item's ref count must be > 0.
    pub unsafe fn release(&mut self, base: *mut u8, id: Id) {
        debug_assert!(id > Id::ZERO && id.to_usize() < self.layout.cap);
        // SAFETY: per caller contract.
        unsafe {
            let item = &mut self.items_slice(base)[id.to_usize()];
            debug_assert!(item.meta.ref_count > Id::ZERO);
            item.meta.ref_count = Id::from_usize(item.meta.ref_count.to_usize() - 1);
            if item.meta.ref_count == Id::ZERO {
                self.living -= 1;
            }
        }
    }

    /// Release `n` references to an item. Port of `releaseMultiple`.
    ///
    /// # Safety
    ///
    /// `0 < id < layout.cap`; the item's ref count must be >= `n`.
    pub unsafe fn release_multiple(&mut self, base: *mut u8, id: Id, n: Id) {
        debug_assert!(id > Id::ZERO && id.to_usize() < self.layout.cap);
        // SAFETY: per caller contract.
        unsafe {
            let item = &mut self.items_slice(base)[id.to_usize()];
            debug_assert!(item.meta.ref_count >= n);
            item.meta.ref_count = Id::from_usize(item.meta.ref_count.to_usize() - n.to_usize());
            if item.meta.ref_count == Id::ZERO {
                self.living -= 1;
            }
        }
    }

    /// Find `value` in the table, returning its ID (never matches dead items).
    /// Port of `lookup`.
    ///
    /// # Safety
    ///
    /// Same base contract as [`RefCountedSet::add`].
    pub unsafe fn lookup(&self, base: *mut u8, value: &T) -> Option<Id> {
        // SAFETY: per caller contract.
        unsafe {
            let table = self.table_slice(base);
            let items = self.items_slice(base);
            let hash = self.context.hash(base, value);
            let mask = self.layout.table_mask as u64;

            for i in 0..=self.max_psl.to_usize() {
                let p = ((hash.wrapping_add(i as u64)) & mask) as usize;
                let id = table[p];
                if id == Id::ZERO {
                    return None;
                }
                let item = &items[id.to_usize()];
                if item.meta.psl.to_usize() < i {
                    return None;
                }
                if item.meta.psl.to_usize() == i
                    && item.meta.ref_count > Id::ZERO
                    && self.context.eql(base, value, &item.value)
                {
                    return Some(id);
                }
            }
            None
        }
    }

    /// Delete an item, removing it from the table and freeing its ID via
    /// backward-shift deletion. Port of `deleteItem`.
    unsafe fn delete_item(&mut self, base: *mut u8, id: Id) {
        // SAFETY: per caller contract of the calling method.
        unsafe {
            let table = self.table_slice(base);
            let items = self.items_slice(base);

            let item = items[id.to_usize()];
            if item.meta.bucket.to_usize() > self.layout.table_cap {
                return;
            }
            debug_assert!(table[item.meta.bucket.to_usize()] == id);

            if C::has_deleted() {
                self.context.deleted(base, &item.value);
            }

            self.psl_stats[item.meta.psl.to_usize()] =
                Id::from_usize(self.psl_stats[item.meta.psl.to_usize()].to_usize() - 1);
            table[item.meta.bucket.to_usize()] = Id::ZERO;
            items[id.to_usize()] = Item {
                value: T::default(),
                meta: Metadata {
                    bucket: Id::MAX,
                    psl: Id::ZERO,
                    ref_count: Id::ZERO,
                },
            };

            let mask = self.layout.table_mask;
            let mut p = item.meta.bucket.to_usize();
            let mut n = (p.wrapping_add(1)) & mask;

            while table[n] != Id::ZERO && items[table[n].to_usize()].meta.psl > Id::ZERO {
                let tn = table[n].to_usize();
                items[tn].meta.bucket = Id::from_usize(p);
                self.psl_stats[items[tn].meta.psl.to_usize()] =
                    Id::from_usize(self.psl_stats[items[tn].meta.psl.to_usize()].to_usize() - 1);
                items[tn].meta.psl = Id::from_usize(items[tn].meta.psl.to_usize() - 1);
                self.psl_stats[items[tn].meta.psl.to_usize()] =
                    Id::from_usize(self.psl_stats[items[tn].meta.psl.to_usize()].to_usize() + 1);
                table[p] = table[n];
                p = n;
                n = (p.wrapping_add(1)) & mask;
            }

            while self.max_psl > Id::ZERO && self.psl_stats[self.max_psl.to_usize()] == Id::ZERO {
                self.max_psl = Id::from_usize(self.max_psl.to_usize() - 1);
            }

            table[p] = Id::ZERO;
        }
    }

    /// Find `value` or insert it with `new_id` if absent. Port of `upsert`.
    unsafe fn upsert(&mut self, base: *mut u8, value: T, new_id: Id) -> Id {
        // SAFETY: per caller contract of the calling method.
        unsafe {
            if let Some(id) = self.lookup(base, &value) {
                if C::has_deleted() {
                    self.context.deleted(base, &value);
                }
                return id;
            }
            self.insert(base, value, new_id)
        }
    }

    /// Insert `value` with `new_id`, Robin-Hood style, possibly reusing a
    /// dead item's (smaller) ID. Returns the chosen ID. Port of `insert`.
    unsafe fn insert(&mut self, base: *mut u8, value: T, new_id: Id) -> Id {
        // SAFETY: per caller contract of the calling method.
        unsafe {
            debug_assert!(self.lookup(base, &value).is_none());

            let table = self.table_slice(base);
            let items = self.items_slice(base);

            let hash = self.context.hash(base, &value);
            let mask = self.layout.table_mask;

            // The item currently "held" as we probe. We track its fields
            // locally and only commit to `items` at the end (for new items) or
            // in place (for swapped resident items).
            let mut new_item: Item<T, Id> = Item {
                value,
                meta: Metadata {
                    bucket: Id::MAX,
                    psl: Id::ZERO,
                    ref_count: Id::ZERO,
                },
            };

            let mut held_id = new_id;
            let mut held_is_new = true; // held item is `new_item`, not in `items`
            let mut chosen_id = new_id;

            let mut i: usize = 0;
            while i < self.layout.table_cap - 1 {
                let p = ((hash.wrapping_add(i as u64)) & mask as u64) as usize;
                let id = table[p];

                // Helper closures can't borrow `items` mutably twice, so inline.
                if id == Id::ZERO {
                    table[p] = held_id;
                    if held_is_new {
                        new_item.meta.bucket = Id::from_usize(p);
                        self.psl_stats[new_item.meta.psl.to_usize()] = Id::from_usize(
                            self.psl_stats[new_item.meta.psl.to_usize()].to_usize() + 1,
                        );
                        self.max_psl = self.max_psl.max(new_item.meta.psl);
                    } else {
                        items[held_id.to_usize()].meta.bucket = Id::from_usize(p);
                        let hp = items[held_id.to_usize()].meta.psl.to_usize();
                        self.psl_stats[hp] = Id::from_usize(self.psl_stats[hp].to_usize() + 1);
                        self.max_psl = self.max_psl.max(items[held_id.to_usize()].meta.psl);
                    }
                    break;
                }

                if items[id.to_usize()].meta.ref_count == Id::ZERO {
                    // Reap the dead item, reuse its bucket for our held item.
                    if C::has_deleted() {
                        let dead_val = items[id.to_usize()].value;
                        self.context.deleted(base, &dead_val);
                    }
                    let dead_psl = items[id.to_usize()].meta.psl.to_usize();
                    self.psl_stats[dead_psl] =
                        Id::from_usize(self.psl_stats[dead_psl].to_usize() - 1);
                    items[id.to_usize()] = Item {
                        value: T::default(),
                        meta: Metadata {
                            bucket: Id::MAX,
                            psl: Id::ZERO,
                            ref_count: Id::ZERO,
                        },
                    };

                    if id < new_id {
                        chosen_id = id;
                    }

                    table[p] = held_id;
                    if held_is_new {
                        new_item.meta.bucket = Id::from_usize(p);
                        self.psl_stats[new_item.meta.psl.to_usize()] = Id::from_usize(
                            self.psl_stats[new_item.meta.psl.to_usize()].to_usize() + 1,
                        );
                        self.max_psl = self.max_psl.max(new_item.meta.psl);
                    } else {
                        items[held_id.to_usize()].meta.bucket = Id::from_usize(p);
                        let hp = items[held_id.to_usize()].meta.psl.to_usize();
                        self.psl_stats[hp] = Id::from_usize(self.psl_stats[hp].to_usize() + 1);
                        self.max_psl = self.max_psl.max(items[held_id.to_usize()].meta.psl);
                    }
                    break;
                }

                // Robin Hood: swap if resident is "richer" (lower PSL, or equal
                // PSL and lower ref count).
                let held_psl = if held_is_new {
                    new_item.meta.psl
                } else {
                    items[held_id.to_usize()].meta.psl
                };
                let held_ref = if held_is_new {
                    new_item.meta.ref_count
                } else {
                    items[held_id.to_usize()].meta.ref_count
                };
                let resident_psl = items[id.to_usize()].meta.psl;
                let resident_ref = items[id.to_usize()].meta.ref_count;

                if resident_psl < held_psl || (resident_psl == held_psl && resident_ref < held_ref)
                {
                    // Place held item in the bucket.
                    table[p] = held_id;
                    if held_is_new {
                        new_item.meta.bucket = Id::from_usize(p);
                        self.psl_stats[new_item.meta.psl.to_usize()] = Id::from_usize(
                            self.psl_stats[new_item.meta.psl.to_usize()].to_usize() + 1,
                        );
                        self.max_psl = self.max_psl.max(new_item.meta.psl);
                    } else {
                        items[held_id.to_usize()].meta.bucket = Id::from_usize(p);
                        let hp = items[held_id.to_usize()].meta.psl.to_usize();
                        self.psl_stats[hp] = Id::from_usize(self.psl_stats[hp].to_usize() + 1);
                        self.max_psl = self.max_psl.max(items[held_id.to_usize()].meta.psl);
                    }

                    // Pick up the resident item (now the held item).
                    held_id = id;
                    held_is_new = false;
                    let rp = items[id.to_usize()].meta.psl.to_usize();
                    self.psl_stats[rp] = Id::from_usize(self.psl_stats[rp].to_usize() - 1);
                }

                // Advance our held item's PSL.
                if held_is_new {
                    new_item.meta.psl = Id::from_usize(new_item.meta.psl.to_usize() + 1);
                } else {
                    items[held_id.to_usize()].meta.psl =
                        Id::from_usize(items[held_id.to_usize()].meta.psl.to_usize() + 1);
                }

                i += 1;
            }

            // Commit the new item's chosen bucket/ID.
            table[new_item.meta.bucket.to_usize()] = chosen_id;
            items[chosen_id.to_usize()] = new_item;

            chosen_id
        }
    }
}
