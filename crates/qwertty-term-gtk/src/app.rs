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
use std::time::Duration;

use adw::prelude::*;
use glow::HasContext;
use gtk::glib;
use gtk::glib::translate::IntoGlib;

use crate::input::gdk_key_to_bytes;
use crate::surface::{Pty, SurfaceState};
use qwertty_term_input::key::Action;
use qwertty_term_input::key_encode::Options as EncodeOptions;

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
    /// Clear-smoke result.
    smoke: RefCell<SmokeOutcome>,
    /// Text-smoke result.
    text: RefCell<TextSmokeOutcome>,
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
    {
        let mut sref = shared.surface.borrow_mut();
        let surface = sref.as_mut().expect("surface present");
        if let Some(bytes) = pty_bytes {
            surface.feed(&bytes);
        } else if feed_banner {
            surface.feed(
                b"qwertty-term (gtk) \xe2\x80\x94 no shell; keyboard input is the next chunk.\r\n",
            );
        }
        let _ = surface.render(dst);
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
                shared.text.borrow_mut().realize_error = Some(msg);
                return;
            }
            ensure_gl_loader();
            *shared.gl.borrow_mut() = Some(make_glow());
            shared.smoke.borrow_mut().realized = true;
            shared.text.borrow_mut().realized = true;

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
                Mode::Interactive => render_interactive(area, gl, &shared),
            }

            if matches!(mode, Mode::ClearSmoke | Mode::TextSmoke) {
                // One frame is all a headless smoke needs; tear down cleanly.
                app.quit();
            }
            glib::Propagation::Stop
        });
    }

    // resize: cache the new size. Mirrors glareaResize (surface.zig:3365-3423).
    // TODO(resize): per-surface resize (re-grid the `Terminal`, `TIOCSWINSZ` on
    // the pty via `Subprocess::resize`, resize the `Engine<OpenGL>` target) is
    // still deferred — the surface is sized once at first init. Wiring it needs
    // a re-grid path on `SurfaceState`; kept out of the keyboard chunk to stay
    // small. See upstream `glareaResize` (surface.zig:3365).
    gl_area.connect_resize(|_area, _width, _height| {});

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
            // Repaint at ~60fps so pty output shows promptly (no dirty-tracking
            // wakeup yet). Cheap: a full redraw of a small grid.
            let area = gl_area.clone();
            glib::timeout_add_local(Duration::from_millis(16), move || {
                area.queue_render();
                glib::ControlFlow::Continue
            });
        }
        Mode::ClearSmoke | Mode::TextSmoke => {
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
