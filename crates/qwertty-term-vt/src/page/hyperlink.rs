//! Hyperlink data model. Port of the data-model parts of `src/terminal/hyperlink.zig`.
//!
//! Only the in-page representation is ported here (the `Hyperlink` heap form,
//! the offset-based [`PageEntry`], the cell->id [`Map`] type, and the
//! ref-counted [`HyperlinkSet`]). The screen/OSC8 plumbing that *produces*
//! hyperlinks lives in later chunks.
//!
//! A [`PageEntry`]'s strings (URI, explicit ID) live in the owning page's
//! string allocator; hashing/eql chase those offsets against a page base, and
//! the set context carries page pointers so a probe value's strings (in a
//! source page) can be compared against resident values (in the destination
//! page). This is what makes cross-page dedup work during `clone_from`.

use super::bitmap::BitmapAllocator;
use super::hash::hash_bytes;
use super::offset_map::OffsetHashMap;
use super::ref_set::{RefCountedSet, SetContext};
use super::size::{HyperlinkCountInt, Offset, OffsetInt, OffsetSlice, get_offset};
use super::{Cell, OutOfMemory};

/// The page's string allocator type (32-byte chunks). Kept in sync with
/// `page_impl::StringAlloc`. Declared here so the hyperlink context can free
/// strings through a pointer to the *field* (disjoint from the hyperlink set),
/// avoiding a self-aliasing `*mut Page` that Stacked Borrows rejects.
pub(crate) type StringAlloc = BitmapAllocator<32>;

/// The unique identifier for a hyperlink. Port of `hyperlink.zig` `Id`.
pub type Id = HyperlinkCountInt;

/// The cell-offset -> hyperlink-id map. Port of `hyperlink.zig` `Map`.
pub type Map = OffsetHashMap<Offset<Cell>, Id>;

/// A hyperlink ID as stored in a [`PageEntry`]. Port of `PageEntry.Id`.
#[derive(Clone, Copy)]
pub enum EntryId {
    /// An explicit OSC8 `id=` string, stored in the page's string allocator.
    Explicit(OffsetSlice<u8>),
    /// An auto-generated implicit ID (a counter value).
    Implicit(OffsetInt),
}

/// An owned, page-independent identity for a cell's hyperlink: the [`EntryId`]
/// resolved to plain bytes plus the URI. Two cells belong to the *same*
/// hyperlink iff their `LinkKey`s are equal — mirroring [`PageEntry::eql`]
/// (implicit compared by counter, explicit by id string, both plus the URI),
/// but comparable across pages without juggling page bases. Produced by
/// `Page::hyperlink_key`; carried through the snapshot so a renderer can match
/// the cells of a hovered link (R7).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkKey {
    /// An implicit link: the per-page counter value + the URI bytes.
    Implicit(OffsetInt, Vec<u8>),
    /// An explicit link: the `id=` string bytes + the URI bytes.
    Explicit(Vec<u8>, Vec<u8>),
}

/// A hyperlink committed to page memory. Port of `hyperlink.zig` `PageEntry`.
///
/// The strings (`uri`, explicit `id`) are offset slices into the owning page's
/// string allocator.
#[derive(Clone, Copy)]
pub struct PageEntry {
    pub id: EntryId,
    pub uri: OffsetSlice<u8>,
}

impl Default for PageEntry {
    fn default() -> Self {
        Self {
            id: EntryId::Implicit(0),
            uri: OffsetSlice::default(),
        }
    }
}

impl PageEntry {
    /// Hash the entry, chasing string offsets against `base`. Port of
    /// `PageEntry.hash`.
    ///
    /// # Safety
    ///
    /// `base` must be the page base whose string allocator holds this entry's
    /// strings.
    pub unsafe fn hash(&self, base: *const u8) -> u64 {
        // Fold the id tag + id payload + uri bytes through the byte hasher.
        // Exact value is internal; must be stable within one implementation.
        // SAFETY: string offsets valid against base per caller contract.
        unsafe {
            let mut h: u64 = 0;
            match self.id {
                EntryId::Implicit(v) => {
                    h = hash_bytes(h ^ 0x01, &v.to_le_bytes());
                }
                EntryId::Explicit(slice) => {
                    h = hash_bytes(h ^ 0x02, slice.slice(base));
                }
            }
            hash_bytes(h, self.uri.slice(base))
        }
    }

    /// Compare two entries; `self` lives against `self_base`, `other` against
    /// `other_base`. Port of `PageEntry.eql`.
    ///
    /// # Safety
    ///
    /// Each entry's string offsets must be valid against its respective base.
    pub unsafe fn eql(
        &self,
        self_base: *const u8,
        other: &PageEntry,
        other_base: *const u8,
    ) -> bool {
        // SAFETY: string offsets valid against their bases per caller contract.
        unsafe {
            match (self.id, other.id) {
                (EntryId::Implicit(a), EntryId::Implicit(b)) => {
                    if a != b {
                        return false;
                    }
                }
                (EntryId::Explicit(a), EntryId::Explicit(b)) => {
                    if a.slice(self_base) != b.slice(other_base) {
                        return false;
                    }
                }
                _ => return false,
            }
            self.uri.slice(self_base) == other.uri.slice(other_base)
        }
    }

    /// Duplicate this entry's strings from `src_base` into `dst_base`, using
    /// `dst_alloc` (the destination page's string allocator). Port of
    /// `PageEntry.dupe`. A shallow copy is returned if the bases are equal.
    ///
    /// # Safety
    ///
    /// Bases and allocator must be valid; `self`'s strings live at `src_base`,
    /// and `dst_alloc` owns `dst_base`'s string region.
    pub unsafe fn dupe(
        &self,
        src_base: *const u8,
        dst_base: *mut u8,
        dst_alloc: *mut StringAlloc,
    ) -> Result<PageEntry, OutOfMemory> {
        // SAFETY: bases/alloc valid per caller contract.
        unsafe {
            let mut copy = *self;
            if std::ptr::eq(src_base, dst_base) {
                return Ok(copy);
            }

            // Copy the URI.
            let uri = self.uri.slice(src_base);
            let buf = (*dst_alloc).alloc::<u8>(dst_base, uri.len())?;
            buf.slice_mut(dst_base).copy_from_slice(uri);
            copy.uri = OffsetSlice {
                offset: get_offset(dst_base, &buf.slice(dst_base)[0]),
                len: uri.len(),
            };

            // Copy the explicit ID string, if any.
            if let EntryId::Explicit(slice) = self.id {
                let id = slice.slice(src_base);
                let idbuf = (*dst_alloc).alloc::<u8>(dst_base, id.len())?;
                idbuf.slice_mut(dst_base).copy_from_slice(id);
                copy.id = EntryId::Explicit(OffsetSlice {
                    offset: get_offset(dst_base, &idbuf.slice(dst_base)[0]),
                    len: id.len(),
                });
            }

            Ok(copy)
        }
    }

    /// Free this entry's strings from the page's string allocator. Port of
    /// `PageEntry.free`.
    ///
    /// # Safety
    ///
    /// `alloc` owns the string region at `base`, and this entry's strings live
    /// there.
    pub unsafe fn free(&self, base: *mut u8, alloc: *mut StringAlloc) {
        // SAFETY: alloc owns the strings per caller contract.
        unsafe {
            if let EntryId::Explicit(slice) = self.id {
                (*alloc).free(base, slice);
            }
            (*alloc).free(base, self.uri);
        }
    }
}

/// The [`SetContext`] for hyperlinks. Port of `hyperlink.zig` `Set`'s context.
///
/// Rather than a self-aliasing `*mut Page` (which Stacked Borrows rejects when
/// the set is a field of that same page), the context holds the disjoint raw
/// pieces the callbacks need: the destination page's memory base and string
/// allocator (which own resident values' strings) and the source memory base
/// for probe values. These are pointers to the page's *backing buffer* and its
/// `string_alloc` *field* — neither aliases the hyperlink set being borrowed.
pub struct HyperlinkContext {
    /// Destination page memory (owns resident strings).
    pub dst_base: *mut u8,
    /// Destination string allocator (a pointer to the page's field).
    pub dst_alloc: *mut StringAlloc,
    /// Source page memory for probe values (equals `dst_base` for same-page).
    pub src_base: *const u8,
}

impl HyperlinkContext {
    /// A null context, replaced before each set operation via the page's
    /// `bind_hyperlink_ctx`.
    pub fn null() -> Self {
        Self {
            dst_base: std::ptr::null_mut(),
            dst_alloc: std::ptr::null_mut(),
            src_base: std::ptr::null(),
        }
    }
}

impl SetContext<PageEntry> for HyperlinkContext {
    fn hash(&self, _base: *const u8, value: &PageEntry) -> u64 {
        // Probe values live against `src_base`.
        // SAFETY: src_base valid per the binding contract.
        unsafe { value.hash(self.src_base) }
    }

    fn eql(&self, _base: *const u8, a: &PageEntry, b: &PageEntry) -> bool {
        // `a` is the probe (src_base), `b` is resident (dst_base).
        // SAFETY: offsets valid against their respective bases.
        unsafe { a.eql(self.src_base, b, self.dst_base) }
    }

    fn deleted(&self, _base: *mut u8, value: &PageEntry) {
        // SAFETY: resident strings live in the destination allocator.
        unsafe { value.free(self.dst_base, self.dst_alloc) }
    }

    fn has_deleted() -> bool {
        true
    }
}

/// The ref-counted set of hyperlinks. Port of `hyperlink.zig` `Set`.
pub type HyperlinkSet = RefCountedSet<PageEntry, Id, HyperlinkContext>;
