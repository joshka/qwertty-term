//! The GTK4 + libadwaita application shell.
//!
//! Mirrors the upstream Ghostty GTK lifecycle (pin `2da015cd6`, line numbers
//! against the confirmed-ancestor `38e49a2`):
//!
//! - `adw::Application` + `activate` → request one window — upstream
//!   `class/application.zig:1459` (`activate`).
//! - `adw::ApplicationWindow` with a single surface widget as its child —
//!   upstream `class/window.zig:272` (`new`); we bypass `AdwTabView`/splits,
//!   which are a later layer.
//! - `gtk::GLArea` configured `has-stencil-buffer=false`,
//!   `has-depth-buffer=false`, GL 4.3 core (upstream `ui/1.2/surface.blp:34-36`
//!   sets `has-*` + `allowed-apis: gl`; requiring 4.3 forces desktop GL for us).
//! - GLArea `realize` → `make_current` + error check + load GL — upstream
//!   `class/surface.zig:3247` (`glareaRealize`).
//! - GLArea `render` → draw one frame into the bound default FBO — upstream
//!   `class/surface.zig:3347` (`glareaRender`), which calls
//!   `renderer.drawFrame(true)` on the GTK main thread
//!   (`must_draw_from_app_thread`, `class/App.zig:20`).
//! - GLArea `resize` → cache size; lazily init the surface on first resize —
//!   upstream `class/surface.zig:3365` (`glareaResize`, lazy init at `:3419`).
//!
//! ## What this renders
//!
//! The `render` callback draws the **real terminal**: it captures the live vt
//! screen, rebuilds the frame through the font grid + `Engine<OpenGL>`, and
//! **presents** it into the GLArea's framebuffer via the on-screen present seam
//! ([`Engine::draw_and_present`] → `OpenGL::present`, a `glBlitFramebuffer` of
//! the engine target onto the GLArea FBO). The per-surface core lives in
//! [`crate::surface`]; it is fed either by a pty running the user's shell (the
//! interactive [`run`] path) or directly by known bytes (the headless
//! [`run_text_smoke`] proof). Keyboard input is the next chunk.

use std::cell::RefCell;
use std::ffi::c_void;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Once;
use std::time::{Duration, Instant};

use adw::prelude::*;
use glow::HasContext;
use gtk::glib::translate::IntoGlib;
use gtk::{gdk, gio, glib};

use crate::input::gdk_key_to_bytes;
use crate::surface::{Pty, SurfaceState};
use qwertty_term::gesture::{DEFAULT_BEHAVIORS, Drag, Geometry, Press, SelectionGesture};
use qwertty_term::tabkeys::TabAction;
use qwertty_term::tabs::inherit_pwd;
use qwertty_term_input::key::Action;
use qwertty_term_input::key_encode::Options as EncodeOptions;
use qwertty_term_renderer::opengl::{MIN_VERSION_MAJOR, MIN_VERSION_MINOR};

/// Word-boundary codepoints for double-click word selection (the vt default
/// set, the same one the macOS gesture path passes).
const WORD_BOUNDARIES: &[u32] = &qwertty_term_vt::screen::DEFAULT_WORD_BOUNDARIES;

/// The click-repeat interval for double/triple-click detection. GDK doesn't
/// surface the desktop double-click time to us here, so use upstream's fallback
/// (`Config.zig` `click-repeat-interval` default 500ms; the macOS path reads
/// `NSEvent.doubleClickInterval` instead — `gesture::os_click_interval`).
const CLICK_REPEAT_INTERVAL: Duration = Duration::from_millis(500);

/// Application id for the GTK/DBus registration.
const APP_ID: &str = "com.qwertty.TerminalGtk";

/// Embedded font size (px) for the surface's grid (reduced cut: no DPI scaling
/// or config yet).
const FONT_SIZE: f64 = 16.0;

/// The framebuffer clear color (linear RGBA, alpha opaque) — a distinctive
/// slate blue. Used by the clear-only smoke path and as the pre-terminal
/// fallback fill. Deliberately non-black so a headless `glReadPixels` readback
/// can tell "we actually cleared" from an untouched (zero) framebuffer.
pub const CLEAR_COLOR: [f32; 4] = [0.12, 0.16, 0.36, 1.0];

/// The bytes the headless text smoke feeds the terminal — a line of known
/// glyphs on row 0, the rest of the screen left blank (so exactly one cell-row
/// band carries ink and the opposite band is background).
const SMOKE_TEXT: &[u8] = b"QWERTTY TERM :: hello world 0123456789";

/// What [`build_window`]'s render callback does with the GLArea.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// Render the live terminal, fed by a pty running the user's shell. Repaints
    /// continuously (a glib tick); runs until the window closes.
    Interactive,
    /// Clear the framebuffer to [`CLEAR_COLOR`] and read the center pixel back
    /// (the GL-plumbing regression smoke). One frame, then quit.
    ClearSmoke,
    /// Feed [`SMOKE_TEXT`] into the terminal, render + present one frame, then
    /// read the presented framebuffer back and measure glyph ink (the
    /// terminal-renders-text proof). One frame, then quit.
    TextSmoke,
    /// Build the surface at the GLArea's size, render one frame, then
    /// [`SurfaceState::resize`] to a different pixel size and render again —
    /// proving the terminal re-grids and the render target re-sizes with no GL
    /// error at the new size (the resize proof). One resize, then quit.
    ResizeSmoke,
}

/// Outcome of the clear-only smoke: what realize/render observed for a single
/// cleared frame.
#[derive(Debug, Clone, Default)]
pub struct SmokeOutcome {
    /// The GLArea `realize` handler ran and `gl_area.error()` was `None`.
    pub realized: bool,
    /// A non-`None` `gl_area.error()` seen in `realize` (context creation
    /// failed — usually a driver/library issue, not our code).
    pub realize_error: Option<String>,
    /// The `render` handler fired at least once.
    pub rendered: bool,
    /// `glGetError()` immediately after the clear (0 == `GL_NO_ERROR`).
    pub gl_error: u32,
    /// The RGBA of the framebuffer center pixel, read back after the clear.
    pub center_pixel: [u8; 4],
}

impl SmokeOutcome {
    /// True iff the GLArea realized without error, rendered a frame with no GL
    /// error, and the framebuffer center pixel holds [`CLEAR_COLOR`].
    pub fn is_ok(&self) -> bool {
        self.realized
            && self.realize_error.is_none()
            && self.rendered
            && self.gl_error == 0
            && pixel_matches(self.center_pixel, CLEAR_COLOR)
    }
}

impl std::fmt::Display for SmokeOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "realized={} realize_error={:?} rendered={} gl_error=0x{:04x} center_pixel={:?}",
            self.realized, self.realize_error, self.rendered, self.gl_error, self.center_pixel
        )
    }
}

/// Outcome of the text smoke: proof that the GLArea presented **real terminal
/// glyphs** (not just a clear color), measured by reading the presented
/// framebuffer back and counting glyph-bright pixels.
#[derive(Debug, Clone, Default)]
pub struct TextSmokeOutcome {
    /// The GLArea realized a GL context without error.
    pub realized: bool,
    /// A context error seen in `realize`, if any.
    pub realize_error: Option<String>,
    /// The per-surface core (engine + grid + terminal) was built.
    pub surface_init: bool,
    /// The terminal render + present ran.
    pub rendered: bool,
    /// A render/present error, if any.
    pub render_error: Option<String>,
    /// `glGetError()` after present (0 == `GL_NO_ERROR`).
    pub gl_error: u32,
    /// Grid `(cols, rows)`.
    pub grid: (usize, usize),
    /// The presented region sampled `(width, height)` px (= grid × cell).
    pub sample: (usize, usize),
    /// Count of glyph-bright pixels across the whole presented region (ink).
    pub bright_pixels: usize,
    /// Bright pixels in the top cell-row band of the readback (GL bottom-up
    /// coords) and the bottom band — one carries the text line, the other is
    /// blank background (which is which depends on the compositor's flip).
    pub top_band_bright: usize,
    pub bottom_band_bright: usize,
}

impl TextSmokeOutcome {
    /// True iff the surface initialized, a frame rendered+presented without GL
    /// error, and the presented framebuffer carries glyph ink.
    pub fn glyphs_rendered(&self) -> bool {
        self.surface_init
            && self.rendered
            && self.render_error.is_none()
            && self.gl_error == 0
            && self.bright_pixels > 0
    }

    /// True iff exactly one cell-row band carries the text and the opposite band
    /// is (near) blank — a real single-line render, orientation-agnostic.
    pub fn one_band_is_text(&self) -> bool {
        const BAND_FLOOR: usize = 20;
        const BLANK_CEIL: usize = 4;
        (self.top_band_bright > BAND_FLOOR && self.bottom_band_bright <= BLANK_CEIL)
            || (self.bottom_band_bright > BAND_FLOOR && self.top_band_bright <= BLANK_CEIL)
    }
}

impl std::fmt::Display for TextSmokeOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "realized={} surface_init={} rendered={} render_error={:?} gl_error=0x{:04x} \
             grid={:?} sample={:?} bright_pixels={} top_band={} bottom_band={}",
            self.realized,
            self.surface_init,
            self.rendered,
            self.render_error,
            self.gl_error,
            self.grid,
            self.sample,
            self.bright_pixels,
            self.top_band_bright,
            self.bottom_band_bright,
        )
    }
}

/// Outcome of the resize smoke: proof that a GLArea resize re-grids the
/// terminal and re-sizes the render target, still rendering cleanly at the new
/// grid. The surface is built at one size, rendered, resized to a different
/// pixel size, then rendered again.
#[derive(Debug, Clone, Default)]
pub struct ResizeSmokeOutcome {
    /// The GLArea realized a GL context without error.
    pub realized: bool,
    /// A context error seen in `realize`, if any.
    pub realize_error: Option<String>,
    /// The per-surface core was built.
    pub surface_init: bool,
    /// Grid `(cols, rows)` at the initial size.
    pub initial_grid: (usize, usize),
    /// Grid `(cols, rows)` after the resize.
    pub resized_grid: (usize, usize),
    /// The frame at the new size rendered + presented without a Rust-side error.
    pub rendered: bool,
    /// A render/present error at the new size, if any.
    pub render_error: Option<String>,
    /// `glGetError()` after the post-resize present (0 == `GL_NO_ERROR`).
    pub gl_error: u32,
    /// The pty's `(cols, rows)` per `TIOCGWINSZ` after the resize, if a pty was
    /// spawned (proves the `TIOCSWINSZ` propagated). `None` if no pty spawned.
    pub pty_grid: Option<(u16, u16)>,
}

impl ResizeSmokeOutcome {
    /// True iff the surface initialized, the resize produced a *different* grid,
    /// and a frame rendered+presented at the new size with no GL error.
    pub fn regridded(&self) -> bool {
        self.surface_init
            && self.rendered
            && self.render_error.is_none()
            && self.gl_error == 0
            && self.resized_grid != self.initial_grid
            && self.resized_grid.0 >= 1
            && self.resized_grid.1 >= 1
    }
}

impl std::fmt::Display for ResizeSmokeOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "realized={} surface_init={} initial_grid={:?} resized_grid={:?} rendered={} \
             render_error={:?} gl_error=0x{:04x} pty_grid={:?}",
            self.realized,
            self.surface_init,
            self.initial_grid,
            self.resized_grid,
            self.rendered,
            self.render_error,
            self.gl_error,
            self.pty_grid,
        )
    }
}

/// Compare an 8-bit RGBA readback against a float clear color (RGB only, with a
/// small tolerance for the float→8-bit round-trip). Alpha is ignored because
/// the GLArea framebuffer's alpha semantics vary by backend.
fn pixel_matches(px: [u8; 4], color: [f32; 4]) -> bool {
    let expect = |c: f32| (c.clamp(0.0, 1.0) * 255.0).round() as i32;
    const TOL: i32 = 6;
    (0..3).all(|i| (px[i] as i32 - expect(color[i])).abs() <= TOL)
}

/// Load the GL function pointers through `libepoxy` exactly once. `epoxy`
/// resolves GTK's own GL context, so this must be paired with
/// `gl_area.make_current()` before any `glow` call. The standard `gtk4-rs`
/// GLArea loader pattern.
fn ensure_gl_loader() {
    static START: Once = Once::new();
    START.call_once(|| {
        // Keep the library alive for the process lifetime by moving it into the
        // loader closure (`epoxy` stores the closure in a static).
        let lib = unsafe {
            libloading::Library::new("libepoxy.so.0")
                .or_else(|_| libloading::Library::new("libepoxy.so"))
        }
        .expect("failed to load libepoxy (install libepoxy-dev / libepoxy0)");
        epoxy::load_with(move |name| {
            unsafe { lib.get::<*const c_void>(name.as_bytes()) }
                .map(|sym| *sym)
                .unwrap_or(std::ptr::null())
        });
    });
}

/// Build a fresh `glow::Context` over GTK's current GL context (via `epoxy`).
/// Cheap (a dispatch table); `ensure_gl_loader` must already have run.
fn make_glow() -> glow::Context {
    // SAFETY: the GLArea's context is current; `epoxy::get_proc_addr` returns
    // valid entry points (or null, which glow treats as unsupported).
    unsafe { glow::Context::from_loader_function(epoxy::get_proc_addr) }
}

/// What we need and how to get unstuck, printed on any GL setup failure. The
/// `LIBGL_ALWAYS_SOFTWARE` hint carries the most weight in a VM: Mesa's llvmpipe
/// advertises GL 4.5 core, so it runs (slowly) where a translated guest GL can't.
// (No `\`-continuation after the opening quote: it would swallow the first
// line's indent along with the newline.)
const GL_HELP: &str =
    "  This terminal needs a desktop OpenGL 4.3 core context: the renderer's shaders
  are `#version 430 core` and bind SSBOs, which require GL 4.3+ (on GLES, 3.1+).
  See what your driver offers with:
      glxinfo -B
  If \"Max core profile version\" is below 4.3 — common in VMs, where the guest's
  GL is translated to the host's — software rendering works:
      LIBGL_ALWAYS_SOFTWARE=1 cargo run -p qwertty-term-gtk";

/// The raw `GL_VERSION` string. Read as a string rather than via
/// `GL_MAJOR_VERSION`, which exists only on GL 3.0+ — querying it would itself
/// fail on exactly the too-old contexts this is here to diagnose — and it's the
/// most useful thing to show a user when we refuse to run.
fn gl_version_string(gl: &glow::Context) -> String {
    // SAFETY: called with the GLArea's context current.
    unsafe { gl.get_parameter_string(glow::VERSION) }
}

/// `<major>.<minor>` from a desktop `GL_VERSION`, whose spec-mandated prefix is
/// exactly that (`"4.6.0 NVIDIA …"`, `"2.1 Mesa …"`).
///
/// `None` for anything else — including a GLES context's `"OpenGL ES 3.2 …"`,
/// which has no leading digit. Rejecting that is correct rather than incidental:
/// the shaders are `#version 430 core`, which GLES cannot compile at any
/// version. We ask for desktop GL via `set_allowed_apis`, so that's belt and
/// braces.
fn parse_gl_version(version: &str) -> Option<(i32, i32)> {
    let (major, rest) = version.split_whitespace().next()?.split_once('.')?;
    Some((major.parse().ok()?, rest.split('.').next()?.parse().ok()?))
}

/// `GL_RENDERER` — the string that actually names the culprit when a VM caps the
/// version, e.g. `virgl (ANGLE (Apple, Apple M2 Max, OpenGL 4.1 Metal))`.
fn gl_renderer(gl: &glow::Context) -> String {
    // SAFETY: called with the GLArea's context current.
    unsafe { gl.get_parameter_string(glow::RENDERER) }
}

/// The framebuffer GTK has bound for drawing (`GL_DRAW_FRAMEBUFFER_BINDING`) —
/// the target the present blit must write into. `None` = FBO 0.
fn current_draw_fbo(gl: &glow::Context) -> Option<glow::NativeFramebuffer> {
    // SAFETY: plain integer query on the current context.
    let id = unsafe { gl.get_parameter_i32(glow::DRAW_FRAMEBUFFER_BINDING) };
    NonZeroU32::new(id as u32).map(glow::NativeFramebuffer)
}

/// Shared per-window state threaded through the GLArea callbacks.
#[derive(Default)]
struct Shared {
    /// App-side `glow` handle for direct GL (fbo query, smoke readback). Built
    /// in `realize`. Distinct from the engine's own context handle.
    gl: RefCell<Option<glow::Context>>,
    /// The per-surface renderer core, initialized lazily on first render.
    surface: RefCell<Option<SurfaceState>>,
    /// The pty feeding the terminal (interactive mode); `None` until spawned or
    /// if the spawn failed (then a static banner is fed once).
    pty: RefCell<Option<Pty>>,
    /// The working directory the pty should spawn in — the cwd inherited from
    /// the active tab when this surface is a newly-opened tab
    /// (`qwertty_term::tabs::inherit_pwd`). `None` inherits the process cwd.
    cwd: RefCell<Option<std::path::PathBuf>>,
    /// Count of interactive frames this surface has presented. A nonzero value
    /// means the GLArea realized, built its surface, and rendered at least once
    /// — the per-tab render signal the tab-lifecycle proof reads.
    frames: std::cell::Cell<u64>,
    /// Whether the fallback banner was already fed (interactive, no pty).
    banner_fed: RefCell<bool>,
    /// The mouse selection-gesture state machine (reused from the app crate:
    /// `qwertty_term::gesture`). Driven by the GLArea's click/drag controllers;
    /// its output (screen-point bounds) sets the terminal's selection.
    gesture: RefCell<SelectionGesture>,
    /// Clear-smoke result.
    smoke: RefCell<SmokeOutcome>,
    /// Text-smoke result.
    text: RefCell<TextSmokeOutcome>,
    /// Resize-smoke result.
    resize: RefCell<ResizeSmokeOutcome>,
}

/// Render the clear-only smoke frame: clear to [`CLEAR_COLOR`], read the center
/// pixel back. Records into `shared.smoke`.
fn render_clear(area: &gtk::GLArea, gl: &glow::Context, shared: &Shared) {
    let w = area.width().max(1);
    let h = area.height().max(1);
    // SAFETY: GTK's context is current and its framebuffer bound (render cb).
    unsafe {
        gl.viewport(0, 0, w, h);
        gl.clear_color(
            CLEAR_COLOR[0],
            CLEAR_COLOR[1],
            CLEAR_COLOR[2],
            CLEAR_COLOR[3],
        );
        gl.clear(glow::COLOR_BUFFER_BIT);
        gl.flush();
        let gl_error = gl.get_error();
        let (cx, cy) = (w / 2, h / 2);
        let mut px = [0u8; 4];
        gl.read_pixels(
            cx,
            cy,
            1,
            1,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut px)),
        );
        let mut o = shared.smoke.borrow_mut();
        o.rendered = true;
        o.gl_error = gl_error;
        o.center_pixel = px;
    }
}

/// Ensure the per-surface core exists (lazy init on first render, once the
/// GLArea has a real size). `feed_shell` spawns a pty; otherwise the caller
/// feeds bytes directly. Returns whether a surface is now present.
fn ensure_surface(area: &gtk::GLArea, shared: &Shared, feed_shell: bool) -> bool {
    if shared.surface.borrow().is_some() {
        return true;
    }
    let (w, h) = (area.width().max(1), area.height().max(1));
    let surface = SurfaceState::new(make_glow(), w, h, FONT_SIZE);
    let Some(surface) = surface else {
        return false;
    };
    if feed_shell {
        let (cols, rows) = surface.grid_size();
        let cwd = shared.cwd.borrow();
        let pty = Pty::spawn(
            cols as u16,
            rows as u16,
            w as u32,
            h as u32,
            cwd.as_ref().and_then(|p| p.to_str()),
        );
        drop(cwd);
        *shared.pty.borrow_mut() = pty;
    }
    *shared.surface.borrow_mut() = Some(surface);
    true
}

/// Count glyph-bright pixels in an RGBA readback: pixels whose channel sum
/// exceeds `BRIGHT` (dark terminal bg ≈ 0x18·3 = 72 is well below; light glyph
/// text ≈ 0xd8·3 = 648 is above). Background-agnostic ink measure.
fn bright_count(px: &[u8]) -> usize {
    const BRIGHT: u32 = 300;
    px.chunks_exact(4)
        .filter(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > BRIGHT)
        .count()
}

/// Render the text-smoke frame: feed [`SMOKE_TEXT`] once, present the terminal,
/// then read the presented region back and measure glyph ink. Records into
/// `shared.text`.
fn render_text_smoke(area: &gtk::GLArea, gl: &glow::Context, shared: &Shared) {
    // Capture the GLArea's framebuffer FIRST — before `ensure_surface` builds
    // the engine, whose FBO/target creation rebinds the draw framebuffer away
    // from GTK's. The present blit must target the framebuffer GTK had bound on
    // entry, not whatever a resource-creation call left bound.
    let dst = current_draw_fbo(gl);

    if !ensure_surface(area, shared, false) {
        return;
    }
    shared.text.borrow_mut().surface_init = true;

    let (sw, sh, top, bottom);
    {
        let mut sref = shared.surface.borrow_mut();
        let surface = sref.as_mut().expect("surface present");
        // Feed the known text exactly once (idempotent: only on the first frame,
        // which is all the smoke renders).
        surface.feed(SMOKE_TEXT);
        let (cols, rows) = surface.grid_size();
        let (cw, ch) = surface.cell_size();
        sw = cols * cw;
        sh = rows * ch;
        top = ch; // one cell-row band
        bottom = ch;

        let mut t = shared.text.borrow_mut();
        t.grid = (cols, rows);
        t.sample = (sw, sh);
        if let Err(e) = surface.render(dst) {
            t.render_error = Some(e);
        } else {
            t.rendered = true;
        }
    }

    // Read the presented region back from the (now-bound) host framebuffer and
    // measure ink. `present` restored `dst` as the bound framebuffer, so this
    // reads exactly what we blitted.
    if sw == 0 || sh == 0 {
        return;
    }
    let mut buf = vec![0u8; sw * sh * 4];
    // SAFETY: current context; buffer is exactly sw*sh*4 bytes for the region.
    let gl_error = unsafe {
        gl.pixel_store_i32(glow::PACK_ALIGNMENT, 1);
        gl.read_pixels(
            0,
            0,
            sw as i32,
            sh as i32,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelPackData::Slice(Some(&mut buf)),
        );
        gl.get_error()
    };

    // Row stride for band sampling (readback is bottom-up: row 0 = bottom).
    let stride = sw * 4;
    let band = |y0: usize, y1: usize| -> usize {
        let y1 = y1.min(sh);
        (y0..y1)
            .map(|y| bright_count(&buf[y * stride..(y + 1) * stride]))
            .sum()
    };

    let mut t = shared.text.borrow_mut();
    t.gl_error = gl_error;
    t.bright_pixels = bright_count(&buf);
    t.bottom_band_bright = band(0, bottom);
    t.top_band_bright = band(sh.saturating_sub(top), sh);
}

/// Render the resize-smoke frames: build the surface, feed [`SMOKE_TEXT`],
/// render once at the GLArea size, then [`SurfaceState::resize`] to half the
/// pixel size and render again — recording the before/after grid and the GL
/// error at the new size. Spawns a pty so the `TIOCSWINSZ` propagation can be
/// checked too. Records into `shared.resize`.
fn render_resize_smoke(area: &gtk::GLArea, gl: &glow::Context, shared: &Shared) {
    // Capture the GLArea's framebuffer before `ensure_surface` (engine/FBO
    // creation on the first frame rebinds the draw framebuffer).
    let dst = current_draw_fbo(gl);

    // Spawn a pty so the resize can exercise `Pty::resize` (TIOCSWINSZ).
    if !ensure_surface(area, shared, true) {
        return;
    }
    shared.resize.borrow_mut().surface_init = true;

    let (init_w, init_h) = (area.width().max(1), area.height().max(1));

    let mut sref = shared.surface.borrow_mut();
    let surface = sref.as_mut().expect("surface present");
    surface.feed(SMOKE_TEXT);
    shared.resize.borrow_mut().initial_grid = surface.grid_size();
    // First frame at the initial size (also sizes the render target).
    let _ = surface.render(dst);

    // Resize to half the pixel size — a guaranteed-different, smaller grid
    // (the 800×600 default window halves to 400×300, well clear of 1×1).
    let (new_w, new_h) = (init_w / 2, init_h / 2);
    let regridded = surface.resize(new_w, new_h);
    if let Some((cols, rows)) = regridded {
        let (cw, ch) = surface.cell_size();
        if let Some(pty) = shared.pty.borrow_mut().as_mut() {
            pty.resize(
                cols as u16,
                rows as u16,
                (cols * cw) as u32,
                (rows * ch) as u32,
            );
            shared.resize.borrow_mut().pty_grid = pty.winsize();
        }
    }
    let resized_grid = surface.grid_size();

    // Render again at the new size; the engine's `update_frame` resizes its
    // contents + target FBO to the smaller grid.
    let render_error = surface.render(dst).err();
    drop(sref);

    // SAFETY: current context; a plain error query after the present.
    let gl_error = unsafe {
        gl.flush();
        gl.get_error()
    };

    let mut o = shared.resize.borrow_mut();
    o.resized_grid = resized_grid;
    o.rendered = render_error.is_none();
    o.render_error = render_error;
    o.gl_error = gl_error;
}

/// Render the live terminal (interactive): drain any pty output into the
/// terminal, then present a frame. Falls back to a static banner if no pty.
fn render_interactive(area: &gtk::GLArea, gl: &glow::Context, shared: &Shared) {
    // Capture the GLArea's framebuffer before `ensure_surface` (engine/FBO
    // creation on the first frame rebinds the draw framebuffer); the present
    // blit must target the framebuffer GTK bound on entry.
    let dst = current_draw_fbo(gl);

    if !ensure_surface(area, shared, true) {
        // Surface not buildable yet — clear so the window isn't garbage.
        render_clear(area, gl, shared);
        return;
    }

    // Drain the pty (if any) and feed the terminal. With no pty, feed a one-shot
    // banner so the window still shows real glyphs.
    let pty_bytes = shared.pty.borrow().as_ref().map(|p| p.drain());
    let feed_banner = pty_bytes.is_none() && !*shared.banner_fed.borrow();
    let mut replies = Vec::new();
    {
        let mut sref = shared.surface.borrow_mut();
        let surface = sref.as_mut().expect("surface present");
        if let Some(bytes) = pty_bytes {
            surface.feed(&bytes);
            // The terminal may queue replies (DA/DSR/CPR/…) in response; those
            // go back to the pty so query-driven programs work.
            replies = surface.take_pty_replies();
        } else if feed_banner {
            surface.feed(
                b"qwertty-term (gtk) \xe2\x80\x94 select with the mouse; Ctrl+Shift+C/V to copy/paste.\r\n",
            );
        }
        let _ = surface.render(dst);
    }
    if let Some(pty) = shared.pty.borrow().as_ref().filter(|_| !replies.is_empty()) {
        pty.write(&replies);
    }
    if feed_banner {
        *shared.banner_fed.borrow_mut() = true;
    }
    // Record that this surface presented an interactive frame (the per-tab
    // render signal the tab-lifecycle proof reads).
    shared.frames.set(shared.frames.get() + 1);
}

/// Attach a [`gtk::EventControllerKey`] to the GLArea and translate each
/// `key-pressed` into pty bytes: GDK keyval/keycode/`ModifierType` →
/// [`qwertty_term_input::key::KeyEvent`] → `key_encode::encode` →
/// [`Pty::write`]. Mirrors upstream's `EventControllerKey` wiring
/// (`class/surface.zig:3644` binds `key_pressed`; the handler is `keyEvent`,
/// `class/surface.zig:1240`).
///
/// TODO(ime): full IME / dead-key / compose needs a `GtkIMMulticontext`
/// filtering the event first (`class/surface.zig:1246-1334`). This is the
/// direct keyval→bytes path only; the seam is [`crate::input`].
///
/// The encoder options are the default (legacy encoder, normal cursor/keypad
/// modes). TODO(modes): thread live terminal state (DECCKM, kitty flags) from
/// the `SurfaceState`'s `Terminal` into [`EncodeOptions`] so app-cursor-mode
/// arrows and the kitty protocol encode correctly.
fn attach_keyboard(gl_area: &gtk::GLArea, shared: Rc<Shared>, tabs: TabControl) {
    let controller = gtk::EventControllerKey::new();
    let area = gl_area.clone();
    controller.connect_key_pressed(move |_ctrl, keyval, keycode, state| {
        // Tab-management chords take precedence over everything: new/close/next/
        // prev/goto-N. They map to `qwertty_term::tabkeys::TabAction` (the same
        // action enum the macOS app dispatches), so shortcuts match across
        // platforms. Upstream binds these as `new_tab` / `close_tab` /
        // `{next,previous}_tab` / `goto_tab N` (`Config.zig` defaults; dispatched
        // by `Window.performBindingAction`, `class/window.zig:1383`).
        if let Some(action) = tab_shortcut(keyval, state) {
            tabs.perform(&area, &shared, action);
            return glib::Propagation::Stop;
        }
        // Clipboard shortcuts take precedence over the encode path: Ctrl+Shift+C
        // copies the selection, Ctrl+Shift+V pastes. Upstream binds these as the
        // default `copy_to_clipboard` / `paste_from_clipboard` keybinds
        // (`Config.zig` defaults; `Surface.zig` `performBindingAction`).
        if let Some(shortcut) = clipboard_shortcut(keyval, state) {
            match shortcut {
                ClipboardShortcut::Copy => copy_selection(&area, &shared),
                ClipboardShortcut::Paste => paste_clipboard(&area, &shared, false),
            }
            return glib::Propagation::Stop;
        }
        let bytes = gdk_key_to_bytes(
            Action::Press,
            keyval.into_glib(),
            keycode,
            state.bits(),
            &EncodeOptions::default(),
        );
        match bytes {
            Some(bytes) => {
                if let Some(pty) = shared.pty.borrow().as_ref() {
                    pty.write(&bytes);
                }
                // Repaint promptly so the shell's echo shows without waiting
                // for the next tick.
                area.queue_render();
                glib::Propagation::Stop
            }
            None => glib::Propagation::Proceed,
        }
    });
    gl_area.add_controller(controller);
    // The GLArea must hold focus to receive key events.
    gl_area.grab_focus();
}

/// Which clipboard shortcut a key event is, if any.
#[derive(Clone, Copy)]
enum ClipboardShortcut {
    Copy,
    Paste,
}

/// Classify a GDK key event as a clipboard shortcut. Ctrl+Shift+C → Copy,
/// Ctrl+Shift+V → Paste. Both modifiers are required (plain Ctrl+C stays the
/// SIGINT the encoder produces).
fn clipboard_shortcut(keyval: gdk::Key, state: gdk::ModifierType) -> Option<ClipboardShortcut> {
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
    if !(ctrl && shift) {
        return None;
    }
    match keyval.to_unicode().map(|c| c.to_ascii_lowercase()) {
        Some('c') => Some(ClipboardShortcut::Copy),
        Some('v') => Some(ClipboardShortcut::Paste),
        _ => None,
    }
}

/// Map a GLArea pointer position (widget/device pixels — this host has no HiDPI
/// scaling wired, so they coincide) to an absolute *screen* point the selection
/// gesture works in: pixel → grid cell ([`crate::mouse::pixel_to_cell`]) →
/// screen point ([`Engine::window_to_screen_point`] at offset 0). `None` when
/// the cell maps to unwritten space (a blank pad row), where nothing can be
/// selected.
fn screen_point(surface: &SurfaceState, x: f64, y: f64) -> Option<(usize, usize)> {
    let (cw, ch) = surface.cell_size();
    let (cols, rows) = surface.grid_size();
    let (col, row) = crate::mouse::pixel_to_cell(x, y, cw, ch, cols, rows);
    surface.engine().window_to_screen_point(col, row, 0)
}

/// Apply a gesture's selection result to the terminal: `Some(bounds)` sets the
/// selection from the two screen points; `None` clears any selection. Repaints.
fn apply_selection(
    area: &gtk::GLArea,
    shared: &Shared,
    bounds: Option<((usize, usize), (usize, usize))>,
) {
    if let Some(mut sref) = shared.surface.try_borrow_mut().ok().filter(|s| s.is_some()) {
        let engine = sref.as_mut().expect("surface present").engine_mut();
        match bounds {
            Some((a, b)) => {
                engine.select_screen_points(a, b, false);
            }
            None => engine.clear_selection(),
        }
    }
    area.queue_render();
}

/// Copy the current selection to both the CLIPBOARD and the PRIMARY selection
/// (the X11/Wayland middle-click buffer). No-op with no selection. Mirrors
/// upstream's copy path (`Surface.zig` `copyToClipboard` writes both).
fn copy_selection(area: &gtk::GLArea, shared: &Shared) {
    let text = shared
        .surface
        .borrow()
        .as_ref()
        .and_then(|s| s.engine().selection_string());
    let Some(text) = text else {
        return;
    };
    let display = area.display();
    display.clipboard().set_text(&text);
    display.primary_clipboard().set_text(&text);
}

/// Read a clipboard asynchronously and write the (bracketed-paste-aware) bytes
/// to the pty. `primary` selects the PRIMARY selection (middle-click paste)
/// rather than the regular CLIPBOARD. Honors the terminal's bracketed-paste
/// mode via [`crate::mouse::encode_paste`]. Port of upstream's paste path
/// (`Surface.zig` reads the clipboard then frames the data for bracketed paste).
fn paste_clipboard(area: &gtk::GLArea, shared: &Rc<Shared>, primary: bool) {
    let display = area.display();
    let clipboard = if primary {
        display.primary_clipboard()
    } else {
        display.clipboard()
    };
    let shared = Rc::clone(shared);
    let area = area.clone();
    clipboard.read_text_async(gio::Cancellable::NONE, move |res| {
        let Ok(Some(text)) = res else {
            return;
        };
        let bracketed = shared
            .surface
            .borrow()
            .as_ref()
            .map(|s| s.engine().bracketed_paste())
            .unwrap_or(false);
        let bytes = crate::mouse::encode_paste(text.as_str(), bracketed);
        if bytes.is_empty() {
            return;
        }
        if let Some(pty) = shared.pty.borrow().as_ref() {
            pty.write(&bytes);
        }
        area.queue_render();
    });
}

/// Attach the mouse controllers to the GLArea: left-button click+drag for text
/// selection (driving the reused [`SelectionGesture`]), middle-click paste of
/// the PRIMARY selection, and right-click for the copy/paste context menu.
/// Mirrors upstream's per-surface gesture wiring (`class/surface.zig`
/// `mouseButtonCallback` / `cursorPosCallback`; the context menu is
/// `SurfaceView_AppKit.swift` `menu(for:)`, reused here as
/// `qwertty_term::context_menu`).
fn attach_mouse(gl_area: &gtk::GLArea, shared: Rc<Shared>) {
    // --- left button: press starts/extends the selection gesture -----------
    let click = gtk::GestureClick::new();
    click.set_button(gdk::BUTTON_PRIMARY);
    {
        let shared = shared.clone();
        let area = gl_area.clone();
        click.connect_pressed(move |_g, _n_press, x, y| {
            let bounds = {
                let mut gref = shared.gesture.borrow_mut();
                let sref = shared.surface.borrow();
                let Some(surface) = sref.as_ref() else {
                    return;
                };
                let Some(point) = screen_point(surface, x, y) else {
                    return;
                };
                let (cw, _) = surface.cell_size();
                let engine = surface.engine();
                let press = Press {
                    time: Instant::now(),
                    point,
                    xpos: x,
                    ypos: y,
                    max_distance: cw as f64,
                    repeat_interval: CLICK_REPEAT_INTERVAL,
                    alt_screen: engine.alt_screen_active(),
                    behaviors: DEFAULT_BEHAVIORS,
                    boundary_codepoints: WORD_BOUNDARIES,
                };
                gref.press(engine, &press)
            };
            apply_selection(&area, &shared, bounds);
        });
    }
    gl_area.add_controller(click);

    // --- left button drag: extend the selection ----------------------------
    let drag = gtk::GestureDrag::new();
    drag.set_button(gdk::BUTTON_PRIMARY);
    {
        let shared = shared.clone();
        let area = gl_area.clone();
        drag.connect_drag_update(move |g, off_x, off_y| {
            let Some((sx, sy)) = g.start_point() else {
                return;
            };
            let (x, y) = (sx + off_x, sy + off_y);
            let bounds = {
                let mut gref = shared.gesture.borrow_mut();
                let sref = shared.surface.borrow();
                let Some(surface) = sref.as_ref() else {
                    return;
                };
                let Some(point) = screen_point(surface, x, y) else {
                    return;
                };
                let (cw, ch) = surface.cell_size();
                let (cols, rows) = surface.grid_size();
                let engine = surface.engine();
                let d = Drag {
                    point,
                    xpos: x,
                    ypos: y,
                    rectangle: false,
                    alt_screen: engine.alt_screen_active(),
                    geometry: Geometry {
                        columns: cols as u32,
                        cell_width: cw as u32,
                        padding_left: 0,
                        screen_height: (rows * ch) as u32,
                    },
                    boundary_codepoints: WORD_BOUNDARIES,
                };
                gref.drag(engine, &d)
            };
            apply_selection(&area, &shared, bounds);
        });
    }
    {
        let shared = shared.clone();
        drag.connect_drag_end(move |g, off_x, off_y| {
            let point = g.start_point().and_then(|(sx, sy)| {
                let sref = shared.surface.borrow();
                let surface = sref.as_ref()?;
                screen_point(surface, sx + off_x, sy + off_y)
            });
            let alt = shared
                .surface
                .borrow()
                .as_ref()
                .map(|s| s.engine().alt_screen_active())
                .unwrap_or(false);
            shared.gesture.borrow_mut().release(point, alt);
        });
    }
    gl_area.add_controller(drag);

    // --- middle button: paste the PRIMARY selection ------------------------
    let middle = gtk::GestureClick::new();
    middle.set_button(gdk::BUTTON_MIDDLE);
    {
        let shared = shared.clone();
        let area = gl_area.clone();
        middle.connect_pressed(move |_g, _n, _x, _y| {
            paste_clipboard(&area, &shared, true);
        });
    }
    gl_area.add_controller(middle);

    // --- right button: copy/paste context menu -----------------------------
    // A `gio::SimpleActionGroup` under the `term` prefix backs the menu items;
    // the menu model is the reused `qwertty_term::context_menu` (Copy is only
    // present when there's a selection).
    let actions = gio::SimpleActionGroup::new();
    let copy_action = gio::SimpleAction::new("copy", None);
    {
        let shared = shared.clone();
        let area = gl_area.clone();
        copy_action.connect_activate(move |_a, _p| copy_selection(&area, &shared));
    }
    let paste_action = gio::SimpleAction::new("paste", None);
    {
        let shared = shared.clone();
        let area = gl_area.clone();
        paste_action.connect_activate(move |_a, _p| paste_clipboard(&area, &shared, false));
    }
    actions.add_action(&copy_action);
    actions.add_action(&paste_action);
    gl_area.insert_action_group("term", Some(&actions));

    let popover = gtk::PopoverMenu::builder().has_arrow(false).build();
    popover.set_parent(gl_area);

    let right = gtk::GestureClick::new();
    right.set_button(gdk::BUTTON_SECONDARY);
    {
        let shared = shared.clone();
        let popover = popover.clone();
        right.connect_pressed(move |_g, _n, x, y| {
            let has_selection = shared
                .surface
                .borrow()
                .as_ref()
                .map(|s| s.engine().selection_string().is_some())
                .unwrap_or(false);
            // Copy is sensitive only with a selection (mirrors the menu model's
            // `context_items(has_selection)` gating).
            copy_action.set_enabled(has_selection);
            popover.set_menu_model(Some(&context_menu_model(has_selection)));
            popover.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
            popover.popup();
        });
    }
    gl_area.add_controller(right);
}

/// Build the right-click menu model from the reused
/// [`qwertty_term::context_menu`] item list, mapping its Copy/Paste actions to
/// the GLArea's `term.copy` / `term.paste` actions. Splits/close from the app
/// crate's model are dropped (no tabs/splits in this host yet).
fn context_menu_model(has_selection: bool) -> gio::Menu {
    use qwertty_term::context_menu::{ContextAction, ContextItem, context_items};
    let menu = gio::Menu::new();
    for item in context_items(has_selection) {
        if let ContextItem::Action(action) = item {
            let name = match action {
                ContextAction::Copy => "term.copy",
                ContextAction::Paste => "term.paste",
                // Splits/close aren't wired in this host; skip them.
                _ => continue,
            };
            menu.append(Some(action.title()), Some(name));
        }
    }
    menu
}

/// Build a single terminal GLArea and wire its realize/render/resize signals
/// for `mode`. This is the **per-surface** widget factory: every tab gets its
/// own independent instance of this (its own [`Shared`] holding a
/// [`SurfaceState`] + [`Pty`]), mirroring upstream's per-tab surface
/// (`class/tab.zig`, one `Surface` widget per `Tab`). The caller parents it
/// (a plain window for the headless smokes, or an `AdwTabView` page for the
/// interactive tabbed app) and, for the interactive path, attaches the
/// keyboard/mouse controllers + repaint tick.
///
/// For the headless smoke modes the render callback quits the app after one
/// frame; the interactive mode renders continuously and never quits here.
fn build_surface_glarea(app: &adw::Application, shared: &Rc<Shared>, mode: Mode) -> gtk::GLArea {
    let gl_area = gtk::GLArea::new();
    gl_area.set_hexpand(true);
    gl_area.set_vexpand(true);
    gl_area.set_focusable(true);
    gl_area.set_focus_on_click(true);
    // upstream ui/1.2/surface.blp:34-36
    gl_area.set_has_depth_buffer(false);
    gl_area.set_has_stencil_buffer(false);
    // Desktop GL, not GLES — upstream's `allowed-apis: gl` (surface.blp:36).
    // GTK's default is BOTH, and GLES can't satisfy us anyway: the shaders are
    // `#version 430 core` and bind SSBOs, which need GL 4.3 / GLES 3.1+.
    //
    // `GtkGLArea:allowed-apis` is GTK **4.12+** — not 4.6, which is when the
    // similarly-named `GdkGLContext` one landed. So it's absent on our oldest
    // supported GTK (Debian bookworm ships 4.8) and `set_property` would panic
    // with "property 'allowed-apis' not found". Set it where it exists — modern
    // GTK is where a spurious GLES pick actually happens — and elsewhere let the
    // version gate in `realize` catch it, which rejects a GLES context anyway.
    //
    // Assigned as a property rather than through `GLAreaExt::set_allowed_apis`
    // so we keep a GTK 4.6 build floor: gtk4-rs gates that setter behind
    // `v4_12`, and it only assigns this same property (auto/gl_area.rs:422-427).
    if gl_area.find_property("allowed-apis").is_some() {
        gl_area.set_property("allowed-apis", gdk::GLAPI::GL);
    }
    // Deliberately NO `set_required_version(4, 3)`, matching upstream (which
    // sets no required-version anywhere). Asking GTK for 4.3 up front makes a
    // too-old driver fail inside GTK with a bare "unable to create GL context",
    // which names neither the version we need nor the one we got. Instead we
    // take whatever context GTK builds and check it ourselves in `realize`,
    // mirroring upstream's own gate in prepareContext (OpenGL.zig:141-148).

    // realize: make current, check for a context error, load `glow`.
    // Mirrors glareaRealize (surface.zig:3247-3282).
    {
        let shared = shared.clone();
        gl_area.connect_realize(move |area| {
            area.make_current();
            let fail = |msg: String| {
                shared.smoke.borrow_mut().realize_error = Some(msg.clone());
                shared.text.borrow_mut().realize_error = Some(msg.clone());
                shared.resize.borrow_mut().realize_error = Some(msg);
            };
            // Fully-qualified: `error()` also exists on `GLContextExt`.
            if let Some(err) = gtk::prelude::GLAreaExt::error(area) {
                let msg = err.to_string();
                // GTK paints its own terse error into the widget; print an
                // actionable one too, since "unable to create GL context" alone
                // tells a user nothing about what their driver lacks.
                // GTK's own text is already a full sentence ("Unable to create a
                // GL context"), so don't wrap it in a second one.
                eprintln!("qwertty-term-gtk: {msg}");
                eprintln!("{GL_HELP}");
                fail(msg);
                return;
            }
            ensure_gl_loader();
            let gl = make_glow();

            // Upstream's version gate (OpenGL.zig:141-148). GTK hands back the
            // best context the driver offers, so a too-old one realizes happily
            // and would otherwise fail later and far less legibly, down in
            // shader compilation.
            let version = gl_version_string(&gl);
            let usable = matches!(
                parse_gl_version(&version),
                Some(v) if v >= (MIN_VERSION_MAJOR, MIN_VERSION_MINOR)
            );
            if !usable {
                let msg = format!(
                    "unusable OpenGL: this driver reports GL_VERSION {version:?}, but \
                     qwertty-term requires OpenGL {MIN_VERSION_MAJOR}.{MIN_VERSION_MINOR} core"
                );
                eprintln!("qwertty-term-gtk: {msg}");
                eprintln!("  GL_RENDERER: {}", gl_renderer(&gl));
                eprintln!("{GL_HELP}");
                fail(msg);
                return;
            }
            *shared.gl.borrow_mut() = Some(gl);
            shared.smoke.borrow_mut().realized = true;
            shared.text.borrow_mut().realized = true;
            shared.resize.borrow_mut().realized = true;

            // === SURFACE INIT SEAM ===
            // The per-surface core (TabIo/pty + vt `Terminal` + `Engine<OpenGL>`)
            // is created lazily on the first `render` (below), once the GLArea
            // has a real size — matching upstream's lazy init on first resize
            // (surface.zig:3419) so the terminal gets correct dimensions.
        });
    }

    // render: draw one frame on the GTK main thread (the only place GL may
    // draw/present — `must_draw_from_app_thread`, App.zig:20). Mirrors
    // glareaRender (surface.zig:3347-3363).
    {
        let shared = shared.clone();
        let app = app.clone();
        gl_area.connect_render(move |area, _ctx| {
            let guard = shared.gl.borrow();
            let Some(gl) = guard.as_ref() else {
                return glib::Propagation::Stop; // not realized yet
            };

            // ===================== TERMINAL RENDER SEAM =====================
            // The GLArea's default framebuffer is bound + current right here.
            // `SurfaceState::render` captures the vt screen, rebuilds the frame
            // through the font grid + `Engine<OpenGL>`, and blits it onto this
            // framebuffer (`draw_and_present` → `OpenGL::present`).
            match mode {
                Mode::ClearSmoke => render_clear(area, gl, &shared),
                Mode::TextSmoke => render_text_smoke(area, gl, &shared),
                Mode::ResizeSmoke => render_resize_smoke(area, gl, &shared),
                Mode::Interactive => render_interactive(area, gl, &shared),
            }

            if matches!(mode, Mode::ClearSmoke | Mode::TextSmoke | Mode::ResizeSmoke) {
                // One frame is all a headless smoke needs; tear down cleanly.
                app.quit();
            }
            glib::Propagation::Stop
        });
    }

    // resize: re-grid the surface to the new GLArea pixel size. Mirrors
    // glareaResize → Surface.resize (surface.zig:3365-3423): recompute the grid,
    // re-grid the terminal + render target, and `TIOCSWINSZ` the pty so the
    // shell gets a SIGWINCH. The GLArea `resize` signal delivers the framebuffer
    // size in pixels; no HiDPI backing scale is wired in this host (scale 1), so
    // widget px == device px — HiDPI is deferred (see the module TODO(scale)).
    {
        let shared = shared.clone();
        gl_area.connect_resize(move |area, width, height| {
            // Nothing to re-grid until the surface has been lazily built on the
            // first render (the initial `resize` fires before that).
            let mut sref = shared.surface.borrow_mut();
            let Some(surface) = sref.as_mut() else {
                return;
            };
            let Some((cols, rows)) = surface.resize(width, height) else {
                return; // same grid — no re-grid, no pty churn
            };
            let (cw, ch) = surface.cell_size();
            drop(sref);
            // Keep the pty in sync so the child shell re-reads the new size.
            if let Some(pty) = shared.pty.borrow_mut().as_mut() {
                pty.resize(
                    cols as u16,
                    rows as u16,
                    (cols * cw) as u32,
                    (rows * ch) as u32,
                );
            }
            // Repaint at the new grid; the renderer resizes its target FBO on the
            // next frame (Engine::update_frame's size-change path).
            area.queue_render();
        });
    }

    // For the headless smokes, tear down after a single frame (and a hard 5s
    // backstop so a run can never hang). The interactive/tabbed path renders
    // continuously and is torn down by the window closing instead.
    if matches!(mode, Mode::ClearSmoke | Mode::TextSmoke | Mode::ResizeSmoke) {
        gl_area.queue_render();
        let app = app.clone();
        glib::timeout_add_local_once(Duration::from_secs(5), move || {
            app.quit();
        });
    }

    gl_area
}

/// Build a single-surface window for one of the headless smoke `mode`s (the
/// GL-plumbing / text-render / resize proofs). The interactive app uses
/// [`build_tabbed_window`] instead. Mirrors the pre-tabs single-surface window
/// (`class/window.zig:272`), kept for the smoke harnesses.
fn build_window(app: &adw::Application, shared: Rc<Shared>, mode: Mode) {
    let gl_area = build_surface_glarea(app, &shared, mode);
    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(800)
        .default_height(600)
        .title("qwertty-term")
        .content(&gl_area)
        .build();
    window.present();
}

// ============================ TABS ============================
//
// The interactive app is a **multi-surface, tabbed** window. Structure
// (mirrors upstream `class/window.zig`, which composes an `AdwToolbarView`
// holding an `AdwTabBar` top bar + an `AdwTabView` content — `window.zig:264`):
//
//   adw::ApplicationWindow
//     └── gtk::Box (vertical)
//           ├── adw::TabBar   (top bar; `+` new-tab button in its start slot)
//           └── adw::TabView  (content; one page per terminal)
//                 └── gtk::GLArea  (a per-tab `build_surface_glarea` surface)
//
// We substitute a plain vertical `gtk::Box` for `AdwToolbarView` because
// `AdwToolbarView` is libadwaita **1.4** and the minimum-supported runtime
// (Debian bookworm) ships libadwaita **1.2**; the Box gives the identical
// layout (tab bar above, tab content below) with no version bump. `AdwTabView`
// / `AdwTabBar` are 1.0/1.2 and present on bookworm.
//
// Each `AdwTabPage`'s child is a per-tab GLArea; its `Rc<Shared>` (holding the
// tab's own `SurfaceState` + `Pty`) is stashed on the widget via glib object
// data so the active tab's pwd can be read when opening a new tab.

/// The glib object-data key under which each tab GLArea stores its `Rc<Shared>`.
const SHARED_KEY: &str = "qwertty-shared";

/// Stash a tab surface's [`Shared`] on its GLArea widget so it can be recovered
/// from an `AdwTabPage` later (e.g. to read the active tab's pwd).
fn store_shared(area: &gtk::GLArea, shared: &Rc<Shared>) {
    // SAFETY: we only ever read this back as the same `Rc<Shared>` type via
    // `shared_of`, and the widget outlives every such read (both happen on the
    // GTK main thread while the page is live).
    unsafe {
        area.set_data(SHARED_KEY, shared.clone());
    }
}

/// Recover the [`Shared`] stashed on a tab GLArea by [`store_shared`].
fn shared_of(area: &gtk::GLArea) -> Option<Rc<Shared>> {
    // SAFETY: the only writer is `store_shared`, which always stores an
    // `Rc<Shared>`; the pointer is valid for as long as the widget lives.
    unsafe {
        area.data::<Rc<Shared>>(SHARED_KEY)
            .map(|p| p.as_ref().clone())
    }
}

/// A handle to the window's tab group, passed to each surface's key controller
/// so tab chords (new/close/next/prev/goto-N) can act on the shared
/// `AdwTabView`. Cheap to clone (GObject ref-counts).
#[derive(Clone)]
struct TabControl {
    app: adw::Application,
    tab_view: adw::TabView,
}

impl TabControl {
    /// Execute a tab chord against the tab group. `area`/`shared` are the
    /// surface the chord came from (its own pwd seeds an inherited new tab).
    fn perform(&self, area: &gtk::GLArea, shared: &Rc<Shared>, action: TabShortcut) {
        match action {
            TabShortcut::New => {
                // Inherit the triggering (active) tab's pwd if it reported one.
                let pwd = shared
                    .surface
                    .borrow()
                    .as_ref()
                    .and_then(|s| s.engine().pwd());
                add_tab(&self.app, &self.tab_view, inherit_pwd(pwd.as_deref()));
            }
            TabShortcut::Close => {
                if let Some(page) = self.tab_view.selected_page() {
                    self.tab_view.close_page(&page);
                }
            }
            TabShortcut::Nav(nav) => {
                let _ = area; // nav acts on the group, not the source surface
                select_tab(&self.tab_view, nav);
            }
        }
    }
}

/// A tab-management chord: create a new tab, close the active one, or a
/// navigation ([`TabAction`], reused from the app crate so semantics match
/// macOS). Upstream splits these across `new_tab` / `close_tab` /
/// `{next,previous,goto,last}_tab` keybinds (`class/window.zig:1383`,
/// `selectTab` at `:517`).
#[derive(Clone, Copy)]
enum TabShortcut {
    New,
    Close,
    Nav(TabAction),
}

/// Classify a GDK key event as a tab-management chord, if it is one:
/// - Ctrl+Shift+T → new tab
/// - Ctrl+Shift+W → close active tab
/// - Ctrl+PageDown / Ctrl+PageUp → next / previous tab
/// - Alt+1..9 → goto tab N (1-based)
///
/// Mirrors upstream's default tab keybinds (`Config.zig`: `ctrl+shift+t`,
/// `ctrl+shift+w`, `ctrl+page_down`/`ctrl+page_up`, `alt+one`..`alt+nine`).
fn tab_shortcut(keyval: gdk::Key, state: gdk::ModifierType) -> Option<TabShortcut> {
    let ctrl = state.contains(gdk::ModifierType::CONTROL_MASK);
    let shift = state.contains(gdk::ModifierType::SHIFT_MASK);
    let alt = state.contains(gdk::ModifierType::ALT_MASK);

    // Alt+1..9 → goto tab N (no Ctrl/Shift). Matches upstream `alt+one`… .
    if alt && !ctrl {
        match keyval.to_unicode().and_then(|c| c.to_digit(10)) {
            Some(n) if (1..=9).contains(&n) => {
                return Some(TabShortcut::Nav(TabAction::GotoTab(n as usize)));
            }
            _ => {}
        }
    }

    if ctrl {
        // Ctrl+PageDown / Ctrl+PageUp → next / previous (upstream default).
        match keyval {
            gdk::Key::Page_Down => return Some(TabShortcut::Nav(TabAction::NextTab)),
            gdk::Key::Page_Up => return Some(TabShortcut::Nav(TabAction::PreviousTab)),
            _ => {}
        }
        if shift {
            match keyval.to_unicode().map(|c| c.to_ascii_lowercase()) {
                Some('t') => return Some(TabShortcut::New),
                Some('w') => return Some(TabShortcut::Close),
                _ => {}
            }
        }
    }
    None
}

/// Select a tab by [`TabAction`] against the `AdwTabView`. Direct port of
/// upstream `Window.selectTab` (`class/window.zig:517-566`): next/previous wrap,
/// `GotoTab(n)` is 1-based and clamps to the last tab, `LastTab` is the last.
/// Returns whether the selection changed.
fn select_tab(tab_view: &adw::TabView, action: TabAction) -> bool {
    let total = tab_view.n_pages();
    if total == 0 {
        return false;
    }
    let Some(selected) = tab_view.selected_page() else {
        return false;
    };
    let current = tab_view.page_position(&selected);
    let goto: i32 = match action {
        TabAction::PreviousTab => {
            if current > 0 {
                current - 1
            } else {
                total - 1
            }
        }
        TabAction::NextTab => {
            if current < total - 1 {
                current + 1
            } else {
                0
            }
        }
        TabAction::LastTab => total - 1,
        TabAction::GotoTab(n) => {
            if n == 0 {
                return false;
            }
            // 1-based; clamp to the last tab (upstream `min(n-1, total-1)`).
            ((n as i32) - 1).min(total - 1)
        }
    };
    if goto == current {
        return false;
    }
    let page = tab_view.nth_page(goto);
    tab_view.set_selected_page(&page);
    true
}

/// Create a new terminal tab: build a fresh per-tab surface (its own [`Shared`]
/// → [`SurfaceState`] + [`Pty`]), wire its keyboard/mouse controllers + repaint
/// tick, append it as an `AdwTabView` page, and select it. `inherit` is the
/// working directory the new tab's shell starts in (the active tab's pwd, via
/// `inherit_pwd`); `None` uses the process cwd. Returns the tab's [`Shared`].
///
/// Mirrors upstream `Window.newTab` → `Tab.new` + `tab_view.insert` +
/// `setSelectedPage` (`class/window.zig:434-470`).
fn add_tab(
    app: &adw::Application,
    tab_view: &adw::TabView,
    inherit: Option<std::path::PathBuf>,
) -> Rc<Shared> {
    let shared = Rc::new(Shared::default());
    *shared.cwd.borrow_mut() = inherit;

    let gl_area = build_surface_glarea(app, &shared, Mode::Interactive);
    store_shared(&gl_area, &shared);

    let tabs = TabControl {
        app: app.clone(),
        tab_view: tab_view.clone(),
    };
    // Wire keyboard (encode + clipboard + tab chords) and mouse/selection to
    // this surface, scoped to its own `Shared`.
    attach_keyboard(&gl_area, shared.clone(), tabs);
    attach_mouse(&gl_area, shared.clone());

    // Repaint this tab at ~60fps while it lives (no dirty-tracking wakeup yet).
    // A weak ref lets the timer stop itself once the page is closed and the
    // GLArea dropped, so closed tabs don't keep ticking.
    let weak = gl_area.downgrade();
    glib::timeout_add_local(Duration::from_millis(16), move || match weak.upgrade() {
        Some(area) => {
            area.queue_render();
            glib::ControlFlow::Continue
        }
        None => glib::ControlFlow::Break,
    });

    let page = tab_view.append(&gl_area);
    tab_view.set_selected_page(&page);
    gl_area.grab_focus();
    shared
}

/// The active tab's inheritable pwd (its OSC 7 cwd if still a directory), for
/// the `+` new-tab button. Reads the selected page's GLArea `Shared`.
fn active_pwd(tab_view: &adw::TabView) -> Option<std::path::PathBuf> {
    let page = tab_view.selected_page()?;
    let area = page.child().downcast::<gtk::GLArea>().ok()?;
    let shared = shared_of(&area)?;
    let pwd = shared
        .surface
        .borrow()
        .as_ref()
        .and_then(|s| s.engine().pwd());
    inherit_pwd(pwd.as_deref())
}

// ============================ HEADERBAR + PRIMARY MENU ============================
//
// The tabbed window wears app chrome: an `adw::HeaderBar` at the top of the
// content `gtk::Box` (an `adw::ApplicationWindow` has no separate titlebar, so
// the HeaderBar lives *in* the content — the documented libadwaita pattern),
// carrying the `+` new-tab button in its start slot and a primary
// `gtk::MenuButton` (hamburger, `open-menu-symbolic`) in its end slot. Mirrors
// upstream `ui/1.5/window.blp:53-80` (the `Adw.HeaderBar` with a start
// `Adw.SplitButton` `tab-new-symbolic` and an end `Gtk.MenuButton`
// `open-menu-symbolic` bound to `main_menu`).
//
// The HeaderBar shows the window title automatically (upstream binds
// `Adw.WindowTitle.title` to `template.title`, `window.blp:44`); we keep an
// `Adw.WindowTitle` off and let the bar reflect `window.set_title`.
//
// The primary menu is a `gio::Menu`; each item drives a `gio::SimpleAction` in
// the window's `win.` action group (New Tab / Copy / Paste / Preferences /
// About). Accelerators are registered on the application so the menu renders the
// Ctrl+Shift+{T,C,V} shortcut labels (upstream registers the same via
// `gtk_application_set_accels_for_action`).

/// The default window/tab title shown before (or absent) an OSC 0/2 title.
const DEFAULT_TITLE: &str = "qwertty-term";

/// The active tab's terminal surface widget (its `GLArea`), if any.
fn active_area(tab_view: &adw::TabView) -> Option<gtk::GLArea> {
    tab_view
        .selected_page()?
        .child()
        .downcast::<gtk::GLArea>()
        .ok()
}

/// The active tab's surface widget together with its [`Shared`] state.
fn active_area_shared(tab_view: &adw::TabView) -> Option<(gtk::GLArea, Rc<Shared>)> {
    let area = active_area(tab_view)?;
    let shared = shared_of(&area)?;
    Some((area, shared))
}

/// A tab surface's OSC 0/2 title (`Engine::title`), if the surface is built and
/// a title has been set.
fn surface_title(shared: &Shared) -> Option<String> {
    shared
        .surface
        .borrow()
        .as_ref()
        .and_then(|s| s.engine().title())
}

/// Refresh the window title and every tab page's title from the corresponding
/// terminal's OSC 0/2 title, falling back to [`DEFAULT_TITLE`] when unset. The
/// window title tracks the *active* tab. Mirrors upstream's surface-title →
/// page-title binding (`class/window.zig:474-479`, `bindProperty("title", …)`)
/// and window-title binding (`window.blp:44`), refreshed here from the 60 Hz
/// tick since we poll rather than emit a `title` GObject property.
fn refresh_titles(window: &adw::ApplicationWindow, tab_view: &adw::TabView) {
    let n = tab_view.n_pages();
    for i in 0..n {
        let page = tab_view.nth_page(i);
        let Ok(area) = page.child().downcast::<gtk::GLArea>() else {
            continue;
        };
        let Some(shared) = shared_of(&area) else {
            continue;
        };
        let title = surface_title(&shared).unwrap_or_else(|| DEFAULT_TITLE.to_string());
        if page.title().as_str() != title.as_str() {
            page.set_title(&title);
        }
    }

    let active = active_area(tab_view)
        .and_then(|a| shared_of(&a))
        .and_then(|s| surface_title(&s))
        .unwrap_or_else(|| DEFAULT_TITLE.to_string());
    if window.title().as_deref() != Some(active.as_str()) {
        window.set_title(Some(&active));
    }
}

/// Build the primary menu model (the hamburger `gio::Menu`): New Tab, a
/// separator, Copy / Paste, a separator, Preferences (stub), a separator, About.
/// The items target `win.` actions installed by [`install_window_actions`].
/// Mirrors upstream `main_menu` (`ui/1.5/window.blp:220-315`): New Tab, a
/// Clipboard section (Copy/Paste live in the surface context menu upstream but
/// are exposed here for discoverability), Open Configuration (our Preferences
/// stub), and About.
fn build_primary_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    menu.append(Some("New Tab"), Some("win.new-tab"));

    let clip = gio::Menu::new();
    clip.append(Some("Copy"), Some("win.copy"));
    clip.append(Some("Paste"), Some("win.paste"));
    menu.append_section(None, &clip);

    let prefs = gio::Menu::new();
    prefs.append(Some("Preferences"), Some("win.preferences"));
    menu.append_section(None, &prefs);

    let about = gio::Menu::new();
    about.append(Some("About"), Some("win.about"));
    menu.append_section(None, &about);

    menu
}

/// Show the About dialog: a `gtk::AboutDialog` (GTK 4.0 baseline — no
/// libadwaita feature bump; upstream falls back to exactly this via
/// `gtk.showAboutDialog` when Adw dialogs are unsupported, `class/window.zig:1859`).
/// Carries the app name + a short blurb. Mirrors upstream `actionAbout`
/// (`class/window.zig:1832`).
fn show_about(parent: &adw::ApplicationWindow) {
    let about = gtk::AboutDialog::builder()
        .program_name("qwertty-term")
        .comments("A terminal emulator — a full-Rust port of Ghostty (GTK4 host).")
        .website("https://github.com/joshka/qwertty-term")
        .website_label("Project home")
        .modal(true)
        .transient_for(parent)
        .build();
    about.present();
}

/// Show the Preferences placeholder: a minimal transient window noting that the
/// real preferences UI is not yet implemented. Upstream opens the config file /
/// a full config UI (`app.open-config`); a real settings surface is deferred
/// here (TODO(prefs)).
fn show_preferences_stub(parent: &adw::ApplicationWindow) {
    let label = gtk::Label::new(Some(
        "Preferences are not yet implemented.\n\nConfiguration UI is a future slice (TODO(prefs)).",
    ));
    label.set_margin_top(24);
    label.set_margin_bottom(24);
    label.set_margin_start(24);
    label.set_margin_end(24);
    let win = gtk::Window::builder()
        .title("Preferences")
        .transient_for(parent)
        .modal(true)
        .resizable(false)
        .child(&label)
        .build();
    win.present();
}

/// Install the window's `win.` action group backing the primary menu: New Tab,
/// Copy, Paste, Preferences, About. Copy/Paste operate on the *active* tab's
/// surface (recovered from the selected `AdwTabPage`'s GLArea). Mirrors
/// upstream's window action set (`class/window.zig:363-367` + the `win.new-tab`
/// / `win.about` actions the menu targets).
fn install_window_actions(
    window: &adw::ApplicationWindow,
    app: &adw::Application,
    tab_view: &adw::TabView,
) {
    // New Tab → the same path as the `+` button and Ctrl+Shift+T.
    let new_tab = gio::SimpleAction::new("new-tab", None);
    {
        let app = app.clone();
        let tab_view = tab_view.clone();
        new_tab.connect_activate(move |_, _| {
            let pwd = active_pwd(&tab_view);
            add_tab(&app, &tab_view, pwd);
        });
    }
    window.add_action(&new_tab);

    // Copy → copy the active tab's selection (no-op with no selection).
    let copy = gio::SimpleAction::new("copy", None);
    {
        let tab_view = tab_view.clone();
        copy.connect_activate(move |_, _| {
            if let Some((area, shared)) = active_area_shared(&tab_view) {
                copy_selection(&area, &shared);
            }
        });
    }
    window.add_action(&copy);

    // Paste → paste the CLIPBOARD into the active tab (bracketed-paste aware).
    let paste = gio::SimpleAction::new("paste", None);
    {
        let tab_view = tab_view.clone();
        paste.connect_activate(move |_, _| {
            if let Some((area, shared)) = active_area_shared(&tab_view) {
                paste_clipboard(&area, &shared, false);
            }
        });
    }
    window.add_action(&paste);

    // Preferences → the placeholder dialog (stub).
    let preferences = gio::SimpleAction::new("preferences", None);
    {
        let weak = window.downgrade();
        preferences.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                show_preferences_stub(&win);
            }
        });
    }
    window.add_action(&preferences);

    // About → the About dialog.
    let about = gio::SimpleAction::new("about", None);
    {
        let weak = window.downgrade();
        about.connect_activate(move |_, _| {
            if let Some(win) = weak.upgrade() {
                show_about(&win);
            }
        });
    }
    window.add_action(&about);

    // Register accelerators so the menu renders the shortcut labels; GTK's
    // window-level (capture-phase) shortcut controller activates these before the
    // GLArea key controller sees the chord, so there is no double-dispatch.
    app.set_accels_for_action("win.new-tab", &["<Ctrl><Shift>t"]);
    app.set_accels_for_action("win.copy", &["<Ctrl><Shift>c"]);
    app.set_accels_for_action("win.paste", &["<Ctrl><Shift>v"]);
}

/// Assemble the tabbed window's widgets: an `adw::HeaderBar` (start `+` button,
/// end primary `MenuButton`) above an `AdwTabBar` above an `AdwTabView`, all in a
/// vertical `gtk::Box` content. Installs the `win.` menu actions, wires close +
/// last-tab-closes-window, and starts the title-refresh tick. Returns the
/// assembled handles so both the interactive path and the headless smoke can
/// drive them. Mirrors upstream `class/window.zig` composition +
/// `tabViewClosePage` (`:1500`) + `tabViewNPages` (`:1662`) + the headerbar
/// (`ui/1.5/window.blp:53-80`).
struct WindowParts {
    window: adw::ApplicationWindow,
    tab_view: adw::TabView,
}

fn build_window_parts(app: &adw::Application) -> WindowParts {
    let tab_view = adw::TabView::new();
    tab_view.set_vexpand(true);

    let tab_bar = adw::TabBar::new();
    tab_bar.set_view(Some(&tab_view));

    // Primary menu button (hamburger) in the headerbar end slot.
    let menu_button = gtk::MenuButton::new();
    menu_button.set_icon_name("open-menu-symbolic");
    menu_button.set_tooltip_text(Some("Main Menu"));
    menu_button.set_menu_model(Some(&build_primary_menu()));

    // The `+` new-tab button in the headerbar start slot, inheriting the active
    // tab's pwd. Upstream exposes new-tab via a header split button
    // (`ui/1.5/window.blp:53`); the chord is Ctrl+Shift+T.
    let new_button = gtk::Button::from_icon_name("tab-new-symbolic");
    new_button.set_tooltip_text(Some("New Tab (Ctrl+Shift+T)"));
    {
        let app = app.clone();
        let tab_view = tab_view.clone();
        new_button.connect_clicked(move |_| {
            let pwd = active_pwd(&tab_view);
            add_tab(&app, &tab_view, pwd);
        });
    }

    let header = adw::HeaderBar::new();
    header.pack_start(&new_button);
    header.pack_end(&menu_button);

    // Content: [HeaderBar, TabBar, TabView] (an adw::ApplicationWindow has no
    // separate titlebar, so the HeaderBar lives in the content box).
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
    vbox.append(&header);
    vbox.append(&tab_bar);
    vbox.append(&tab_view);

    // Close a page deterministically when requested (no confirmation dialog in
    // this MVP): finish the close immediately. Upstream `tabViewClosePage`
    // (`class/window.zig:1500`) optionally shows a confirmation first.
    tab_view.connect_close_page(move |tv, page| {
        tv.close_page_finish(page, true);
        glib::Propagation::Stop
    });

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(800)
        .default_height(600)
        .title(DEFAULT_TITLE)
        .content(&vbox)
        .build();

    install_window_actions(&window, app, &tab_view);

    // When the last tab closes, close the window (upstream `tabViewNPages`,
    // `class/window.zig:1662`). Weak ref so the closure doesn't pin the window.
    {
        let weak = window.downgrade();
        tab_view.connect_n_pages_notify(move |tv| {
            if tv.n_pages() != 0 {
                return;
            }
            if let Some(win) = weak.upgrade() {
                win.close();
            }
        });
    }

    // Refresh the window title immediately when the active tab changes…
    {
        let window_weak = window.downgrade();
        let tv = tab_view.clone();
        tab_view.connect_selected_page_notify(move |_| {
            if let Some(win) = window_weak.upgrade() {
                refresh_titles(&win, &tv);
            }
        });
    }
    // …and poll the active surface's OSC 0/2 title a few times a second so the
    // window/tab titles track the running program (no `title` GObject property to
    // bind against, so we poll rather than notify).
    {
        let window_weak = window.downgrade();
        let tab_view = tab_view.clone();
        glib::timeout_add_local(Duration::from_millis(150), move || {
            match window_weak.upgrade() {
                Some(win) => {
                    refresh_titles(&win, &tab_view);
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            }
        });
    }

    WindowParts { window, tab_view }
}

/// Assemble and present the interactive tabbed window, then add the first tab.
fn build_tabbed_window(app: &adw::Application) {
    let parts = build_window_parts(app);
    parts.window.present();
    // The first tab (inherits nothing — process cwd).
    add_tab(app, &parts.tab_view, None);
}

/// Run the GTK application interactively — opens the tabbed window, each tab an
/// independent terminal (its own `GLArea` + `SurfaceState` + `Pty`). Returns the
/// process exit code.
pub fn run() -> std::process::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(build_tabbed_window);
    // Empty arg list: our own flags (e.g. `--smoke`) are handled before this and
    // must not be parsed by GTK.
    let code = app.run_with_args::<&str>(&[]);
    if code.value() == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// Run the clear-only smoke headlessly for a single frame (GL-plumbing
/// regression). Intended for the `--smoke` bin flag and the headless GTK test.
pub fn run_smoke() -> SmokeOutcome {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let shared = Rc::new(Shared::default());
    {
        let shared = shared.clone();
        app.connect_activate(move |app| {
            build_window(app, shared.clone(), Mode::ClearSmoke);
        });
    }
    let _ = app.run_with_args::<&str>(&[]);
    shared.smoke.borrow().clone()
}

/// Run the text smoke headlessly for a single frame: feed known text into the
/// terminal, render + present it into the GLArea, and read the presented pixels
/// back to prove **real glyph ink** reached the framebuffer. Intended for the
/// `--text-smoke` bin flag and the headless GTK text-render test.
pub fn run_text_smoke() -> TextSmokeOutcome {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let shared = Rc::new(Shared::default());
    {
        let shared = shared.clone();
        app.connect_activate(move |app| {
            build_window(app, shared.clone(), Mode::TextSmoke);
        });
    }
    let _ = app.run_with_args::<&str>(&[]);
    shared.text.borrow().clone()
}

/// Run the resize smoke headlessly: build the surface, render a frame, resize
/// the surface to a different pixel size, and render again — proving the
/// terminal re-grids and the render target re-sizes with no GL error. Intended
/// for the headless GTK resize test.
pub fn run_resize_smoke() -> ResizeSmokeOutcome {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let shared = Rc::new(Shared::default());
    {
        let shared = shared.clone();
        app.connect_activate(move |app| {
            build_window(app, shared.clone(), Mode::ResizeSmoke);
        });
    }
    let _ = app.run_with_args::<&str>(&[]);
    shared.resize.borrow().clone()
}

/// Outcome of the tab-lifecycle smoke: proof that the tabbed window creates,
/// switches, and closes independent per-tab terminals. Two tabs are opened, each
/// given a chance to realize + render its own surface, the selection is switched
/// between them, and one is closed — recording the tab count and per-tab surface
/// independence at each step.
#[derive(Debug, Clone, Default)]
pub struct TabLifecycleOutcome {
    /// `AdwTabView.n_pages()` after opening the two tabs (expect 2).
    pub pages_after_two_opened: usize,
    /// The two tabs' `Shared` are distinct allocations (independent state).
    pub distinct_surfaces: bool,
    /// Tab 1 built its own `SurfaceState` / spawned its own `Pty` / rendered.
    pub tab1_surface_init: bool,
    pub tab1_pty: bool,
    pub tab1_rendered: bool,
    /// Tab 2 built its own `SurfaceState` / spawned its own `Pty` / rendered.
    pub tab2_surface_init: bool,
    pub tab2_pty: bool,
    pub tab2_rendered: bool,
    /// Switching the selected page (to tab 1) took effect.
    pub switch_selected_ok: bool,
    /// `AdwTabView.n_pages()` after closing one tab (expect 1).
    pub pages_after_close: usize,
    /// A GL context error seen in any tab's `realize`, if any.
    pub realize_error: Option<String>,
}

impl TabLifecycleOutcome {
    /// True iff the full lifecycle held: two independent tabs each realized +
    /// rendered their own surface + pty, switching worked, and closing dropped
    /// the count from 2 to 1.
    pub fn is_ok(&self) -> bool {
        self.realize_error.is_none()
            && self.pages_after_two_opened == 2
            && self.distinct_surfaces
            && self.tab1_surface_init
            && self.tab1_pty
            && self.tab1_rendered
            && self.tab2_surface_init
            && self.tab2_pty
            && self.tab2_rendered
            && self.switch_selected_ok
            && self.pages_after_close == 1
    }
}

impl std::fmt::Display for TabLifecycleOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "pages_after_two_opened={} distinct_surfaces={} \
             tab1(surface_init={} pty={} rendered={}) \
             tab2(surface_init={} pty={} rendered={}) \
             switch_selected_ok={} pages_after_close={} realize_error={:?}",
            self.pages_after_two_opened,
            self.distinct_surfaces,
            self.tab1_surface_init,
            self.tab1_pty,
            self.tab1_rendered,
            self.tab2_surface_init,
            self.tab2_pty,
            self.tab2_rendered,
            self.switch_selected_ok,
            self.pages_after_close,
            self.realize_error,
        )
    }
}

/// Record one tab's per-surface state (surface built / pty spawned / rendered /
/// realize error) into the lifecycle outcome.
fn record_tab(o: &mut TabLifecycleOutcome, shared: &Rc<Shared>, tab1: bool) {
    let surface_init = shared.surface.borrow().is_some();
    let pty = shared.pty.borrow().is_some();
    let rendered = shared.frames.get() > 0;
    if o.realize_error.is_none() {
        o.realize_error = shared.text.borrow().realize_error.clone();
    }
    if tab1 {
        o.tab1_surface_init = surface_init;
        o.tab1_pty = pty;
        o.tab1_rendered = rendered;
    } else {
        o.tab2_surface_init = surface_init;
        o.tab2_pty = pty;
        o.tab2_rendered = rendered;
    }
}

/// Run the tab-lifecycle smoke headlessly: build the tabbed window, open a
/// second tab, let each realize + render its own surface, switch the selected
/// page, then close one tab — proving the multi-surface tab model
/// (create/switch/close, one independent `SurfaceState`+`Pty` per tab). Driven
/// by chained main-loop timeouts (rather than injected GTK key events) so it
/// stays deterministic under Xvfb.
pub fn run_tab_lifecycle_smoke() -> TabLifecycleOutcome {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let outcome = Rc::new(RefCell::new(TabLifecycleOutcome::default()));

    // Per-step dwell: enough for a headless GLArea to map + realize + render.
    const STEP: Duration = Duration::from_millis(300);

    {
        let outcome = outcome.clone();
        app.connect_activate(move |app| {
            // Build the tab group directly (like `build_tabbed_window`) but keep
            // the handles so the orchestrator can drive it.
            let tab_view = adw::TabView::new();
            tab_view.set_vexpand(true);
            let tab_bar = adw::TabBar::new();
            tab_bar.set_view(Some(&tab_view));
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 0);
            vbox.append(&tab_bar);
            vbox.append(&tab_view);
            tab_view.connect_close_page(move |tv, page| {
                tv.close_page_finish(page, true);
                glib::Propagation::Stop
            });
            let window = adw::ApplicationWindow::builder()
                .application(app)
                .default_width(800)
                .default_height(600)
                .title("qwertty-term")
                .content(&vbox)
                .build();
            window.present();

            // Tab 1.
            let s1 = add_tab(app, &tab_view, None);

            // Step 1: after tab 1 has rendered, open tab 2.
            let app1 = app.clone();
            let tv1 = tab_view.clone();
            let out1 = outcome.clone();
            glib::timeout_add_local_once(STEP, move || {
                let s2 = add_tab(&app1, &tv1, None);

                // Step 2: after tab 2 (now selected) has rendered, record both,
                // then switch the selection back to tab 1.
                let tv2 = tv1.clone();
                let out2 = out1.clone();
                let s1b = s1.clone();
                let s2b = s2.clone();
                glib::timeout_add_local_once(STEP, move || {
                    {
                        let mut o = out2.borrow_mut();
                        o.pages_after_two_opened = tv2.n_pages() as usize;
                        o.distinct_surfaces = !Rc::ptr_eq(&s1b, &s2b);
                        // Tab 2 is currently selected → it has had its render.
                        record_tab(&mut o, &s2b, false);
                    }
                    // Switch selection to tab 1 (page 0).
                    let page0 = tv2.nth_page(0);
                    tv2.set_selected_page(&page0);
                    {
                        let mut o = out2.borrow_mut();
                        o.switch_selected_ok = tv2
                            .selected_page()
                            .map(|p| tv2.page_position(&p) == 0)
                            .unwrap_or(false);
                    }

                    // Step 3: after tab 1 re-renders as the selected page, record
                    // it, then close tab 2.
                    let tv3 = tv2.clone();
                    let out3 = out2.clone();
                    let s1c = s1b.clone();
                    glib::timeout_add_local_once(STEP, move || {
                        record_tab(&mut out3.borrow_mut(), &s1c, true);
                        if tv3.n_pages() >= 2 {
                            let last = tv3.nth_page(1);
                            tv3.close_page(&last);
                        }

                        // Step 4: record the post-close count and quit.
                        let tv4 = tv3.clone();
                        let out4 = out3.clone();
                        glib::timeout_add_local_once(STEP, move || {
                            out4.borrow_mut().pages_after_close = tv4.n_pages() as usize;
                            if let Some(app) = tv4
                                .root()
                                .and_then(|r| r.downcast::<gtk::Window>().ok())
                                .and_then(|w| w.application())
                            {
                                app.quit();
                            }
                        });
                    });
                });
            });
        });
    }

    // Hard backstop so a run can never hang (e.g. a GLArea that never maps).
    {
        let app_weak = app.downgrade();
        glib::timeout_add_local_once(Duration::from_secs(20), move || {
            if let Some(app) = app_weak.upgrade() {
                app.quit();
            }
        });
    }

    let _ = app.run_with_args::<&str>(&[]);
    // Clone the result out of the shared cell: the `connect_activate` closure
    // still holds an `Rc` clone (it lives as long as `app`), so `Rc::into_inner`
    // would see >1 strong ref. The same pattern the other smoke runners use.
    outcome.borrow().clone()
}

/// Depth-first search of a widget subtree for the first descendant that
/// downcasts to `T`. Used by the headerbar smoke to prove the `adw::HeaderBar`
/// and its `gtk::MenuButton` are actually present in the presented window tree
/// (not merely constructed).
fn find_descendant<T: glib::object::IsA<gtk::Widget>>(root: &gtk::Widget) -> Option<T> {
    if let Ok(found) = root.clone().downcast::<T>() {
        return Some(found);
    }
    let mut child = root.first_child();
    while let Some(c) = child {
        if let Some(found) = find_descendant::<T>(&c) {
            return Some(found);
        }
        child = c.next_sibling();
    }
    None
}

/// Inspect the presented window for its app chrome: `(has_headerbar,
/// has_menu_button, primary_menu_item_count)`. Walks the real widget tree so the
/// headerbar smoke proves the chrome is present, not merely constructed.
fn inspect_chrome(window: &adw::ApplicationWindow) -> (bool, bool, usize) {
    let Some(root) = window.content() else {
        return (false, false, 0);
    };
    let Some(header) = find_descendant::<adw::HeaderBar>(&root) else {
        return (false, false, 0);
    };
    let Some(button) = find_descendant::<gtk::MenuButton>(header.upcast_ref::<gtk::Widget>())
    else {
        return (true, false, 0);
    };
    let count = button
        .menu_model()
        .map(|m| m.n_items().max(0) as usize)
        .unwrap_or(0);
    (true, true, count)
}

/// Outcome of the headerbar smoke: proof that the tabbed window wears its app
/// chrome (a HeaderBar with a primary MenuButton), that the primary menu's
/// **New Tab** action creates a second tab, and that the active terminal's OSC
/// 0/2 title propagates to both the tab page title and the window title.
#[derive(Debug, Clone, Default)]
pub struct HeaderbarOutcome {
    /// An `adw::HeaderBar` is present in the presented window's widget tree.
    pub has_headerbar: bool,
    /// A `gtk::MenuButton` is present inside that HeaderBar.
    pub has_menu_button: bool,
    /// The MenuButton carries a non-empty primary menu model.
    pub menu_item_count: usize,
    /// `AdwTabView.n_pages()` after activating the `win.new-tab` menu action once
    /// (the window opened with one tab, so expect 2).
    pub pages_after_new_tab_action: usize,
    /// The active tab page's title after feeding `ESC]0;hello BEL` (expect
    /// "hello").
    pub tab_title: Option<String>,
    /// The window title after the same feed (expect "hello").
    pub window_title: Option<String>,
    /// A GL context error seen in any tab's `realize`, if any.
    pub realize_error: Option<String>,
}

impl HeaderbarOutcome {
    /// True iff the chrome is present, the New Tab menu action opened a second
    /// tab, and the OSC 0/2 title reached both the tab page and the window title.
    pub fn is_ok(&self) -> bool {
        self.realize_error.is_none()
            && self.has_headerbar
            && self.has_menu_button
            && self.menu_item_count > 0
            && self.pages_after_new_tab_action == 2
            && self.tab_title.as_deref() == Some("hello")
            && self.window_title.as_deref() == Some("hello")
    }
}

impl std::fmt::Display for HeaderbarOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "has_headerbar={} has_menu_button={} menu_item_count={} \
             pages_after_new_tab_action={} tab_title={:?} window_title={:?} realize_error={:?}",
            self.has_headerbar,
            self.has_menu_button,
            self.menu_item_count,
            self.pages_after_new_tab_action,
            self.tab_title,
            self.window_title,
            self.realize_error,
        )
    }
}

/// Run the headerbar smoke headlessly: build the real tabbed window (headerbar +
/// primary menu + title binding via [`build_window_parts`]), assert the chrome
/// is present, drive the **New Tab** `gio::Action` to open a second tab, feed
/// `ESC]0;hello BEL` (OSC 0) into the active tab's terminal, refresh titles, and
/// read the tab page title + window title back — proving the menu wiring and the
/// OSC-title binding end-to-end. Driven by chained main-loop timeouts so it stays
/// deterministic under Xvfb.
pub fn run_headerbar_smoke() -> HeaderbarOutcome {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let outcome = Rc::new(RefCell::new(HeaderbarOutcome::default()));

    const STEP: Duration = Duration::from_millis(300);

    {
        let outcome = outcome.clone();
        app.connect_activate(move |app| {
            let parts = build_window_parts(app);
            let window = parts.window.clone();
            let tab_view = parts.tab_view.clone();

            // Structural chrome checks against the *presented* widget tree.
            {
                let (has_headerbar, has_menu_button, menu_item_count) = inspect_chrome(&window);
                let mut o = outcome.borrow_mut();
                o.has_headerbar = has_headerbar;
                o.has_menu_button = has_menu_button;
                o.menu_item_count = menu_item_count;
            }

            window.present();
            // Open the first tab (as the interactive path does).
            add_tab(app, &tab_view, None);

            // Step 1: after tab 1 renders, fire the New Tab *menu action*.
            let out1 = outcome.clone();
            let window1 = window.clone();
            let tv1 = tab_view.clone();
            glib::timeout_add_local_once(STEP, move || {
                // Drive the gio::Action directly — the same path the menu item hits.
                // (ActionGroupExt activates on the window's own group, bare name.)
                gio::prelude::ActionGroupExt::activate_action(&window1, "new-tab", None);

                // Step 2: after the second tab renders, record the page count,
                // feed OSC 0 into the active tab, refresh titles, read them back.
                let out2 = out1.clone();
                let window2 = window1.clone();
                let tv2 = tv1.clone();
                glib::timeout_add_local_once(STEP, move || {
                    out2.borrow_mut().pages_after_new_tab_action = tv2.n_pages() as usize;

                    // Feed an OSC 0 title-set into the active tab's terminal.
                    if let Some((_area, shared)) = active_area_shared(&tv2) {
                        if let Some(surface) = shared.surface.borrow_mut().as_mut() {
                            surface.feed(b"\x1b]0;hello\x07");
                        }
                        if let Some(err) = shared.text.borrow().realize_error.clone() {
                            out2.borrow_mut().realize_error = Some(err);
                        }
                    }

                    // Bind the titles from the freshly-set OSC title.
                    refresh_titles(&window2, &tv2);

                    {
                        let mut o = out2.borrow_mut();
                        o.tab_title = tv2.selected_page().map(|p| p.title().to_string());
                        o.window_title = window2.title().map(|t| t.to_string());
                    }

                    // Quit.
                    if let Some(app) = window2.application() {
                        app.quit();
                    }
                });
            });
        });
    }

    // Hard backstop so a run can never hang.
    {
        let app_weak = app.downgrade();
        glib::timeout_add_local_once(Duration::from_secs(20), move || {
            if let Some(app) = app_weak.upgrade() {
                app.quit();
            }
        });
    }

    let _ = app.run_with_args::<&str>(&[]);
    outcome.borrow().clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `GL_VERSION` strings. The 2.1/virgl case is a QEMU guest on an Apple
    /// Silicon host, where the guest's GL is translated through ANGLE onto Metal
    /// and lands far below our floor — the failure this gate exists to explain.
    #[test]
    fn parses_desktop_gl_versions() {
        assert_eq!(parse_gl_version("4.6.0 NVIDIA 550.54.14"), Some((4, 6)));
        assert_eq!(
            parse_gl_version("4.5 (Core Profile) Mesa 24.0.0 (llvmpipe)"),
            Some((4, 5))
        );
        assert_eq!(parse_gl_version("4.3.0 build 31.0.101"), Some((4, 3)));
        assert_eq!(parse_gl_version("2.1 Mesa 26.0.3-1ubuntu1"), Some((2, 1)));
    }

    /// A GLES context has no leading digit, so it fails to parse — and the
    /// caller treats "unparseable" as unusable, which is the right answer for
    /// GLES regardless of version: `#version 430 core` is desktop-only.
    #[test]
    fn rejects_gles_and_garbage() {
        assert_eq!(parse_gl_version("OpenGL ES 3.2 Mesa 26.0.3"), None);
        assert_eq!(parse_gl_version("OpenGL ES 3.0 Mesa 26.0.3"), None);
        assert_eq!(parse_gl_version(""), None);
        assert_eq!(parse_gl_version("nonsense"), None);
    }

    /// The gate itself: only >= 4.3 desktop survives.
    #[test]
    fn version_floor_matches_upstream() {
        let usable = |s: &str| matches!(parse_gl_version(s), Some(v) if v >= (MIN_VERSION_MAJOR, MIN_VERSION_MINOR));
        assert!(usable("4.5 (Core Profile) Mesa 24.0.0 (llvmpipe)"));
        assert!(usable("4.3.0 build 31.0.101"));
        assert!(!usable("4.2.0 build 31.0.101"));
        assert!(!usable("2.1 Mesa 26.0.3-1ubuntu1"));
        assert!(!usable("OpenGL ES 3.2 Mesa 26.0.3"));
    }
}
