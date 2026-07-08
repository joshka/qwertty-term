//! The AppKit application host (chunk R5): `NSApplication` + `AppDelegate`, a
//! window/tab per terminal, the menu bar, and the main-thread pace loop that
//! pumps each tab's PTY and renders it.
//!
//! Object graph:
//!
//! - [`Controller`] (`Rc<RefCell<ControllerState>>`): the shared brain. Owns the
//!   [`TabRegistry`](crate::tabs::TabRegistry) and the per-tab [`Tab`] bundles,
//!   the config, and the input config. Menu actions and view keystrokes call
//!   into it. The controller itself is single-threaded (main thread), so
//!   `Rc`/`RefCell`. The terminal engine, however, is shared with the termio io
//!   threads as `Arc<Mutex<Engine>>` (M2 chunk E): the parse thread applies pty
//!   output behind that lock, and the main pace tick locks it to render + drain
//!   replies (the upstream `processOutput`-under-`renderer_state.mutex` design,
//!   see `docs/analysis/termio-hub.md` §3).
//! - [`Tab`]: one terminal — a shared [`Engine`](crate::engine::Engine) behind
//!   an `Arc<Mutex>`, a [`TabIo`](crate::termio::TabIo) (the real termio stack:
//!   rustix pty + two-stage read pipeline + mailbox writer loop), a render
//!   [`RenderEngine`](ghostty_renderer::engine::Engine), a
//!   [`FontGrid`](crate::font::FontGrid), a [`FontSize`](crate::font_size::FontSize),
//!   an owning `NSWindow` + [`TerminalView`](crate::view::TerminalView), and the
//!   current grid dims.
//! - [`AppDelegate`]: builds the menu, opens the first window, starts the pace
//!   timer, and (for smoke) schedules an auto-exit.
//!
//! Pacing: an `NSTimer` on the main run loop ticks ~every 16 ms (plan decision
//! 3, timer-first). The termio parse thread feeds each tab's engine off-thread
//! (behind the engine mutex); each tick locks the engine to render via
//! [`RenderEngine::draw_and_present`], drains engine reply bytes to the pty, and
//! handles child-exit events. AppKit owns `NSApplication.run`
//! (the appkit-input verdict), so the draw must run on the main thread — hence a
//! run-loop timer rather than the renderer's background-thread `TimerPacer`.
//! CVDisplayLink is a later swap-in behind this same tick shape (deferred; noted
//! in `docs/analysis/renderer-r5.md`).

#![cfg(target_os = "macos")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSColor, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem, NSWindow, NSWindowDelegate,
    NSWindowStyleMask, NSWindowTabbingMode,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString,
};

use crate::engine::Engine;
use crate::font::{self, FontGrid};
use crate::font_size::FontSize;
use crate::geometry;
use crate::input::translate::{InputConfig, RawKeyEvent};
use crate::menu::{MenuAction, TopMenu};
use crate::selection::{SelectionColors, tint_selection};
use crate::tabs::{self, TabId, TabRegistry};
use crate::termio::{IoEvent, TabIo};
use crate::view::TerminalView;
use ghostty_renderer::engine::{Engine as RenderEngine, FrameOptions};
use ghostty_renderer::snapshot::FullSnapshot;

/// The initial window content size in points.
const INITIAL_WIDTH: f64 = 800.0;
const INITIAL_HEIGHT: f64 = 480.0;

/// A geometry snapshot of a tab's window used to diagnose (and regression-test)
/// the "spurious dark band under the titlebar" bug. All rects are in the
/// window's flipped-to-AppKit-native coordinate space as AppKit reports them
/// (`contentLayoutRect` and view `frame` are both bottom-left-origin window
/// coordinates); the fields we assert on are size and the top-edge gap, both
/// origin-independent, so the flip doesn't matter.
#[derive(Debug, Clone, Copy)]
pub struct WindowGeometry {
    /// Whether the window is part of a tab group whose tab bar is showing.
    pub tab_bar_visible: bool,
    /// Number of windows (tabs) in this window's tab group (1 if ungrouped).
    pub tab_count: usize,
    /// Whether the window has a (non-nil) `NSToolbar` — we never set one, so
    /// this should be false; a toolbar would inset content and expose a band.
    pub has_toolbar: bool,
    /// The window's `contentLayoutRect` size in points (the un-obscured content
    /// area — below the titlebar and any visible tab bar/toolbar).
    pub content_layout: NSSize,
    /// The terminal view's `frame` in window coordinates (origin + size).
    pub view_frame: NSRect,
    /// The `contentView`'s `frame` size (should equal the whole content area).
    pub content_view: NSSize,
    /// Total chrome height in points: the window frame height minus the standard
    /// content height for this style mask (`window.frame - contentRectForFrameRect`).
    /// A plain titled window is ~28pt; a value materially larger means AppKit is
    /// reserving extra chrome (an in-titlebar tab strip / accessory) that a
    /// content-flush terminal never gets to cover — the band's home when it
    /// lives in window chrome rather than the content area.
    pub chrome_height: f64,
    /// The host layer's `bounds` in points (should equal the view bounds).
    pub layer_bounds: NSSize,
    /// The presented surface's size in device pixels (`cols*cw × rows*ch`), i.e.
    /// the render target the grid produced.
    pub surface_px: (usize, usize),
    /// The layer's `contentsScale`.
    pub contents_scale: f64,
    /// Whether the window's background colour matches the terminal's default
    /// background — the fix that makes any sub-cell remainder strip seamless
    /// instead of chrome-grey. `false` means the band would show window chrome.
    pub bg_matches_terminal: bool,
    /// Whether the host layer's `contentsGravity` pins the surface to the visual
    /// top (`kCAGravityBottomLeft` under the view's flipped geometry) — the fix
    /// that moves the sub-cell remainder from a top band to the bottom edge.
    pub gravity_pins_visual_top: bool,
}

impl WindowGeometry {
    /// Probe `window`/`view` for the geometry fields. `surface_px` is the tab's
    /// current render-target pixel size (the caller has the tab; the window/view
    /// don't expose the grid). `default_bg` is the terminal's background colour
    /// (to confirm the window background was painted to match). The backing scale
    /// is read from the layer's `contentsScale`.
    fn probe(
        window: &NSWindow,
        view: &TerminalView,
        surface_px: (usize, usize),
        default_bg: (u8, u8, u8),
    ) -> Self {
        let (tab_bar_visible, tab_count) = match window.tabGroup() {
            Some(group) => (group.isTabBarVisible(), group.windows().count()),
            None => (false, 1),
        };
        let has_toolbar = window.toolbar().is_some();
        let content_layout = window.contentLayoutRect().size;
        let view_frame = view.frame();
        let content_view = window
            .contentView()
            .map(|cv| cv.frame().size)
            .unwrap_or(NSSize::new(0.0, 0.0));
        let window_frame = window.frame();
        let style_content = window.contentRectForFrameRect(window_frame);
        let chrome_height = window_frame.size.height - style_content.size.height;
        let layer = view.host_layer().as_layer();
        let layer_bounds = layer.bounds().size;
        let contents_scale = layer.contentsScale();
        let bg_matches_terminal = window_bg_matches(window, default_bg);
        let gravity_pins_visual_top = view.surface_pinned_to_top();
        WindowGeometry {
            tab_bar_visible,
            tab_count,
            has_toolbar,
            content_layout,
            view_frame,
            content_view,
            chrome_height,
            layer_bounds,
            surface_px,
            contents_scale,
            bg_matches_terminal,
            gravity_pins_visual_top,
        }
    }

    /// The vertical shortfall in points between the layer (view) height and the
    /// surface height mapped back to points (`surface_h / scale`) — the floor-
    /// division remainder of the grid fit: the strip of layer the surface does
    /// not cover. With `kCAGravityTopLeft` in a *flipped* view this exposed strip
    /// lands at the **top** of the terminal, which is the reported dark band that
    /// lives in *our surface layer*, not window chrome. A correct fit rounds the
    /// view/layer down to a whole number of cells so this is ~0.
    pub fn surface_gap_points(&self) -> f64 {
        let scale = if self.contents_scale > 0.0 {
            self.contents_scale
        } else {
            1.0
        };
        let surface_h_pt = self.surface_px.1 as f64 / scale;
        (self.layer_bounds.height - surface_h_pt).max(0.0)
    }

    /// The vertical gap (in points) between the top of the un-obscured content
    /// area and the top of the terminal view — the height of any exposed
    /// window-chrome band above the terminal. `contentLayoutRect` and the view
    /// frame share the window's bottom-left origin, so the view's top edge is
    /// `view_frame.origin.y + view_frame.size.height` and the content area's top
    /// edge is `content_view_height` (the full content view spans the layout
    /// height when it fills). We compute the band as the difference between the
    /// un-obscured content height and the view height plus the view's bottom
    /// inset — i.e. how much content-area height the view fails to cover.
    pub fn top_band_points(&self) -> f64 {
        // The view should fill the content-layout height. Any shortfall (the
        // content area is taller than the view, or the view is pushed down) is
        // exposed chrome. Bottom inset + top shortfall both count.
        let uncovered = self.content_layout.height - self.view_frame.size.height;
        uncovered.max(0.0)
    }

    /// Log the snapshot to stderr with a label (the smoke diagnostic dump).
    pub fn log(&self, label: &str) {
        eprintln!(
            "GEOMETRY[{label}]: tab_bar_visible={} tab_count={} has_toolbar={} \
             content_layout={:.1}x{:.1} content_view={:.1}x{:.1} \
             view_frame=({:.1},{:.1} {:.1}x{:.1}) chrome_height={:.1}pt \
             layer_bounds={:.1}x{:.1} surface_px={}x{} scale={:.1} \
             surface_gap={:.2}pt top_band={:.1}pt bg_matches_terminal={} \
             gravity_pins_visual_top={}",
            self.tab_bar_visible,
            self.tab_count,
            self.has_toolbar,
            self.content_layout.width,
            self.content_layout.height,
            self.content_view.width,
            self.content_view.height,
            self.view_frame.origin.x,
            self.view_frame.origin.y,
            self.view_frame.size.width,
            self.view_frame.size.height,
            self.chrome_height,
            self.layer_bounds.width,
            self.layer_bounds.height,
            self.surface_px.0,
            self.surface_px.1,
            self.contents_scale,
            self.surface_gap_points(),
            self.top_band_points(),
            self.bg_matches_terminal,
            self.gravity_pins_visual_top,
        );
    }
}

/// One terminal tab: engine + termio IO + renderer + window/view.
struct Tab {
    /// The vt engine (parser + terminal state), shared with the termio parse
    /// thread. The parse thread locks it to apply pty output; the main pace
    /// tick locks it to render + drain replies (`docs/analysis/termio-hub.md`
    /// §3).
    engine: Arc<Mutex<Engine>>,
    /// The real terminal IO stack (rustix pty + read pipeline + mailbox writer
    /// loop). Dropping it joins the io threads.
    io: TabIo,
    /// The render engine (cell buffers + Metal draw), if a Metal device exists.
    render: Option<RenderEngine>,
    /// The font grid the renderer shapes through.
    font: FontGrid,
    /// The current font size (drives font grid rebuilds).
    font_size: FontSize,
    /// The owning window (one NSWindow per tab; macOS groups them as tabs).
    window: Retained<NSWindow>,
    /// The terminal view hosting the render layer + input.
    view: Retained<TerminalView>,
    /// The window's delegate (keeps the controller's active tab in sync with
    /// the OS key window). AppKit holds window delegates weakly, so the `Tab`
    /// owns this to keep it alive for the window's lifetime.
    _window_delegate: Retained<WindowDelegate>,
    /// Current grid dimensions.
    cols: usize,
    rows: usize,
    /// Backing scale (contentsScale) last applied.
    scale: f64,
    /// Last reported mouse cell (motion dedup for mouse reporting).
    last_mouse_cell: Option<(i64, i64)>,
    /// Whether a mouse button is currently held (for out-of-viewport motion).
    mouse_button_down: bool,
    /// The cell the current selection drag started at, if a drag is in
    /// progress. `None` when no drag is live (a fresh press starts one; a
    /// release ends it). The live selection value itself is engine-owned
    /// (`Engine::select`/`selection`); this is just drag-in-progress state.
    selection_anchor: Option<(usize, usize)>,
    /// Selection highlight colors resolved from the tab's theme at startup
    /// (or [`SelectionColors::Inverse`] if the theme had none / no theme was
    /// configured).
    selection_colors: SelectionColors,
    /// The terminal's default background as `(r, g, b)` — the baseline the
    /// presented-frame coverage metric measures against (glyphs and non-default
    /// cell backgrounds show up as pixels far from this). Resolved from the
    /// startup theme (or ghostty-vt's default `0x18` grey).
    default_bg: (u8, u8, u8),
    /// Debug frame-dump (env `GHOSTTY_APP_DUMP_FRAME`), if enabled. When set,
    /// the render path reads the presented IOSurface back and writes periodic
    /// PNGs — the decisive "does the presented surface contain glyphs" probe.
    frame_dump: Option<crate::frame_dump::FrameDump>,
    /// Max per-pixel L1 delta from `default_bg` in the most recently *presented*
    /// frame (not the engine buffer). `0` before the first present. The windowed
    /// typing smoke reads this to assert the layer actually received glyph
    /// pixels — the gap the old engine-text-only assertion missed.
    last_present_delta: i32,
    /// Whether the render path should read the presented frame back each tick to
    /// record [`Tab::last_present_delta`]. Enabled by a frame dump or by the
    /// presented-pixel smoke (`GHOSTTY_APP_ASSERT_PRESENT`); off in normal use
    /// (readback is a per-frame CPU copy we don't want in the steady loop).
    capture_present: bool,
}

impl Tab {
    /// Lock the shared engine. Held only briefly per call (a snapshot, a
    /// resize, or a state read) so the parse thread's line-rate feed is barely
    /// contended (`docs/analysis/termio-hub.md` §3.3).
    fn engine(&self) -> std::sync::MutexGuard<'_, Engine> {
        self.engine.lock().expect("engine mutex poisoned")
    }

    /// Rebuild the render target + grid for the current view size and scale,
    /// resizing the engine + pty to match. Called on creation and resize.
    fn reflow(&mut self) {
        // Keep the host layer's contentsScale in lockstep with the backing
        // scale. The presented IOSurface is device-pixel-sized while the
        // layer's bounds are in points; without this the frame renders at the
        // wrong scale (only the top-left 1/scale of it is visible — the blank
        // window / garbled-sliver bug on Retina). See
        // `IOSurfaceLayer::set_contents_scale`.
        self.apply_contents_scale();

        let (cols, rows) = self.current_grid_size();
        if cols != self.cols || rows != self.rows {
            self.cols = cols;
            self.rows = rows;
            self.engine().resize(cols, rows);
            self.io.resize(
                cols as u16,
                rows as u16,
                self.font.cell_width,
                self.font.cell_height,
            );
        }
    }

    /// Apply the tab's current backing scale to the host render layer's
    /// `contentsScale`. Idempotent; safe to call every reflow.
    fn apply_contents_scale(&self) {
        self.view.host_layer().set_contents_scale(self.scale);
    }

    /// The grid size that fits the view's current pixel bounds.
    fn current_grid_size(&self) -> (usize, usize) {
        let bounds = self.view.bounds();
        let w = (bounds.size.width * self.scale) as usize;
        let h = (bounds.size.height * self.scale) as usize;
        geometry::grid_size(w, h, self.font.cell_width, self.font.cell_height)
    }

    /// Rebuild the font grid at the tab's current font size × backing scale.
    fn rebuild_font(&mut self, family: Option<&str>) {
        let px = (self.font_size.get() as f64) * self.scale;
        if let Ok(fg) = font::build(family, px) {
            self.font = fg;
            // A new cell size changes the fitting grid; reflow.
            self.reflow();
        }
    }

    /// Per-tick IO servicing. The termio parse thread already fed pty output
    /// into the engine off-thread; here we (1) drain engine reply bytes
    /// (DSR/DA/CPR) back to the pty and (2) act on surface events. Returns
    /// whether the child shell exited (so the caller closes the tab).
    fn pump(&mut self) -> bool {
        // Drain engine replies under the lock, then release before writing to
        // the pty (the write goes through the mailbox, not the engine lock).
        let out = self.engine().take_output();
        if !out.is_empty() {
            self.io.write(&out);
        }

        // Surface events from the io threads (child-exit / password).
        let mut exited = false;
        for event in self.io.drain_events() {
            match event {
                IoEvent::ChildExited { exit_code, .. } => {
                    // Match the interim behavior: the tab closes when its shell
                    // exits. Log the code so a non-zero exit is visible.
                    if exit_code != 0 {
                        eprintln!("ghostty-app: shell exited with code {exit_code}");
                    }
                    exited = true;
                }
                IoEvent::PasswordInput(active) => {
                    // Surfacing-only for M2-E: reflect it in the window title
                    // suffix (a lock icon marker) so the state is visible.
                    self.set_password_marker(active);
                }
            }
        }
        exited
    }

    /// Reflect password-input state in the window title (M2-E surfacing). A
    /// lock marker is appended while a program is reading a secret.
    fn set_password_marker(&self, active: bool) {
        let base = self
            .engine()
            .title()
            .unwrap_or_else(|| "ghostty-rs".to_string());
        let title = if active { format!("{base} 🔒") } else { base };
        self.window.setTitle(&NSString::from_str(&title));
    }

    /// Render one frame into the view's layer.
    fn render(&mut self) {
        if self.render.is_none() {
            return;
        }
        // Snapshot + resolve the selection range under one lock acquisition,
        // then release before the Metal draw (which doesn't touch the engine).
        let (mut window, range) = {
            let engine = self.engine();
            let window = engine.snapshot_window(0);
            let range = engine
                .selection()
                .and_then(|(start, end, rect)| engine.screen_range(start, end, rect));
            (window, range)
        };
        if let Some(range) = range {
            tint_selection(&mut window, range, self.selection_colors);
        }
        let snapshot = FullSnapshot::from_window(window);
        let render = self.render.as_mut().expect("checked above");
        render.update_frame(&snapshot, &mut self.font.grid, FrameOptions::default());
        if render.sync_atlas(&self.font.grid).is_err() {
            return;
        }

        if self.capture_present {
            // Debug / smoke path: present *and* read the presented surface back,
            // so we can both dump it and assert on real presented pixels.
            let (sw, sh) = render.screen_size();
            if let Ok(Some(pixels)) = render.draw_and_present_readback(self.view.host_layer()) {
                self.last_present_delta = crate::frame_dump::max_bg_delta(&pixels, self.default_bg);
                if let Some(dump) = self.frame_dump.as_mut()
                    && dump.should_dump()
                {
                    dump.write(&pixels, sw, sh);
                }
            }
        } else {
            let _ = render.draw_and_present(self.view.host_layer());
        }
    }

    /// Convert a device-pixel viewport position into a `(col, row)` cell
    /// coordinate, or `None` if it falls outside the grid.
    fn cell_at(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        geometry::cell_at(
            x,
            y,
            self.cols,
            self.rows,
            self.font.cell_width,
            self.font.cell_height,
        )
    }
}

/// Shared, main-thread controller state.
pub struct ControllerState {
    registry: TabRegistry,
    tabs: HashMap<TabId, Tab>,
    input_config: InputConfig,
    font_family: Option<String>,
    default_font_size: f32,
    mtm: MainThreadMarker,
    /// The engine startup colors resolved from `config.theme` (palette +
    /// default fg/bg/cursor), applied to every new tab's engine. Falls back
    /// to `ghostty-vt`'s built-in default `Colors` if no theme is configured
    /// or it fails to load (a warning is printed to stderr in that case; see
    /// `crate::theme::load_theme`).
    startup_colors: ghostty_vt::terminal::Colors,
    /// Selection highlight colors resolved from the same theme (explicit
    /// `selection-background`/`selection-foreground` if the theme set them,
    /// else a plain inverse-video swap).
    selection_colors: SelectionColors,
    /// Whether finishing a mouse-drag selection immediately copies it to the
    /// clipboard (`copy-on-select` config key).
    copy_on_select: bool,
}

/// The controller handle passed to views and menu targets.
#[derive(Clone)]
pub struct Controller(Rc<RefCell<ControllerState>>);

impl Controller {
    /// Build a controller from loaded config.
    pub fn new(config: &crate::config::Config, mtm: MainThreadMarker) -> Self {
        let default_font_size = config
            .font_size
            .unwrap_or(crate::font_size::DEFAULT_FONT_SIZE);

        // Resolve the configured theme (if any) into engine startup colors +
        // selection highlight colors. Mirrors the reference spike's
        // `WindowTerminal::new` theme lookup (`crates/spike/src/window/mod.rs`).
        let theme = config.theme.as_deref().and_then(crate::theme::load_theme);
        let startup_colors = theme
            .as_ref()
            .map(crate::theme::ThemeColors::to_colors)
            .unwrap_or_default();
        let selection_colors = match theme
            .as_ref()
            .and_then(|t| t.selection_background.zip(t.selection_foreground))
        {
            Some((bg, fg)) => SelectionColors::Explicit { bg, fg },
            None => SelectionColors::Inverse,
        };

        Controller(Rc::new(RefCell::new(ControllerState {
            registry: TabRegistry::new(),
            tabs: HashMap::new(),
            input_config: InputConfig::default(),
            font_family: config.font_family.clone(),
            default_font_size,
            mtm,
            startup_colors,
            selection_colors,
            copy_on_select: config.copy_on_select,
        })))
    }

    /// Number of live tabs (for tests / smoke assertions).
    pub fn tab_count(&self) -> usize {
        self.0.borrow().registry.len()
    }

    /// Open a brand-new window (its own tab). Returns the new tab id.
    pub fn new_window(&self) -> Option<TabId> {
        self.spawn_tab(None, None)
    }

    /// Open a new tab in `parent`'s window group, inheriting `parent`'s pwd.
    pub fn new_tab_in(&self, parent: TabId) -> Option<TabId> {
        let pwd = {
            let state = self.0.borrow();
            state
                .tabs
                .get(&parent)
                .and_then(|t| t.engine().pwd())
                .and_then(|p| tabs::inherit_pwd(Some(&p)))
        };
        self.spawn_tab(pwd, Some(parent))
    }

    /// Close a tab: drop its bundle, close its window, update the registry.
    pub fn close_tab(&self, tab: TabId) {
        let mut state = self.0.borrow_mut();
        if let Some(t) = state.tabs.remove(&tab) {
            t.window.close();
        }
        state.registry.remove(tab);
    }

    /// The active tab, if any.
    pub fn active_tab(&self) -> Option<TabId> {
        self.0.borrow().registry.active()
    }

    /// Re-resolve `tab`'s backing scale from its window and reflow it. Called on
    /// window resize and on a backing-property change (e.g. the window moved to
    /// a display with a different scale). Keeps three things in lockstep after
    /// the initial `spawn_tab`: the font grid (rebuilt if the scale changed),
    /// the engine/PTY grid (reflowed to the new view size), and the render
    /// layer's `contentsScale` (re-applied in `reflow`). Without this, resizing
    /// the window or dragging it between a Retina and non-Retina display leaves
    /// the presented surface at the wrong size/scale — the same
    /// device-pixel-vs-points mismatch that blanks the window at startup.
    pub fn resync_tab_geometry(&self, tab: TabId) {
        let mut state = self.0.borrow_mut();
        let family = state.font_family.clone();
        if let Some(t) = state.tabs.get_mut(&tab) {
            let new_scale = t.window.backingScaleFactor();
            if (new_scale - t.scale).abs() > f64::EPSILON {
                t.scale = new_scale;
                // Rebuilds the font at the new scale, which itself reflows
                // (and re-applies contentsScale).
                t.rebuild_font(family.as_deref());
            } else {
                t.reflow();
            }
        }
    }

    /// The plain-text screen dump of the active tab's engine (smoke/test only).
    /// `None` if there is no active tab. Used by the synthetic-input smoke
    /// (`GHOSTTY_APP_SMOKE_TYPE`) to assert a typed command round-tripped
    /// through keyDown → encode → PTY → engine.
    pub fn active_screen_text(&self) -> Option<String> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).map(|t| t.engine().screen_dump())
    }

    /// The active tab's most recently *presented* frame coverage: the max
    /// per-pixel L1 delta from the theme background in the last frame actually
    /// attached to the CoreAnimation layer (smoke/test only; only populated
    /// when presented-pixel capture is enabled — `GHOSTTY_APP_ASSERT_PRESENT`
    /// or a frame dump). `None` if there is no active tab. A value near `0`
    /// means the presented surface was blank (background only); a large value
    /// means real glyph/foreground pixels reached the layer.
    pub fn active_present_delta(&self) -> Option<i32> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).map(|t| t.last_present_delta)
    }

    /// The active tab's `NSWindow` (smoke/test only): the target the synthetic
    /// key events are delivered to. `None` if there is no active tab.
    pub fn active_window(&self) -> Option<Retained<NSWindow>> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).map(|t| t.window.clone())
    }

    /// The active tab's terminal view (smoke/test only): used to force it to
    /// become first responder before delivering synthetic key events.
    pub fn active_view(&self) -> Option<Retained<TerminalView>> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).map(|t| t.view.clone())
    }

    /// Mark `tab` active (called when its window becomes key).
    pub fn set_active(&self, tab: TabId) {
        self.0.borrow_mut().registry.activate(tab);
    }

    /// Snapshot the active tab's window/view/tab-group geometry (smoke/test
    /// only). The field the "spurious top band" bug lives in: if the terminal
    /// view does not fill `contentLayoutRect` exactly, the exposed slice is
    /// window chrome (the dark band). `None` if there is no active tab.
    pub fn active_geometry(&self) -> Option<WindowGeometry> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        let t = state.tabs.get(&tab)?;
        let surface_px =
            crate::geometry::pixel_size(t.cols, t.rows, t.font.cell_width, t.font.cell_height);
        Some(WindowGeometry::probe(
            &t.window,
            &t.view,
            surface_px,
            t.default_bg,
        ))
    }

    /// Encode a raw key event and write it to `tab`'s PTY. Reads the tab's live
    /// terminal encode options + the user's option-as-alt config.
    pub fn encode_key_to_tab(&self, tab: TabId, raw: &RawKeyEvent) {
        let mut state = self.0.borrow_mut();
        let cfg = state.input_config;
        if let Some(t) = state.tabs.get_mut(&tab) {
            let opts = t.engine().key_encode_options();
            let bytes = crate::input::translate::encode_raw(raw, &cfg, opts);
            if !bytes.is_empty() {
                t.io.write(&bytes);
            }
        }
    }

    /// Send already-composed text (IME commit) to `tab`'s pty.
    pub fn send_text_to_tab(&self, tab: TabId, text: &str) {
        let state = self.0.borrow();
        if let Some(t) = state.tabs.get(&tab) {
            t.io.write(text.as_bytes());
        }
    }

    /// Encode a mouse event (view-space pixels) against `tab`'s live mouse
    /// tracking mode/format and write it to the PTY. No-op when the program has
    /// not enabled mouse reporting. `pressed` updates the held-button state used
    /// for out-of-viewport motion.
    ///
    /// Also drives left-button selection: a press starts (or, if the program
    /// has enabled mouse reporting and shift isn't held, defers to) a
    /// selection anchor; drag motion while the button is down extends it;
    /// release finalizes it and, if `copy-on-select` is configured, copies
    /// the selected text. This mirrors the reference spike's
    /// `handle_pointer_selection` (`crates/spike/src/window/mod.rs`).
    #[allow(clippy::too_many_arguments)]
    pub fn mouse_to_tab(
        &self,
        tab: TabId,
        action: ghostty_input::mouse::Action,
        button: Option<ghostty_input::mouse::Button>,
        mods: ghostty_input::key_mods::Mods,
        x: f32,
        y: f32,
        pressed: Option<bool>,
    ) {
        let copy_on_select = self.0.borrow().copy_on_select;
        let mut state = self.0.borrow_mut();
        let Some(t) = state.tabs.get_mut(&tab) else {
            return;
        };
        if let Some(p) = pressed {
            t.mouse_button_down = p;
        }

        if button == Some(ghostty_input::mouse::Button::Left) {
            let reporting_active =
                t.engine().mouse_event() != ghostty_input::mouse_encode::MouseEvent::None;
            let selection_allowed = !reporting_active || mods.shift;
            if selection_allowed {
                match action {
                    ghostty_input::mouse::Action::Press => {
                        t.engine().clear_selection();
                        t.selection_anchor = None;
                        if let Some(cell) = t.cell_at(x, y) {
                            t.selection_anchor = Some(cell);
                        }
                    }
                    ghostty_input::mouse::Action::Motion => {
                        if t.mouse_button_down
                            && let Some(anchor) = t.selection_anchor
                            && let Some(cell) = t.cell_at(x, y)
                        {
                            let mut engine = t.engine();
                            if let (Some(start), Some(end)) = (
                                engine.pin_at(anchor.0, anchor.1),
                                engine.pin_at(cell.0, cell.1),
                            ) {
                                engine.select(start, end, false);
                            }
                        }
                    }
                    ghostty_input::mouse::Action::Release => {
                        if copy_on_select && t.selection_anchor.is_some() {
                            let text = t.engine().selection_string();
                            if let Some(text) = text {
                                crate::clipboard::write(&text);
                            }
                        }
                        t.selection_anchor = None;
                    }
                }
            }
        }

        let (event_mode, format) = {
            let engine = t.engine();
            (engine.mouse_event(), engine.mouse_format())
        };
        let ctx = crate::input::mouse::MouseContext {
            event_mode,
            format,
            screen_width: (t.cols * t.font.cell_width as usize) as f64,
            screen_height: (t.rows * t.font.cell_height as usize) as f64,
            cell_width: t.font.cell_width as f64,
            cell_height: t.font.cell_height as f64,
            any_button_pressed: t.mouse_button_down,
        };
        let bytes =
            crate::input::mouse::encode(action, button, mods, x, y, &ctx, &mut t.last_mouse_cell);
        if !bytes.is_empty() {
            t.io.write(&bytes);
        }
    }

    /// Dispatch a resolved [`MenuAction`] against the active tab / app.
    pub fn handle_action(&self, action: MenuAction) {
        match action {
            MenuAction::NewWindow => {
                self.new_window();
            }
            MenuAction::NewTab => {
                if let Some(active) = self.active_tab() {
                    self.new_tab_in(active);
                } else {
                    self.new_window();
                }
            }
            MenuAction::CloseTab => {
                if let Some(active) = self.active_tab() {
                    self.close_tab(active);
                }
            }
            MenuAction::Copy => self.copy_selection_from_active(),
            MenuAction::Paste => self.paste_into_active(),
            MenuAction::FontSizeUp => self.font_size_active(FontStep::Up),
            MenuAction::FontSizeDown => self.font_size_active(FontStep::Down),
            MenuAction::FontSizeReset => self.font_size_active(FontStep::Reset),
            MenuAction::Quit => {
                let mtm = self.0.borrow().mtm;
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
    }

    /// Copy the active tab's current selection to the system clipboard
    /// (Cmd-C). No-op if there is no selection.
    fn copy_selection_from_active(&self) {
        let Some(tab) = self.active_tab() else { return };
        let state = self.0.borrow();
        if let Some(t) = state.tabs.get(&tab)
            && let Some(text) = t.engine().selection_string()
        {
            crate::clipboard::write(&text);
        }
    }

    /// Paste the clipboard into the active tab's PTY, bracketed if the program
    /// enabled bracketed paste.
    fn paste_into_active(&self) {
        let Some(tab) = self.active_tab() else { return };
        let Some(text) = crate::clipboard::read() else {
            return;
        };
        let state = self.0.borrow();
        if let Some(t) = state.tabs.get(&tab) {
            let payload = if t.engine().bracketed_paste() {
                let mut p = Vec::with_capacity(text.len() + 12);
                p.extend_from_slice(b"\x1b[200~");
                p.extend_from_slice(text.as_bytes());
                p.extend_from_slice(b"\x1b[201~");
                p
            } else {
                text.into_bytes()
            };
            t.io.write(&payload);
        }
    }

    /// Apply a font-size step to the active tab and rebuild its grid.
    fn font_size_active(&self, step: FontStep) {
        let Some(tab) = self.active_tab() else { return };
        let mut state = self.0.borrow_mut();
        let family = state.font_family.clone();
        if let Some(t) = state.tabs.get_mut(&tab) {
            let changed = match step {
                FontStep::Up => t.font_size.increase(),
                FontStep::Down => t.font_size.decrease(),
                FontStep::Reset => t.font_size.reset(),
            };
            if changed {
                t.rebuild_font(family.as_deref());
            }
        }
    }

    /// Pump + render every live tab. Called each pace tick. Closes tabs whose
    /// shell exited.
    pub fn tick(&self) {
        let exited: Vec<TabId> = {
            let mut state = self.0.borrow_mut();
            let mut dead = Vec::new();
            for (id, tab) in state.tabs.iter_mut() {
                if tab.pump() {
                    dead.push(*id);
                } else {
                    tab.render();
                }
            }
            dead
        };
        for id in exited {
            self.close_tab(id);
        }
        // Quit when the last tab's shell exits.
        if self.tab_count() == 0 {
            let mtm = self.0.borrow().mtm;
            NSApplication::sharedApplication(mtm).terminate(None);
        }
    }

    /// Create a tab (window + view + engine + PTY + renderer), register it, and
    /// show the window. `cwd` is the new shell's directory; `tab_group_parent`,
    /// if set, adds the new window as a native tab of the parent's window.
    fn spawn_tab(&self, cwd: Option<PathBuf>, tab_group_parent: Option<TabId>) -> Option<TabId> {
        let mtm = self.0.borrow().mtm;
        let (family, default_size, startup_colors, selection_colors) = {
            let s = self.0.borrow();
            (
                s.font_family.clone(),
                s.default_font_size,
                s.startup_colors.clone(),
                s.selection_colors,
            )
        };

        // Backing scale: default to 2.0 (Retina) before the window is on a
        // screen; corrected on first reflow via the window's actual scale.
        let scale = 2.0;
        let font_size = FontSize::new(default_size);
        let fg = font::build(family.as_deref(), (font_size.get() as f64) * scale).ok()?;

        // Provisional grid from the initial content size.
        let (cw, ch) = (fg.cell_width, fg.cell_height);
        let init_w = (INITIAL_WIDTH * scale) as usize;
        let init_h = (INITIAL_HEIGHT * scale) as usize;
        let (cols, rows) = geometry::grid_size(init_w, init_h, cw, ch);

        // The theme background is what fills the presented surface (the frame
        // clears to it); it's the baseline the presented-pixel coverage metric
        // measures glyphs against. Fall back to the renderer's default grey
        // (`0x18`) when no theme sets an explicit background.
        let default_bg = startup_colors
            .background
            .get()
            .map(|c| (c.r, c.g, c.b))
            .unwrap_or((0x18, 0x18, 0x18));

        let engine = Arc::new(Mutex::new(Engine::with_colors(cols, rows, startup_colors)));
        // Spawn the real termio stack: rustix pty + read pipeline + writer loop.
        // The parse thread feeds `engine` behind its mutex (see `crate::termio`).
        let io = TabIo::spawn(
            Arc::clone(&engine),
            cols as u16,
            rows as u16,
            cw,
            ch,
            cwd.as_deref(),
        )
        .ok()?;
        let render = RenderEngine::new(cw, ch).ok();

        // Debug frame dump + presented-pixel capture (both env-gated; off in
        // normal use). `capture_present` also turns on when the presented-pixel
        // smoke asks for it, so `last_present_delta` is populated for the
        // assertion.
        let frame_dump = crate::frame_dump::FrameDump::from_env();
        let capture_present =
            frame_dump.is_some() || std::env::var_os("GHOSTTY_APP_ASSERT_PRESENT").is_some();

        // Register first so the view can carry the id.
        let id = self.0.borrow_mut().registry.add();

        let controller_ptr: *const Controller = self;
        let view = TerminalView::new(mtm, id, controller_ptr);
        let window = make_window(mtm, &view);

        // Paint the window background with the terminal's default background.
        // The presented IOSurface is sized to a whole number of cells, so it is
        // up to (cell_width-1)×(cell_height-1) device pixels smaller than the
        // layer; that sub-cell remainder is uncovered layer area. Without this,
        // the uncovered strip shows the window's default chrome-grey background
        // — the reported "~25px dark band" whose colour "matches window chrome".
        // Painting the window background the terminal colour makes any such strip
        // seamless (a partial-cell of terminal background, indistinguishable from
        // the grid). See `TerminalView::pin_surface_to_top` for the companion
        // fix that moves the strip to the bottom edge.
        set_window_background(&window, default_bg);
        // Pin the surface flush to the visual top (under the titlebar) so the
        // sub-cell remainder falls at the *bottom* — where a terminal's partial
        // last row naturally belongs — instead of as a band under the titlebar.
        view.pin_surface_to_top();

        // Per-window delegate: sync the controller's active tab to the OS key
        // window on tab switch. Owned by the Tab (AppKit weak-holds delegates).
        let window_delegate = WindowDelegate::new(mtm, self.clone(), id);
        window.setDelegate(Some(ProtocolObject::from_ref(&*window_delegate)));

        let mut tab = Tab {
            engine,
            io,
            render,
            font: fg,
            font_size,
            window: window.clone(),
            view: view.clone(),
            _window_delegate: window_delegate,
            cols,
            rows,
            scale,
            last_mouse_cell: None,
            mouse_button_down: false,
            selection_anchor: None,
            selection_colors,
            default_bg,
            frame_dump,
            last_present_delta: 0,
            capture_present,
        };

        // Correct the scale from the real window, then reflow to the actual view
        // size.
        tab.scale = window.backingScaleFactor();
        if (tab.scale - scale).abs() > f64::EPSILON {
            tab.rebuild_font(family.as_deref());
        }
        tab.reflow();

        self.0.borrow_mut().tabs.insert(id, tab);

        // Native tabbing: add to the parent's window group if requested.
        if let Some(parent) = tab_group_parent {
            let parent_window = self.0.borrow().tabs.get(&parent).map(|t| t.window.clone());
            if let Some(pw) = parent_window {
                pw.addTabbedWindow_ordered(&window, objc2_app_kit::NSWindowOrderingMode::Above);
            }
        }

        window.makeKeyAndOrderFront(None);
        window.makeFirstResponder(Some(&view));
        Some(id)
    }
}

/// Which way a font-size step goes.
enum FontStep {
    Up,
    Down,
    Reset,
}

/// Build an `NSWindow` sized to the initial content, tabbing-enabled, hosting
/// `view` as its content view.
fn make_window(mtm: MainThreadMarker, view: &TerminalView) -> Retained<NSWindow> {
    let content = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(INITIAL_WIDTH, INITIAL_HEIGHT),
    );
    let style = NSWindowStyleMask::Titled
        | NSWindowStyleMask::Closable
        | NSWindowStyleMask::Miniaturizable
        | NSWindowStyleMask::Resizable;

    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            content,
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };

    unsafe {
        window.setTitle(&NSString::from_str("ghostty-rs"));
        // Native tabbing: `.automatic` (the AppKit default) lets a lone window
        // stay tab-bar-free, matching macOS convention (and real Ghostty) —
        // the tab bar only appears once a window has 2+ tabs. This does not
        // affect Cmd-T: `new_tab_in` always calls `addTabbedWindow:ordered:`
        // explicitly (below), which groups windows into the same tabbed
        // window regardless of `tabbingMode`; that mode only governs the
        // *implicit* behavior AppKit applies to windows opened without an
        // explicit group (e.g. Cmd-N's `new_window`). `.preferred` used to be
        // set here, which forces the tab bar always-on even for a single
        // window — the reported "empty dark strip" bug.
        window.setTabbingMode(NSWindowTabbingMode::Automatic);
        window.setContentView(Some(view));
        window.setReleasedWhenClosed(false);
    }
    window
}

/// Set `window`'s background colour to `(r, g, b)` (0–255 sRGB), so any sub-cell
/// remainder of the content area not covered by the terminal surface reads as
/// terminal background rather than the default system chrome grey.
fn set_window_background(window: &NSWindow, (r, g, b): (u8, u8, u8)) {
    let color = NSColor::colorWithSRGBRed_green_blue_alpha(
        r as f64 / 255.0,
        g as f64 / 255.0,
        b as f64 / 255.0,
        1.0,
    );
    window.setBackgroundColor(Some(&color));
}

/// Whether `window`'s background colour matches `(r, g, b)` (0–255 sRGB) within a
/// 1/255 rounding tolerance. Used by the geometry smoke to confirm the window
/// background was painted the terminal colour so any sub-cell remainder strip is
/// seamless. Converts the window colour into the sRGB space first (a named/system
/// colour has no direct components).
fn window_bg_matches(window: &NSWindow, (r, g, b): (u8, u8, u8)) -> bool {
    let color = window.backgroundColor();
    let Some(srgb) = color.colorUsingColorSpace(&objc2_app_kit::NSColorSpace::sRGBColorSpace())
    else {
        return false;
    };
    let cr = (srgb.redComponent() * 255.0).round() as i32;
    let cg = (srgb.greenComponent() * 255.0).round() as i32;
    let cb = (srgb.blueComponent() * 255.0).round() as i32;
    (cr - r as i32).abs() <= 1 && (cg - g as i32).abs() <= 1 && (cb - b as i32).abs() <= 1
}

// ---------------------------------------------------------------------------
// Per-window delegate: keep the controller's active tab in sync with the OS
// ---------------------------------------------------------------------------

/// Ivars for a per-window delegate: the controller and the tab this window
/// hosts. When the OS makes this window key (tab switch, click, Cmd-`{`/`}`),
/// [`WindowDelegate`] tells the controller which tab is now active, so
/// menu-driven actions (Copy/Paste/New Tab/Close/font-size) target the tab the
/// user is actually looking at rather than whichever tab was created last.
pub struct WindowDelegateIvars {
    controller: Controller,
    tab: TabId,
}

define_class!(
    // SAFETY: NSObject subclass implementing NSWindowDelegate; no unsafe Drop.
    #[unsafe(super(NSObject))]
    #[name = "GhosttyWindowDelegate"]
    #[ivars = WindowDelegateIvars]
    #[thread_kind = MainThreadOnly]
    pub struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        /// The window (tab) became key: mark its tab active in the controller.
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            let ivars = self.ivars();
            ivars.controller.set_active(ivars.tab);
        }

        /// The window resized: reflow the tab to the new view size (and
        /// re-apply the layer's contentsScale). Without this the grid + surface
        /// stay frozen at the initial content size while the view grows, so the
        /// terminal fills only the initial corner of a resized window.
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            let ivars = self.ivars();
            ivars.controller.resync_tab_geometry(ivars.tab);
        }

        /// The window's backing properties changed — most importantly the
        /// backing-scale factor when the window moves between a Retina and a
        /// non-Retina display. Re-resolve the scale and rebuild so the
        /// device-pixel surface and the layer's contentsScale match the new
        /// display (the cross-display half of the "blank window" bug).
        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing_properties(&self, _notification: &NSNotification) {
            let ivars = self.ivars();
            ivars.controller.resync_tab_geometry(ivars.tab);
        }
    }
);

impl WindowDelegate {
    /// Create a window delegate bound to `controller` + `tab`.
    fn new(mtm: MainThreadMarker, controller: Controller, tab: TabId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(WindowDelegateIvars { controller, tab });
        unsafe { msg_send![super(this), init] }
    }
}

// ---------------------------------------------------------------------------
// AppDelegate + menu target
// ---------------------------------------------------------------------------

/// Ivars for the app delegate: the controller and a smoke auto-exit deadline.
pub struct DelegateIvars {
    controller: Controller,
    /// Auto-exit after this many milliseconds (smoke mode), 0 = never.
    smoke_ms: u64,
    /// Synthetic-input smoke: text to type into the active tab after launch,
    /// then assert echoes/output before exiting. Empty = disabled. See
    /// [`AppDelegate::run_type_smoke`].
    smoke_type: RefCell<String>,
    /// Tab-strip geometry smoke (`GHOSTTY_APP_SMOKE_GEOMETRY`): dump + assert the
    /// window geometry across the 1-tab → 2-tab → 1-tab transition, then exit.
    /// See [`AppDelegate::run_geometry_smoke`].
    smoke_geometry: bool,
}

define_class!(
    // SAFETY: NSObject subclass; implements NSApplicationDelegate + a menu action
    // selector. No unsafe Drop.
    #[unsafe(super(NSObject))]
    #[name = "GhosttyAppDelegate"]
    #[ivars = DelegateIvars]
    #[thread_kind = MainThreadOnly]
    pub struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            let mtm = self.mtm();
            let app = NSApplication::sharedApplication(mtm);

            // Menu bar.
            let menu = build_menu(mtm, self);
            app.setMainMenu(Some(&menu));

            // First window.
            self.ivars().controller.new_window();

            // Pace timer (~60Hz) on the main run loop.
            self.start_pace_timer();

            // Geometry smoke: dump + assert window geometry across the
            // 1-tab→2-tab→1-tab transition, then exit. Takes precedence over the
            // other smokes (it exits itself). Then synthetic-input, then the
            // plain auto-exit.
            let has_geometry = self.ivars().smoke_geometry;
            let has_type = !self.ivars().smoke_type.borrow().is_empty();
            if has_geometry {
                self.schedule_geometry_smoke();
            } else if has_type {
                self.schedule_type_smoke();
            } else {
                // Smoke auto-exit.
                let smoke_ms = self.ivars().smoke_ms;
                if smoke_ms > 0 {
                    self.schedule_auto_exit(smoke_ms);
                }
            }

            // Claim frontmost/focus. `activate()` alone is the modern
            // *cooperative* form: when the binary is launched from a terminal
            // (no .app bundle, activation policy set programmatically), it
            // often does NOT steal key focus from the launching terminal, so
            // the window renders but hardware keystrokes never reach `keyDown:`
            // (the "I can see tabs but can't type" symptom). Forcibly take
            // focus so a terminal-launched build is typable.
            app.activate();
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last(&self, _app: &NSApplication) -> bool {
            true
        }
    }

    impl AppDelegate {
        /// Menu-item / key-equivalent action: recover the [`MenuAction`] from the
        /// sender's tag and dispatch it.
        #[unsafe(method(ghosttyMenuAction:))]
        fn menu_action(&self, sender: &AnyObject) {
            let tag: isize = unsafe { msg_send![sender, tag] };
            if let Some(action) = MenuAction::from_tag(tag) {
                self.ivars().controller.handle_action(action);
            }
        }

        /// Pace-timer callback: pump + render every tab.
        #[unsafe(method(ghosttyPaceTick:))]
        fn pace_tick(&self, _timer: &AnyObject) {
            self.ivars().controller.tick();
        }

        /// Smoke auto-exit callback.
        #[unsafe(method(ghosttyAutoExit:))]
        fn auto_exit(&self, _timer: &AnyObject) {
            NSApplication::sharedApplication(self.mtm()).terminate(None);
        }

        /// Synthetic-input smoke: deliver the scripted keystrokes now (the shell
        /// has had time to draw its prompt), then schedule the assertion.
        #[unsafe(method(ghosttyTypeSmokeSend:))]
        fn type_smoke_send(&self, _timer: &AnyObject) {
            self.run_type_smoke();
        }

        /// Synthetic-input smoke: read the active tab's screen and assert the
        /// typed command's output appeared, then exit 0/1.
        #[unsafe(method(ghosttyTypeSmokeCheck:))]
        fn type_smoke_check(&self, _timer: &AnyObject) {
            self.finish_type_smoke();
        }

        /// Geometry smoke, phase 1: dump/assert the 1-tab state, then open a
        /// second tab and schedule phase 2.
        #[unsafe(method(ghosttyGeomSmokeOneTab:))]
        fn geom_smoke_one_tab(&self, _timer: &AnyObject) {
            self.geometry_smoke_one_tab();
        }

        /// Geometry smoke, phase 2: dump/assert the 2-tab state, then close the
        /// second tab and schedule phase 3.
        #[unsafe(method(ghosttyGeomSmokeTwoTabs:))]
        fn geom_smoke_two_tabs(&self, _timer: &AnyObject) {
            self.geometry_smoke_two_tabs();
        }

        /// Geometry smoke, phase 3: dump/assert the back-to-1-tab state, then
        /// exit 0/1.
        #[unsafe(method(ghosttyGeomSmokeClosed:))]
        fn geom_smoke_closed(&self, _timer: &AnyObject) {
            self.geometry_smoke_closed();
        }
    }
);

impl AppDelegate {
    /// Create the delegate.
    pub fn new(
        mtm: MainThreadMarker,
        controller: Controller,
        smoke_ms: u64,
        smoke_type: String,
        smoke_geometry: bool,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            controller,
            smoke_ms,
            smoke_type: RefCell::new(smoke_type),
            smoke_geometry,
        });
        unsafe { msg_send![super(this), init] }
    }

    fn mtm(&self) -> MainThreadMarker {
        MainThreadMarker::from(self)
    }

    /// Schedule the synthetic-input smoke: give the shell ~700 ms to draw its
    /// prompt, then send the scripted keystrokes (a follow-on timer reads the
    /// result and exits).
    fn schedule_type_smoke(&self) {
        let target: &AnyObject = self.as_ref();
        // SAFETY: the delegate outlives the timer; the selector is implemented
        // on this class; main-thread call.
        unsafe {
            let _ = objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.7,
                target,
                sel!(ghosttyTypeSmokeSend:),
                None,
                false,
            );
        }
    }

    /// Deliver each character of the smoke script as a synthetic `keyDown`
    /// `NSEvent` through the real AppKit responder chain (`app.sendEvent`), so
    /// the whole window input path — first responder, `keyDown:`,
    /// `interpretKeyEvents`, `insertText`/encode, PTY write — is exercised
    /// exactly as a human keystroke would be. Then schedule the assertion for
    /// ~900 ms out to let the shell round-trip.
    fn run_type_smoke(&self) {
        let mtm = self.mtm();
        let app = NSApplication::sharedApplication(mtm);
        let controller = &self.ivars().controller;

        // The regression this smoke guards is that a terminal-launched build
        // never became frontmost, leaving no key window and no responder to
        // receive keystrokes. So drive the OS *key* window — the exact target a
        // real keystroke would hit. If activation is broken again, there is no
        // key window and the assertion below fails, which is precisely the
        // point. Fall back to the active tab's window only if a key window is
        // somehow absent, so the harness still delivers something to assert on.
        let win_num: isize = app
            .keyWindow()
            .or_else(|| controller.active_window())
            .map(|w| w.windowNumber())
            .unwrap_or(0);

        let script = self.ivars().smoke_type.borrow().clone();
        for ch in script.chars() {
            let (keycode, chars) = synth_key_for_char(ch);
            let ns_chars = NSString::from_str(&chars);
            // SAFETY: constructing a standard keyDown NSEvent via the class
            // method; all pointers valid, context nil. Then dispatch it through
            // the app like a real event.
            unsafe {
                let cls = objc2::class!(NSEvent);
                let event: Option<Retained<objc2_app_kit::NSEvent>> = msg_send![
                    cls,
                    keyEventWithType: NSEventType::KeyDown,
                    location: NSPoint::new(0.0, 0.0),
                    modifierFlags: NSEventModifierFlags::empty(),
                    timestamp: 0.0_f64,
                    windowNumber: win_num,
                    context: std::ptr::null::<AnyObject>(),
                    characters: &*ns_chars,
                    charactersIgnoringModifiers: &*ns_chars,
                    isARepeat: false,
                    keyCode: keycode,
                ];
                if let Some(event) = event {
                    app.sendEvent(&event);
                }
            }
        }

        // Schedule the assertion.
        let target: &AnyObject = self.as_ref();
        unsafe {
            let _ = objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                0.9,
                target,
                sel!(ghosttyTypeSmokeCheck:),
                None,
                false,
            );
        }
    }

    /// Read the active tab's engine screen and assert the smoke marker appeared
    /// (proof the keystroke reached the shell and its output rendered back).
    /// Exits the process `0` on success, `1` on failure.
    fn finish_type_smoke(&self) {
        let controller = &self.ivars().controller;
        let script = self.ivars().smoke_type.borrow().clone();
        let marker = smoke_marker(&script);
        let screen = controller.active_screen_text().unwrap_or_default();

        let occurrences = screen.matches(&marker).count();
        // The command line itself echoes the marker once; running it prints it
        // again. We require at least one occurrence (the echo) as proof the
        // keystrokes reached the PTY and rendered; two means the command also
        // ran. Either way, zero means typing was completely dead.
        if occurrences < 1 {
            eprintln!(
                "FAIL: synthetic-input smoke — marker '{marker}' not found; \
                 typing produced no output.\n----- screen -----\n{screen}\n------------------"
            );
            std::process::exit(1);
        }

        // Presented-pixel assertion (the gap the engine-text-only check missed):
        // when capture is enabled, require that the frame actually attached to
        // the CoreAnimation layer contains real glyph coverage — not just that
        // the engine's text buffer does. This is what catches the "window shows
        // only the theme background, zero glyphs" bug (presentation geometry /
        // never-re-presenting), which the old assertion sailed past because it
        // read the engine, never the screen.
        if std::env::var_os("GHOSTTY_APP_ASSERT_PRESENT").is_some() {
            // 40 matches the offscreen smoke's coverage floor: a blank clear
            // leaves max-delta ~0; a single rasterized glyph pushes it well
            // past this.
            const COVERAGE_FLOOR: i32 = 40;
            let delta = self.ivars().controller.active_present_delta().unwrap_or(0);
            if delta <= COVERAGE_FLOOR {
                eprintln!(
                    "FAIL: presented-frame smoke — the marker reached the engine \
                     (found {occurrences}x) but the PRESENTED frame is blank \
                     (max background delta {delta} <= {COVERAGE_FLOOR}). The layer \
                     is showing only the background: presentation geometry \
                     (contentsScale) or re-present path is broken."
                );
                std::process::exit(1);
            }
            println!(
                "OK: synthetic-input + presented-pixel smoke — marker '{marker}' \
                 found {occurrences}x in engine AND presented frame has glyph \
                 coverage (max background delta {delta})"
            );
            std::process::exit(0);
        }

        println!("OK: synthetic-input smoke — marker '{marker}' found {occurrences}x in screen");
        std::process::exit(0);
    }

    /// Schedule the geometry smoke: give the first window a beat to lay out on a
    /// screen (backing scale + content layout settle), then run phase 1.
    fn schedule_geometry_smoke(&self) {
        self.schedule_selector(0.5, sel!(ghosttyGeomSmokeOneTab:));
    }

    /// Geometry smoke phase 1 — the reported bug's state: a single tab. Assert
    /// the tab bar is hidden and the terminal view fills the content-layout
    /// rect (no exposed chrome band). Then open a second tab and hand off to
    /// phase 2 after AppKit has shown the tab bar and re-laid-out.
    fn geometry_smoke_one_tab(&self) {
        let controller = &self.ivars().controller;
        let Some(geom) = controller.active_geometry() else {
            eprintln!("FAIL: geometry smoke — no active tab at phase 1");
            std::process::exit(1);
        };
        geom.log("1-tab");

        let mut failed = false;
        if geom.tab_bar_visible {
            eprintln!(
                "FAIL: geometry smoke — tab bar is VISIBLE with a single tab \
                 (tab_count={}). The dark band under the titlebar is the native \
                 tab strip showing when it shouldn't.",
                geom.tab_count
            );
            failed = true;
        }
        if geom.has_toolbar {
            eprintln!(
                "FAIL: geometry smoke — window has an unexpected NSToolbar; its \
                 height is exposed as a band under the titlebar."
            );
            failed = true;
        }
        // The view must cover the full un-obscured content height. Allow a
        // sub-pixel rounding slack; the reported band was ~25px, far above it.
        const BAND_SLACK_PT: f64 = 1.0;
        if geom.top_band_points() > BAND_SLACK_PT {
            eprintln!(
                "FAIL: geometry smoke — the terminal view does not fill the \
                 content-layout rect at 1 tab: a {:.1}pt band of window chrome \
                 is exposed above/below the terminal (content_layout={:.1}h, \
                 view={:.1}h).",
                geom.top_band_points(),
                geom.content_layout.height,
                geom.view_frame.size.height,
            );
            failed = true;
        }
        // The band-fix assertions: whatever sub-cell remainder the grid fit
        // leaves (it lives in *our surface layer*, not window chrome), it must be
        // (a) painted the terminal background so it is seamless, and (b) pinned to
        // the bottom edge (not shown as a band under the titlebar). These are the
        // two changes that actually kill the reported band; the window-layout
        // asserts above only prove the *chrome* was already correct.
        if !geom.bg_matches_terminal {
            eprintln!(
                "FAIL: geometry smoke — the window background does not match the \
                 terminal background, so the sub-cell remainder strip \
                 ({:.1}pt of layer the surface can't cover) would show as a \
                 chrome-grey band. Set the window background to the terminal bg.",
                geom.surface_gap_points(),
            );
            failed = true;
        }
        if !geom.gravity_pins_visual_top {
            eprintln!(
                "FAIL: geometry smoke — the host layer's contentsGravity does not \
                 pin the surface to the visual top; under the flipped view this \
                 leaves the {:.1}pt sub-cell remainder as a band directly under \
                 the titlebar (the reported bug).",
                geom.surface_gap_points(),
            );
            failed = true;
        }
        if failed {
            std::process::exit(1);
        }

        // Transition: open a second tab in the same window group.
        if let Some(active) = controller.active_tab() {
            controller.new_tab_in(active);
        }
        // Let AppKit show the tab bar and re-lay-out before phase 2.
        self.schedule_selector(0.5, sel!(ghosttyGeomSmokeTwoTabs:));
    }

    /// Geometry smoke phase 2 — two tabs: the native tab bar must now be
    /// visible, and the view must fill the *reduced* content-layout rect (the
    /// content shrinks to make room for the bar, and the terminal must follow).
    fn geometry_smoke_two_tabs(&self) {
        let controller = &self.ivars().controller;
        let Some(geom) = controller.active_geometry() else {
            eprintln!("FAIL: geometry smoke — no active tab at phase 2");
            std::process::exit(1);
        };
        geom.log("2-tab");

        let mut failed = false;
        if geom.tab_count < 2 {
            eprintln!(
                "FAIL: geometry smoke — expected 2 tabs in the group, saw {}.",
                geom.tab_count
            );
            failed = true;
        }
        if !geom.tab_bar_visible {
            eprintln!(
                "FAIL: geometry smoke — tab bar is NOT visible with 2 tabs; the \
                 native tab strip should appear."
            );
            failed = true;
        }
        // Even with the tab bar taking a slice, the view must fill whatever
        // content-layout rect remains — no band on either side of the terminal.
        const BAND_SLACK_PT: f64 = 1.0;
        if geom.top_band_points() > BAND_SLACK_PT {
            eprintln!(
                "FAIL: geometry smoke — with the tab bar visible the view does \
                 not fill the reduced content-layout rect: {:.1}pt uncovered \
                 (content_layout={:.1}h, view={:.1}h).",
                geom.top_band_points(),
                geom.content_layout.height,
                geom.view_frame.size.height,
            );
            failed = true;
        }
        if failed {
            std::process::exit(1);
        }

        // Transition back: close the active (second) tab.
        if let Some(active) = controller.active_tab() {
            controller.close_tab(active);
        }
        self.schedule_selector(0.5, sel!(ghosttyGeomSmokeClosed:));
    }

    /// Geometry smoke phase 3 — back to a single tab after Cmd-W: the tab bar
    /// must be hidden again and the view must re-expand to fill the restored
    /// (larger) content-layout rect. Exits 0 on success.
    fn geometry_smoke_closed(&self) {
        let controller = &self.ivars().controller;
        let Some(geom) = controller.active_geometry() else {
            eprintln!("FAIL: geometry smoke — no active tab at phase 3");
            std::process::exit(1);
        };
        geom.log("closed-back-to-1");

        let mut failed = false;
        if geom.tab_bar_visible {
            eprintln!(
                "FAIL: geometry smoke — tab bar still visible after closing back \
                 to a single tab (tab_count={}).",
                geom.tab_count
            );
            failed = true;
        }
        const BAND_SLACK_PT: f64 = 1.0;
        if geom.top_band_points() > BAND_SLACK_PT {
            eprintln!(
                "FAIL: geometry smoke — after the tab bar hid, the view did not \
                 re-expand to fill the content-layout rect: {:.1}pt uncovered.",
                geom.top_band_points(),
            );
            failed = true;
        }
        if failed {
            std::process::exit(1);
        }

        println!(
            "OK: tab-strip geometry smoke — 1 tab has no chrome band (tab bar \
             hidden, view fills content-layout), 2 tabs show the bar with the \
             view filling the reduced rect, and closing back to 1 tab hides the \
             bar and re-expands the view."
        );
        std::process::exit(0);
    }

    /// Schedule a one-shot `selector` on `self` `secs` out.
    fn schedule_selector(&self, secs: f64, selector: Sel) {
        let target: &AnyObject = self.as_ref();
        // SAFETY: the delegate outlives the timer; the selector is implemented
        // on this class; main-thread call.
        unsafe {
            let _ = objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                secs,
                target,
                selector,
                None,
                false,
            );
        }
    }

    /// Start the ~16 ms pace timer (repeating) on the main run loop.
    fn start_pace_timer(&self) {
        let interval = 1.0 / 60.0;
        let target: &AnyObject = self.as_ref();
        // SAFETY: the delegate outlives the timer; the selector is implemented
        // on this class; main-thread call.
        unsafe {
            let _ = objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                interval,
                target,
                sel!(ghosttyPaceTick:),
                None,
                true,
            );
        }
    }

    /// Schedule a one-shot auto-exit `ms` milliseconds out (smoke mode).
    fn schedule_auto_exit(&self, ms: u64) {
        let interval = ms as f64 / 1000.0;
        let target: &AnyObject = self.as_ref();
        // SAFETY: as above; one-shot (repeats = false).
        unsafe {
            let _ = objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                interval,
                target,
                sel!(ghosttyAutoExit:),
                None,
                false,
            );
        }
    }
}

/// Build the full menu bar (App / Shell / Edit / View) from [`MenuAction`],
/// wiring each item's Cmd-key equivalent + target/action to the delegate.
fn build_menu(mtm: MainThreadMarker, target: &AppDelegate) -> Retained<NSMenu> {
    let main = NSMenu::new(mtm);

    for top in TopMenu::ALL {
        let item = NSMenuItem::new(mtm);
        let submenu = NSMenu::new(mtm);
        submenu.setTitle(&NSString::from_str(top.title()));

        for action in MenuAction::ALL {
            if action.menu() != top {
                continue;
            }
            let title = NSString::from_str(action.title());
            let key = NSString::from_str(&action.key_equivalent().to_string());
            let menu_item = unsafe {
                NSMenuItem::initWithTitle_action_keyEquivalent(
                    mtm.alloc(),
                    &title,
                    Some(sel!(ghosttyMenuAction:)),
                    &key,
                )
            };
            unsafe {
                menu_item.setTag(action.tag());
                menu_item.setTarget(Some(target));
                menu_item.setKeyEquivalentModifierMask(NSEventModifierFlags::Command);
            }
            submenu.addItem(&menu_item);
        }

        item.setSubmenu(Some(&submenu));
        main.addItem(&item);
    }

    main
}

/// Map a script character to a synthetic `(macOS keyCode, characters-string)`
/// pair for building a `keyDown` `NSEvent`. Only the characters used by the
/// smoke script (`echo <marker>\n`) need real keycodes; everything else falls
/// back to keycode 0 with its literal character, which the `NSTextInputClient`
/// `insertText` path still delivers verbatim (the keycode only matters for
/// keys that encode as control sequences, e.g. Enter). Newline maps to Return.
fn synth_key_for_char(ch: char) -> (u16, String) {
    match ch {
        '\n' | '\r' => (0x24, "\r".to_string()), // Return
        ' ' => (0x31, " ".to_string()),          // Space
        // Letters/digits/punctuation: keycode is irrelevant to the insertText
        // path; use 0 and carry the literal character.
        other => (0, other.to_string()),
    }
}

/// The substring the type-smoke asserts on: the argument of the scripted
/// `echo <marker>` (everything after the first space, trimmed of the trailing
/// newline). For a non-echo script, falls back to the whole trimmed script.
fn smoke_marker(script: &str) -> String {
    let trimmed = script.trim_end_matches(['\n', '\r']);
    trimmed
        .split_once(' ')
        .map(|(_, rest)| rest.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or(trimmed)
        .to_string()
}

/// Run the app: build the controller + delegate, set the activation policy, and
/// enter the run loop. `smoke_ms > 0` schedules an auto-exit for the launch
/// smoke test; a non-empty `smoke_type` instead runs the synthetic-input smoke
/// (type the string through the real keyDown path, assert its round-trip, exit);
/// `smoke_geometry` instead runs the tab-strip geometry smoke (dump + assert the
/// window geometry across the 1-tab→2-tab→1-tab transition, exit). Returns after
/// the run loop exits.
pub fn run(
    config: &crate::config::Config,
    smoke_ms: u64,
    smoke_type: String,
    smoke_geometry: bool,
) {
    let mtm = MainThreadMarker::new().expect("run() must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let controller = Controller::new(config, mtm);
    let delegate = AppDelegate::new(mtm, controller, smoke_ms, smoke_type, smoke_geometry);
    let object = ProtocolObject::from_ref(&*delegate);
    app.setDelegate(Some(object));

    app.run();
}
