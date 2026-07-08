//! Typed byte offsets into page memory. Port of `src/terminal/size.zig`.
//!
//! Everything inside a page is addressed by a byte offset from the *true base*
//! of the page allocation instead of by pointer, so the entire backing memory
//! can be memcpy'd/relocated without fixups. See `docs/analysis/page-memory.md`.

use std::marker::PhantomData;

/// The maximum size of a page in bytes (`size.zig:8`). Offsets are u32.
pub const MAX_PAGE_SIZE: usize = u32::MAX as usize;

/// The int type that can contain the maximum memory offset in bytes.
pub type OffsetInt = u32;

/// Total number of cells possible in each dimension (row/col).
pub type CellCountInt = u16;

/// Total number of styles/hyperlinks possible in a page (`size.zig:24-32`).
pub type StyleCountInt = CellCountInt;
pub type HyperlinkCountInt = CellCountInt;

/// Total number of bytes for grapheme/string data (`size.zig:34-38`).
pub type GraphemeBytesInt = u32;
pub type StringBytesInt = u32;

/// The offset from the base address of the page to the start of some data,
/// typed for ease of use. Port of `size.zig` `Offset(T)`.
pub struct Offset<T> {
    offset: OffsetInt,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Copy for Offset<T> {}
impl<T> Clone for Offset<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Default for Offset<T> {
    fn default() -> Self {
        Self::new(0)
    }
}
impl<T> PartialEq for Offset<T> {
    fn eq(&self, other: &Self) -> bool {
        self.offset == other.offset
    }
}
impl<T> Eq for Offset<T> {}
impl<T> std::fmt::Debug for Offset<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Offset({})", self.offset)
    }
}

impl<T> Offset<T> {
    pub const fn new(offset: OffsetInt) -> Self {
        Self {
            offset,
            _marker: PhantomData,
        }
    }

    /// The raw byte offset.
    pub const fn get(self) -> OffsetInt {
        self.offset
    }

    /// Returns a pointer to the start of the data, properly typed.
    ///
    /// # Safety
    ///
    /// - `base` must be the true base of the allocation this offset was minted
    ///   against, and `base + offset .. base + offset + size_of::<T>()` (or the
    ///   full array this offset addresses) must be in bounds of that allocation.
    /// - `base + offset` must be aligned for `T` (asserted in debug).
    #[inline]
    pub unsafe fn ptr(self, base: *mut u8) -> *mut T {
        // SAFETY: in-bounds per the caller contract.
        let addr = unsafe { base.add(self.offset as usize) };
        debug_assert!(addr.addr() % align_of::<T>() == 0);
        addr.cast::<T>()
    }

    /// Const-pointer variant of [`Offset::ptr`].
    ///
    /// # Safety
    ///
    /// Same contract as [`Offset::ptr`] (reads only).
    #[inline]
    pub unsafe fn ptr_const(self, base: *const u8) -> *const T {
        // SAFETY: in-bounds per the caller contract.
        let addr = unsafe { base.add(self.offset as usize) };
        debug_assert!(addr.addr() % align_of::<T>() == 0);
        addr.cast::<T>()
    }
}

/// A slice of type T stored as a base offset plus a length.
/// Port of `size.zig` `Offset(T).Slice`.
pub struct OffsetSlice<T> {
    pub offset: Offset<T>,
    pub len: usize,
}

impl<T> Copy for OffsetSlice<T> {}
impl<T> Clone for OffsetSlice<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Default for OffsetSlice<T> {
    fn default() -> Self {
        Self {
            offset: Offset::default(),
            len: 0,
        }
    }
}
impl<T> PartialEq for OffsetSlice<T> {
    fn eq(&self, other: &Self) -> bool {
        self.offset == other.offset && self.len == other.len
    }
}
impl<T> std::fmt::Debug for OffsetSlice<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "OffsetSlice({}, len={})", self.offset.get(), self.len)
    }
}

impl<T> OffsetSlice<T> {
    /// Returns the slice for the data, properly typed.
    ///
    /// # Safety
    ///
    /// Same contract as [`Offset::ptr`] for `len` elements; the memory must
    /// contain `len` valid `T`s and must not be mutated for `'a`.
    #[inline]
    pub unsafe fn slice<'a>(self, base: *const u8) -> &'a [T] {
        // SAFETY: per the caller contract.
        unsafe { std::slice::from_raw_parts(self.offset.ptr_const(base), self.len) }
    }

    /// Mutable variant of [`OffsetSlice::slice`].
    ///
    /// # Safety
    ///
    /// Same contract as [`OffsetSlice::slice`], plus exclusive access to the
    /// addressed range for `'a`.
    #[inline]
    pub unsafe fn slice_mut<'a>(self, base: *mut u8) -> &'a mut [T] {
        // SAFETY: per the caller contract.
        unsafe { std::slice::from_raw_parts_mut(self.offset.ptr(base), self.len) }
    }
}

/// Get the offset for a given type from some base pointer to the actual
/// pointer of the type. Port of `size.zig` `getOffset`.
///
/// # Safety
///
/// `ptr` must point into the allocation starting at `base`, at a distance
/// representable as `u32`.
#[inline]
pub unsafe fn get_offset<T>(base: *const u8, ptr: *const T) -> Offset<T> {
    let off = ptr.addr() - base.addr();
    debug_assert!(off <= MAX_PAGE_SIZE);
    Offset::new(off as OffsetInt)
}

/// Represents a buffer that is offset from some base pointer, used while
/// laying out offset-based structures. Port of `size.zig` `OffsetBuf`.
///
/// All offsets minted through [`OffsetBuf::member`] are relative to the
/// *true base* so runtime accessors can always be passed the page base.
#[derive(Copy, Clone)]
pub struct OffsetBuf {
    /// The true base pointer of the backing memory ("byte zero").
    base: *mut u8,
    /// Offset from base where *this* structure's data begins.
    offset: usize,
}

impl OffsetBuf {
    pub fn new(base: *mut u8) -> Self {
        Self { base, offset: 0 }
    }

    pub fn base(self) -> *mut u8 {
        self.base
    }

    /// The base address for the start of the data for the user of this
    /// OffsetBuf. Anything before this is not your memory.
    ///
    /// # Safety
    ///
    /// The buffer's `offset` must be in bounds of the allocation at `base`.
    pub unsafe fn start(self) -> *mut u8 {
        // SAFETY: in bounds per the caller contract.
        unsafe { self.base.add(self.offset) }
    }

    /// Returns an Offset for some child member at `len` bytes past the start
    /// of this buffer. The offset is against the true base pointer.
    pub fn member<T>(self, len: usize) -> Offset<T> {
        Offset::new((self.offset + len) as OffsetInt)
    }

    /// Add an offset to the current offset. Port of `OffsetBuf.add`.
    #[allow(clippy::should_implement_trait)]
    pub fn add(self, offset: usize) -> Self {
        Self {
            base: self.base,
            offset: self.offset + offset,
        }
    }

    /// Rebase the buffer so `start() + offset` becomes the new true base.
    ///
    /// # Safety
    ///
    /// `offset` past `start()` must remain in bounds of the allocation.
    pub unsafe fn rebase(self, offset: usize) -> Self {
        Self {
            // SAFETY: in bounds per the caller contract.
            base: unsafe { self.start().add(offset) },
            offset: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of size.zig "Offset": if OffsetInt changes, think hard about it.
    #[test]
    fn offset_int_is_u32() {
        assert_eq!(size_of::<OffsetInt>(), size_of::<u32>());
        assert_eq!(size_of::<Offset<u8>>(), 4);
    }

    // Port of size.zig "Offset ptr u8".
    #[test]
    fn offset_ptr_u8() {
        let buf = [0u8; 64];
        let base = buf.as_ptr() as *mut u8;
        let offset: Offset<u8> = Offset::new(42);
        // SAFETY: 42 < 64, u8 has alignment 1.
        let actual = unsafe { offset.ptr(base) };
        assert_eq!(actual.addr(), base.addr() + 42);
    }

    // Port of size.zig "Offset ptr structural".
    #[test]
    fn offset_ptr_structural() {
        #[repr(C)]
        struct S {
            _x: u32,
            _y: u32,
        }
        let buf = [0u64; 32];
        let base = buf.as_ptr() as *mut u8;
        let offset: Offset<S> = Offset::new(align_of::<S>() as u32 * 4);
        // SAFETY: in bounds and aligned (base is u64-aligned).
        let actual = unsafe { offset.ptr(base) };
        assert_eq!(actual.addr(), base.addr() + offset.get() as usize);
    }

    // Port of size.zig "getOffset bytes".
    #[test]
    fn get_offset_bytes() {
        let widgets: &[u8] = b"ABCD";
        // SAFETY: &widgets[2] points into the same slice as the base.
        let offset = unsafe { get_offset(widgets.as_ptr(), &widgets[2]) };
        assert_eq!(offset.get(), 2);
    }

    // Port of size.zig "getOffset structs".
    #[test]
    fn get_offset_structs() {
        #[repr(C)]
        struct Widget {
            _x: u32,
            _y: u32,
        }
        let widgets = [
            Widget { _x: 1, _y: 2 },
            Widget { _x: 3, _y: 4 },
            Widget { _x: 5, _y: 6 },
            Widget { _x: 7, _y: 8 },
            Widget { _x: 9, _y: 10 },
        ];
        // SAFETY: &widgets[2] points into the same array as the base.
        let offset = unsafe { get_offset(widgets.as_ptr().cast::<u8>(), &widgets[2]) };
        assert_eq!(offset.get() as usize, size_of::<Widget>() * 2);
    }
}
