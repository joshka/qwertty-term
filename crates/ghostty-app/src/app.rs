//! The AppKit application host (chunk R5): `NSApplication` + `AppDelegate`, a
//! window/tab per terminal, the menu bar, and the main-thread pace loop that
//! pumps each tab's PTY and renders it.
//!
//! Object graph:
//!
//! - [`Controller`] (`Rc<RefCell<ControllerState>>`): the shared brain. Owns the
//!   [`TabRegistry`](crate::tabs::TabRegistry) and the per-tab [`Tab`] bundles,
//!   the config, and the input config. Menu actions and view keystrokes call
//!   into it. Single-threaded (main thread), so `Rc`/`RefCell`, not `Arc`/`Mutex`
//!   — everything terminal-side lives on the run loop; only the PTY reader
//!   threads (inside [`PtySession`](crate::pty::PtySession)) are off-thread, and
//!   they communicate through an mpsc channel the pace loop drains.
//! - [`Tab`]: one terminal — a vt [`Engine`](crate::engine::Engine), a
//!   [`PtySession`], a render [`RenderEngine`](ghostty_renderer::engine::Engine),
//!   a [`FontGrid`](crate::font::FontGrid), a [`FontSize`](crate::font_size::FontSize),
//!   an owning `NSWindow` + [`TerminalView`](crate::view::TerminalView), and the
//!   current grid dims.
//! - [`AppDelegate`]: builds the menu, opens the first window, starts the pace
//!   timer, and (for smoke) schedules an auto-exit.
//!
//! Pacing: an `NSTimer` on the main run loop ticks ~every 16 ms (plan decision
//! 3, timer-first). Each tick drains every tab's PTY output into its engine and
//! redraws via [`RenderEngine::draw_and_present`]. AppKit owns `NSApplication.run`
//! (the appkit-input verdict), so the draw must run on the main thread — hence a
//! run-loop timer rather than the renderer's background-thread `TimerPacer`.
//! CVDisplayLink is a later swap-in behind this same tick shape (deferred; noted
//! in `docs/analysis/renderer-r5.md`).

#![cfg(target_os = "macos")]

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem, NSWindow, NSWindowDelegate,
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
use crate::pty::PtySession;
use crate::selection::{ScreenRange, SelectionColors, tint_selection};
use crate::tabs::{self, TabId, TabRegistry};
use crate::view::TerminalView;
use ghostty_renderer::engine::{Engine as RenderEngine, FrameOptions};
use ghostty_renderer::snapshot::FullSnapshot;

/// The initial window content size in points.
const INITIAL_WIDTH: f64 = 800.0;
const INITIAL_HEIGHT: f64 = 480.0;

/// One terminal tab: engine + PTY + renderer + window/view.
struct Tab {
    /// The vt engine (parser + terminal state).
    engine: Engine,
    /// The PTY + child shell.
    pty: PtySession,
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
}

impl Tab {
    /// Rebuild the render target + grid for the current view size and scale,
    /// resizing the engine + PTY to match. Called on creation and resize.
    fn reflow(&mut self) {
        let (cols, rows) = self.current_grid_size();
        if cols != self.cols || rows != self.rows {
            self.cols = cols;
            self.rows = rows;
            self.engine.resize(cols, rows);
            let _ = self.pty.resize(cols as u16, rows as u16);
        }
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

    /// Pump available PTY output into the engine and drain engine replies back
    /// to the PTY. Returns whether the child shell has exited.
    fn pump(&mut self) -> bool {
        while let Some(chunk) = self.pty.try_read() {
            self.engine.write(&chunk);
        }
        let out = self.engine.take_output();
        if !out.is_empty() {
            let _ = self.pty.write_all(&out);
        }
        self.pty.child_exited()
    }

    /// Render one frame into the view's layer.
    fn render(&mut self) {
        if self.render.is_none() {
            return;
        }
        let mut window = self.engine.snapshot_window(0);
        if let Some(range) = self.selection_screen_range() {
            tint_selection(&mut window, range, self.selection_colors);
        }
        let snapshot = FullSnapshot::from_window(window);
        let render = self.render.as_mut().expect("checked above");
        render.update_frame(&snapshot, &mut self.font.grid, FrameOptions::default());
        if render.sync_atlas(&self.font.grid).is_err() {
            return;
        }
        let _ = render.draw_and_present(self.view.host_layer());
    }

    /// The engine's current selection, resolved to absolute screen
    /// coordinates the render path can test cells against, or `None` if there
    /// is no selection. Pin resolution (`point_from_pin`) walks the pagelist,
    /// so this is done once per frame here rather than per cell.
    fn selection_screen_range(&self) -> Option<ScreenRange> {
        let (start, end, rectangle) = self.engine.selection()?;
        self.engine.screen_range(start, end, rectangle)
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
                .and_then(|t| t.engine.pwd())
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

    /// The plain-text screen dump of the active tab's engine (smoke/test only).
    /// `None` if there is no active tab. Used by the synthetic-input smoke
    /// (`GHOSTTY_APP_SMOKE_TYPE`) to assert a typed command round-tripped
    /// through keyDown → encode → PTY → engine.
    pub fn active_screen_text(&self) -> Option<String> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).map(|t| t.engine.screen_dump())
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

    /// Encode a raw key event and write it to `tab`'s PTY. Reads the tab's live
    /// terminal encode options + the user's option-as-alt config.
    pub fn encode_key_to_tab(&self, tab: TabId, raw: &RawKeyEvent) {
        let mut state = self.0.borrow_mut();
        let cfg = state.input_config;
        if let Some(t) = state.tabs.get_mut(&tab) {
            let opts = t.engine.key_encode_options();
            let bytes = crate::input::translate::encode_raw(raw, &cfg, opts);
            if !bytes.is_empty() {
                let _ = t.pty.write_all(&bytes);
            }
        }
    }

    /// Send already-composed text (IME commit) to `tab`'s PTY.
    pub fn send_text_to_tab(&self, tab: TabId, text: &str) {
        let mut state = self.0.borrow_mut();
        if let Some(t) = state.tabs.get_mut(&tab) {
            let _ = t.pty.write_all(text.as_bytes());
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
                t.engine.mouse_event() != ghostty_input::mouse_encode::MouseEvent::None;
            let selection_allowed = !reporting_active || mods.shift;
            if selection_allowed {
                match action {
                    ghostty_input::mouse::Action::Press => {
                        t.engine.clear_selection();
                        t.selection_anchor = None;
                        if let Some(cell) = t.cell_at(x, y) {
                            t.selection_anchor = Some(cell);
                        }
                    }
                    ghostty_input::mouse::Action::Motion => {
                        if t.mouse_button_down
                            && let Some(anchor) = t.selection_anchor
                            && let Some(cell) = t.cell_at(x, y)
                            && let (Some(start), Some(end)) = (
                                t.engine.pin_at(anchor.0, anchor.1),
                                t.engine.pin_at(cell.0, cell.1),
                            )
                        {
                            t.engine.select(start, end, false);
                        }
                    }
                    ghostty_input::mouse::Action::Release => {
                        if copy_on_select
                            && t.selection_anchor.is_some()
                            && let Some(text) = t.engine.selection_string()
                        {
                            crate::clipboard::write(&text);
                        }
                        t.selection_anchor = None;
                    }
                }
            }
        }

        let ctx = crate::input::mouse::MouseContext {
            event_mode: t.engine.mouse_event(),
            format: t.engine.mouse_format(),
            screen_width: (t.cols * t.font.cell_width as usize) as f64,
            screen_height: (t.rows * t.font.cell_height as usize) as f64,
            cell_width: t.font.cell_width as f64,
            cell_height: t.font.cell_height as f64,
            any_button_pressed: t.mouse_button_down,
        };
        let bytes =
            crate::input::mouse::encode(action, button, mods, x, y, &ctx, &mut t.last_mouse_cell);
        if !bytes.is_empty() {
            let _ = t.pty.write_all(&bytes);
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
            && let Some(text) = t.engine.selection_string()
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
        let mut state = self.0.borrow_mut();
        if let Some(t) = state.tabs.get_mut(&tab) {
            let payload = if t.engine.bracketed_paste() {
                let mut p = Vec::with_capacity(text.len() + 12);
                p.extend_from_slice(b"\x1b[200~");
                p.extend_from_slice(text.as_bytes());
                p.extend_from_slice(b"\x1b[201~");
                p
            } else {
                text.into_bytes()
            };
            let _ = t.pty.write_all(&payload);
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

        let engine = Engine::with_colors(cols, rows, startup_colors);
        let pty = PtySession::spawn_in_dir(cols as u16, rows as u16, cwd.as_deref()).ok()?;
        let render = RenderEngine::new(cw, ch).ok();

        // Register first so the view can carry the id.
        let id = self.0.borrow_mut().registry.add();

        let controller_ptr: *const Controller = self;
        let view = TerminalView::new(mtm, id, controller_ptr);
        let window = make_window(mtm, &view);

        // Per-window delegate: sync the controller's active tab to the OS key
        // window on tab switch. Owned by the Tab (AppKit weak-holds delegates).
        let window_delegate = WindowDelegate::new(mtm, self.clone(), id);
        window.setDelegate(Some(ProtocolObject::from_ref(&*window_delegate)));

        let mut tab = Tab {
            engine,
            pty,
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
        // Native tabbing: prefer tabs so Cmd-T / drag-out behave like Terminal.
        window.setTabbingMode(NSWindowTabbingMode::Preferred);
        window.setContentView(Some(view));
        window.setReleasedWhenClosed(false);
    }
    window
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

            // Synthetic-input smoke: type a scripted command through the real
            // keyDown path, then assert its round-trip and exit. Takes
            // precedence over the plain auto-exit (it exits itself).
            let has_type = !self.ivars().smoke_type.borrow().is_empty();
            if has_type {
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
    }
);

impl AppDelegate {
    /// Create the delegate.
    pub fn new(
        mtm: MainThreadMarker,
        controller: Controller,
        smoke_ms: u64,
        smoke_type: String,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            controller,
            smoke_ms,
            smoke_type: RefCell::new(smoke_type),
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
        if occurrences >= 1 {
            println!(
                "OK: synthetic-input smoke — marker '{marker}' found {occurrences}x in screen"
            );
            std::process::exit(0);
        } else {
            eprintln!(
                "FAIL: synthetic-input smoke — marker '{marker}' not found; \
                 typing produced no output.\n----- screen -----\n{screen}\n------------------"
            );
            std::process::exit(1);
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
/// (type the string through the real keyDown path, assert its round-trip, exit).
/// Returns after the run loop exits.
pub fn run(config: &crate::config::Config, smoke_ms: u64, smoke_type: String) {
    let mtm = MainThreadMarker::new().expect("run() must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);

    let controller = Controller::new(config, mtm);
    let delegate = AppDelegate::new(mtm, controller, smoke_ms, smoke_type);
    let object = ProtocolObject::from_ref(&*delegate);
    app.setDelegate(Some(object));

    app.run();
}
