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
    NSColor, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem, NSView, NSWindow,
    NSWindowDelegate, NSWindowStyleMask, NSWindowTabbingMode,
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
use crate::splitkeys::SplitAction;
use crate::splits::{Direction, Sequential, SplitTree, SurfaceId};
use crate::tabkeys::TabAction;
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

/// One pane within a tab: the multiplied unit of (view + engine + io). A
/// single-leaf tab has exactly one of these; a split tab has one per pane, keyed
/// by [`SurfaceId`] in [`Tab::surfaces`].
struct Surface {
    /// The vt engine (parser + terminal state), shared with this surface's
    /// termio parse thread. The parse thread locks it to apply pty output; the
    /// main pace tick locks it to render + drain replies
    /// (`docs/analysis/termio-hub.md` §3).
    engine: Arc<Mutex<Engine>>,
    /// This surface's own terminal IO stack (rustix pty + read pipeline + mailbox
    /// writer loop). Dropping it joins the io threads.
    io: TabIo,
    /// The render engine (cell buffers + Metal draw), if a Metal device exists.
    /// One per surface (multiple already coexist across tabs today).
    render: Option<RenderEngine>,
    /// The font grid the renderer shapes through.
    font: FontGrid,
    /// The current font size (drives font grid rebuilds).
    font_size: FontSize,
    /// This pane's own terminal view (its layer is the presented IOSurface).
    view: Retained<TerminalView>,
    /// Current grid dimensions (fit to this pane's rect, not the whole window).
    cols: usize,
    rows: usize,
    /// Backing scale (contentsScale) last applied.
    scale: f64,
    /// This pane's current pixel rect within the tab container (device pixels),
    /// last applied by the layout pass. Used for divider resize + neighbour
    /// geometry.
    rect: crate::splits::Rect,
    /// Last reported mouse cell (motion dedup for mouse reporting).
    last_mouse_cell: Option<(i64, i64)>,
    /// Whether a mouse button is currently held (for out-of-viewport motion).
    mouse_button_down: bool,
    /// The cell the current selection drag started at, if a drag is in progress.
    selection_anchor: Option<(usize, usize)>,
    /// Selection highlight colors resolved from the tab's theme at startup.
    selection_colors: SelectionColors,
    /// The terminal's default background as `(r, g, b)` — the presented-frame
    /// coverage baseline.
    default_bg: (u8, u8, u8),
    /// Debug frame-dump (env `GHOSTTY_APP_DUMP_FRAME`), if enabled.
    frame_dump: Option<crate::frame_dump::FrameDump>,
    /// Max per-pixel L1 delta from `default_bg` in the most recently *presented*
    /// frame.
    last_present_delta: i32,
    /// Whether the render path reads the presented frame back each tick.
    capture_present: bool,
    /// This pane's scrollback viewport offset in rows *up from the bottom*
    /// (0 = the live active area). The render path snapshots `snapshot_window`
    /// at this offset, so each split pane scrolls independently. Clamped to the
    /// pane's scrollback length on each wheel event; reset to 0 on key input
    /// (upstream `scroll-to-bottom.keystroke`, default on).
    scrollback_offset: usize,
    /// Per-pane wheel accumulator (sub-cell pixel remainder). Port of
    /// upstream `mouse.pending_scroll_y`.
    wheel: crate::scroll::WheelState,
}

impl Surface {
    /// Lock the shared engine. Held only briefly per call.
    fn engine(&self) -> std::sync::MutexGuard<'_, Engine> {
        self.engine.lock().expect("engine mutex poisoned")
    }

    /// Rebuild the render target + grid for this pane's current view size and
    /// scale, resizing the engine + pty to match. `focused` drives the
    /// hollow-cursor treatment on the next render.
    fn reflow(&mut self) {
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

    /// Apply this pane's backing scale to its host render layer's contentsScale.
    fn apply_contents_scale(&self) {
        self.view.host_layer().set_contents_scale(self.scale);
    }

    /// The grid size that fits this pane view's current pixel bounds.
    fn current_grid_size(&self) -> (usize, usize) {
        let bounds = self.view.bounds();
        let w = (bounds.size.width * self.scale) as usize;
        let h = (bounds.size.height * self.scale) as usize;
        geometry::grid_size(w, h, self.font.cell_width, self.font.cell_height)
    }

    /// Rebuild the font grid at this pane's current font size × backing scale.
    fn rebuild_font(&mut self, family: Option<&str>) {
        let px = (self.font_size.get() as f64) * self.scale;
        if let Ok(fg) = font::build(family, px) {
            self.font = fg;
            self.reflow();
        }
    }

    /// Per-tick IO servicing for this pane. Returns whether its child shell
    /// exited (so the caller closes this surface). `title_sink` receives the
    /// title/password state so the tab can reflect the *focused* pane's title in
    /// the window title.
    fn pump(&mut self) -> (bool, Option<bool>) {
        let out = self.engine().take_output();
        if !out.is_empty() {
            self.io.write(&out);
        }
        let mut exited = false;
        let mut password: Option<bool> = None;
        for event in self.io.drain_events() {
            match event {
                IoEvent::ChildExited { exit_code, .. } => {
                    if exit_code != 0 {
                        eprintln!("ghostty-app: shell exited with code {exit_code}");
                    }
                    exited = true;
                }
                IoEvent::PasswordInput(active) => {
                    password = Some(active);
                }
            }
        }
        (exited, password)
    }

    /// Render one frame into this pane's layer. `focused` selects the hollow
    /// (unfocused) vs. solid cursor via `FrameOptions.focused` — the renderer
    /// draws a hollow box for an unfocused pane at no extra cost.
    fn render(&mut self, focused: bool) {
        if self.render.is_none() {
            return;
        }
        let (mut window, range) = {
            let engine = self.engine();
            let window = engine.snapshot_window(self.scrollback_offset);
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
        let opts = FrameOptions {
            focused,
            ..FrameOptions::default()
        };
        render.update_frame(&snapshot, &mut self.font.grid, opts);
        if render.sync_atlas(&self.font.grid).is_err() {
            return;
        }

        if self.capture_present {
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

    /// Convert a device-pixel viewport position into a `(col, row)` cell.
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

    /// Snap this pane's viewport back to the live active area (offset 0) and
    /// clear the wheel accumulator. Called on key input to this pane (upstream
    /// `scroll-to-bottom.keystroke`, default on).
    fn snap_to_bottom(&mut self) {
        self.scrollback_offset = 0;
        self.wheel = crate::scroll::WheelState::default();
    }

    /// Apply one wheel event to this pane: run the decision ladder over the
    /// live mode state and either report (buttons 4/5), emit alternate-scroll
    /// cursor keys, or move the scrollback viewport. Port of the body of
    /// upstream `scrollCallback` (Surface.zig 3437–3599).
    ///
    /// `yoff` is the raw vertical scroll (macOS `scrollingDeltaY`; positive =
    /// up), `precision` whether it is a precision (trackpad) delta, `mods` the
    /// live modifiers (for the reporting path). `mult` is the configured
    /// multiplier.
    fn apply_wheel(
        &mut self,
        yoff: f64,
        precision: bool,
        mods: ghostty_input::key_mods::Mods,
        mult: crate::scroll::ScrollMultiplier,
    ) {
        let cell_h = self.font.cell_height as f64;
        let delta = self.wheel.row_delta(yoff, precision, cell_h, mult);
        if delta == 0 {
            return;
        }

        let (reporting_active, alt_screen, alt_scroll, cursor_keys) = {
            let engine = self.engine();
            (
                engine.mouse_event() != ghostty_input::mouse_encode::MouseEvent::None,
                engine.alt_screen_active(),
                engine.mouse_alternate_scroll(),
                engine.key_encode_options().cursor_key_application,
            )
        };

        match crate::scroll::decide(delta, reporting_active, alt_screen, alt_scroll) {
            crate::scroll::WheelOutcome::None => {}
            crate::scroll::WheelOutcome::AltScrollKeys { count, up } => {
                // Upstream clears the selection before sending cursor keys.
                self.engine().clear_selection();
                self.selection_anchor = None;
                let bytes = arrow_key_bytes(up, cursor_keys);
                for _ in 0..count {
                    self.io.write(&bytes);
                }
            }
            crate::scroll::WheelOutcome::Report { count, up } => {
                // Upstream clears the selection when reporting is active
                // (a shift-override selection could exist).
                self.engine().clear_selection();
                self.selection_anchor = None;
                for _ in 0..count {
                    self.report_wheel(up, mods);
                }
            }
            crate::scroll::WheelOutcome::Viewport { rows_up } => {
                // Positive `rows_up` scrolls *up* into history (increases the
                // offset); negative scrolls back down toward the active area.
                let max = self.engine().scrollback_len();
                let cur = self.scrollback_offset as isize;
                let next = (cur + rows_up).clamp(0, max as isize);
                self.scrollback_offset = next as usize;
            }
        }
    }

    /// Emit one xterm wheel button-4 (up) / button-5 (down) press report,
    /// re-using the existing mouse encode path so the bytes are byte-identical
    /// to a direct wheel report. A no-op if reporting produced nothing.
    fn report_wheel(&mut self, up: bool, mods: ghostty_input::key_mods::Mods) {
        use ghostty_input::mouse::{Action, Button};
        let button = if up { Button::Four } else { Button::Five };
        let (event_mode, format) = {
            let engine = self.engine();
            (engine.mouse_event(), engine.mouse_format())
        };
        let ctx = crate::input::mouse::MouseContext {
            event_mode,
            format,
            screen_width: (self.cols * self.font.cell_width as usize) as f64,
            screen_height: (self.rows * self.font.cell_height as usize) as f64,
            cell_width: self.font.cell_width as f64,
            cell_height: self.font.cell_height as f64,
            any_button_pressed: self.mouse_button_down,
        };
        // Wheel reports are a press at the current pointer cell. We don't track
        // a live pointer position on the wheel path (upstream reads the OS
        // cursor pos); report at the top-left cell (0,0), which the common SGR
        // decoders accept. Wheel buttons carry no motion, so the cell is
        // informational only for scroll.
        let bytes = crate::input::mouse::encode(
            Action::Press,
            Some(button),
            mods,
            0.0,
            0.0,
            &ctx,
            &mut self.last_mouse_cell,
        );
        if !bytes.is_empty() {
            self.io.write(&bytes);
        }
    }
}

/// The byte sequence for a synthetic cursor-up/down arrow key on the
/// alternate-scroll path, respecting DECCKM (`cursor_keys` application mode).
/// Matches upstream `scrollCallback`: application mode `ESC O A`/`ESC O B`,
/// normal mode `ESC [ A`/`ESC [ B`.
fn arrow_key_bytes(up: bool, cursor_key_application: bool) -> Vec<u8> {
    match (up, cursor_key_application) {
        (true, true) => b"\x1bOA".to_vec(),
        (false, true) => b"\x1bOB".to_vec(),
        (true, false) => b"\x1b[A".to_vec(),
        (false, false) => b"\x1b[B".to_vec(),
    }
}

/// One terminal tab: a window hosting a split tree of [`Surface`]s inside a
/// [`SplitContainer`]. A single-leaf tree is the one-pane tab (behaviourally
/// identical to the pre-splits `Tab`).
struct Tab {
    /// The split tree (pure model): which surfaces exist, how they're arranged,
    /// and which one is focused.
    tree: crate::splits::SplitTree,
    /// The pane bundles, keyed by the tree's leaf ids.
    surfaces: HashMap<crate::splits::SurfaceId, Surface>,
    /// The owning window (one NSWindow per tab; macOS groups them as tabs).
    window: Retained<NSWindow>,
    /// The flipped container hosting the pane views + divider strips.
    container: Retained<crate::splitview::SplitContainer>,
    /// The live divider strip views (rebuilt on every layout).
    dividers: Vec<Retained<crate::splitview::SplitDivider>>,
    /// The window's delegate (keeps the controller's active tab in sync).
    _window_delegate: Retained<WindowDelegate>,
    /// Next surface id to mint within this tab.
    next_surface: u64,
}

impl Tab {
    /// Mint a fresh surface id for this tab.
    fn mint_surface_id(&mut self) -> crate::splits::SurfaceId {
        let id = crate::splits::SurfaceId(self.next_surface);
        self.next_surface += 1;
        id
    }

    /// The focused surface's bundle.
    fn focused_surface(&self) -> Option<&Surface> {
        self.surfaces.get(&self.tree.focused())
    }

    /// Re-lay-out the panes for the container's current bounds: recompute each
    /// leaf's pixel rect from the tree, set the pane view frames, rebuild the
    /// divider strips, and reflow each surface's grid to its new rect. Called on
    /// creation, split, close, window resize, and divider drag. `scale` is the
    /// window's backing scale (points→pixels).
    fn relayout(
        &mut self,
        controller_ptr: *const Controller,
        tab_id: TabId,
        mtm: MainThreadMarker,
    ) {
        let scale = self.window.backingScaleFactor();
        let bounds = self.container.bounds();
        // Work in device pixels so grid math matches the single-pane path.
        let container_px = crate::splits::Rect::new(
            0.0,
            0.0,
            bounds.size.width * scale,
            bounds.size.height * scale,
        );
        let divider_px = (DIVIDER_THICKNESS_PT * scale).round();
        let layout = self.tree.layout(container_px, divider_px);

        // Position each pane view + reflow its grid.
        for (id, rect) in &layout.panes {
            if let Some(surface) = self.surfaces.get_mut(id) {
                surface.scale = scale;
                surface.rect = *rect;
                let frame = crate::splitview::ns_rect_from_tree(*rect, scale);
                surface.view.setFrame(frame);
                surface.reflow();
            }
        }

        // Rebuild divider strips (cheap; count is small).
        for d in self.dividers.drain(..) {
            d.removeFromSuperview();
        }
        for div in &layout.dividers {
            let frame = crate::splitview::ns_rect_from_tree(div.rect, scale);
            let view = crate::splitview::SplitDivider::new(
                mtm,
                controller_ptr,
                tab_id,
                div.path.clone(),
                div.axis,
                frame,
            );
            self.container.addSubview(&view);
            self.dividers.push(view);
        }
        // A changed divider layout invalidates cursor rects.
        self.window.invalidateCursorRectsForView(&self.container);
    }

    /// Reflect the *focused* pane's title (+ password marker) in the window
    /// title.
    fn update_window_title(&self, password: bool) {
        let base = self
            .focused_surface()
            .and_then(|s| s.engine().title())
            .unwrap_or_else(|| "ghostty-rs".to_string());
        let title = if password {
            format!("{base} 🔒")
        } else {
            base
        };
        self.window.setTitle(&NSString::from_str(&title));
    }
}

/// The divider strip thickness in points.
const DIVIDER_THICKNESS_PT: f64 = 4.0;

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
    /// Wheel-scroll multipliers (`mouse-scroll-multiplier` config), clamped to
    /// upstream's valid range.
    scroll_multiplier: crate::scroll::ScrollMultiplier,
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
            scroll_multiplier: crate::scroll::ScrollMultiplier {
                precision: config.mouse_scroll_multiplier.precision,
                discrete: config.mouse_scroll_multiplier.discrete,
            }
            .clamped(),
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

    /// Open a new tab in `parent`'s window group, inheriting the *focused*
    /// surface's pwd.
    pub fn new_tab_in(&self, parent: TabId) -> Option<TabId> {
        let pwd = {
            let state = self.0.borrow();
            state
                .tabs
                .get(&parent)
                .and_then(|t| t.focused_surface())
                .and_then(|s| s.engine().pwd())
                .and_then(|p| tabs::inherit_pwd(Some(&p)))
        };
        self.spawn_tab(pwd, Some(parent))
    }

    /// Close a tab: drop its bundle, close its window, update the registry.
    ///
    /// `NSWindow::close` synchronously transfers key focus to a sibling tab,
    /// which fires `windowDidBecomeKey:` → [`Controller::set_active`] →
    /// `borrow_mut`. So the controller borrow must be released *before* the
    /// close, or that re-entrant borrow panics ("already borrowed"). We remove
    /// the tab bundle under a scoped borrow, drop it, then close the window with
    /// no borrow held, then re-borrow to update the registry.
    pub fn close_tab(&self, tab: TabId) {
        let removed = self.0.borrow_mut().tabs.remove(&tab);
        if let Some(t) = removed {
            // No controller borrow is held here: the synchronous
            // windowDidBecomeKey → set_active re-entrancy is now safe.
            t.window.close();
        }
        self.0.borrow_mut().registry.remove(tab);
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
        let controller_ptr: *const Controller = self;
        let mut state = self.0.borrow_mut();
        let family = state.font_family.clone();
        let mtm = state.mtm;
        if let Some(t) = state.tabs.get_mut(&tab) {
            let new_scale = t.window.backingScaleFactor();
            // The container is the window's content view, so AppKit has already
            // sized it; re-lay-out (per-pane frames + grids follow the new
            // bounds/scale) and rebuild the fonts per pane if the scale changed.
            let scale_changed = t
                .focused_surface()
                .map(|s| (new_scale - s.scale).abs() > f64::EPSILON)
                .unwrap_or(true);
            if scale_changed {
                let family = family.clone();
                for surface in t.surfaces.values_mut() {
                    surface.scale = new_scale;
                    surface.rebuild_font(family.as_deref());
                }
            }
            t.relayout(controller_ptr, tab, mtm);
        }
    }

    /// The tab's [`SplitContainer`](crate::splitview::SplitContainer) was resized
    /// by AppKit (`setFrameSize:`). This fires for window resizes *and* for
    /// content-area changes that never post `windowDidResize:` — the native tab
    /// bar appearing/disappearing. Re-lay-out the panes to the new bounds.
    ///
    /// Re-entrancy: `relayout` (under `resync_tab_geometry` or `spawn_tab`) can
    /// itself provoke AppKit into a synchronous `setFrameSize:` while the
    /// controller is already borrowed; those outer callers re-lay-out anyway, so
    /// a failed `try_borrow_mut` is safely skipped rather than panicking.
    pub fn container_resized(&self, tab: TabId) {
        let controller_ptr: *const Controller = self;
        let Ok(mut state) = self.0.try_borrow_mut() else {
            return;
        };
        let mtm = state.mtm;
        if let Some(t) = state.tabs.get_mut(&tab) {
            t.relayout(controller_ptr, tab, mtm);
        }
    }

    /// The plain-text screen dump of the active tab's *focused* surface
    /// (smoke/test only).
    pub fn active_screen_text(&self) -> Option<String> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.focused_surface())
            .map(|s| s.engine().screen_dump())
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
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.focused_surface())
            .map(|s| s.last_present_delta)
    }

    /// The active tab's `NSWindow` (smoke/test only): the target the synthetic
    /// key events are delivered to. `None` if there is no active tab.
    pub fn active_window(&self) -> Option<Retained<NSWindow>> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).map(|t| t.window.clone())
    }

    /// The active tab's *focused* pane view (smoke/test only): used to force it
    /// to become first responder before delivering synthetic key events.
    pub fn active_view(&self) -> Option<Retained<TerminalView>> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.focused_surface())
            .map(|s| s.view.clone())
    }

    /// Mark `tab` active (called when its window becomes key).
    pub fn set_active(&self, tab: TabId) {
        self.0.borrow_mut().registry.activate(tab);
    }

    // -- splits smoke/test accessors --------------------------------------

    /// The number of panes (surfaces) in the active tab (smoke/test only).
    pub fn active_surface_count(&self) -> Option<usize> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).map(|t| t.tree.len())
    }

    /// The focused surface id of the active tab (smoke/test only).
    pub fn active_focused_surface(&self) -> Option<SurfaceId> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).map(|t| t.tree.focused())
    }

    /// The active tab id + every surface id in the active tab, in flatten order
    /// (smoke/test only).
    pub fn active_surfaces(&self) -> Option<(TabId, Vec<SurfaceId>)> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).map(|t| (tab, t.tree.surfaces()))
    }

    /// The plain-text screen dump of a specific surface in a tab (smoke/test).
    pub fn surface_screen_text(&self, tab: TabId, surface: SurfaceId) -> Option<String> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.engine().screen_dump())
    }

    /// A specific surface's `(cols, rows)` grid (smoke/test) — the divider-resize
    /// smoke asserts these change when a divider moves.
    pub fn surface_grid(&self, tab: TabId, surface: SurfaceId) -> Option<(usize, usize)> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| (s.cols, s.rows))
    }

    /// A specific surface's presented-frame coverage (max background delta) —
    /// the presented-pixel smoke reads this to confirm each pane rendered ink in
    /// its own rect (smoke/test; needs `GHOSTTY_APP_ASSERT_PRESENT`).
    pub fn surface_present_delta(&self, tab: TabId, surface: SurfaceId) -> Option<i32> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.last_present_delta)
    }

    /// Write raw bytes directly to a specific surface's PTY (smoke/test only):
    /// the isolation probe writes a distinct marker to each pane and asserts it
    /// lands only in that pane's engine.
    pub fn write_to_surface(&self, tab: TabId, surface: SurfaceId, bytes: &[u8]) {
        let state = self.0.borrow();
        if let Some(s) = state.tabs.get(&tab).and_then(|t| t.surfaces.get(&surface)) {
            s.io.write(bytes);
        }
    }

    /// Feed bytes straight into a surface's *engine* (parser), as if they were
    /// pty output (smoke/test only). Unlike [`Controller::write_to_surface`]
    /// (which writes to the shell's stdin and waits for the async round-trip),
    /// this fills the terminal/scrollback synchronously — used by the
    /// scrollback-isolation probe so the offset is deterministic.
    pub fn feed_surface_output(&self, tab: TabId, surface: SurfaceId, bytes: &[u8]) {
        let state = self.0.borrow();
        if let Some(s) = state.tabs.get(&tab).and_then(|t| t.surfaces.get(&surface)) {
            s.engine().write(bytes);
        }
    }

    /// The active tab's divider paths in layout order (smoke/test only) — the
    /// divider-resize smoke picks one to drag.
    pub fn active_divider_paths(&self) -> Vec<Vec<bool>> {
        let state = self.0.borrow();
        let Some(tab) = state.registry.active() else {
            return Vec::new();
        };
        let Some(t) = state.tabs.get(&tab) else {
            return Vec::new();
        };
        let scale = t.window.backingScaleFactor();
        let bounds = t.container.bounds();
        let container_px = crate::splits::Rect::new(
            0.0,
            0.0,
            bounds.size.width * scale,
            bounds.size.height * scale,
        );
        let divider_px = (DIVIDER_THICKNESS_PT * scale).round();
        t.tree
            .layout(container_px, divider_px)
            .dividers
            .into_iter()
            .map(|d| d.path)
            .collect()
    }

    /// Move the split at `path` in the active tab to `ratio` (smoke/test only):
    /// a deterministic stand-in for a pointer divider drag that then re-lays-out
    /// both adjacent panes (engine + PTY resize).
    pub fn set_active_split_ratio(&self, path: &[bool], ratio: f64) {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;
        let mut state = self.0.borrow_mut();
        let Some(tab) = state.registry.active() else {
            return;
        };
        if let Some(t) = state.tabs.get_mut(&tab) {
            t.tree.set_ratio(path, ratio);
            t.relayout(controller_ptr, tab, mtm);
        }
    }

    /// Execute a built-in tab-navigation action against the active window's
    /// native tab group. Returns whether it did anything (a live tab existed).
    ///
    /// Runtime semantics are ported from upstream `onGotoTab`
    /// (`macos/Sources/Features/Terminal/TerminalController.swift` ~1500–1546):
    /// next/previous **wrap** (cyclic), `GotoTab(N)` is 1-based and **clamps**
    /// to the last tab (`min(N-1, count-1)`), `LastTab` selects the last tab.
    ///
    /// Ordering follows the native `tabGroup.windows` order (what the user sees
    /// in the tab bar), not our registry's insertion order — they can differ
    /// once tabs are reordered by drag. We select the target window with
    /// `makeKeyAndOrderFront`, exactly as upstream does; that fires
    /// `windowDidBecomeKey:` on the target's [`WindowDelegate`], which calls
    /// [`Controller::set_active`], so the registry's active pointer resyncs
    /// through the same path a manual tab-bar click uses (no separate
    /// bookkeeping to drift).
    pub fn handle_tab_action(&self, action: TabAction) -> bool {
        // The window we're currently on (the OS key window of the active tab).
        let Some(window) = self.active_window() else {
            return false;
        };

        // The tab group's windows in visual (tab-bar) order. A lone,
        // ungrouped window has no tab group; treat it as a single-tab group of
        // just itself so chords are harmless no-ops (never beep/crash) at 1 tab.
        let (windows, selected): (Vec<Retained<NSWindow>>, Option<Retained<NSWindow>>) =
            match window.tabGroup() {
                Some(group) => (
                    group.windows().iter().collect(),
                    group.selectedWindow().or(Some(window.clone())),
                ),
                None => (vec![window.clone()], Some(window.clone())),
            };
        let count = windows.len();
        if count == 0 {
            return false;
        }

        // Index of the currently selected window in the visual order.
        let selected_idx = selected
            .as_ref()
            .and_then(|sel| windows.iter().position(|w| w == sel))
            .unwrap_or(0);

        // Resolve the target index per upstream semantics.
        let target_idx = match action {
            TabAction::NextTab => {
                if selected_idx + 1 >= count {
                    0
                } else {
                    selected_idx + 1
                }
            }
            TabAction::PreviousTab => {
                if selected_idx == 0 {
                    count - 1
                } else {
                    selected_idx - 1
                }
            }
            // 1-based; clamp to the last tab (upstream `min(N-1, count-1)`).
            TabAction::GotoTab(n) => n.saturating_sub(1).min(count - 1),
            TabAction::LastTab => count - 1,
        };

        // Selecting the same window is a harmless no-op (the 1-tab case). Bring
        // the target to the front; its `windowDidBecomeKey:` resyncs `active`.
        windows[target_idx].makeKeyAndOrderFront(None);
        true
    }

    /// Snapshot the active tab's window/view/tab-group geometry (smoke/test
    /// only). The field the "spurious top band" bug lives in: if the terminal
    /// view does not fill `contentLayoutRect` exactly, the exposed slice is
    /// window chrome (the dark band). `None` if there is no active tab.
    pub fn active_geometry(&self) -> Option<WindowGeometry> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        let t = state.tabs.get(&tab)?;
        let s = t.focused_surface()?;
        let surface_px =
            crate::geometry::pixel_size(s.cols, s.rows, s.font.cell_width, s.font.cell_height);
        Some(WindowGeometry::probe(
            &t.window,
            &s.view,
            surface_px,
            s.default_bg,
        ))
    }

    /// The active window's 1-based position in its native tab group's visual
    /// order, plus the tab count (smoke/test only). `None` if there is no active
    /// tab. The tab-keys smoke asserts on this after each chord.
    pub fn active_visual_index(&self) -> Option<(usize, usize)> {
        let window = self.active_window()?;
        match window.tabGroup() {
            Some(group) => {
                let windows: Vec<Retained<NSWindow>> = group.windows().iter().collect();
                let count = windows.len();
                let selected = group.selectedWindow().unwrap_or_else(|| window.clone());
                let idx = windows.iter().position(|w| *w == selected).unwrap_or(0);
                Some((idx + 1, count))
            }
            None => Some((1, 1)),
        }
    }

    /// Encode a raw key event and write it to `surface`'s PTY (within `tab`).
    /// Input isolation: only the focused pane's view is first responder, so this
    /// only ever fires for the pane the user is looking at.
    pub fn encode_key_to_surface(&self, tab: TabId, surface: SurfaceId, raw: &RawKeyEvent) {
        let mut state = self.0.borrow_mut();
        let cfg = state.input_config;
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            let opts = s.engine().key_encode_options();
            let bytes = crate::input::translate::encode_raw(raw, &cfg, opts);
            if !bytes.is_empty() {
                // A key that produced bytes snaps the viewport back to the live
                // area (upstream `scroll-to-bottom.keystroke`, default on). Only
                // scrolled-back panes are affected; this pane is the focused
                // (first-responder) one that received the key.
                s.snap_to_bottom();
                s.io.write(&bytes);
            }
        }
    }

    /// Send already-composed text (IME commit) to `surface`'s pty. Committed
    /// text is user input, so it snaps this pane's viewport to the bottom.
    pub fn send_text_to_surface(&self, tab: TabId, surface: SurfaceId, text: &str) {
        let mut state = self.0.borrow_mut();
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            s.snap_to_bottom();
            s.io.write(text.as_bytes());
        }
    }

    /// Encode a mouse event (view-space pixels, relative to *this pane's* grid)
    /// against `surface`'s live mouse tracking mode/format and write it to the
    /// PTY. Mouse coordinates are per-pane because each pane view is flipped and
    /// its `locationInWindow` is converted to the pane view's own space, so the
    /// grid origin is the pane's top-left, not the window's.
    #[allow(clippy::too_many_arguments)]
    pub fn mouse_to_surface(
        &self,
        tab: TabId,
        surface: SurfaceId,
        action: ghostty_input::mouse::Action,
        button: Option<ghostty_input::mouse::Button>,
        mods: ghostty_input::key_mods::Mods,
        x: f32,
        y: f32,
        pressed: Option<bool>,
    ) {
        let copy_on_select = self.0.borrow().copy_on_select;
        let mut state = self.0.borrow_mut();
        let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        else {
            return;
        };
        if let Some(p) = pressed {
            s.mouse_button_down = p;
        }

        if button == Some(ghostty_input::mouse::Button::Left) {
            let reporting_active =
                s.engine().mouse_event() != ghostty_input::mouse_encode::MouseEvent::None;
            let selection_allowed = !reporting_active || mods.shift;
            if selection_allowed {
                match action {
                    ghostty_input::mouse::Action::Press => {
                        s.engine().clear_selection();
                        s.selection_anchor = None;
                        if let Some(cell) = s.cell_at(x, y) {
                            s.selection_anchor = Some(cell);
                        }
                    }
                    ghostty_input::mouse::Action::Motion => {
                        if s.mouse_button_down
                            && let Some(anchor) = s.selection_anchor
                            && let Some(cell) = s.cell_at(x, y)
                        {
                            let mut engine = s.engine();
                            if let (Some(start), Some(end)) = (
                                engine.pin_at(anchor.0, anchor.1),
                                engine.pin_at(cell.0, cell.1),
                            ) {
                                engine.select(start, end, false);
                            }
                        }
                    }
                    ghostty_input::mouse::Action::Release => {
                        if copy_on_select && s.selection_anchor.is_some() {
                            let text = s.engine().selection_string();
                            if let Some(text) = text {
                                crate::clipboard::write(&text);
                            }
                        }
                        s.selection_anchor = None;
                    }
                }
            }
        }

        let (event_mode, format) = {
            let engine = s.engine();
            (engine.mouse_event(), engine.mouse_format())
        };
        let ctx = crate::input::mouse::MouseContext {
            event_mode,
            format,
            screen_width: (s.cols * s.font.cell_width as usize) as f64,
            screen_height: (s.rows * s.font.cell_height as usize) as f64,
            cell_width: s.font.cell_width as f64,
            cell_height: s.font.cell_height as f64,
            any_button_pressed: s.mouse_button_down,
        };
        let bytes =
            crate::input::mouse::encode(action, button, mods, x, y, &ctx, &mut s.last_mouse_cell);
        if !bytes.is_empty() {
            s.io.write(&bytes);
        }
    }

    /// Route one wheel event to `surface` within `tab`. Runs the wheel-scroll
    /// decision ladder (report / alternate-scroll cursor keys / scrollback
    /// viewport) against that pane's live mode state; each pane owns its own
    /// scrollback offset so panes scroll independently. `yoff` is the raw
    /// vertical scroll (positive = up), `precision` whether it's a trackpad
    /// (pixel) delta, `mods` the live modifiers (for the reporting path).
    pub fn wheel_to_surface(
        &self,
        tab: TabId,
        surface: SurfaceId,
        yoff: f64,
        precision: bool,
        mods: ghostty_input::key_mods::Mods,
    ) {
        let mult = self.0.borrow().scroll_multiplier;
        let mut state = self.0.borrow_mut();
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            s.apply_wheel(yoff, precision, mods, mult);
        }
    }

    /// This pane's current scrollback viewport offset in rows up from the
    /// bottom (0 = live active area). Smoke/test only.
    pub fn surface_scrollback_offset(&self, tab: TabId, surface: SurfaceId) -> Option<usize> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.scrollback_offset)
    }

    /// Focus `surface` within `tab` (click-to-focus / directional nav). Updates
    /// the tree's focused leaf, makes that pane's view first responder (so
    /// keystrokes/IME route there), and marks the tab active. A no-op if the
    /// surface isn't in the tab.
    pub fn focus_surface_in_tab(&self, tab: TabId, surface: SurfaceId) {
        let view = {
            let mut state = self.0.borrow_mut();
            let Some(t) = state.tabs.get_mut(&tab) else {
                return;
            };
            if !t.tree.focus(surface) {
                return;
            }
            t.focused_surface().map(|s| s.view.clone())
        };
        // Make the newly-focused pane the first responder outside the borrow
        // (AppKit may re-enter). Its window is already key (we don't change
        // tabs here).
        if let Some(view) = view
            && let Some(window) = view.window()
        {
            window.makeFirstResponder(Some(&view));
        }
        self.0.borrow_mut().registry.activate(tab);
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
                // cmd+w closes the *focused pane*; the last pane collapse closes
                // the tab (today's behaviour, preserved for single-pane tabs).
                if let Some(active) = self.active_tab() {
                    let focused = self.0.borrow().tabs.get(&active).map(|t| t.tree.focused());
                    if let Some(surface) = focused {
                        self.close_surface(active, surface);
                    }
                }
            }
            MenuAction::Copy => self.copy_selection_from_active(),
            MenuAction::Paste => self.paste_into_active(),
            MenuAction::FontSizeUp => self.font_size_active(FontStep::Up),
            MenuAction::FontSizeDown => self.font_size_active(FontStep::Down),
            MenuAction::FontSizeReset => self.font_size_active(FontStep::Reset),
            MenuAction::ShowNextTab => {
                self.handle_tab_action(TabAction::NextTab);
            }
            MenuAction::ShowPreviousTab => {
                self.handle_tab_action(TabAction::PreviousTab);
            }
            MenuAction::Quit => {
                let mtm = self.0.borrow().mtm;
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
    }

    /// Copy the *focused* pane's current selection to the system clipboard
    /// (Cmd-C). No-op if there is no selection.
    fn copy_selection_from_active(&self) {
        let Some(tab) = self.active_tab() else { return };
        let state = self.0.borrow();
        if let Some(s) = state.tabs.get(&tab).and_then(|t| t.focused_surface())
            && let Some(text) = s.engine().selection_string()
        {
            crate::clipboard::write(&text);
        }
    }

    /// Paste the clipboard into the *focused* pane's PTY, bracketed if the
    /// program enabled bracketed paste.
    fn paste_into_active(&self) {
        let Some(tab) = self.active_tab() else { return };
        let Some(text) = crate::clipboard::read() else {
            return;
        };
        let state = self.0.borrow();
        if let Some(s) = state.tabs.get(&tab).and_then(|t| t.focused_surface()) {
            let payload = if s.engine().bracketed_paste() {
                let mut p = Vec::with_capacity(text.len() + 12);
                p.extend_from_slice(b"\x1b[200~");
                p.extend_from_slice(text.as_bytes());
                p.extend_from_slice(b"\x1b[201~");
                p
            } else {
                text.into_bytes()
            };
            s.io.write(&payload);
        }
    }

    /// Apply a font-size step to *every* pane in the active tab and rebuild
    /// their grids. Font size is a tab-wide setting (all panes share the
    /// configured size), so a step applies to all of them, then re-lay-out so
    /// the changed cell metrics re-fit each pane's grid.
    fn font_size_active(&self, step: FontStep) {
        let Some(tab) = self.active_tab() else { return };
        let controller_ptr: *const Controller = self;
        let mut state = self.0.borrow_mut();
        let family = state.font_family.clone();
        let mtm = state.mtm;
        if let Some(t) = state.tabs.get_mut(&tab) {
            let mut any_changed = false;
            for s in t.surfaces.values_mut() {
                let changed = match step {
                    FontStep::Up => s.font_size.increase(),
                    FontStep::Down => s.font_size.decrease(),
                    FontStep::Reset => s.font_size.reset(),
                };
                if changed {
                    s.rebuild_font(family.as_deref());
                    any_changed = true;
                }
            }
            if any_changed {
                t.relayout(controller_ptr, tab, mtm);
            }
        }
    }

    /// Pump + render every pane of every live tab. Called each pace tick. A
    /// tab is closed when its *last* pane's shell exits; individual pane exits
    /// collapse the split (handled here via `close_surface`).
    pub fn tick(&self) {
        // Collect (tab, surface) pairs whose shell exited, plus per-tab focused
        // title/password state, under one borrow. Render every pane.
        let exited: Vec<(TabId, SurfaceId)> = {
            let mut state = self.0.borrow_mut();
            let mut dead = Vec::new();
            for (tid, tab) in state.tabs.iter_mut() {
                let focused = tab.tree.focused();
                let mut password_focused = false;
                for (sid, surface) in tab.surfaces.iter_mut() {
                    let (surface_exited, password) = surface.pump();
                    if surface_exited {
                        dead.push((*tid, *sid));
                    } else {
                        surface.render(*sid == focused);
                    }
                    if *sid == focused
                        && let Some(active) = password
                    {
                        password_focused = active;
                    }
                }
                tab.update_window_title(password_focused);
            }
            dead
        };
        for (tab, surface) in exited {
            self.close_surface(tab, surface);
        }
        // Quit when the last tab's last pane exits.
        if self.tab_count() == 0 {
            let mtm = self.0.borrow().mtm;
            NSApplication::sharedApplication(mtm).terminate(None);
        }
    }

    /// Build one [`Surface`] (view + engine + PTY + renderer) for `tab`, with a
    /// fresh `surface` id and provisional grid at `scale`. Shared by the initial
    /// pane in [`spawn_tab`] and every [`new_split`](Self::new_split). Spawns the
    /// shell in `cwd`. The pane view is created but not yet framed / added to a
    /// container (the caller lays it out).
    fn build_surface(
        &self,
        mtm: MainThreadMarker,
        tab: TabId,
        surface: SurfaceId,
        scale: f64,
        cwd: Option<&std::path::Path>,
    ) -> Option<Surface> {
        let (family, default_size, startup_colors, selection_colors) = {
            let s = self.0.borrow();
            (
                s.font_family.clone(),
                s.default_font_size,
                s.startup_colors.clone(),
                s.selection_colors,
            )
        };

        let font_size = FontSize::new(default_size);
        let fg = font::build(family.as_deref(), (font_size.get() as f64) * scale).ok()?;
        let (cw, ch) = (fg.cell_width, fg.cell_height);
        let init_w = (INITIAL_WIDTH * scale) as usize;
        let init_h = (INITIAL_HEIGHT * scale) as usize;
        let (cols, rows) = geometry::grid_size(init_w, init_h, cw, ch);

        let default_bg = startup_colors
            .background
            .get()
            .map(|c| (c.r, c.g, c.b))
            .unwrap_or((0x18, 0x18, 0x18));

        let engine = Arc::new(Mutex::new(Engine::with_colors(cols, rows, startup_colors)));
        let io = TabIo::spawn(Arc::clone(&engine), cols as u16, rows as u16, cw, ch, cwd).ok()?;
        let render = RenderEngine::new(cw, ch).ok();

        let frame_dump = crate::frame_dump::FrameDump::from_env();
        let capture_present =
            frame_dump.is_some() || std::env::var_os("GHOSTTY_APP_ASSERT_PRESENT").is_some();

        let controller_ptr: *const Controller = self;
        let view = TerminalView::new(mtm, tab, surface, controller_ptr);
        // Pin the surface flush to the visual top (kills the sub-cell dark band).
        view.pin_surface_to_top();

        Some(Surface {
            engine,
            io,
            render,
            font: fg,
            font_size,
            view,
            cols,
            rows,
            scale,
            rect: crate::splits::Rect::new(0.0, 0.0, 0.0, 0.0),
            last_mouse_cell: None,
            mouse_button_down: false,
            selection_anchor: None,
            selection_colors,
            default_bg,
            frame_dump,
            last_present_delta: 0,
            capture_present,
            scrollback_offset: 0,
            wheel: crate::scroll::WheelState::default(),
        })
    }

    /// Create a tab: a window whose content view is a [`SplitContainer`] hosting
    /// a single initial [`Surface`]. `cwd` is the new shell's directory;
    /// `tab_group_parent`, if set, adds the new window as a native tab of the
    /// parent's window.
    fn spawn_tab(&self, cwd: Option<PathBuf>, tab_group_parent: Option<TabId>) -> Option<TabId> {
        let mtm = self.0.borrow().mtm;
        let scale = 2.0; // provisional; corrected below from the real window.

        // Register the tab, mint the first surface id.
        let id = self.0.borrow_mut().registry.add();
        let surface_id = SurfaceId(0);

        let mut surface = self.build_surface(mtm, id, surface_id, scale, cwd.as_deref())?;
        let default_bg = surface.default_bg;

        // The container hosts the pane view(s); it is the window's content view,
        // so AppKit keeps it sized to the content area (its `setFrameSize:`
        // override triggers `container_resized` → relayout — the path that also
        // covers the tab bar appearing/disappearing).
        let controller_ptr: *const Controller = self;
        let container = crate::splitview::SplitContainer::new(
            mtm,
            controller_ptr,
            id,
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(INITIAL_WIDTH, INITIAL_HEIGHT),
            ),
        );
        container.addSubview(&surface.view);

        let window = make_window(mtm, &container);
        // Paint the window background the terminal colour so any sub-cell
        // remainder strip is seamless (see the R5 dark-band fix).
        set_window_background(&window, default_bg);

        let window_delegate = WindowDelegate::new(mtm, self.clone(), id);
        window.setDelegate(Some(ProtocolObject::from_ref(&*window_delegate)));

        let mut tree = SplitTree::leaf(surface_id);
        tree.focus(surface_id);

        // Correct the scale from the real window before first layout.
        let real_scale = window.backingScaleFactor();
        surface.scale = real_scale;
        if (real_scale - scale).abs() > f64::EPSILON {
            let family = self.0.borrow().font_family.clone();
            surface.rebuild_font(family.as_deref());
        }

        let view = surface.view.clone();
        let mut surfaces = HashMap::new();
        surfaces.insert(surface_id, surface);

        let mut tab = Tab {
            tree,
            surfaces,
            window: window.clone(),
            container: container.clone(),
            dividers: Vec::new(),
            _window_delegate: window_delegate,
            next_surface: 1,
        };
        // Lay out the single pane to the container's (AppKit-sized) bounds.
        tab.relayout(controller_ptr, id, mtm);

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

    // -- splits -----------------------------------------------------------

    /// Dispatch a resolved [`SplitAction`] against `tab`. Split chords come in
    /// through the view's `performKeyEquivalent:` (see [`crate::splitkeys`]).
    pub fn handle_split_action(&self, tab: TabId, action: SplitAction) {
        match action {
            SplitAction::NewSplit(dir) => self.new_split(tab, dir),
            SplitAction::GotoSplit(dir) => self.goto_split(tab, dir),
            SplitAction::GotoAdjacent(seq) => self.goto_adjacent(tab, seq),
        }
    }

    /// Split the focused pane of `tab` in `direction`, spawning a new surface
    /// (its own shell, inheriting the focused pane's pwd) at a 50/50 ratio, then
    /// re-lay-out. The new pane becomes focused (first responder).
    pub fn new_split(&self, tab: TabId, direction: Direction) {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;

        // Resolve the inherited pwd + a fresh surface id under a scoped borrow.
        let (surface_id, cwd, scale) = {
            let mut state = self.0.borrow_mut();
            let Some(t) = state.tabs.get_mut(&tab) else {
                return;
            };
            let cwd = t
                .focused_surface()
                .and_then(|s| s.engine().pwd())
                .and_then(|p| tabs::inherit_pwd(Some(&p)));
            let scale = t.window.backingScaleFactor();
            let sid = t.mint_surface_id();
            (sid, cwd, scale)
        };

        // Build the new surface outside the borrow (spawning a shell is heavy).
        let Some(surface) = self.build_surface(mtm, tab, surface_id, scale, cwd.as_deref()) else {
            return;
        };
        let view = surface.view.clone();

        // Insert into the tree + container + surface map, then re-lay-out.
        {
            let mut state = self.0.borrow_mut();
            let Some(t) = state.tabs.get_mut(&tab) else {
                return;
            };
            t.container.addSubview(&surface.view);
            t.surfaces.insert(surface_id, surface);
            t.tree.split(surface_id, direction);
            t.relayout(controller_ptr, tab, mtm);
        }

        // Focus the new pane (first responder) outside the borrow.
        if let Some(window) = view.window() {
            window.makeFirstResponder(Some(&view));
        }
    }

    /// Move focus to the spatially-adjacent pane of `tab` in `direction`, if one
    /// exists. No-op otherwise (mirrors upstream's performable check).
    pub fn goto_split(&self, tab: TabId, direction: Direction) {
        let target = {
            let state = self.0.borrow();
            let Some(t) = state.tabs.get(&tab) else {
                return;
            };
            let bounds = t.container.bounds();
            let scale = t.window.backingScaleFactor();
            let container_px = crate::splits::Rect::new(
                0.0,
                0.0,
                bounds.size.width * scale,
                bounds.size.height * scale,
            );
            let divider_px = (DIVIDER_THICKNESS_PT * scale).round();
            let layout = t.tree.layout(container_px, divider_px);
            t.tree.neighbor(direction, &layout)
        };
        if let Some(target) = target {
            self.focus_surface_in_tab(tab, target);
        }
    }

    /// Move focus to the previous / next pane of `tab` in flatten order (wraps).
    pub fn goto_adjacent(&self, tab: TabId, seq: Sequential) {
        let target = {
            let state = self.0.borrow();
            state.tabs.get(&tab).and_then(|t| t.tree.adjacent(seq))
        };
        if let Some(target) = target {
            self.focus_surface_in_tab(tab, target);
        }
    }

    /// Close a surface (pane) within `tab`: collapse its parent split so the
    /// sibling absorbs the space, drop the pane's view + IO, re-lay-out, and move
    /// focus to the sibling. If it was the tab's last pane, close the whole tab
    /// (today's behaviour). Called on `cmd+w` (focused pane) and on a pane's
    /// shell exit.
    pub fn close_surface(&self, tab: TabId, surface: SurfaceId) {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;

        // Mutate the tree + surface map under a scoped borrow; capture whether
        // the tab should close and the new focus target.
        enum Outcome {
            CloseTab,
            Refocus(Option<Retained<TerminalView>>),
        }
        let outcome = {
            let mut state = self.0.borrow_mut();
            let Some(t) = state.tabs.get_mut(&tab) else {
                return;
            };
            match t.tree.close(surface) {
                None => Outcome::CloseTab,
                Some(new_focus) => {
                    // Remove the pane bundle (drops its view + joins io threads).
                    if let Some(dead) = t.surfaces.remove(&surface) {
                        dead.view.removeFromSuperview();
                    } else {
                        return; // already gone
                    }
                    t.relayout(controller_ptr, tab, mtm);
                    let view = t.surfaces.get(&new_focus).map(|s| s.view.clone());
                    Outcome::Refocus(view)
                }
            }
        };

        match outcome {
            Outcome::CloseTab => self.close_tab(tab),
            Outcome::Refocus(view) => {
                if let Some(view) = view
                    && let Some(window) = view.window()
                {
                    window.makeFirstResponder(Some(&view));
                }
            }
        }
    }

    /// A divider drag: `coord` is the pointer's position along the split's axis
    /// in *container point space*. Convert it to a ratio against the split's own
    /// rect (from [`SplitTree::split_rect`]), set it, and re-lay-out both
    /// adjacent panes (each pane's engine + PTY resizes to its new rect).
    pub fn drag_divider(&self, tab: TabId, path: &[bool], coord: f64) {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;
        let mut state = self.0.borrow_mut();
        let Some(t) = state.tabs.get_mut(&tab) else {
            return;
        };
        let scale = t.window.backingScaleFactor();
        let bounds = t.container.bounds();
        let container_px = crate::splits::Rect::new(
            0.0,
            0.0,
            bounds.size.width * scale,
            bounds.size.height * scale,
        );
        let divider_px = (DIVIDER_THICKNESS_PT * scale).round();
        let Some((split_rect, axis)) = t.tree.split_rect(path, container_px, divider_px) else {
            return;
        };
        // Convert the pointer position into a ratio within the split's own rect.
        // `coord` is in points; scale to pixels to match `split_rect`.
        let coord_px = coord * scale;
        let (origin, span) = match axis {
            crate::splits::Axis::Horizontal => (split_rect.x, (split_rect.w - divider_px).max(1.0)),
            crate::splits::Axis::Vertical => (split_rect.y, (split_rect.h - divider_px).max(1.0)),
        };
        let ratio = (coord_px - origin) / span;
        t.tree.set_ratio(path, ratio);
        t.relayout(controller_ptr, tab, mtm);
    }
}

/// Which way a font-size step goes.
enum FontStep {
    Up,
    Down,
    Reset,
}

/// Build an `NSWindow` sized to the initial content, tabbing-enabled, hosting
/// `content_view` (the tab's split container) as its content view.
fn make_window(mtm: MainThreadMarker, content_view: &NSView) -> Retained<NSWindow> {
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
        window.setContentView(Some(content_view));
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
    /// Tab-navigation keybind smoke (`GHOSTTY_APP_SMOKE_TABKEYS`): open 3 tabs,
    /// drive the built-in tab chords, and assert the active-tab index after each
    /// (plus the pty-encoding regression: tab chords send nothing, plain Tab /
    /// Shift+Tab still encode). See [`AppDelegate::run_tabkeys_smoke`].
    smoke_tabkeys: bool,
    /// Splits smoke (`GHOSTTY_APP_SMOKE_SPLITS`): split right then down (3 panes),
    /// assert 3 live shells with isolated input, directional focus walk, divider
    /// resize, and close-collapse. See [`AppDelegate::run_splits_smoke`].
    smoke_splits: bool,
    /// Splits smoke phase state carried from phase 1 to phase 2: the tab under
    /// test and each pane's `(SurfaceId, unique marker)`.
    splits_state: RefCell<Option<SplitsSmokeState>>,
}

/// Phase-1→phase-2 handoff for the splits smoke: the tab under test and each
/// pane's `(SurfaceId, unique marker)`.
type SplitsSmokeState = (TabId, Vec<(SurfaceId, String)>);

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
            let has_tabkeys = self.ivars().smoke_tabkeys;
            let has_splits = self.ivars().smoke_splits;
            let has_type = !self.ivars().smoke_type.borrow().is_empty();
            if has_splits {
                self.schedule_splits_smoke();
            } else if has_tabkeys {
                self.schedule_tabkeys_smoke();
            } else if has_geometry {
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

        /// Tab-keys smoke: run the whole chord sequence and exit 0/1.
        #[unsafe(method(ghosttyTabKeysSmoke:))]
        fn tabkeys_smoke(&self, _timer: &AnyObject) {
            self.run_tabkeys_smoke();
        }

        /// Splits smoke phase 1: build 3 panes, assert focus walk, write markers.
        #[unsafe(method(ghosttySplitsSmoke:))]
        fn splits_smoke(&self, _timer: &AnyObject) {
            self.run_splits_smoke();
        }

        /// Splits smoke phase 2: assert isolation + resize + close-collapse, exit.
        #[unsafe(method(ghosttySplitsSmokeCheck:))]
        fn splits_smoke_check(&self, _timer: &AnyObject) {
            self.finish_splits_smoke();
        }
    }
);

impl AppDelegate {
    /// Create the delegate.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        mtm: MainThreadMarker,
        controller: Controller,
        smoke_ms: u64,
        smoke_type: String,
        smoke_geometry: bool,
        smoke_tabkeys: bool,
        smoke_splits: bool,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            controller,
            smoke_ms,
            smoke_type: RefCell::new(smoke_type),
            smoke_geometry,
            smoke_tabkeys,
            smoke_splits,
            splits_state: RefCell::new(None),
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

    /// Schedule the tab-keys smoke: give the first window a beat to settle on a
    /// screen, then run the whole chord sequence in one shot.
    fn schedule_tabkeys_smoke(&self) {
        self.schedule_selector(0.5, sel!(ghosttyTabKeysSmoke:));
    }

    /// Tab-navigation keybind smoke. Opens 3 tabs, drives the built-in chords as
    /// real synthetic `performKeyEquivalent:` events through the active view, and
    /// asserts the active-tab (1-based visual) index after each. Also runs the
    /// pty-encoding regression: the tab chords are *consumed* by
    /// `performKeyEquivalent:` (return `true` → never reach the encoder), while
    /// plain Tab, Shift+Tab, and Ctrl+I are *not* (return `false` → fall through
    /// to `keyDown:` → the encoder, which still emits their exact bytes). Exits
    /// 0 on success, 1 on the first failed assertion.
    fn run_tabkeys_smoke(&self) {
        let controller = &self.ivars().controller;
        let mtm = self.mtm();
        // Alias the free `!`-returning failure fn so the call sites read cleanly.
        let fail = tabkeys_fail;

        // --- Open two more tabs so the group has 3 (the first already exists). ---
        for _ in 0..2 {
            if let Some(active) = controller.active_tab() {
                controller.new_tab_in(active);
            }
        }
        // A freshly opened tab is the active/selected one → index 3 of 3.
        let (idx, count) = controller
            .active_visual_index()
            .unwrap_or_else(|| fail("no active tab after opening 3".into()));
        if count != 3 {
            fail(format!("expected 3 tabs, saw {count}"));
        }
        if idx != 3 {
            fail(format!("newest tab should be selected (3), saw {idx}"));
        }

        // --- ctrl+tab cycles 1→2→3→1 (wrapping). We start at 3, so one ctrl+tab
        // wraps to 1, then 2, then 3. Drive as real synthetic events through the
        // view's performKeyEquivalent so interception is exercised, not asserted. ---
        // Move to tab 1 first via cmd+1 so the cycle assertion starts from a
        // known point.
        Self::send_key_equiv(controller, mtm, KEYCODE_1, TAB_MOD_CMD);
        Self::expect_index(controller, 1, "cmd+1 → tab 1");

        Self::send_key_equiv(controller, mtm, KEYCODE_TAB, TAB_MOD_CTRL);
        Self::expect_index(controller, 2, "ctrl+tab 1→2");
        Self::send_key_equiv(controller, mtm, KEYCODE_TAB, TAB_MOD_CTRL);
        Self::expect_index(controller, 3, "ctrl+tab 2→3");
        Self::send_key_equiv(controller, mtm, KEYCODE_TAB, TAB_MOD_CTRL);
        Self::expect_index(controller, 1, "ctrl+tab 3→1 (wrap)");

        // --- ctrl+shift+tab reverses: 1→3→2→1 (wrapping backward). ---
        Self::send_key_equiv(controller, mtm, KEYCODE_TAB, TAB_MOD_CTRL_SHIFT);
        Self::expect_index(controller, 3, "ctrl+shift+tab 1→3 (wrap back)");
        Self::send_key_equiv(controller, mtm, KEYCODE_TAB, TAB_MOD_CTRL_SHIFT);
        Self::expect_index(controller, 2, "ctrl+shift+tab 3→2");

        // --- cmd+3 selects the third tab; cmd+9 selects the last. ---
        Self::send_key_equiv(controller, mtm, KEYCODE_3, TAB_MOD_CMD);
        Self::expect_index(controller, 3, "cmd+3 → tab 3");
        Self::send_key_equiv(controller, mtm, KEYCODE_1, TAB_MOD_CMD);
        Self::expect_index(controller, 1, "cmd+1 → tab 1");
        Self::send_key_equiv(controller, mtm, KEYCODE_9, TAB_MOD_CMD);
        Self::expect_index(controller, 3, "cmd+9 → last tab (3)");

        // --- cmd+5 (beyond the 3 tabs) clamps to the last tab (upstream min). ---
        Self::send_key_equiv(controller, mtm, KEYCODE_1, TAB_MOD_CMD);
        Self::expect_index(controller, 1, "cmd+1 → tab 1 (reset)");
        Self::send_key_equiv(controller, mtm, KEYCODE_5, TAB_MOD_CMD);
        Self::expect_index(controller, 3, "cmd+5 clamps to last (3)");

        // --- Interception / pty-encoding regression on the ACTIVE view. ---
        // Tab chords must be consumed by performKeyEquivalent (return true), so
        // they never reach keyDown → the encoder → the pty.
        let view = controller
            .active_view()
            .unwrap_or_else(|| fail("no active view for interception check".into()));
        let consumed_ctrl_tab = Self::perform_on_view(&view, mtm, KEYCODE_TAB, TAB_MOD_CTRL);
        if !consumed_ctrl_tab {
            fail("ctrl+tab was NOT consumed by performKeyEquivalent (would reach the pty)".into());
        }
        let consumed_ctrl_shift_tab =
            Self::perform_on_view(&view, mtm, KEYCODE_TAB, TAB_MOD_CTRL_SHIFT);
        if !consumed_ctrl_shift_tab {
            fail("ctrl+shift+tab was NOT consumed by performKeyEquivalent".into());
        }
        let consumed_cmd_3 = Self::perform_on_view(&view, mtm, KEYCODE_3, TAB_MOD_CMD);
        if !consumed_cmd_3 {
            fail("cmd+3 was NOT consumed by performKeyEquivalent".into());
        }

        // Plain Tab, Shift+Tab, and Ctrl+I must NOT be consumed — they fall
        // through to keyDown → the encoder. (performKeyEquivalent returns false.)
        let consumed_plain_tab = Self::perform_on_view(&view, mtm, KEYCODE_TAB, TAB_MOD_NONE);
        if consumed_plain_tab {
            fail(
                "plain Tab was WRONGLY consumed by performKeyEquivalent (won't reach the pty)"
                    .into(),
            );
        }
        let consumed_shift_tab = Self::perform_on_view(&view, mtm, KEYCODE_TAB, TAB_MOD_SHIFT);
        if consumed_shift_tab {
            fail("Shift+Tab was WRONGLY consumed (CSI Z won't reach the pty)".into());
        }
        let consumed_ctrl_i = Self::perform_on_view(&view, mtm, KEYCODE_I, TAB_MOD_CTRL);
        if consumed_ctrl_i {
            fail("Ctrl+I was WRONGLY consumed (its byte won't reach the pty)".into());
        }

        // The exact-bytes half of the regression: plain Tab and Shift+Tab still
        // encode to their PTY bytes via the pure encoder path the view uses.
        let tab_bytes = encode_tab_key(false);
        if tab_bytes != b"\t" {
            fail(format!("plain Tab must encode to \\t, got {tab_bytes:?}"));
        }
        let shift_tab_bytes = encode_tab_key(true);
        // Shift+Tab is CSI Z (back-tab) in the default (non-kitty) encoder.
        if shift_tab_bytes != b"\x1b[Z" {
            fail(format!(
                "Shift+Tab must encode to CSI Z, got {shift_tab_bytes:?}"
            ));
        }

        // --- Chords at 1 tab are harmless no-ops (no beep/crash). Close down to
        // a single tab and drive every chord; each must leave index=1/count=1. ---
        controller.close_tab(controller.active_tab().unwrap());
        controller.close_tab(controller.active_tab().unwrap());
        let (idx1, count1) = controller
            .active_visual_index()
            .unwrap_or_else(|| fail("no active tab after closing to 1".into()));
        if count1 != 1 || idx1 != 1 {
            fail(format!(
                "expected 1 tab at index 1, saw index {idx1} of {count1}"
            ));
        }
        for (kc, m, label) in [
            (KEYCODE_TAB, TAB_MOD_CTRL, "ctrl+tab @1"),
            (KEYCODE_TAB, TAB_MOD_CTRL_SHIFT, "ctrl+shift+tab @1"),
            (KEYCODE_3, TAB_MOD_CMD, "cmd+3 @1"),
            (KEYCODE_9, TAB_MOD_CMD, "cmd+9 @1"),
        ] {
            Self::send_key_equiv(controller, mtm, kc, m);
            let (i, c) = controller
                .active_visual_index()
                .unwrap_or_else(|| fail(format!("{label}: lost the active tab")));
            if i != 1 || c != 1 {
                fail(format!("{label} was not a no-op: index {i} of {c}"));
            }
        }

        println!(
            "OK: tab-keys smoke — ctrl+tab cycles 1→2→3→1, ctrl+shift+tab \
             reverses, cmd+3/cmd+9 select tab 3/last, cmd+5 clamps to last; tab \
             chords are consumed by performKeyEquivalent while plain Tab \
             (\\t) / Shift+Tab (CSI Z) / Ctrl+I fall through to the encoder; and \
             every chord at 1 tab is a no-op."
        );
        std::process::exit(0);
    }

    /// Resolve the active view and dispatch a synthetic `performKeyEquivalent:`
    /// event with the given physical keycode + tab modifiers, so the chord goes
    /// through the *real* interception path. No-op (returns) if there is no view.
    fn send_key_equiv(
        controller: &Controller,
        mtm: MainThreadMarker,
        keycode: u16,
        mods: NSEventModifierFlags,
    ) {
        if let Some(view) = controller.active_view() {
            let _ = Self::perform_on_view(&view, mtm, keycode, mods);
        }
    }

    /// Build a synthetic keyDown `NSEvent` for `keycode`+`mods` and send it to
    /// `view.performKeyEquivalent:`. Returns whether the view consumed it.
    fn perform_on_view(
        view: &TerminalView,
        _mtm: MainThreadMarker,
        keycode: u16,
        mods: NSEventModifierFlags,
    ) -> bool {
        let empty = NSString::from_str("");
        // SAFETY: standard keyEvent constructor; nil context; then a normal
        // performKeyEquivalent: dispatch on the main thread.
        unsafe {
            let cls = objc2::class!(NSEvent);
            let event: Option<Retained<objc2_app_kit::NSEvent>> = msg_send![
                cls,
                keyEventWithType: NSEventType::KeyDown,
                location: NSPoint::new(0.0, 0.0),
                modifierFlags: mods,
                timestamp: 0.0_f64,
                windowNumber: 0_isize,
                context: std::ptr::null::<AnyObject>(),
                characters: &*empty,
                charactersIgnoringModifiers: &*empty,
                isARepeat: false,
                keyCode: keycode,
            ];
            match event {
                Some(event) => msg_send![view, performKeyEquivalent: &*event],
                None => false,
            }
        }
    }

    /// Assert the active tab's 1-based visual index equals `want`, else fail.
    fn expect_index(controller: &Controller, want: usize, label: &str) {
        match controller.active_visual_index() {
            Some((idx, _)) if idx == want => {}
            Some((idx, count)) => tabkeys_fail(format!(
                "{label}: expected index {want}, saw {idx} of {count}"
            )),
            None => tabkeys_fail(format!("{label}: no active tab")),
        }
    }

    /// Schedule the splits smoke: let the first shell draw its prompt, then run
    /// phase 1.
    fn schedule_splits_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttySplitsSmoke:));
    }

    /// Splits smoke phase 1. Split the single pane right then down (3 panes),
    /// assert the tree shape + directional focus walk (pure geometry, no shell
    /// round-trip needed), then write a distinct marker to *each* pane's pty and
    /// schedule phase 2 to assert isolation + resize + close-collapse.
    fn run_splits_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = splits_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at splits smoke start".into());
        };

        // Start: exactly one pane.
        if controller.active_surface_count() != Some(1) {
            fail(format!(
                "expected 1 pane at start, saw {:?}",
                controller.active_surface_count()
            ));
        }

        // Split right → 2 panes; the new pane is focused.
        controller.new_split(tab, Direction::Right);
        if controller.active_surface_count() != Some(2) {
            fail(format!(
                "after split-right expected 2 panes, saw {:?}",
                controller.active_surface_count()
            ));
        }
        // Split down → 3 panes; new pane focused.
        controller.new_split(tab, Direction::Down);
        if controller.active_surface_count() != Some(3) {
            fail(format!(
                "after split-down expected 3 panes, saw {:?}",
                controller.active_surface_count()
            ));
        }

        // Layout is 0 | (1 / 2) in flatten order. Confirm and grab ids.
        let (tab_id, surfaces) = controller
            .active_surfaces()
            .unwrap_or_else(|| fail("no active surfaces after 2 splits".into()));
        if surfaces.len() != 3 {
            fail(format!("expected 3 surfaces, saw {}", surfaces.len()));
        }
        let (left, top_right, bottom_right) = (surfaces[0], surfaces[1], surfaces[2]);

        // Focus is on the last-created pane (bottom-right).
        if controller.active_focused_surface() != Some(bottom_right) {
            fail("newest pane (bottom-right) should be focused after split".into());
        }

        // --- Directional focus walk (pure geometry). From bottom-right: ---
        //   left  → left pane; up → top-right; then from left, up/down stay.
        controller.goto_split(tab_id, Direction::Left);
        if controller.active_focused_surface() != Some(left) {
            fail("goto-left from bottom-right should focus the left pane".into());
        }
        controller.goto_split(tab_id, Direction::Right);
        // From the left pane, right goes to one of the right column panes.
        let after_right = controller.active_focused_surface();
        if after_right != Some(top_right) && after_right != Some(bottom_right) {
            fail(format!(
                "goto-right from left should focus a right-column pane, saw {after_right:?}"
            ));
        }
        // Normalise: focus top-right, then down → bottom-right.
        controller.goto_split(tab_id, Direction::Up);
        if controller.active_focused_surface() != Some(top_right) {
            fail("goto-up should reach the top-right pane".into());
        }
        controller.goto_split(tab_id, Direction::Down);
        if controller.active_focused_surface() != Some(bottom_right) {
            fail("goto-down from top-right should reach bottom-right".into());
        }

        // --- Write a distinct marker into each pane's pty. Isolation is asserted
        // in phase 2 (each marker must appear ONLY in its own pane). ---
        let markers = [
            (left, "zz-split-marker-LEFT"),
            (top_right, "zz-split-marker-TOPRIGHT"),
            (bottom_right, "zz-split-marker-BOTRIGHT"),
        ];
        for (sid, marker) in markers {
            let cmd = format!("printf '{marker}\\n'\n");
            controller.write_to_surface(tab_id, sid, cmd.as_bytes());
        }

        *self.ivars().splits_state.borrow_mut() = Some((
            tab_id,
            markers
                .iter()
                .map(|(s, m)| (*s, (*m).to_string()))
                .collect(),
        ));

        // Give the three shells time to echo + run their printf, then check.
        self.schedule_selector(1.2, sel!(ghosttySplitsSmokeCheck:));
    }

    /// Splits smoke phase 2. Assert input isolation (each marker only in its own
    /// pane), presented-pixel coverage per pane (if enabled), divider resize
    /// (adjacent panes' grids change), and close-collapse (middle pane close →
    /// 2 panes, sibling absorbs; close all → tab closes → app quits). Exits 0/1.
    fn finish_splits_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = splits_fail;

        let (tab, markers) = self
            .ivars()
            .splits_state
            .borrow()
            .clone()
            .unwrap_or_else(|| fail("splits smoke phase-2 state missing".into()));

        // --- Input isolation: each pane's engine shows ITS marker (proof the
        // shell ran) and NONE of the others' (proof the ptys are independent and
        // keystrokes route only to the focused pane's writer). ---
        for (sid, marker) in &markers {
            let screen = controller
                .surface_screen_text(tab, *sid)
                .unwrap_or_default();
            if !screen.contains(marker) {
                fail(format!(
                    "pane {sid:?} is missing its own marker '{marker}' — its shell \
                     never ran / rendered.\n--- screen ---\n{screen}"
                ));
            }
            for (other_sid, other_marker) in &markers {
                if other_sid == sid {
                    continue;
                }
                if screen.contains(other_marker) {
                    fail(format!(
                        "input LEAK: pane {sid:?} contains another pane's marker \
                         '{other_marker}'. Writes are not isolated per surface."
                    ));
                }
            }
        }

        // --- Presented-pixel coverage: each pane rendered ink in its own rect.
        // Only when capture is enabled (GHOSTTY_APP_ASSERT_PRESENT). ---
        if std::env::var_os("GHOSTTY_APP_ASSERT_PRESENT").is_some() {
            const COVERAGE_FLOOR: i32 = 40;
            for (sid, _) in &markers {
                let delta = controller.surface_present_delta(tab, *sid).unwrap_or(0);
                if delta <= COVERAGE_FLOOR {
                    fail(format!(
                        "pane {sid:?} presented a blank frame (max bg delta {delta} \
                         <= {COVERAGE_FLOOR}): no ink in its own rect."
                    ));
                }
            }
        }

        // --- Divider resize: moving a divider changes both adjacent panes'
        // grids (engine cols/rows) and delivers a WINCH via TabIo::resize. ---
        let (left, top_right, bottom_right) = (markers[0].0, markers[1].0, markers[2].0);
        let before_left = controller
            .surface_grid(tab, left)
            .unwrap_or_else(|| fail("no grid for left pane".into()));
        let before_right = controller
            .surface_grid(tab, top_right)
            .unwrap_or_else(|| fail("no grid for top-right pane".into()));
        // The root split (path []) divides left | right. Shrink the left pane to
        // 25%.
        controller.set_active_split_ratio(&[], 0.25);
        let after_left = controller
            .surface_grid(tab, left)
            .unwrap_or_else(|| fail("no grid for left pane after resize".into()));
        let after_right = controller
            .surface_grid(tab, top_right)
            .unwrap_or_else(|| fail("no grid for top-right pane after resize".into()));
        if after_left.0 >= before_left.0 {
            fail(format!(
                "divider resize did not shrink the left pane's columns: {before_left:?} -> {after_left:?}"
            ));
        }
        if after_right.0 <= before_right.0 {
            fail(format!(
                "divider resize did not grow the right column's columns: {before_right:?} -> {after_right:?}"
            ));
        }

        // --- Per-pane scrollback isolation: fill the left pane's scrollback,
        // scroll IT back with wheel-up events, and assert only that pane's
        // viewport offset moved (the top-right pane stays pinned to the live
        // area). Proves each split pane owns its own scrollback offset. ---
        {
            // Push plenty of lines straight into the left pane's engine so it
            // has scrollback to reveal. Feeding the engine directly (not the
            // shell's stdin) makes the fill synchronous so the offset check
            // below is deterministic. Its grid is small, so ~120 lines is well
            // over a screen.
            let mut fill = String::new();
            for i in 0..120 {
                fill.push_str(&format!("SCROLLBACK-{i:03}\r\n"));
            }
            controller.feed_surface_output(tab, left, fill.as_bytes());
        }
        // Both panes start pinned to the bottom.
        if controller.surface_scrollback_offset(tab, left) != Some(0)
            || controller.surface_scrollback_offset(tab, top_right) != Some(0)
        {
            fail("panes should start at scrollback offset 0".into());
        }
        // Wheel the LEFT pane up several ticks (positive yoff = up into history).
        for _ in 0..5 {
            controller.wheel_to_surface(
                tab,
                left,
                1.0,
                false,
                ghostty_input::key_mods::Mods::default(),
            );
        }
        let left_off = controller.surface_scrollback_offset(tab, left).unwrap_or(0);
        if left_off == 0 {
            fail("wheel-up on the left pane did not move its scrollback offset".into());
        }
        // The OTHER pane must be unaffected.
        if controller.surface_scrollback_offset(tab, top_right) != Some(0) {
            fail(format!(
                "scrolling the left pane moved the top-right pane's viewport \
                 (offset {:?}); per-pane scrollback is not isolated",
                controller.surface_scrollback_offset(tab, top_right)
            ));
        }

        // --- Close-collapse: close the middle pane (top-right). The tree
        // collapses so the sibling (bottom-right) absorbs the right column →
        // 2 panes remain. ---
        controller.close_surface(tab, top_right);
        if controller.active_surface_count() != Some(2) {
            fail(format!(
                "closing the middle pane should leave 2, saw {:?}",
                controller.active_surface_count()
            ));
        }
        let (_, remaining) = controller
            .active_surfaces()
            .unwrap_or_else(|| fail("no surfaces after middle close".into()));
        if remaining.contains(&top_right) {
            fail("closed pane still present after collapse".into());
        }
        if !remaining.contains(&left) || !remaining.contains(&bottom_right) {
            fail("collapse dropped the wrong panes".into());
        }

        // --- Close the rest: closing every pane closes the tab, and (last tab)
        // quits the app. Close the two survivors; the second close removes the
        // last pane → close_tab → 0 tabs → app terminate. ---
        controller.close_surface(tab, left);
        if controller.active_surface_count() != Some(1) {
            fail(format!(
                "after closing another pane expected 1, saw {:?}",
                controller.active_surface_count()
            ));
        }
        // The final pane close collapses to a tab close.
        controller.close_surface(tab, bottom_right);
        if controller.tab_count() != 0 {
            fail(format!(
                "closing the last pane should close the tab (0 tabs), saw {}",
                controller.tab_count()
            ));
        }

        println!(
            "OK: splits smoke — split-right + split-down build 3 isolated shells \
             (each marker only in its own pane), directional focus walks the grid, \
             a divider drag resizes both adjacent panes' grids, wheel-scrolling one \
             pane back leaves the others pinned to the live area (per-pane scrollback \
             isolation), closing the middle pane collapses to 2 with the sibling \
             absorbing the space, and closing every pane closes the tab."
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
                // Every item carries Command; the tab-cycling items add Shift
                // (standard macOS Cmd-Shift-]/[ ).
                let mut mask = NSEventModifierFlags::Command;
                if action.key_equivalent_shift() {
                    mask |= NSEventModifierFlags::Shift;
                }
                menu_item.setKeyEquivalentModifierMask(mask);
            }
            submenu.addItem(&menu_item);
        }

        item.setSubmenu(Some(&submenu));
        main.addItem(&item);
    }

    main
}

/// Print a splits smoke failure and exit non-zero.
fn splits_fail(msg: String) -> ! {
    eprintln!("FAIL: splits smoke — {msg}");
    std::process::exit(1);
}

/// Print a tab-keys smoke failure and exit non-zero. Free `!`-returning fn so it
/// works inside `Option::unwrap_or_else` closures (which need the never type).
fn tabkeys_fail(msg: String) -> ! {
    eprintln!("FAIL: tab-keys smoke — {msg}");
    std::process::exit(1);
}

// macOS physical (Carbon `kVK_*`) keycodes used by the tab-keys smoke's
// synthetic events. Layout-independent, matching `crate::input::keymap`.
const KEYCODE_TAB: u16 = 0x30; // kVK_Tab
const KEYCODE_I: u16 = 0x22; // kVK_ANSI_I
const KEYCODE_1: u16 = 0x12; // kVK_ANSI_1
const KEYCODE_3: u16 = 0x14; // kVK_ANSI_3
const KEYCODE_5: u16 = 0x17; // kVK_ANSI_5
const KEYCODE_9: u16 = 0x19; // kVK_ANSI_9

// Modifier-flag combos for the tab-keys smoke's synthetic events.
const TAB_MOD_NONE: NSEventModifierFlags = NSEventModifierFlags::empty();
const TAB_MOD_SHIFT: NSEventModifierFlags = NSEventModifierFlags::Shift;
const TAB_MOD_CTRL: NSEventModifierFlags = NSEventModifierFlags::Control;
const TAB_MOD_CMD: NSEventModifierFlags = NSEventModifierFlags::Command;
const TAB_MOD_CTRL_SHIFT: NSEventModifierFlags =
    NSEventModifierFlags(NSEventModifierFlags::Control.0 | NSEventModifierFlags::Shift.0);

/// Encode a Tab keypress (optionally Shift-modified) through the *pure* encoder
/// with default (non-kitty) options — the exact path the view's `keyDown:` uses
/// for a tab that is not a built-in chord. Used by the tab-keys smoke's
/// pty-encoding regression: plain Tab → `\t`, Shift+Tab → CSI Z.
fn encode_tab_key(shift: bool) -> Vec<u8> {
    let raw = RawKeyEvent {
        keycode: KEYCODE_TAB,
        shift,
        text: if shift {
            String::new()
        } else {
            "\t".to_string()
        },
        ..Default::default()
    };
    crate::input::translate::encode_raw(
        &raw,
        &InputConfig::default(),
        ghostty_input::key_encode::Options::default(),
    )
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
/// window geometry across the 1-tab→2-tab→1-tab transition, exit);
/// `smoke_tabkeys` instead runs the tab-navigation keybind smoke (open 3 tabs,
/// drive the built-in tab chords via real `performKeyEquivalent:` events, assert
/// the active-tab index after each, exit); `smoke_splits` instead runs the
/// splits smoke (split right + down into 3 panes, assert isolated shells,
/// directional focus, divider resize, and close-collapse, exit). Returns after
/// the run loop exits.
#[allow(clippy::too_many_arguments)]
pub fn run(
    config: &crate::config::Config,
    smoke_ms: u64,
    smoke_type: String,
    smoke_geometry: bool,
    smoke_tabkeys: bool,
    smoke_splits: bool,
) {
    let mtm = MainThreadMarker::new().expect("run() must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let controller = Controller::new(config, mtm);
    let delegate = AppDelegate::new(
        mtm,
        controller,
        smoke_ms,
        smoke_type,
        smoke_geometry,
        smoke_tabkeys,
        smoke_splits,
    );
    let object = ProtocolObject::from_ref(&*delegate);
    app.setDelegate(Some(object));

    app.run();
}

#[cfg(test)]
mod tests {
    use super::arrow_key_bytes;

    #[test]
    fn arrow_key_bytes_match_upstream_alternate_scroll_sequences() {
        // Normal mode (DECCKM off): CSI A / CSI B.
        assert_eq!(arrow_key_bytes(true, false), b"\x1b[A");
        assert_eq!(arrow_key_bytes(false, false), b"\x1b[B");
        // Application mode (DECCKM on): SS3 A / SS3 B.
        assert_eq!(arrow_key_bytes(true, true), b"\x1bOA");
        assert_eq!(arrow_key_bytes(false, true), b"\x1bOB");
    }
}
