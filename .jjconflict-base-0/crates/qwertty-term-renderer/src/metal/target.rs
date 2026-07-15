//! A render target: an IOSurface-backed MTLTexture.
//!
//! Port of `src/renderer/metal/Target.zig` (commit `2da015cd6`). The
//! IOSurface is what later gets assigned to a plain `CALayer`'s `contents`
//! for presentation (upstream `IOSurfaceLayer.zig`, chunk R2) — rendering
//! goes through the Metal texture view of the same pixels, presentation
//! through the surface, no copy.

use objc2::runtime::ProtocolObject;
use objc2_core_foundation::{CFDictionary, CFNumber, CFRetained, CFString};
use objc2_core_graphics::{CGColorSpace, kCGColorSpaceDisplayP3};
use objc2_io_surface::{
    IOSurfaceLockOptions, IOSurfaceRef, kIOSurfaceBytesPerElement, kIOSurfaceColorSpace,
    kIOSurfaceHeight, kIOSurfacePixelFormat, kIOSurfaceWidth,
};
use objc2_metal::{
    MTLDevice, MTLPixelFormat, MTLResourceOptions, MTLStorageMode, MTLTexture,
    MTLTextureDescriptor, MTLTextureUsage,
};

use super::{Metal, MetalError};
use crate::gpu::GpuTarget;

/// The `32BGRA` IOSurface pixel format: the fourcc `'BGRA'`
/// (`kCVPixelFormatType_32BGRA`).
const PIXEL_FORMAT_32BGRA: u32 = u32::from_be_bytes(*b"BGRA");

/// Options for initializing a [`Target`]. Port of `Target.Options`.
pub(super) struct Options<'a> {
    /// MTLDevice.
    pub device: &'a ProtocolObject<dyn MTLDevice>,
    /// Desired width/height in pixels.
    pub width: usize,
    pub height: usize,
    /// Pixel format for the MTLTexture view.
    pub pixel_format: MTLPixelFormat,
    /// Storage mode for the MTLTexture.
    pub storage_mode: MTLStorageMode,
}

/// An IOSurface-backed render target.
pub struct Target {
    /// The underlying IOSurface.
    surface: CFRetained<IOSurfaceRef>,
    /// The MTLTexture view over the surface's pixels.
    texture: objc2::rc::Retained<ProtocolObject<dyn MTLTexture>>,
    /// Current dimensions of this target.
    width: usize,
    height: usize,
}

impl Target {
    /// Port of `Target.init`.
    pub(super) fn new(opts: &Options<'_>) -> Result<Self, MetalError> {
        let surface = new_iosurface(opts.width, opts.height)?;

        // Create our texture descriptor. The texture is a render target
        // whose storage *is* the IOSurface (plane 0).
        let desc = MTLTextureDescriptor::new();
        unsafe {
            desc.setWidth(opts.width);
            desc.setHeight(opts.height);
        }
        desc.setPixelFormat(opts.pixel_format);
        desc.setUsage(MTLTextureUsage::RenderTarget);
        desc.setResourceOptions(
            // CPU writes but never reads this resource.
            MTLResourceOptions::CPUCacheModeWriteCombined
                | Metal::storage_mode_options(opts.storage_mode),
        );

        let texture = opts
            .device
            .newTextureWithDescriptor_iosurface_plane(&desc, &surface, 0)
            .ok_or(MetalError::MetalFailed)?;

        Ok(Self {
            surface,
            texture,
            width: opts.width,
            height: opts.height,
        })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// The IOSurface backing this target (presentation side, R2).
    pub fn surface(&self) -> &IOSurfaceRef {
        &self.surface
    }

    /// The Metal texture view of this target (rendering side).
    pub fn texture(&self) -> &ProtocolObject<dyn MTLTexture> {
        &self.texture
    }

    /// Read the target's pixels back through the IOSurface base address:
    /// `height` rows of `width * 4` BGRA bytes (row padding stripped).
    ///
    /// This is the offscreen-readback seam the M3 verification strategy
    /// relies on ("render into the IOSurface target, read pixels, assert" —
    /// `docs/plans/m3-first-pixels.md`); it has no upstream equivalent
    /// because upstream verifies with eyes on a window.
    ///
    /// Correct for CPU writes (`replaceRegion`) and — because IOSurface and
    /// shared/managed textures are coherent once the GPU work completes —
    /// for GPU renders after `waitUntilCompleted` (R2's concern).
    pub fn read_pixels(&self) -> Vec<u8> {
        let bpp = 4;
        let mut out = vec![0u8; self.width * self.height * bpp];
        let bytes_per_row = self.surface.bytes_per_row();
        // SAFETY: lock/unlock are balanced and the base address is only
        // read while the read-only lock is held. `seed` may be null.
        unsafe {
            self.surface
                .lock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut());
            let base = self.surface.base_address().cast::<u8>();
            for row in 0..self.height {
                let src = base.as_ptr().add(row * bytes_per_row);
                let dst = out.as_mut_ptr().add(row * self.width * bpp);
                std::ptr::copy_nonoverlapping(src, dst, self.width * bpp);
            }
            self.surface
                .unlock(IOSurfaceLockOptions::ReadOnly, std::ptr::null_mut());
        }
        out
    }
}

impl GpuTarget for Target {
    fn width(&self) -> usize {
        self.width
    }
    fn height(&self) -> usize {
        self.height
    }
    fn read_pixels(&self) -> Vec<u8> {
        Target::read_pixels(self)
    }
}

impl std::fmt::Debug for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Target")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Create the backing IOSurface: `32BGRA`, 4 bytes per element, tagged with
/// the Display P3 color space.
///
/// Display P3 gives "Apple-style" alpha blending: Apple apps like Terminal
/// and TextEdit render text in the display's color space using converted
/// colors, which reduces (but doesn't fully eliminate) blending artifacts.
/// (Comment ported from `Target.init`.)
fn new_iosurface(width: usize, height: usize) -> Result<CFRetained<IOSurfaceRef>, MetalError> {
    let width_n = CFNumber::new_i32(i32::try_from(width).map_err(|_| MetalError::MetalFailed)?);
    let height_n = CFNumber::new_i32(i32::try_from(height).map_err(|_| MetalError::MetalFailed)?);
    #[expect(
        clippy::cast_possible_wrap,
        reason = "fourcc 'BGRA' fits in i32; CFNumber round-trips the bit pattern"
    )]
    let format_n = CFNumber::new_i32(PIXEL_FORMAT_32BGRA as i32);
    let bpe_n = CFNumber::new_i32(4);

    let keys: [&CFString; 4] = unsafe {
        [
            kIOSurfaceWidth,
            kIOSurfaceHeight,
            kIOSurfacePixelFormat,
            kIOSurfaceBytesPerElement,
        ]
    };
    let values = [&*width_n, &*height_n, &*format_n, &*bpe_n];
    let properties = CFDictionary::from_slices(&keys, &values);

    // SAFETY: the dictionary keys/values are the documented IOSurface
    // property types (CFString → CFNumber).
    let surface =
        unsafe { IOSurfaceRef::new(properties.as_opaque()) }.ok_or(MetalError::MetalFailed)?;

    // We set our surface's color space to Display P3 (see fn docs). Port of
    // `IOSurface.setColorSpace(.displayP3)`: serialize the color space to a
    // property list and set it as the surface's color space value.
    let colorspace = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceDisplayP3 }))
        .ok_or(MetalError::MetalFailed)?;
    let plist = colorspace.property_list().ok_or(MetalError::MetalFailed)?;
    // SAFETY: kIOSurfaceColorSpace expects a serialized color space, which
    // is exactly what `CGColorSpaceCopyPropertyList` produces.
    unsafe { surface.set_value(kIOSurfaceColorSpace, plist.as_ref()) };

    Ok(surface)
}

#[cfg(test)]
mod tests {
    use super::super::test_metal;
    use super::*;
    use crate::gpu::GpuBackend;

    #[test]
    fn target_surface_properties() {
        let Some(metal) = test_metal() else { return };
        let target = metal.new_target(7, 5).expect("target");
        assert_eq!(target.surface().width(), 7);
        assert_eq!(target.surface().height(), 5);
        assert_eq!(target.surface().bytes_per_element(), 4);
        assert_eq!(target.surface().pixel_format(), PIXEL_FORMAT_32BGRA);
        // Row stride is at least the packed width and 4-byte aligned.
        assert!(target.surface().bytes_per_row() >= 7 * 4);
        // The texture view matches the surface dimensions.
        assert_eq!(target.texture().width(), 7);
        assert_eq!(target.texture().height(), 5);
    }

    #[test]
    fn target_upload_and_readback_roundtrip() {
        let Some(metal) = test_metal() else { return };
        let (w, h) = (4usize, 3usize);
        let target = metal.new_target(w, h).expect("target");

        // Distinct BGRA byte per pixel-channel so any stride/offset mistake
        // shows up as a mismatch.
        let pixels: Vec<u8> = (0..w * h * 4).map(|i| (i % 251) as u8).collect();

        // Upload via replaceRegion on the target's texture (CPU streaming
        // path; the same message `Texture.replace_region` sends).
        let region = objc2_metal::MTLRegion {
            origin: objc2_metal::MTLOrigin { x: 0, y: 0, z: 0 },
            size: objc2_metal::MTLSize {
                width: w,
                height: h,
                depth: 1,
            },
        };
        // SAFETY: `pixels` holds `h` rows of `w * 4` bytes as promised to
        // replaceRegion.
        unsafe {
            target
                .texture()
                .replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                    region,
                    0,
                    std::ptr::NonNull::new(pixels.as_ptr().cast_mut().cast()).unwrap(),
                    w * 4,
                );
        }

        // Read back through the IOSurface base address and compare bytes.
        assert_eq!(target.read_pixels(), pixels);
    }
}
