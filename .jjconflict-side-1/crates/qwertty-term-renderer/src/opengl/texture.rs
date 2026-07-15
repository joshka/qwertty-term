//! Wrapper for handling textures.
//!
//! Port of `src/renderer/opengl/Texture.zig` (commit `2da015cd6`).
//!
//! Every texture is created as `GL_TEXTURE_RECTANGLE` with nearest filtering
//! and clamp-to-edge wrapping — the glyph-atlas configuration upstream's
//! `initAtlasTexture` uses (`OpenGL.zig:322-347`; the atlas is sampled as a
//! `sampler2DRect` with pixel coordinates in `cell_text.f.glsl`). The
//! first-pixels path only ever creates the grayscale + color atlases, both
//! rectangle textures. Kitty image textures (2D `sampler2D`) are a follow-up,
//! matching the Software backend's deferral.

use std::rc::Rc;

use glow::HasContext;

use super::{GlError, GlState};
use crate::gpu::{GpuTexture, TextureFormat};

/// A GL texture with CPU streaming via `glTexSubImage2D`. Port of `Texture`.
pub struct Texture {
    state: Rc<GlState>,
    texture: glow::Texture,
    width: usize,
    height: usize,
    /// Bytes per pixel for `replace_region` bounds/uploads.
    bpp: usize,
    /// The client pixel format (`GL_RED` / `GL_RGBA` / `GL_BGRA`) used for
    /// uploads — the second half of the `(internal_format, format)` pair.
    format: u32,
}

impl Texture {
    /// Initialize a texture, optionally uploading `data` (`width * height *
    /// bpp` bytes). Port of `Texture.init` with `initAtlasTexture`'s
    /// rectangle/nearest/clamp options.
    pub(super) fn new(
        state: Rc<GlState>,
        texture_format: TextureFormat,
        width: usize,
        height: usize,
        data: Option<&[u8]>,
    ) -> Result<Self, GlError> {
        let (internal_format, format) = gl_formats(texture_format);
        let bpp = texture_format.bytes_per_pixel();
        if let Some(d) = data {
            assert_eq!(
                d.len(),
                width * height * bpp,
                "texture init data size mismatch",
            );
        }

        let gl = state.gl();
        // SAFETY: create/bind/parameterize/allocate on the current context.
        // `UNPACK_ALIGNMENT = 1` so single-channel/odd-width rows aren't
        // misread. `tex_image_2d` reads exactly `width*height*bpp` bytes when
        // `data` is `Some` (asserted above), else allocates uninitialized.
        let texture = unsafe {
            let texture = gl
                .create_texture()
                .map_err(|e| GlError::GlFailed(format!("glGenTextures: {e}")))?;
            gl.bind_texture(glow::TEXTURE_RECTANGLE, Some(texture));
            gl.tex_parameter_i32(
                glow::TEXTURE_RECTANGLE,
                glow::TEXTURE_WRAP_S,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_RECTANGLE,
                glow::TEXTURE_WRAP_T,
                glow::CLAMP_TO_EDGE as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_RECTANGLE,
                glow::TEXTURE_MIN_FILTER,
                glow::NEAREST as i32,
            );
            gl.tex_parameter_i32(
                glow::TEXTURE_RECTANGLE,
                glow::TEXTURE_MAG_FILTER,
                glow::NEAREST as i32,
            );
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_image_2d(
                glow::TEXTURE_RECTANGLE,
                0,
                internal_format as i32,
                width as i32,
                height as i32,
                0,
                format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(data),
            );
            gl.bind_texture(glow::TEXTURE_RECTANGLE, None);
            texture
        };

        Ok(Self {
            state,
            texture,
            width,
            height,
            bpp,
            format,
        })
    }

    /// The underlying GL texture name (bound by [`super::RenderPass`]).
    pub(super) fn texture(&self) -> glow::Texture {
        self.texture
    }
}

impl GpuTexture for Texture {
    type Error = GlError;

    fn width(&self) -> usize {
        self.width
    }

    fn height(&self) -> usize {
        self.height
    }

    /// Replace a region of the texture with `data`. Port of
    /// `Texture.replaceRegion`.
    ///
    /// Divergence: upstream documents "does NOT check the dimensions of the
    /// data"; this port asserts `data` is at least `width * height * bpp` bytes
    /// (an out-of-bounds GL read would be UB, not just a glitch).
    fn replace_region(
        &self,
        x: usize,
        y: usize,
        width: usize,
        height: usize,
        data: &[u8],
    ) -> Result<(), GlError> {
        assert!(
            data.len() >= width * height * self.bpp,
            "replace_region data too small: {} < {}x{}x{}",
            data.len(),
            width,
            height,
            self.bpp,
        );
        let gl = self.state.gl();
        // SAFETY: current context; `data` holds `height` tightly-packed rows of
        // `width*bpp` bytes (asserted), `UNPACK_ALIGNMENT = 1`.
        unsafe {
            gl.bind_texture(glow::TEXTURE_RECTANGLE, Some(self.texture));
            gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            gl.tex_sub_image_2d(
                glow::TEXTURE_RECTANGLE,
                0,
                x as i32,
                y as i32,
                width as i32,
                height as i32,
                self.format,
                glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(&data[..width * height * self.bpp])),
            );
            gl.bind_texture(glow::TEXTURE_RECTANGLE, None);
        }
        Ok(())
    }
}

impl Drop for Texture {
    fn drop(&mut self) {
        // SAFETY: current context; texture name is live and owned here.
        unsafe { self.state.gl().delete_texture(self.texture) };
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

/// Map the backend-agnostic [`TextureFormat`] to a GL `(internal_format,
/// client_format)` pair. Mirrors upstream `initAtlasTexture`
/// (`grayscale => (.red, .red)`, `bgra => (.bgra, .srgba)`) and
/// `ImageTextureFormat.toPixelFormat`.
fn gl_formats(format: TextureFormat) -> (u32, u32) {
    match format {
        // Grayscale coverage mask (text atlas): single-channel, raw (non-srgb).
        TextureFormat::R8Unorm | TextureFormat::R8UnormSrgb => (glow::R8, glow::RED),
        TextureFormat::Rgba8Unorm => (glow::RGBA8, glow::RGBA),
        TextureFormat::Rgba8UnormSrgb => (glow::SRGB8_ALPHA8, glow::RGBA),
        TextureFormat::Bgra8Unorm => (glow::RGBA8, glow::BGRA),
        // Color (emoji) atlas: srgb internal so the GPU linearizes on sample.
        TextureFormat::Bgra8UnormSrgb => (glow::SRGB8_ALPHA8, glow::BGRA),
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_opengl;
    use crate::gpu::{GpuBackend, GpuTexture, TextureFormat, TextureOptions, TextureUsage};

    #[test]
    fn texture_replace_region_in_bounds() {
        let Some(gl) = test_opengl() else { return };
        let texture = gl
            .new_texture(
                TextureOptions {
                    format: TextureFormat::R8Unorm,
                    usage: TextureUsage::SHADER_READ,
                },
                4,
                4,
                Some(&[0u8; 16]),
            )
            .expect("texture");
        texture
            .replace_region(1, 1, 2, 2, &[0xAB; 4])
            .expect("replace_region");
        assert_eq!((texture.width(), texture.height()), (4, 4));
    }
}
