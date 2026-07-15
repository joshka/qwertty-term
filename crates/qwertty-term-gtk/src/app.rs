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

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Once;
use std::time::Duration;

use adw::prelude::*;
use gtk::glib;

/// Application id for the GTK/DBus registration.
const APP_ID: &str = "com.qwertty.TerminalGtk";

/// The framebuffer clear color (linear RGBA, alpha opaque) — a distinctive
/// slate blue. Deliberately non-black so a headless `glReadPixels` readback can
/// tell "we actually cleared" from an untouched (zero) framebuffer.
pub const CLEAR_COLOR: [f32; 4] = [0.12, 0.16, 0.36, 1.0];

/// Outcome of a headless smoke run: what the realize/render callbacks observed
/// while driving the GTK main loop for a single frame.
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

/// Build the GLArea and its parent window, wiring the realize/render/resize
/// signals. `smoke` = true tears the app down after the first rendered frame so
/// a headless run terminates deterministically.
fn build_window(
    app: &adw::Application,
    gl_state: Rc<RefCell<Option<glow::Context>>>,
    outcome: Rc<RefCell<SmokeOutcome>>,
    smoke: bool,
) {
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
    // `allowed-apis: gl` (surface.blp:36) achieves — so we don't set the
    // `allowed-apis` property separately (its `set_allowed_apis` binding isn't on
    // `GLAreaExt` in gtk4-rs 0.9). PR-C can pin it via the property if needed.
    gl_area.set_required_version(4, 3);

    // realize: make current, check for a context error, load `glow`.
    // Mirrors glareaRealize (surface.zig:3247-3282).
    {
        let gl_state = gl_state.clone();
        let outcome = outcome.clone();
        gl_area.connect_realize(move |area| {
            area.make_current();
            // Fully-qualified: `error()` also exists on `GLContextExt`, which is
            // in scope via the preludes, so plain `area.error()` is ambiguous.
            if let Some(err) = gtk::prelude::GLAreaExt::error(area) {
                // A context error here is almost always a driver/library issue
                // rather than our code (upstream logs the same guidance).
                outcome.borrow_mut().realize_error = Some(err.to_string());
                return;
            }
            ensure_gl_loader();
            let gl = unsafe { glow::Context::from_loader_function(epoxy::get_proc_addr) };
            *gl_state.borrow_mut() = Some(gl);
            outcome.borrow_mut().realized = true;

            // === SURFACE INIT SEAM (PR-C) ===
            // Upstream initializes the core surface/renderer here when one
            // already exists, else lazily on first resize (surface.zig:3268,
            // 3419). Slice PR-C spawns TabIo (termio) + the vt `Terminal` +
            // `Engine<OpenGL>` at that lazy point so the terminal gets correct
            // initial dimensions.
        });
    }

    // render: clear the bound default framebuffer on the GTK main thread.
    // Mirrors glareaRender (surface.zig:3347-3363).
    {
        let gl_state = gl_state.clone();
        let outcome = outcome.clone();
        let app = app.clone();
        gl_area.connect_render(move |area, _ctx| {
            use glow::HasContext;
            let guard = gl_state.borrow();
            let Some(gl) = guard.as_ref() else {
                // Not yet realized; nothing to draw.
                return glib::Propagation::Stop;
            };

            let w = area.width().max(1);
            let h = area.height().max(1);
            unsafe {
                gl.viewport(0, 0, w, h);
                gl.clear_color(
                    CLEAR_COLOR[0],
                    CLEAR_COLOR[1],
                    CLEAR_COLOR[2],
                    CLEAR_COLOR[3],
                );
                gl.clear(glow::COLOR_BUFFER_BIT);

                // ===================== TERMINAL RENDER SEAM =====================
                // (ADR 005 PR-C, gated on the present seam PR-A — plan §2.)
                //
                // The GLArea's default framebuffer (FBO 0) is bound and current
                // *right here*, on the GTK main thread — the only place GL may
                // draw/present (`must_draw_from_app_thread`, App.zig:20-23). Once
                // `GpuBackend::present` + `Engine::<OpenGL>::draw_and_present`
                // land, replace the clear above with the terminal frame:
                //
                //     let snap = FullSnapshot::capture_tracking(&mut terminal);
                //     engine.update_frame(&snap, &mut grid, opts);
                //     engine.sync_atlas(&grid);
                //     engine.draw_and_present(); // blits engine FBO -> FBO 0,
                //                                // GL_FRAMEBUFFER_SRGB disabled
                //                                // during the blit (OpenGL.zig:302).
                //
                // `engine`/`terminal`/`grid` would be owned by the per-surface
                // state created at the SURFACE INIT SEAM above.
                // ================================================================

                gl.flush();
                let gl_error = gl.get_error();

                // Read the center pixel back to prove the clear reached the
                // framebuffer (the headless analog of the offscreen readback
                // that slice 1 uses to prove pixel-correctness).
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

                let mut o = outcome.borrow_mut();
                o.rendered = true;
                o.gl_error = gl_error;
                o.center_pixel = px;
            }

            if smoke {
                // One frame is all the headless smoke needs; tear down cleanly.
                app.quit();
            }
            glib::Propagation::Stop
        });
    }

    // resize: cache the new size. Mirrors glareaResize (surface.zig:3365-3423).
    // The scaffold has no per-surface state to resize yet; PR-C hooks the lazy
    // surface init (first resize) + engine-target/pty-winsize resize here.
    gl_area.connect_resize(|_area, _width, _height| {
        // no-op for the scaffold.
    });

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .default_width(800)
        .default_height(600)
        .title("qwertty-term")
        .content(&gl_area)
        .build();
    window.present();

    if smoke {
        // Force a first frame and guarantee termination even if `render` never
        // fires (e.g. the GLArea never maps), so a headless run cannot hang.
        gl_area.queue_render();
        let app = app.clone();
        glib::timeout_add_local_once(Duration::from_secs(5), move || {
            app.quit();
        });
    }
}

/// Run the GTK application interactively (opens a window that GL-clears to
/// [`CLEAR_COLOR`]). Returns the process exit code.
pub fn run() -> std::process::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let gl_state: Rc<RefCell<Option<glow::Context>>> = Rc::new(RefCell::new(None));
    let outcome = Rc::new(RefCell::new(SmokeOutcome::default()));
    app.connect_activate(move |app| {
        build_window(app, gl_state.clone(), outcome.clone(), false);
    });
    // Empty arg list: our own flags (e.g. `--smoke`) are handled before this
    // and must not be parsed by GTK.
    let code = app.run_with_args::<&str>(&[]);
    if code.value() == 0 {
        std::process::ExitCode::SUCCESS
    } else {
        std::process::ExitCode::FAILURE
    }
}

/// Run the app headlessly for a single frame and report what the realize/render
/// callbacks observed. Intended for the `--smoke` bin flag and the headless
/// integration test under Xvfb + Mesa llvmpipe.
pub fn run_smoke() -> SmokeOutcome {
    let app = adw::Application::builder().application_id(APP_ID).build();
    let gl_state: Rc<RefCell<Option<glow::Context>>> = Rc::new(RefCell::new(None));
    let outcome = Rc::new(RefCell::new(SmokeOutcome::default()));
    {
        let gl_state = gl_state.clone();
        let outcome = outcome.clone();
        app.connect_activate(move |app| {
            build_window(app, gl_state.clone(), outcome.clone(), true);
        });
    }
    let _ = app.run_with_args::<&str>(&[]);
    outcome.borrow().clone()
}
