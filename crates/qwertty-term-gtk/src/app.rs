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
use qwertty_term_input::key::Action;
use qwertty_term_input::key_encode::Options as EncodeOptions;

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
        let pty = Pty::spawn(cols as u16, rows as u16, w as u32, h as u32);
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
fn attach_keyboard(gl_area: &gtk::GLArea, shared: Rc<Shared>) {
    let controller = gtk::EventControllerKey::new();
    let area = gl_area.clone();
    controller.connect_key_pressed(move |_ctrl, keyval, keycode, state| {
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

/// Build the GLArea and its parent window, wiring the realize/render/resize
/// signals for `mode`.
fn build_window(app: &adw::Application, shared: Rc<Shared>, mode: Mode) {
    let gl_area = gtk::GLArea::new();
    gl_area.set_hexpand(true);
    gl_area.set_vexpand(true);
    gl_area.set_focusable(true);
    gl_area.set_focus_on_click(true);
    // upstream ui/1.2/surface.blp:34-36
    gl_area.set_has_depth_buffer(false);
    gl_area.set_has_stencil_buffer(false);
    // GL 4.3 core (upstream renderer/OpenGL.zig). Requiring 4.3 already forces a
    // *desktop* GL context (GLES has no 4.3), which is what upstream's
    // `allowed-apis: gl` (surface.blp:36) achieves.
    gl_area.set_required_version(4, 3);

    // realize: make current, check for a context error, load `glow`.
    // Mirrors glareaRealize (surface.zig:3247-3282).
    {
        let shared = shared.clone();
        gl_area.connect_realize(move |area| {
            area.make_current();
            // Fully-qualified: `error()` also exists on `GLContextExt`.
            if let Some(err) = gtk::prelude::GLAreaExt::error(area) {
                let msg = err.to_string();
                shared.smoke.borrow_mut().realize_error = Some(msg.clone());
                shared.text.borrow_mut().realize_error = Some(msg.clone());
                shared.resize.borrow_mut().realize_error = Some(msg);
                return;
            }
            ensure_gl_loader();
            *shared.gl.borrow_mut() = Some(make_glow());
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

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(800)
        .default_height(600)
        .title("qwertty-term")
        .content(&gl_area)
        .build();
    window.present();

    match mode {
        Mode::Interactive => {
            // Wire keyboard input: keystrokes → encoded bytes → pty.
            attach_keyboard(&gl_area, shared.clone());
            // Wire mouse selection, clipboard, and the right-click menu.
            attach_mouse(&gl_area, shared.clone());
            // Repaint at ~60fps so pty output shows promptly (no dirty-tracking
            // wakeup yet). Cheap: a full redraw of a small grid.
            let area = gl_area.clone();
            glib::timeout_add_local(Duration::from_millis(16), move || {
                area.queue_render();
                glib::ControlFlow::Continue
            });
        }
        Mode::ClearSmoke | Mode::TextSmoke | Mode::ResizeSmoke => {
            // Force a first frame and guarantee termination even if `render`
            // never fires (e.g. the GLArea never maps), so a headless run can't
            // hang.
            gl_area.queue_render();
            let app = app.clone();
            glib::timeout_add_local_once(Duration::from_secs(5), move || {
                app.quit();
            });
        }
    }
}

/// Run the GTK application interactively — opens a window that renders the live
/// terminal (a pty running the user's shell). Returns the process exit code.
pub fn run() -> std::process::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let shared = Rc::new(Shared::default());
    app.connect_activate(move |app| {
        build_window(app, shared.clone(), Mode::Interactive);
    });
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
