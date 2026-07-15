//! Typed, growable OpenGL buffers.
//!
//! Port of `src/renderer/opengl/buffer.zig` (commit `2da015cd6`): "OpenGL data
//! storage for a certain set of equal types … makes it easy to prealloc,
//! shrink, grow, sync buffers with OpenGL." Used for instance/uniform data —
//! in practice the frozen wire structs from [`crate::wire`].
//!
//! Growth semantics (the contract R4's cell engine relies on), matching
//! `buffer.zig`'s `sync`/`syncFromArrayLists`:
//!
//! - `sync` treats `data` as the buffer's complete new contents.
//! - If the data doesn't fit, the store is reallocated (`glBufferData` with a
//!   null pointer) to hold **double** the required element count; it never
//!   shrinks.
//! - If the data is smaller than the buffer, the remaining bytes are left
//!   untouched (`glBufferSubData` only rewrites the prefix).
//!
//! Divergence from upstream: `buffer.zig`'s `initFill` sets `len =
//! data.len * @sizeOf(T)` (element *bytes*, an apparent off-by-`sizeOf` bug
//! that its own `sync` then compares against `data.len`). This port keeps
//! `len` as an element **count** consistently (as the Metal port does), which
//! is what the [`GpuBuffer::len`] contract documents.

use std::marker::PhantomData;
use std::rc::Rc;

use glow::HasContext;

use super::{GlError, GlState};
use crate::gpu::GpuBuffer;

/// A typed GL buffer object. Port of `buffer.zig Buffer(T)`. The underlying
/// store is bound to `GL_ARRAY_BUFFER` for uploads (upstream `bufferOptions`
/// uses `.array` / `.dynamic_draw`); its actual binding point (vertex / UBO /
/// SSBO) is chosen per draw by [`super::RenderPass`].
pub struct Buffer<T> {
    state: Rc<GlState>,
    buffer: glow::Buffer,
    /// Allocated capacity in number of `T`s (not bytes).
    len: usize,
    _contents: PhantomData<T>,
}

impl<T: Copy + 'static> Buffer<T> {
    /// Initialize a buffer with `len` values pre-allocated. Port of
    /// `Buffer.init`.
    pub(super) fn new(state: Rc<GlState>, len: usize) -> Result<Self, GlError> {
        const {
            assert!(size_of::<T>() > 0, "zero-sized types have no GPU layout");
        }
        let gl = state.gl();
        // SAFETY: create/bind/allocate on the current context; `len.max(1)`
        // avoids a zero-size store (upstream call sites pass len >= 1).
        let buffer = unsafe {
            let buffer = gl
                .create_buffer()
                .map_err(|e| GlError::GlFailed(format!("glGenBuffers: {e}")))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_size(
                glow::ARRAY_BUFFER,
                (len.max(1) * size_of::<T>()) as i32,
                glow::DYNAMIC_DRAW,
            );
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            buffer
        };
        Ok(Self {
            state,
            buffer,
            len,
            _contents: PhantomData,
        })
    }

    /// Initialize a buffer filled with `data`. Port of `Buffer.initFill`.
    pub(super) fn new_with_data(state: Rc<GlState>, data: &[T]) -> Result<Self, GlError> {
        if data.is_empty() {
            return Self::new(state, 0);
        }
        let gl = state.gl();
        // SAFETY: `data` is a live, initialized slice; we reinterpret it as its
        // exact byte span (T is `Copy` plain data) and GL copies it out.
        let buffer = unsafe {
            let buffer = gl
                .create_buffer()
                .map_err(|e| GlError::GlFailed(format!("glGenBuffers: {e}")))?;
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buffer));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(data), glow::DYNAMIC_DRAW);
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
            buffer
        };
        Ok(Self {
            state,
            buffer,
            len: data.len(),
            _contents: PhantomData,
        })
    }

    /// Ensure the store can hold `required` elements; on growth, reallocate at
    /// double the requirement (upstream `sync`/`syncFromArrayLists` growth).
    /// The `GL_ARRAY_BUFFER` binding is left active for the caller's following
    /// `buffer_sub_data`.
    ///
    /// # Safety
    /// The GL context must be current (it always is for this backend).
    unsafe fn ensure_capacity(&mut self, required: usize) {
        let gl = self.state.gl();
        unsafe {
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.buffer));
            if required > self.len {
                self.len = required * 2;
                gl.buffer_data_size(
                    glow::ARRAY_BUFFER,
                    (self.len * size_of::<T>()) as i32,
                    glow::DYNAMIC_DRAW,
                );
            }
        }
    }
}

impl<T: Copy + 'static> GpuBuffer<T> for Buffer<T> {
    type Error = GlError;
    type Handle = glow::Buffer;

    fn handle(&self) -> &glow::Buffer {
        &self.buffer
    }

    fn len(&self) -> usize {
        self.len
    }

    /// Port of `Buffer.sync` (see module docs for the growth semantics).
    fn sync(&mut self, data: &[T]) -> Result<(), GlError> {
        // SAFETY: current context; `ensure_capacity` (mutably borrowing `self`)
        // leaves the store bound and large enough for `data`'s bytes, which we
        // then upload from offset 0 via a fresh (immutable) `gl` borrow.
        unsafe {
            self.ensure_capacity(data.len());
            let gl = self.state.gl();
            gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, 0, bytes_of(data));
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        Ok(())
    }

    /// Port of `Buffer.syncFromArrayLists`: gather from multiple lists
    /// (the renderer's per-row cell lists), concatenated in order. Returns the
    /// total number of items synced.
    fn sync_from_slices(&mut self, lists: &[&[T]]) -> Result<usize, GlError> {
        let total: usize = lists.iter().map(|l| l.len()).sum();
        // SAFETY: current context; capacity covers `total` elements, and the
        // running byte offset stays within it since the lists sum to `total`.
        unsafe {
            self.ensure_capacity(total);
            let gl = self.state.gl();
            let mut offset = 0i32;
            for list in lists {
                let bytes = bytes_of(list);
                gl.buffer_sub_data_u8_slice(glow::ARRAY_BUFFER, offset, bytes);
                offset += bytes.len() as i32;
            }
            gl.bind_buffer(glow::ARRAY_BUFFER, None);
        }
        Ok(total)
    }
}

impl<T> Drop for Buffer<T> {
    fn drop(&mut self) {
        // SAFETY: current context; the buffer name is live and owned here.
        unsafe { self.state.gl().delete_buffer(self.buffer) };
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

/// Reinterpret a `Copy` slice as its raw bytes (plain-data wire structs only).
fn bytes_of<T: Copy>(data: &[T]) -> &[u8] {
    // SAFETY: `T: Copy` plain data; we expose its exact, tightly-packed byte
    // span read-only for the duration of the GL upload.
    unsafe { std::slice::from_raw_parts(data.as_ptr().cast::<u8>(), size_of_val(data)) }
}

#[cfg(test)]
mod tests {
    use super::super::test_opengl;
    use crate::gpu::{GpuBackend, GpuBuffer};
    use crate::wire::CellText;

    #[test]
    fn buffer_init_and_len() {
        let Some(gl) = test_opengl() else { return };
        let buffer = gl.new_buffer::<u32>(4).expect("buffer");
        assert_eq!(buffer.len(), 4);
        let filled = gl.new_buffer_with_data(&[1u32, 2, 3]).expect("fill");
        assert_eq!(filled.len(), 3);
    }

    #[test]
    fn sync_grows_at_double_required_and_never_shrinks() {
        let Some(gl) = test_opengl() else { return };
        let mut buffer = gl.new_buffer::<u32>(2).expect("buffer");
        buffer.sync(&[1u32, 2, 3, 4]).expect("grow");
        // 4 elements didn't fit in 2 → reallocated to double the requirement.
        assert_eq!(buffer.len(), 8);
        buffer.sync(&[9u32]).expect("smaller sync");
        assert_eq!(buffer.len(), 8, "never shrinks");
    }

    #[test]
    fn sync_from_slices_reports_total() {
        let Some(gl) = test_opengl() else { return };
        let mut buffer = gl.new_buffer::<u32>(1).expect("buffer");
        let n = buffer
            .sync_from_slices(&[&[1u32, 2], &[], &[3], &[4, 5, 6]])
            .expect("sync");
        assert_eq!(n, 6);
    }

    #[test]
    fn sync_wire_structs() {
        let Some(gl) = test_opengl() else { return };
        let cells: Vec<CellText> = (0..3)
            .map(|i| CellText::new([i, 0], [255, 0, 0, 255], crate::wire::Atlas::Grayscale))
            .collect();
        let mut buffer = gl.new_buffer::<CellText>(1).expect("buffer");
        buffer.sync(&cells).expect("sync");
        assert!(buffer.len() >= 3);
    }
}
