//! OpenGL backend: a surfaceless GL 4.3-core implementation of
//! [`GpuBackend`](crate::gpu::GpuBackend) (ADR 005 P4, slice 1).
//!
//! Port of Ghostty's `src/renderer/OpenGL.zig` + `src/renderer/opengl/`
//! (commit `2da015cd6`), reduced to the **headless / offscreen** subset: a
//! surfaceless EGL context (so it runs in a container with no display server,
//! under Mesa software GL / `llvmpipe`), the resource wrappers
//! (`Target`/`Texture`/`Sampler`/`Buffer(T)`), the frame lifecycle
//! (`Frame`/`RenderPass`/`Pipeline`), and an FBO-readback path. It is
//! **additive**: a third [`GpuBackend`] alongside `metal`/`software`, behind the
//! generic `Engine<B>` seam, changing nothing in the trait or the other
//! backends.
//!
//! What upstream expresses as glad-loaded global GL bindings this port drives
//! through the [`glow`] crate over an EGL context created with
//! [`khronos_egl`]'s dynamically-loaded `libEGL`. Both are Linux-only deps
//! (see `Cargo.toml`'s `cfg(target_os = "linux")` block), so the macOS/default
//! build is unaffected.
//!
//! **GL context / threading.** Upstream OpenGL is always-sync with
//! `swap_chain_count = 1` (`OpenGL.zig:31-33,36-38`); this port matches that.
//! The single EGL context is made current at construction and kept current for
//! the backend's lifetime (headless, single-threaded). All GL resources share
//! it via an [`Rc<GlState>`], so they can free themselves on `Drop` while the
//! context is still live (the `Rc` outlives every resource).
//!
//! **Uniform packing note.** The vendored GLSL reads the *frozen* wire structs
//! ([`crate::wire`]) exactly as the MSL does — the std140 `Globals` UBO block
//! in `common.glsl` matches [`wire::Uniforms`](crate::wire::Uniforms)
//! field-for-field. The one representational seam is `Uniforms.bools`: wire
//! stores four 1-byte bools, whereas the GLSL reads a single `uint` of bit
//! flags. For the current engine that is a no-op — the engine only ever sets
//! `cursor_wide` (bit 0, which coincides in both layouts) and hardcodes the
//! other three to `false` (`engine.rs` `build_uniforms`), so the little-endian
//! `uint` reads back correctly. If the engine ever sets `use_display_p3` /
//! `use_linear_blending` / `use_linear_correction`, this backend would need to
//! repack those bytes into bit flags before upload; that is out of slice-1
//! scope and documented here rather than silently assumed.

#![cfg(target_os = "linux")]

mod buffer;
mod frame;
mod pipeline;
mod render_pass;
mod sampler;
pub(crate) mod shaders;
mod target;
mod texture;

use std::fmt;
use std::rc::Rc;

use glow::HasContext;
use khronos_egl as egl;

pub use self::buffer::Buffer;
pub use self::frame::Frame;
pub use self::pipeline::Pipeline;
pub use self::render_pass::RenderPass;
pub use self::sampler::Sampler;
pub use self::target::Target;
pub use self::texture::Texture;
use crate::gpu::{FrameCompletion, GpuBackend, SamplerOptions, ShaderSource, TextureOptions};

/// The EGL API level we load. 1.5 (or 1.4 + `EGL_KHR_create_context`, which
/// Mesa provides) is required for a desktop-GL core-profile context.
type Egl = egl::DynamicInstance<egl::EGL1_5>;

/// Minimum OpenGL version, matching upstream (`OpenGL.zig:41-43`).
pub const MIN_VERSION_MAJOR: i32 = 4;
pub const MIN_VERSION_MINOR: i32 = 3;

// EGL context-creation attributes (EGL 1.5 / `EGL_KHR_create_context`). Named
// locally so the intent is legible next to the attribute list.
const EGL_CONTEXT_OPENGL_PROFILE_MASK: egl::Int = egl::CONTEXT_OPENGL_PROFILE_MASK;
const EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT: egl::Int = egl::CONTEXT_OPENGL_CORE_PROFILE_BIT;

/// Errors from the OpenGL backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GlError {
    /// No usable EGL/GL context could be created headlessly (no `libEGL`, no
    /// surfaceless display, or the driver is too old). The offscreen test
    /// treats this as a graceful skip, never a hard failure — mirrors the
    /// Metal backend's "no device" skip.
    NoContext(String),
    /// A GL object (buffer/texture/program/…) could not be created, or a
    /// shader failed to compile/link. Upstream `error.OpenGLFailed`.
    GlFailed(String),
}

impl fmt::Display for GlError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoContext(m) => write!(f, "no usable OpenGL context: {m}"),
            Self::GlFailed(m) => write!(f, "an OpenGL API call failed: {m}"),
        }
    }
}

impl std::error::Error for GlError {}

/// The shared GL context: the `glow` wrapper plus the EGL handles that keep it
/// current and alive. Held behind an [`Rc`] by the backend and every resource,
/// so GL objects can be freed on `Drop` while the context is still valid.
pub struct GlState {
    /// The `glow` GL function table.
    gl: glow::Context,
    /// The EGL instance (owns the loaded `libEGL`), display, and context. Kept
    /// so the context stays current and is torn down last, after all resources.
    egl: Egl,
    display: egl::Display,
    context: egl::Context,
    /// A 1×1 pbuffer surface, only if the driver rejected a truly surfaceless
    /// (`EGL_NO_SURFACE`) `make_current`. `None` on the common surfaceless path.
    surface: Option<egl::Surface>,
}

impl GlState {
    /// The `glow` GL function table.
    pub(crate) fn gl(&self) -> &glow::Context {
        &self.gl
    }
}

impl Drop for GlState {
    fn drop(&mut self) {
        // Release the context and tear down EGL. Runs only after every resource
        // holding an `Rc<GlState>` has dropped, so no GL frees race this.
        let _ = self.egl.make_current(self.display, None, None, None);
        if let Some(surface) = self.surface.take() {
            let _ = self.egl.destroy_surface(self.display, surface);
        }
        let _ = self.egl.destroy_context(self.display, self.context);
        let _ = self.egl.terminate(self.display);
    }
}

/// The OpenGL graphics API context. Port of the headless subset of the `OpenGL`
/// struct in `OpenGL.zig`: the shared GL state + the `blending` mode that
/// selects the render target's (s)RGB internal format.
pub struct OpenGL {
    state: Rc<GlState>,
    /// Whether render targets use an `*_srgb` internal format so blending
    /// happens in linear space. Stands in for upstream's `blending` config
    /// field; upstream's default (`.native`) is non-linear, i.e. `false`
    /// (`OpenGL.zig:52`, `initTarget`). Kept for parity with the Metal
    /// backend's `linear_blending`.
    linear_blending: bool,
}

impl OpenGL {
    /// Create the backend: load `libEGL`, bring up a surfaceless GL 4.3-core
    /// context, and load `glow`. Reduced/headless analog of upstream
    /// `prepareContext` + `surfaceInit` (`OpenGL.zig:150-210`), which for a
    /// windowed app loads GL against the toolkit's context; here we own a
    /// surfaceless one for offscreen rendering.
    ///
    /// Returns [`GlError::NoContext`] (a *skip* signal, not a hard error) if no
    /// headless context is available — e.g. `libEGL` is missing or the platform
    /// has no surfaceless display.
    pub fn new() -> Result<Self, GlError> {
        let egl = load_egl()?;

        // The surfaceless display. With `EGL_PLATFORM=surfaceless` (Mesa) the
        // default display is the surfaceless one; otherwise this still yields a
        // usable offscreen display on most drivers.
        // SAFETY: `DEFAULT_DISPLAY` is the documented sentinel for `eglGetDisplay`.
        let display = unsafe { egl.get_display(egl::DEFAULT_DISPLAY) }
            .ok_or_else(|| GlError::NoContext("eglGetDisplay returned no display".into()))?;
        egl.initialize(display)
            .map_err(|e| GlError::NoContext(format!("eglInitialize failed: {e}")))?;

        // We want desktop OpenGL (not GLES) so we can use GL 4.3 core.
        egl.bind_api(egl::OPENGL_API)
            .map_err(|e| GlError::NoContext(format!("eglBindAPI(OpenGL) failed: {e}")))?;

        // A config that can render OpenGL. We request a pbuffer-capable config
        // so the pbuffer fallback below is available if truly-surfaceless
        // contexts are unsupported; the actual rendering is always to an FBO.
        let config_attribs = [
            egl::RENDERABLE_TYPE,
            egl::OPENGL_BIT,
            egl::SURFACE_TYPE,
            egl::PBUFFER_BIT,
            egl::RED_SIZE,
            8,
            egl::GREEN_SIZE,
            8,
            egl::BLUE_SIZE,
            8,
            egl::ALPHA_SIZE,
            8,
            egl::NONE,
        ];
        let config = egl
            .choose_first_config(display, &config_attribs)
            .map_err(|e| GlError::NoContext(format!("eglChooseConfig failed: {e}")))?
            .ok_or_else(|| GlError::NoContext("no EGL config supports OpenGL".into()))?;

        let context_attribs = [
            egl::CONTEXT_MAJOR_VERSION,
            MIN_VERSION_MAJOR,
            egl::CONTEXT_MINOR_VERSION,
            MIN_VERSION_MINOR,
            EGL_CONTEXT_OPENGL_PROFILE_MASK,
            EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT,
            egl::NONE,
        ];
        let context = egl
            .create_context(display, config, None, &context_attribs)
            .map_err(|e| GlError::NoContext(format!("eglCreateContext(4.3 core) failed: {e}")))?;

        // Prefer a truly surfaceless context (`EGL_KHR_surfaceless_context`,
        // which Mesa supports); fall back to a 1×1 pbuffer if the driver
        // rejects `EGL_NO_SURFACE`.
        let surface = match egl.make_current(display, None, None, Some(context)) {
            Ok(()) => None,
            Err(_) => {
                let pbuffer_attribs = [egl::WIDTH, 1, egl::HEIGHT, 1, egl::NONE];
                let surface = egl
                    .create_pbuffer_surface(display, config, &pbuffer_attribs)
                    .map_err(|e| {
                        GlError::NoContext(format!("eglCreatePbufferSurface failed: {e}"))
                    })?;
                egl.make_current(display, Some(surface), Some(surface), Some(context))
                    .map_err(|e| {
                        GlError::NoContext(format!("eglMakeCurrent (pbuffer) failed: {e}"))
                    })?;
                Some(surface)
            }
        };

        // Load `glow` against the current context.
        // SAFETY: the context is current; `get_proc_address` returns valid GL
        // entry points (or null, which glow treats as an unsupported function).
        let gl = unsafe {
            glow::Context::from_loader_function(|name| {
                egl.get_proc_address(name)
                    .map_or(std::ptr::null(), |f| f as *const std::ffi::c_void)
            })
        };

        // Enable SRGB framebuffer support for linear blending, matching
        // upstream `prepareContext` (`OpenGL.zig:207-208`). On our default
        // (non-linear) `RGBA8` render target this is a no-op; it only bites
        // when `linear_blending` selects an `SRGB8_ALPHA8` target.
        // SAFETY: plain GL enable on the current context.
        unsafe { gl.enable(glow::FRAMEBUFFER_SRGB) };

        Ok(Self {
            state: Rc::new(GlState {
                gl,
                egl,
                display,
                context,
                surface,
            }),
            linear_blending: false,
        })
    }

    /// Clone the shared GL state (for resource constructors).
    pub(crate) fn state(&self) -> Rc<GlState> {
        Rc::clone(&self.state)
    }

    /// The render-target renderbuffer internal format: `SRGB8_ALPHA8` iff
    /// linear blending, else `RGBA8`. Port of `initTarget`'s
    /// `if (blending.isLinear()) .srgba else .rgba` (`OpenGL.zig:214-220`).
    fn target_internal_format(&self) -> u32 {
        if self.linear_blending {
            glow::SRGB8_ALPHA8
        } else {
            glow::RGBA8
        }
    }
}

impl fmt::Debug for OpenGL {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OpenGL")
            .field("linear_blending", &self.linear_blending)
            .field("surfaceless", &self.state.surface.is_none())
            .finish_non_exhaustive()
    }
}

impl GpuBackend for OpenGL {
    /// Always-sync, no multi-buffering (upstream `swap_chain_count = 1`,
    /// `OpenGL.zig:31-33`).
    const SWAP_CHAIN_COUNT: usize = 1;

    type Error = GlError;
    type Target = Target;
    type Frame = Frame;
    type RenderPass = RenderPass;
    type Pipeline = Pipeline;
    type Buffer<T: Copy + 'static> = Buffer<T>;
    type Texture = Texture;
    type Sampler = Sampler;
    type BufferHandle = glow::Buffer;

    fn max_texture_size(&self) -> u32 {
        // GL 4.3 core guarantees at least 16384; query the driver for the real
        // limit (`GL_MAX_TEXTURE_SIZE`). Upstream doesn't clamp for GL, but the
        // generic renderer expects a value.
        // SAFETY: plain GL integer query on the current context.
        let n = unsafe { self.state.gl().get_parameter_i32(glow::MAX_TEXTURE_SIZE) };
        u32::try_from(n).unwrap_or(16384).max(16384)
    }

    /// Upstream `OpenGL.initTarget` (`OpenGL.zig:213-220`).
    fn new_target(&self, width: usize, height: usize) -> Result<Target, GlError> {
        Target::new(self.state(), width, height, self.target_internal_format())
    }

    /// Upstream `Buffer.init` with `OpenGL.bufferOptions` (array /
    /// dynamic_draw).
    fn new_buffer<T: Copy + 'static>(&self, len: usize) -> Result<Buffer<T>, GlError> {
        Buffer::new(self.state(), len)
    }

    /// Upstream `Buffer.initFill`.
    fn new_buffer_with_data<T: Copy + 'static>(&self, data: &[T]) -> Result<Buffer<T>, GlError> {
        Buffer::new_with_data(self.state(), data)
    }

    /// Upstream `Texture.init` / `initAtlasTexture`. The trait carries no
    /// texture *target* (2D vs rectangle); the only textures the first-pixels
    /// path creates are the glyph atlases, which upstream makes `Rectangle`
    /// (`sampler2DRect`, pixel-addressed) — so this backend creates every
    /// texture as `GL_TEXTURE_RECTANGLE`. Kitty image textures (2D
    /// `sampler2D`) are a follow-up, exactly as in the Software backend.
    fn new_texture(
        &self,
        options: TextureOptions,
        width: usize,
        height: usize,
        data: Option<&[u8]>,
    ) -> Result<Texture, GlError> {
        Texture::new(self.state(), options.format, width, height, data)
    }

    /// Upstream `Sampler.init`.
    fn new_sampler(&self, options: SamplerOptions) -> Result<Sampler, GlError> {
        Sampler::new(self.state(), options)
    }

    /// Begin a frame. Upstream `OpenGL.beginFrame` / `Frame.begin` — GL has no
    /// command buffer, so a frame is just the completion hook plus the shared
    /// state used to `glFinish` on completion.
    fn begin_frame(&self, completion: FrameCompletion) -> Result<Frame, GlError> {
        Ok(Frame::begin(self.state(), completion))
    }

    /// Compile one pipeline. The engine hands every backend the *Metal*
    /// [`ShaderSource`] (it can't know which backend it's driving), so — like
    /// the Software backend keying off `desc.name` — the GL backend ignores the
    /// MSL and selects its own vendored GLSL by `desc.name`
    /// ([`shaders::shader_set`]). Port of `opengl/shaders.zig`'s per-pipeline
    /// `Pipeline.init` + the `autoAttribute` vertex layout.
    fn build_pipeline(
        &self,
        desc: &crate::shaders::PipelineDescription,
        _source: ShaderSource<'_>,
    ) -> Result<Pipeline, GlError> {
        let set = shaders::shader_set(desc.name)
            .ok_or_else(|| GlError::GlFailed(format!("no GLSL for pipeline `{}`", desc.name)))?;
        Pipeline::new(self.state(), desc, &set)
    }
}

/// Load `libEGL` dynamically (so the macOS/default build never links it) and
/// wrap it as an EGL 1.5 instance. Tries `libEGL.so.1` then `libEGL.so`
/// ([`khronos_egl`]'s `load_required`).
fn load_egl() -> Result<Egl, GlError> {
    // SAFETY: loading `libEGL` and reading its symbol table is the documented
    // use of `DynamicInstance::load_required`; the library is a trusted system
    // component.
    unsafe { Egl::load_required() }
        .map_err(|e| GlError::NoContext(format!("could not load libEGL: {e}")))
}

/// Test-only constructor mirroring `metal::test_metal`: build the backend, or
/// print a SKIP note and return `None` when no headless GL context is
/// available (no `libEGL`, no surfaceless display, driver too old) so tests
/// skip rather than fail where GL is unavailable.
#[cfg(test)]
pub(crate) fn test_opengl() -> Option<OpenGL> {
    match OpenGL::new() {
        Ok(gl) => Some(gl),
        Err(err) => {
            eprintln!("SKIP: no usable OpenGL context ({err}); skipping GL test");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gpu::{GpuBuffer, GpuTarget, GpuTexture, TextureFormat, TextureUsage};

    #[test]
    fn context_init_and_resources() {
        let Some(gl) = test_opengl() else { return };
        assert!(gl.max_texture_size() >= 16384);

        let target = gl.new_target(4, 4).expect("target");
        assert_eq!((target.width(), target.height()), (4, 4));

        let buffer = gl.new_buffer::<u32>(4).expect("buffer");
        assert_eq!(buffer.len(), 4);

        let texture = gl
            .new_texture(
                TextureOptions {
                    format: TextureFormat::R8Unorm,
                    usage: TextureUsage::SHADER_READ,
                },
                8,
                8,
                None,
            )
            .expect("texture");
        assert_eq!((texture.width(), texture.height()), (8, 8));

        gl.new_sampler(SamplerOptions::default()).expect("sampler");
    }
}
