//! Wrapper for handling textures.
//!
//! Port of `src/renderer/metal/Texture.zig` (commit `2da015cd6`).

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::{
    MTLDevice, MTLOrigin, MTLPixelFormat, MTLRegion, MTLResourceOptions, MTLSize, MTLTexture,
    MTLTextureDescriptor, MTLTextureUsage,
};

use super::MetalError;
use crate::gpu::{GpuTexture, TextureUsage};

/// Options for initializing a texture. Port of `Texture.Options`.
pub(super) struct Options<'a> {
    /// MTLDevice.
    pub device: &'a ProtocolObject<dyn MTLDevice>,
    pub pixel_format: MTLPixelFormat,
    pub resource_options: MTLResourceOptions,
    pub usage: MTLTextureUsage,
}

/// Convert the backend-agnostic usage flags to Metal bits.
pub(super) fn usage_bits(usage: TextureUsage) -> MTLTextureUsage {
    let mut bits = MTLTextureUsage::empty();
    if usage.shader_read {
        bits |= MTLTextureUsage::ShaderRead;
    }
    if usage.shader_write {
        bits |= MTLTextureUsage::ShaderWrite;
    }
    if usage.render_target {
        bits |= MTLTextureUsage::RenderTarget;
    }
    bits
}

/// A 2D Metal texture with CPU streaming via `replaceRegion`.
pub struct Texture {
    /// The underlying MTLTexture object.
    texture: Retained<ProtocolObject<dyn MTLTexture>>,
    /// The dimensions of this texture.
    width: usize,
    height: usize,
    /// Bytes per pixel for this texture.
    bpp: usize,
}

impl Texture {
    /// Initialize a texture, optionally uploading `data`
    /// (`width * height * bpp` bytes). Port of `Texture.init`.
    pub(super) fn new(
        opts: &Options<'_>,
        width: usize,
        height: usize,
        data: Option<&[u8]>,
    ) -> Result<Self, MetalError> {
        // Create our descriptor and set our properties.
        let desc = MTLTextureDescriptor::new();
        desc.setPixelFormat(opts.pixel_format);
        unsafe {
            desc.setWidth(width);
            desc.setHeight(height);
        }
        desc.setResourceOptions(opts.resource_options);
        desc.setUsage(opts.usage);

        let texture = opts
            .device
            .newTextureWithDescriptor(&desc)
            .ok_or(MetalError::MetalFailed)?;

        let this = Self {
            texture,
            width,
            height,
            bpp: bpp_of(opts.pixel_format),
        };

        // If we have data, we set it here.
        if let Some(data) = data {
            assert_eq!(data.len(), width * height * this.bpp);
            this.replace_region(0, 0, width, height, data)?;
        }

        Ok(this)
    }

    /// The underlying MTLTexture.
    pub fn texture(&self) -> &ProtocolObject<dyn MTLTexture> {
        &self.texture
    }

    /// Bytes per pixel.
    pub fn bytes_per_pixel(&self) -> usize {
        self.bpp
    }
}

impl GpuTexture for Texture {
    type Error = MetalError;

    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    /// Replace a region of the texture with the provided data. Port of
    /// `Texture.replaceRegion`.
    ///
    /// Divergence: upstream documents "does NOT check the dimensions of the
    /// data" — this port asserts `data` holds at least `width * height *
    /// bpp` bytes, since we hand Metal a raw pointer and an out-of-bounds
    /// read would be UB rather than just a rendering glitch.
    fn replace_region(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        data: &[u8],
    ) -> Result<(), MetalError> {
        assert!(
            data.len() >= width * height * self.bpp,
            "replace_region data too small: {} < {}x{}x{}",
            data.len(),
            width,
            height,
            self.bpp,
        );
        let region = MTLRegion {
            origin: MTLOrigin { x, y, z: 0 },
            size: MTLSize {
                width,
                height,
                depth: 1,
            },
        };
        let bytes = std::ptr::NonNull::new(data.as_ptr().cast_mut().cast())
            .expect("slice pointer is never null");
        // SAFETY: `bytes` points at `height` rows of `width * bpp` valid
        // bytes (asserted above); rows are tightly packed.
        unsafe {
            self.texture
                .replaceRegion_mipmapLevel_withBytes_bytesPerRow(
                    region,
                    0,
                    bytes,
                    self.bpp * width,
                );
        }
        Ok(())
    }
}

impl std::fmt::Debug for Texture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Texture")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bpp", &self.bpp)
            .finish_non_exhaustive()
    }
}

/// Returns the bytes per pixel for the provided pixel format. Port of
/// `Texture.bppOf` — same format groups, same panics for invalid/unknown
/// formats ("unlikely that this format was actually used, could be memory
/// corruption").
///
/// Divergence: upstream returns `128` for the 128-bit formats — bits, not
/// bytes, an upstream bug that's harmless there because no call site uses
/// those formats. This port returns 16 (bytes) instead.
pub(super) fn bpp_of(pixel_format: MTLPixelFormat) -> usize {
    match pixel_format {
        // Invalid
        MTLPixelFormat::Invalid => panic!("invalid pixel format"),

        // 8-bit pixel formats
        MTLPixelFormat::A8Unorm
        | MTLPixelFormat::R8Unorm
        | MTLPixelFormat::R8Unorm_sRGB
        | MTLPixelFormat::R8Snorm
        | MTLPixelFormat::R8Uint
        | MTLPixelFormat::R8Sint
        | MTLPixelFormat::Stencil8 => 1,

        // 16-bit pixel formats
        MTLPixelFormat::RG8Unorm
        | MTLPixelFormat::RG8Unorm_sRGB
        | MTLPixelFormat::RG8Snorm
        | MTLPixelFormat::RG8Uint
        | MTLPixelFormat::RG8Sint
        | MTLPixelFormat::R16Unorm
        | MTLPixelFormat::R16Snorm
        | MTLPixelFormat::R16Uint
        | MTLPixelFormat::R16Sint
        | MTLPixelFormat::R16Float
        | MTLPixelFormat::B5G6R5Unorm
        | MTLPixelFormat::A1BGR5Unorm
        | MTLPixelFormat::ABGR4Unorm
        | MTLPixelFormat::BGR5A1Unorm
        | MTLPixelFormat::Depth16Unorm => 2,

        // 32-bit pixel formats
        MTLPixelFormat::RG16Unorm
        | MTLPixelFormat::RG16Snorm
        | MTLPixelFormat::RG16Uint
        | MTLPixelFormat::RG16Sint
        | MTLPixelFormat::RG16Float
        | MTLPixelFormat::RGBA8Unorm
        | MTLPixelFormat::RGBA8Unorm_sRGB
        | MTLPixelFormat::RGBA8Snorm
        | MTLPixelFormat::RGBA8Uint
        | MTLPixelFormat::RGBA8Sint
        | MTLPixelFormat::BGRA8Unorm
        | MTLPixelFormat::BGRA8Unorm_sRGB
        | MTLPixelFormat::RGB10A2Unorm
        | MTLPixelFormat::RGB10A2Uint
        | MTLPixelFormat::RG11B10Float
        | MTLPixelFormat::RGB9E5Float
        | MTLPixelFormat::BGR10A2Unorm
        | MTLPixelFormat::BGR10_XR
        | MTLPixelFormat::BGR10_XR_sRGB
        | MTLPixelFormat::R32Uint
        | MTLPixelFormat::R32Sint
        | MTLPixelFormat::R32Float
        | MTLPixelFormat::Depth32Float
        | MTLPixelFormat::Depth24Unorm_Stencil8 => 4,

        // 64-bit pixel formats
        MTLPixelFormat::RG32Uint
        | MTLPixelFormat::RG32Sint
        | MTLPixelFormat::RG32Float
        | MTLPixelFormat::RGBA16Unorm
        | MTLPixelFormat::RGBA16Snorm
        | MTLPixelFormat::RGBA16Uint
        | MTLPixelFormat::RGBA16Sint
        | MTLPixelFormat::RGBA16Float
        | MTLPixelFormat::BGRA10_XR
        | MTLPixelFormat::BGRA10_XR_sRGB => 8,

        // 128-bit pixel formats (16 bytes; upstream says 128 — see fn docs)
        MTLPixelFormat::RGBA32Uint | MTLPixelFormat::RGBA32Sint | MTLPixelFormat::RGBA32Float => 16,

        // Weird formats upstream was "too lazy to get the sizes of"
        _ => panic!(
            "pixel format size unknown (unlikely that this format was actually used, \
             could be memory corruption)"
        ),
    }
}

#[cfg(test)]
mod tests {
    use objc2_metal::MTLStorageMode;

    use super::super::test_metal;
    use super::*;
    use crate::gpu::{GpuBackend, TextureFormat, TextureOptions};

    #[test]
    fn bpp_of_matches_upstream_groups() {
        assert_eq!(bpp_of(MTLPixelFormat::R8Unorm), 1);
        assert_eq!(bpp_of(MTLPixelFormat::Depth16Unorm), 2);
        assert_eq!(bpp_of(MTLPixelFormat::BGRA8Unorm_sRGB), 4);
        assert_eq!(bpp_of(MTLPixelFormat::RGBA16Float), 8);
        assert_eq!(bpp_of(MTLPixelFormat::RGBA32Float), 16);
    }

    #[test]
    #[should_panic(expected = "invalid pixel format")]
    fn bpp_of_invalid_panics() {
        let _ = bpp_of(MTLPixelFormat::Invalid);
    }

    /// Upload a grayscale atlas-style texture and read it back via
    /// `getBytes` (only valid for shared storage, which is what unified-
    /// memory devices give us; skipped on discrete GPUs where readback
    /// would need a blit sync).
    #[test]
    fn texture_upload_readback_roundtrip() {
        let Some(metal) = test_metal() else { return };
        if metal.default_storage_mode() != MTLStorageMode::Shared {
            eprintln!("SKIP: managed storage; getBytes readback needs a blit sync (R2)");
            return;
        }

        let (w, h) = (8usize, 4usize);
        let data: Vec<u8> = (0..w * h).map(|i| i as u8).collect();
        let texture = metal
            .new_texture(
                TextureOptions {
                    format: TextureFormat::R8Unorm,
                    usage: crate::gpu::TextureUsage::SHADER_READ,
                },
                w,
                h,
                Some(&data),
            )
            .expect("texture");
        assert_eq!(texture.bytes_per_pixel(), 1);

        let mut out = vec![0u8; w * h];
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: w,
                height: h,
                depth: 1,
            },
        };
        // SAFETY: `out` holds `h` rows of `w` bytes.
        unsafe {
            texture
                .texture()
                .getBytes_bytesPerRow_fromRegion_mipmapLevel(
                    std::ptr::NonNull::new(out.as_mut_ptr().cast()).unwrap(),
                    w,
                    region,
                    0,
                );
        }
        assert_eq!(out, data);
    }

    /// `replace_region` at an offset only touches the requested region.
    #[test]
    fn replace_region_partial_update() {
        let Some(metal) = test_metal() else { return };
        if metal.default_storage_mode() != MTLStorageMode::Shared {
            eprintln!("SKIP: managed storage; getBytes readback needs a blit sync (R2)");
            return;
        }

        let (w, h) = (4usize, 4usize);
        let texture = metal
            .new_texture(
                TextureOptions {
                    format: TextureFormat::R8Unorm,
                    usage: crate::gpu::TextureUsage::SHADER_READ,
                },
                w,
                h,
                Some(&vec![0u8; w * h]),
            )
            .expect("texture");

        // Write a 2x2 patch of 0xAB at (1, 1).
        texture
            .replace_region(1, 1, 2, 2, &[0xAB; 4])
            .expect("replace_region");

        let mut out = vec![0u8; w * h];
        let region = MTLRegion {
            origin: MTLOrigin { x: 0, y: 0, z: 0 },
            size: MTLSize {
                width: w,
                height: h,
                depth: 1,
            },
        };
        // SAFETY: `out` holds `h` rows of `w` bytes.
        unsafe {
            texture
                .texture()
                .getBytes_bytesPerRow_fromRegion_mipmapLevel(
                    std::ptr::NonNull::new(out.as_mut_ptr().cast()).unwrap(),
                    w,
                    region,
                    0,
                );
        }
        #[rustfmt::skip]
        let expected = vec![
            0, 0,    0,    0,
            0, 0xAB, 0xAB, 0,
            0, 0xAB, 0xAB, 0,
            0, 0,    0,    0,
        ];
        assert_eq!(out, expected);
    }
}
