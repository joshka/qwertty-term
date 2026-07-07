//! Typed, growable Metal buffers.
//!
//! Port of `src/renderer/metal/buffer.zig` (commit `2da015cd6`): "Metal
//! data storage for a certain set of equal types … makes it easy to
//! prealloc, shrink, grow, sync buffers with Metal." Used for
//! instance/uniform data — in practice the frozen wire structs from
//! [`crate::wire`].
//!
//! Growth semantics (the contract R4's cell engine relies on):
//!
//! - `sync` treats `data` as the buffer's complete new contents.
//! - If the data doesn't fit, the MTLBuffer is released and reallocated at
//!   **double the required byte size**; it never shrinks.
//!   (Divergence: upstream computes `size = req_bytes * 2` and then
//!   passes `size * @sizeOf(T)` to `newBufferWithLength:` — a double
//!   multiplication that over-allocates by another factor of `sizeOf(T)`.
//!   This port allocates `req_bytes * 2` bytes, the evident intent.)
//! - If the data is smaller than the buffer, remaining bytes are left
//!   untouched.
//! - After CPU writes, managed-storage buffers get `didModifyRange:` over
//!   the written range so Metal synchronizes the GPU copy (shared-storage
//!   buffers need nothing; unified memory).

use std::marker::PhantomData;
use std::ptr::NonNull;

use objc2::Message;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSRange;
use objc2_metal::{MTLBuffer, MTLDevice, MTLResourceOptions};

use super::MetalError;
use crate::gpu::GpuBuffer;

/// A typed MTLBuffer. Port of `buffer.zig Buffer(T)`.
pub struct Buffer<T> {
    /// The device, retained so the buffer can reallocate itself on growth
    /// (upstream keeps it in `opts`).
    device: Retained<ProtocolObject<dyn MTLDevice>>,
    /// The resource options this buffer was initialized with.
    resource_options: MTLResourceOptions,
    /// The underlying MTLBuffer object.
    buffer: Retained<ProtocolObject<dyn MTLBuffer>>,
    /// The allocated capacity of the buffer, in number of `T`s (not bytes).
    ///
    /// Divergence: upstream never updates its `len` field after a growth
    /// reallocation (it reads the true capacity back from the MTLBuffer's
    /// `length` property instead); this port keeps `len` accurate.
    len: usize,
    _contents: PhantomData<T>,
}

impl<T: Copy + 'static> Buffer<T> {
    /// Initialize a buffer with `len` values of `T` pre-allocated. Port of
    /// `Buffer.init`.
    pub(super) fn new(
        device: &ProtocolObject<dyn MTLDevice>,
        resource_options: MTLResourceOptions,
        len: usize,
    ) -> Result<Self, MetalError> {
        const {
            assert!(size_of::<T>() > 0, "zero-sized types have no GPU layout");
        }
        // `newBufferWithLength:0` returns nil; a zero-len buffer still
        // allocates room for one T so the object exists (upstream call
        // sites always pass len >= 1).
        let bytes = len.max(1) * size_of::<T>();
        let buffer = device
            .newBufferWithLength_options(bytes, resource_options)
            .ok_or(MetalError::MetalFailed)?;
        Ok(Self {
            device: device.retain(),
            resource_options,
            buffer,
            len,
            _contents: PhantomData,
        })
    }

    /// Initialize a buffer filled with `data`. Port of `Buffer.initFill`.
    pub(super) fn new_with_data(
        device: &ProtocolObject<dyn MTLDevice>,
        resource_options: MTLResourceOptions,
        data: &[T],
    ) -> Result<Self, MetalError> {
        if data.is_empty() {
            return Self::new(device, resource_options, 0);
        }
        // SAFETY: `data` is a live, initialized slice of `data.len() * size_of::<T>()`
        // bytes; Metal copies out of it before the call returns.
        let buffer = unsafe {
            device.newBufferWithBytes_length_options(
                NonNull::new_unchecked(data.as_ptr().cast_mut().cast()),
                size_of_val(data),
                resource_options,
            )
        }
        .ok_or(MetalError::MetalFailed)?;
        Ok(Self {
            device: device.retain(),
            resource_options,
            buffer,
            len: data.len(),
            _contents: PhantomData,
        })
    }

    /// The underlying MTLBuffer.
    pub fn buffer(&self) -> &ProtocolObject<dyn MTLBuffer> {
        &self.buffer
    }

    /// Allocated capacity in bytes (the MTLBuffer's `length` property —
    /// what upstream's growth check reads).
    pub fn capacity_bytes(&self) -> usize {
        self.buffer.length()
    }

    /// Ensure the buffer can hold `req_bytes`; on growth, release and
    /// reallocate at double the requirement (see module docs).
    fn ensure_capacity(&mut self, req_bytes: usize) -> Result<(), MetalError> {
        let avail_bytes = self.buffer.length();
        if req_bytes > avail_bytes {
            let size = req_bytes * 2;
            self.buffer = self
                .device
                .newBufferWithLength_options(size, self.resource_options)
                .ok_or(MetalError::MetalFailed)?;
            self.len = size / size_of::<T>();
        }
        Ok(())
    }

    /// If we're using the managed resource storage mode, signal Metal to
    /// synchronize the buffer data.
    ///
    /// Ref: <https://developer.apple.com/documentation/metal/synchronizing-a-managed-resource-in-macos>
    fn did_modify(&self, req_bytes: usize) {
        if self
            .resource_options
            .contains(MTLResourceOptions::StorageModeManaged)
        {
            self.buffer.didModifyRange(NSRange::new(0, req_bytes));
        }
    }

    /// Read back `count` values (test/debug seam; upstream reads via
    /// `contents()` inline). Only meaningful for shared/managed storage.
    #[cfg(test)]
    fn read(&self, count: usize) -> Vec<T> {
        assert!(count * size_of::<T>() <= self.buffer.length());
        let mut out = Vec::with_capacity(count);
        // SAFETY: contents() is valid for `length` bytes; we read at most
        // that many, into a fresh Vec.
        unsafe {
            std::ptr::copy_nonoverlapping(
                self.buffer.contents().cast::<T>().as_ptr(),
                out.as_mut_ptr(),
                count,
            );
            out.set_len(count);
        }
        out
    }
}

impl<T: Copy + 'static> GpuBuffer<T> for Buffer<T> {
    type Error = MetalError;

    fn len(&self) -> usize {
        self.len
    }

    /// Port of `Buffer.sync` (see module docs for the semantics).
    fn sync(&mut self, data: &[T]) -> Result<(), MetalError> {
        let req_bytes = size_of_val(data);
        self.ensure_capacity(req_bytes)?;

        // We fit within the buffer, so just replace bytes.
        //
        // SAFETY: `contents()` is valid for at least `req_bytes` (ensured
        // above); `data` provides exactly `req_bytes`. Raw byte copy, no
        // overlapping (CPU slice vs GPU allocation).
        unsafe {
            std::ptr::copy_nonoverlapping(
                data.as_ptr().cast::<u8>(),
                self.buffer.contents().cast::<u8>().as_ptr(),
                req_bytes,
            );
        }

        self.did_modify(req_bytes);
        Ok(())
    }

    /// Port of `Buffer.syncFromArrayLists`: gather from multiple lists
    /// (the renderer's per-row cell lists), concatenated in order. Returns
    /// the total number of items synced.
    fn sync_from_slices(&mut self, lists: &[&[T]]) -> Result<usize, MetalError> {
        let total_len: usize = lists.iter().map(|list| list.len()).sum();
        let req_bytes = total_len * size_of::<T>();
        self.ensure_capacity(req_bytes)?;

        // SAFETY: as in `sync`; `offset` stays within `req_bytes` because
        // the lists sum to exactly `total_len` items.
        unsafe {
            let dst = self.buffer.contents().cast::<u8>().as_ptr();
            let mut offset = 0usize;
            for list in lists {
                let bytes = size_of_val(*list);
                std::ptr::copy_nonoverlapping(list.as_ptr().cast::<u8>(), dst.add(offset), bytes);
                offset += bytes;
            }
        }

        self.did_modify(req_bytes);
        Ok(total_len)
    }
}

impl<T> std::fmt::Debug for Buffer<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Buffer")
            .field("len", &self.len)
            .field("type", &std::any::type_name::<T>())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_metal;
    use crate::gpu::{GpuBackend, GpuBuffer};
    use crate::wire::CellText;

    #[test]
    fn buffer_init_and_fill() {
        let Some(metal) = test_metal() else { return };

        let buffer = metal.new_buffer::<u32>(4).expect("buffer");
        assert_eq!(buffer.len(), 4);
        assert_eq!(buffer.capacity_bytes(), 16);

        let filled = metal
            .new_buffer_with_data(&[1u32, 2, 3])
            .expect("buffer with data");
        assert_eq!(filled.len(), 3);
        assert_eq!(filled.read(3), vec![1, 2, 3]);
    }

    #[test]
    fn sync_grows_at_double_required_size() {
        let Some(metal) = test_metal() else { return };

        let mut buffer = metal.new_buffer::<u32>(2).expect("buffer");
        assert_eq!(buffer.capacity_bytes(), 8);

        // 4 u32s don't fit in 8 bytes: reallocate at double the 16 required.
        buffer.sync(&[1u32, 2, 3, 4]).expect("sync");
        assert_eq!(buffer.capacity_bytes(), 32);
        assert_eq!(buffer.len(), 8);
        assert_eq!(buffer.read(4), vec![1, 2, 3, 4]);
    }

    #[test]
    fn sync_smaller_leaves_remainder_untouched_and_never_shrinks() {
        let Some(metal) = test_metal() else { return };

        let mut buffer = metal.new_buffer::<u32>(1).expect("buffer");
        buffer.sync(&[1u32, 2, 3, 4]).expect("grow");
        let grown = buffer.capacity_bytes();

        // Smaller sync: contents overwritten only up to data.len.
        buffer.sync(&[9u32]).expect("sync smaller");
        assert_eq!(buffer.capacity_bytes(), grown, "never shrinks");
        assert_eq!(buffer.read(4), vec![9, 2, 3, 4]);
    }

    #[test]
    fn sync_from_slices_concatenates_in_order() {
        let Some(metal) = test_metal() else { return };

        let mut buffer = metal.new_buffer::<u32>(1).expect("buffer");
        let n = buffer
            .sync_from_slices(&[&[1u32, 2], &[], &[3], &[4, 5, 6]])
            .expect("sync_from_slices");
        assert_eq!(n, 6);
        assert_eq!(buffer.read(6), vec![1, 2, 3, 4, 5, 6]);
    }

    /// The R4 hot path: instance buffers of wire structs.
    #[test]
    fn sync_wire_structs() {
        let Some(metal) = test_metal() else { return };

        let cells: Vec<CellText> = (0..3)
            .map(|i| CellText::new([i, 0], [255, 0, 0, 255], crate::wire::Atlas::Grayscale))
            .collect();
        let mut buffer = metal.new_buffer::<CellText>(1).expect("buffer");
        buffer.sync(&cells).expect("sync");
        assert_eq!(buffer.read(3), cells);
        // Stride sanity: capacity is a multiple of the frozen 32-byte size.
        assert_eq!(buffer.capacity_bytes() % 32, 0);
    }
}
