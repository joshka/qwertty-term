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
//!   [`RenderEngine`](qwertty_term_renderer::engine::Engine), a
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

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAnimatablePropertyContainer, NSAnimationContext, NSApplication,
    NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType, NSColor,
    NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem, NSScreen,
    NSUserInterfaceItemIdentification, NSView, NSWindow, NSWindowDelegate, NSWindowLevel,
    NSWindowRestoration, NSWindowStyleMask, NSWindowTabbingMode,
};
use objc2_foundation::{
    MainThreadMarker, NSCoder, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect,
    NSRunLoop, NSRunLoopCommonModes, NSSize, NSString,
};
use objc2_quartz_core::CADisplayLink;
use objc2_quartz_core::{CALayer, CATransaction};

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
use qwertty_term_renderer::engine::{Engine as RenderEngine, FrameOptions};
use qwertty_term_renderer::snapshot::FullSnapshot;

/// The initial window content size in points.
/// Whether to run without stealing the user's keyboard focus.
///
/// A normal terminal-launched run must claim focus (see the `activate` call in
/// `applicationDidFinishLaunching`) or hardware keystrokes never reach
/// `keyDown:`. Smokes don't need that: they drive the app with *synthetic*
/// events, so activating only yanks focus away from whatever the developer is
/// doing — which makes the GUI smokes hostile to run locally.
///
/// So: any `QWERTTY_TERM_SMOKE_*` run defaults to background (accessory
/// activation policy, no `activate`). `QWERTTY_TERM_BACKGROUND=0` forces the
/// normal foreground behaviour, and setting it to anything else forces
/// background for a non-smoke run.
fn background_mode() -> bool {
    match std::env::var("QWERTTY_TERM_BACKGROUND") {
        Ok(v) => v != "0",
        Err(_) => {
            std::env::vars_os().any(|(k, _)| k.to_string_lossy().starts_with("QWERTTY_TERM_SMOKE"))
        }
    }
}

/// Opt-in tmux control-mode diagnostics: set `QWERTTY_TERM_TMUX_TRACE=1` to log
/// the tab/session lifecycle (notifications received, reconcile ops, tab
/// open/close, teardown decisions). Off by default and costs one relaxed atomic
/// load when off, so it can live in release builds — tmux lifecycle bugs are
/// timing-dependent and only reproduce through real GUI interaction, where
/// attaching a debugger or a one-off build is impractical.
fn tmux_trace_enabled() -> bool {
    use std::sync::atomic::{AtomicU8, Ordering};
    static ON: AtomicU8 = AtomicU8::new(0);
    match ON.load(Ordering::Relaxed) {
        0 => {
            let on = std::env::var_os("QWERTTY_TERM_TMUX_TRACE").is_some();
            ON.store(if on { 2 } else { 1 }, Ordering::Relaxed);
            on
        }
        2 => true,
        _ => false,
    }
}

macro_rules! tmux_trace {
    ($($arg:tt)*) => {
        if crate::app::tmux_trace_enabled() {
            eprintln!("qwertty-term[tmux-trace] {}", format!($($arg)*));
        }
    };
}

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
    ///
    /// `None` for a **display-only** tmux pane surface (ADR 006 slice 5b-native):
    /// such a surface has no pty and no shell — its bytes arrive via `%output`
    /// through the control surface's [`TmuxSession`] Viewer, which owns the pane
    /// `Terminal` it renders (see [`Surface::display`]). Every pty write / resize
    /// / event drain is a no-op for it; input+resize routing to tmux panes is
    /// slice 5d.
    io: Option<TabIo>,
    /// The render engine (cell buffers + Metal draw), if a Metal device exists.
    /// One per surface (multiple already coexist across tabs today).
    render: Option<RenderEngine>,
    /// The font grid the renderer shapes through.
    font: FontGrid,
    /// The current font size (drives font grid rebuilds).
    font_size: FontSize,
    /// The user's `adjust-*` metric nudges, applied on every font-grid (re)build
    /// so a font-size change keeps the configured cell/underline/cursor overrides.
    metric_modifiers: qwertty_term_font::metrics::ModifierSet,
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
    /// The viewport cell the mouse is currently over (`None` when outside the
    /// grid). Drives the OSC8 hyperlink hover underline (R7): passed to the
    /// renderer via `FrameOptions.hovered_cell`, which underlines every cell of
    /// a hovered link.
    hovered_cell: Option<(usize, usize)>,
    /// This pane's selection-gesture state machine (click counting, word/line
    /// behaviors, drag threshold, autoscroll) — port of upstream
    /// `SelectionGesture.zig`. See [`crate::gesture`].
    gesture: crate::gesture::SelectionGesture,
    /// Word-boundary codepoints for double/triple-click word selection
    /// (`selection-word-chars` config, or the built-in default set), shared from
    /// the controller.
    word_boundaries: std::sync::Arc<[u32]>,
    /// Selection highlight colors resolved from the tab's theme at startup.
    selection_colors: SelectionColors,
    /// The terminal's default background as `(r, g, b)` — the presented-frame
    /// coverage baseline.
    default_bg: (u8, u8, u8),
    /// Debug frame-dump (env `QWERTTY_TERM_DUMP_FRAME`), if enabled.
    frame_dump: Option<crate::frame_dump::FrameDump>,
    /// Max per-pixel L1 delta from `default_bg` in the most recently *presented*
    /// frame.
    last_present_delta: i32,
    /// Mean Rec.601 luma of the most recently *presented* frame (`[0, 255]`).
    /// The dimming smoke asserts an unfocused pane's mean luma drops below the
    /// focused baseline. Only updated when `capture_present` is on.
    last_present_luma: f64,
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
    /// Mouse-selection anchor for a **display-only tmux pane** (ADR 006 slice
    /// 5d): the `(col, row, scrollback_offset)` of the left-button press, in
    /// this surface's visible-window space. A display pane's selection lives on
    /// the Viewer's pane `Terminal` (this surface's own engine is empty), so the
    /// anchor is tracked here and mapped through the pane terminal on each drag.
    /// `None` when not mid-drag. Unused by ordinary pty panes.
    pane_sel_anchor: Option<(usize, usize, usize)>,
    /// Poison-resilience (app-hardening): set once this surface's engine mutex
    /// is observed poisoned — i.e. some thread (the io-reader parse thread is
    /// the field-observed culprit) panicked while holding the engine lock. A
    /// dead surface has had its io shut down and shows a "terminal crashed"
    /// banner; it is NOT closed, and it never panics the app. `Cell` so the
    /// `&self` [`Surface::engine`] accessor can flip it on first poison
    /// observation. See [`Surface::engine`] and [`Surface::mark_dead`].
    dead: Cell<bool>,
    /// The short reason shown in the crash banner (e.g. the panic that poisoned
    /// the lock). `RefCell` for the same `&self`-accessor reason as `dead`.
    dead_reason: RefCell<Option<String>>,
    /// Whether the one-shot crash banner has already been painted into the
    /// (recovered) engine. Prevents re-writing it every tick.
    banner_drawn: Cell<bool>,
    /// This pane's scrollback-search state (needle, matches, current index).
    /// Drives the match highlights in [`Surface::render`] and the counter in
    /// the overlay. Empty/inactive when search is closed.
    search: crate::search::SearchState,
    /// Set whenever the search-match highlight state changes (open/close, needle
    /// re-run, navigation) so the next [`Surface::render`] forces a full cell
    /// rebuild. The highlight is a host-side tint the engine's dirty tracking
    /// doesn't know about, so without this a needle change whose matches are
    /// already on screen (no viewport move, no dirtied row) would not repaint —
    /// the tint would never reach the GPU. Cleared once the forcing frame draws.
    search_highlight_dirty: Cell<bool>,
    /// The AppKit search-bar overlay for this pane, created lazily on first
    /// Cmd+F and reused thereafter (shown/hidden). `None` until first opened.
    search_overlay: Option<Retained<crate::search_overlay::SearchOverlay>>,
    /// Match-highlight tint colors (amber matches, salmon current), from the
    /// selection-tint family. Static defaults for slice 1.
    match_colors: crate::selection::MatchColors,
    /// Unfocused-split dimming colors for this pane: the fill (configured, or
    /// this surface's own background) + overlay alpha. Applied in
    /// [`Surface::render`] when the pane is an unfocused member of a multi-pane
    /// tab. See [`crate::selection::dim_window`].
    dim_colors: crate::selection::DimColors,
    /// When the current command's output started (`OSC 133 ; C`), for
    /// `notify-on-command-finish` timing. `Some` between a `C` mark and the
    /// matching `D`; `None` otherwise. Set/cleared in [`Surface::pump`].
    command_started_at: Option<std::time::Instant>,
    /// Current OSC 9;4 progress-bar display state, or `None` when no bar is
    /// shown. Derived from the drained report + gated by `progress-style`.
    progress: Option<crate::progress::ProgressDisplay>,
    /// When the current progress bar auto-clears if no further updates arrive
    /// (upstream's 15s timer). `None` when no bar is shown.
    progress_deadline: Option<std::time::Instant>,
    /// The lazily-created `CALayer` that draws the progress bar as a bottom
    /// strip over this pane's terminal content. `None` until the first bar.
    progress_layer: Option<Retained<CALayer>>,
    /// The live tmux control-mode session, present only while this surface is
    /// running `tmux -CC` (constructed on the `Enter` the DCS `1000p` seam
    /// emits, dropped on `Exit`). [`Surface::pump`] drains
    /// `take_tmux_notifications` into it, writes its outgoing command bytes back
    /// to this surface's own pty (control mode is in-band), and surfaces its
    /// [`ReconcilePlan`](crate::tmux_reconcile::ReconcilePlan) for the native
    /// tab/split layer (ADR 006 slice 5c). `None` for an ordinary shell pane.
    tmux: Option<crate::tmux_session::TmuxSession>,
    /// The app's configured palette/theme (the same `Colors` this surface's own
    /// engine was built with). Threaded into [`TmuxSession::new`] so tmux panes
    /// render on the user's theme, not the engine default (ADR 006 theme fix).
    startup_colors: qwertty_term_vt::terminal::Colors,
    /// Set on a **display-only** tmux pane surface (ADR 006 slice 5b-native):
    /// this surface renders a `Terminal` it does *not* own — the one the control
    /// surface's [`TmuxSession`] Viewer owns for this tmux pane. The render pass
    /// snapshots that foreign terminal by reference each frame
    /// ([`Controller::render_tmux_panes`]); this surface's own [`Surface::engine`]
    /// stays empty and unused. `None` for an ordinary shell pane (which renders
    /// its own engine). See [`DisplaySource`].
    display: Option<DisplaySource>,
}

/// Where a display-only tmux pane surface sources its `Terminal` from (ADR 006
/// slice 5b-native, Option (a): the Viewer stays the single owner/feeder of pane
/// bytes; the pane surface renders it by reference).
///
/// The surface's own [`SurfaceId`] (its key in the tab's surface map, minted by
/// the [`Reconciler`](crate::tmux_reconcile::Reconciler)) is what
/// [`TmuxSession::pane_terminal`](crate::tmux_session::TmuxSession::pane_terminal)
/// resolves back to the pane `Terminal`, so this only needs to locate the
/// control surface that owns the session.
#[derive(Debug, Clone, Copy)]
struct DisplaySource {
    /// The tab holding the control surface running `tmux -CC`.
    control_tab: TabId,
    /// The control surface (within `control_tab`) whose `TmuxSession` owns this
    /// pane's `Terminal`.
    control_surface: SurfaceId,
}

/// Lock an engine mutex, recovering a poisoned lock instead of panicking.
///
/// Returns the guard and whether the lock was poisoned. Poison means a thread
/// panicked while holding this lock (the io-reader/parse thread is the
/// field-observed culprit — see `crate::termio`). `PoisonError::into_inner`
/// hands back the guard regardless: the engine's memory is always valid to
/// read (Rust guarantees no cross-poison UB), so a poisoned engine is safe for
/// a final render and a one-shot banner write. This is the whole poison-
/// resilience mechanism; we do not switch mutex crates. Callers use the
/// `poisoned` flag to mark the owning surface dead.
fn lock_or_recover(engine: &Mutex<Engine>) -> (std::sync::MutexGuard<'_, Engine>, bool) {
    match engine.lock() {
        Ok(guard) => (guard, false),
        Err(poison) => (poison.into_inner(), true),
    }
}

/// What one [`Surface::pump`] observed this tick: whether the shell exited,
/// the latest password-input mode change (if any), and whether a BEL rang.
#[derive(Default)]
struct PumpResult {
    exited: bool,
    password: Option<bool>,
    bell: bool,
    /// The most recent OSC 9 / OSC 777 desktop notification `(title, body)`
    /// observed this tick, if any (latest-wins per frame).
    notification: Option<(String, String)>,
    /// A command that finished this tick (OSC 133 `C`→`D`) as
    /// `(exit_code, elapsed)`, for `notify-on-command-finish`.
    command_finished: Option<(Option<i32>, std::time::Duration)>,
    /// The latest OSC 9;4 progress report observed this tick, if any.
    progress_report: Option<qwertty_term_vt::osc::ProgressReport>,
    /// A tmux control-mode native-surface reconcile plan produced this tick, if
    /// the window/pane tree changed (ADR 006 slice 5c). The controller applies
    /// it to native tabs/splits after the per-surface borrow drops.
    tmux_plan: Option<crate::tmux_reconcile::ReconcilePlan>,
    /// tmux left control mode this tick (`Exit`): the controller tears down the
    /// native tabs bound to this surface's session.
    tmux_exit: bool,
    /// A tmux-initiated active-pane change this tick: the surface the app should
    /// move keyboard focus to (ADR 006 slice 5e — tmux→app focus sync).
    tmux_focus: Option<SurfaceId>,
}

/// One tmux control-mode reconcile event, collected during the per-surface
/// pump loop and applied to native tabs after that borrow drops (ADR 006 slice
/// 5b-native). `plan` is the native tab/split intent; `exit` tears the session's
/// tabs down.
struct TmuxReconcileEvent {
    tab: TabId,
    surface: SurfaceId,
    plan: Option<crate::tmux_reconcile::ReconcilePlan>,
    exit: bool,
    /// A tmux-initiated active-pane change: the surface to move keyboard focus to
    /// after the plan is applied (ADR 006 slice 5e — tmux→app focus sync).
    focus: Option<SurfaceId>,
}

/// A native window action on a tmux-managed tab, redirected to its tmux
/// control-command equivalent (ADR 006 slice 5e). The Viewer owns the tab's
/// layout, so the native handler enqueues one of these through the control
/// session instead of mutating the native `SplitTree`; the effect returns as a
/// `%layout-change` / `%window-*` reconcile.
enum TmuxNativeAction {
    /// A split (`split-window`): `horizontal` picks a left/right (`-h`) vs
    /// top/bottom (`-v`) split, `before` (`-b`) places the new pane first.
    Split { horizontal: bool, before: bool },
    /// A new tab while focus is in a tmux window → `new-window` (iTerm2-style).
    NewWindow,
    /// A pane close (`kill-pane`).
    KillPane,
    /// A focus change onto a tmux pane → `select-pane` (make it tmux-active so
    /// bare `split-window` / the active-pane indicator target it).
    SelectPane,
}

impl Surface {
    /// Lock the shared engine, degrading a *poisoned* lock to a dead surface
    /// instead of panicking the whole app.
    ///
    /// Poison resilience (app-hardening, field-observed cascade): the engine is
    /// shared `Arc<Mutex<Engine>>` with this surface's io-reader/parse thread
    /// (see the parse sink in `crate::termio`). If that thread panics *while
    /// holding the lock* — a real field crash did exactly this — the mutex is
    /// poisoned. The previous `.expect("engine mutex poisoned")` here turned the
    /// main thread's very next lock into a panic that took down the entire app
    /// with *every* tab. Now we instead recover the guard via
    /// [`PoisonError::into_inner`]: the engine's bytes are still valid to READ
    /// (Rust guarantees no memory-unsafety across a poison; at worst the
    /// terminal grid is one half-applied batch stale — perfectly fine for a
    /// final render), so we hand back a live guard and mark THIS surface dead.
    /// The pace tick ([`Controller::tick`]) then shuts this pane's io down and
    /// shows a "terminal crashed" banner; other panes/tabs are untouched.
    ///
    /// We do not switch mutex crates (the task forbids it) — `into_inner` on the
    /// std `PoisonError` is the whole mechanism.
    fn engine(&self) -> std::sync::MutexGuard<'_, Engine> {
        match lock_or_recover(&self.engine) {
            (guard, false) => guard,
            (guard, true) => {
                // First observation of poison flips this surface to dead so the
                // next tick tears its io down + banners it. The engine data is
                // still readable, so return the recovered guard.
                if !self.dead.get() {
                    self.dead.set(true);
                    *self.dead_reason.borrow_mut() =
                        Some("engine thread panicked (lock poisoned)".to_string());
                }
                guard
            }
        }
    }

    /// Write bytes to this surface's pty, if it has one. A display-only tmux
    /// pane surface ([`Surface::display`] set) has no pty — its bytes arrive via
    /// `%output` through the Viewer — so the write is silently dropped. Routing
    /// real input to a tmux pane (`send-keys`) is slice 5d.
    fn send_pty(&self, bytes: &[u8]) {
        if let Some(io) = &self.io {
            io.write(bytes);
        }
    }

    /// This surface's display-only source, if it is a tmux pane surface.
    fn display_source(&self) -> Option<DisplaySource> {
        self.display
    }

    /// Whether this surface has been marked dead by a poison observation.
    fn is_dead(&self) -> bool {
        self.dead.get()
    }

    /// The short crash reason, if dead.
    fn dead_reason(&self) -> Option<String> {
        self.dead_reason.borrow().clone()
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
            // A display-only tmux pane surface has no pty to resize; its grid is
            // driven by tmux's layout, and native→tmux resize is slice 5d.
            if let Some(io) = &self.io {
                io.resize(
                    cols as u16,
                    rows as u16,
                    self.font.cell_width,
                    self.font.cell_height,
                );
            }
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

    /// Rebuild the font grid at this pane's current font size × backing scale,
    /// re-applying the configured `adjust-*` metric nudges.
    fn rebuild_font(&mut self, family: Option<&str>) {
        let px = (self.font_size.get() as f64) * self.scale;
        if let Ok(fg) = font::build(family, px, &self.metric_modifiers) {
            self.font = fg;
            // Tell the render engine the font grid was rebuilt: adopt the new
            // cell metrics (projection/target/placement) and invalidate its
            // per-slot atlas-upload trackers, so the fresh atlas re-uploads into
            // every swap-chain slot. Without this the zoom renders garbled — new
            // glyph instances sampling the stale old-size atlas.
            if let Some(render) = self.render.as_mut() {
                render.on_font_rebuilt(self.font.cell_width, self.font.cell_height);
            }
            self.reflow();
        }
    }

    /// Per-tick IO servicing for this pane. Returns whether its child shell
    /// exited (so the caller closes this surface). `title_sink` receives the
    /// title/password state so the tab can reflect the *focused* pane's title in
    /// the window title.
    fn pump(&mut self) -> PumpResult {
        // A display-only tmux pane surface has no pty and no engine of its own to
        // service — its bytes flow through the control surface's `TmuxSession`
        // (pumped when that control surface is pumped) and it renders that
        // session's pane `Terminal` by reference in `render_tmux_panes`. Nothing
        // to drain here.
        if self.display.is_some() {
            return PumpResult::default();
        }
        // A poison-dead surface has (or is about to have) its io shut down; skip
        // all io servicing for it. It is neither "exited" (which would close it)
        // nor generating password events — it just shows its crash banner.
        if self.is_dead() {
            return PumpResult::default();
        }
        // `take_output`/`take_bell` lock the engine; if the parse thread poisoned
        // the lock, `engine()` recovers it and flips `dead`, so re-check before
        // touching io. Drain both under one lock.
        let (out, bell, notification, boundaries, progress_report, tmux_notifications) = {
            let mut engine = self.engine();
            (
                engine.take_output(),
                engine.take_bell(),
                engine.take_notification(),
                engine.take_command_boundaries(),
                engine.take_progress_report(),
                engine.take_tmux_notifications(),
            )
        };
        if self.is_dead() {
            return PumpResult::default();
        }
        if !out.is_empty() {
            self.send_pty(&out);
        }
        // tmux control-mode lifecycle (ADR 006 slice 5c). Feed the drained
        // notifications to this surface's live `TmuxSession` — created on the
        // `Enter` the DCS `1000p` seam emits — write its outgoing command bytes
        // back to this same pty (control mode is in-band), and surface its
        // reconcile plan / exit for the native tab layer.
        let (tmux_plan, tmux_exit, tmux_focus) = self.pump_tmux(tmux_notifications);
        // Pair OSC 133 `C` (output start) with the following `D` (command end)
        // to time each command for `notify-on-command-finish`. Multiple
        // commands finishing in one ~16ms tick is vanishingly rare; keep the
        // most recent finish for this tick.
        let mut command_finished: Option<(Option<i32>, std::time::Duration)> = None;
        for boundary in boundaries {
            match boundary {
                qwertty_term_vt::stream::CommandBoundary::OutputStart => {
                    self.command_started_at = Some(std::time::Instant::now());
                }
                qwertty_term_vt::stream::CommandBoundary::End { exit_code } => {
                    if let Some(start) = self.command_started_at.take() {
                        command_finished = Some((exit_code, start.elapsed()));
                    }
                }
            }
        }
        let mut exited = false;
        let mut password: Option<bool> = None;
        let io_events = self
            .io
            .as_ref()
            .map(|io| io.drain_events())
            .unwrap_or_default();
        for event in io_events {
            match event {
                IoEvent::ChildExited { exit_code, .. } => {
                    if exit_code != 0 {
                        eprintln!("qwertty-term: shell exited with code {exit_code}");
                    }
                    exited = true;
                }
                IoEvent::PasswordInput(active) => {
                    password = Some(active);
                }
            }
        }
        PumpResult {
            exited,
            password,
            bell,
            notification,
            command_finished,
            progress_report,
            tmux_plan,
            tmux_exit,
            tmux_focus,
        }
    }

    /// Drive the tmux control-mode lifecycle for this surface from the drained
    /// notification batch (ADR 006 slice 5c). Constructs the [`TmuxSession`] on
    /// the first `Enter`, feeds the batch, writes the session's outgoing
    /// command bytes back to this surface's own pty (control mode is in-band on
    /// the same pty), tears the session down on `Exit`, and returns the native
    /// reconcile plan + exit flag for the controller to apply after the
    /// per-surface borrow drops. Returns `(None, false)` for an ordinary
    /// (non-tmux) pane, which drains an empty notification vec.
    fn pump_tmux(
        &mut self,
        notifications: Vec<qwertty_term_vt::tmux::Notification>,
    ) -> (
        Option<crate::tmux_reconcile::ReconcilePlan>,
        bool,
        Option<SurfaceId>,
    ) {
        use qwertty_term_vt::tmux::Notification;

        if notifications.is_empty() {
            return (None, false, None);
        }
        // Lazily start a session on the first `Enter` (the only notification
        // emitted before a session exists). A stray notification with no session
        // and no `Enter` is ignored — nothing to drive.
        if self.tmux.is_none()
            && notifications
                .iter()
                .any(|n| matches!(n, Notification::Enter))
        {
            let mut session = crate::tmux_session::TmuxSession::new(self.startup_colors.clone());
            // Declare this control client's grid before the session starts up, so
            // tmux lays windows/panes out at the size the UI actually draws at.
            // A control client's size comes from `refresh-client -C`, not the pty
            // winsize — without this tmux uses the *session's* size and every pane
            // terminal ends up sized to a grid we never draw at (garbled panes).
            // The Viewer folds it into its startup sequence, ahead of the first
            // `list-windows`, so the very first layout is already correct.
            let _ = session.set_client_size(self.cols, self.rows);
            self.tmux = Some(session);
            // Control mode is live on this surface: stop painting its grid. tmux's
            // `tmux%` prompt and any stray non-DCS `%…` bytes would otherwise paint
            // this (soon-hidden) control surface's real grid and flash into view
            // whenever it is momentarily shown (ADR 006 gap 5 — exit-flash).
            // Cleared on `%exit` below so the underlying shell repaints.
            //
            // Do NOT write an erase sequence here: control-mode output is one long
            // DCS passthrough (`\eP1000p …`) that stays open for the whole session,
            // and injecting an `ESC`-based CSI from this (main) thread would
            // terminate that live DCS mid-stream — the parser unhooks, emits a
            // spurious tmux `Exit`, and every following `%output` prints as raw
            // text. Suppression alone (below) prevents the flash without touching
            // the byte stream.
            self.engine().set_tmux_suppress_print(true);
        }
        let Some(update) = self.tmux.as_mut().map(|s| s.ingest(notifications)) else {
            return (None, false, None);
        };
        // Write each command block back to the control pty (already newline-
        // terminated). This is the in-band request half of control mode.
        for command in &update.commands {
            self.send_pty(command);
        }
        let focus = update.focus;
        // On exit (or a defunct session) drop it so a later re-entry starts
        // fresh; the controller tears the native tabs down from `tmux_exit`.
        if update.exit || self.tmux.as_ref().is_some_and(|s| s.is_defunct()) {
            self.tmux = None;
            // Control mode ended: resume normal grid painting so the underlying
            // shell (which regains this pty as `tmux -CC` exits) repaints.
            self.engine().set_tmux_suppress_print(false);
        }
        (update.plan, update.exit, focus)
    }

    /// Read access to this surface's live tmux control-mode session, if it is
    /// running `tmux -CC`. The native tab layer reads pane terminals out of it
    /// for rendering (ADR 006 slice 5b-native).
    fn tmux_session(&self) -> Option<&crate::tmux_session::TmuxSession> {
        self.tmux.as_ref()
    }

    /// Bring a poison-dead surface to rest exactly once: shut its io down (the
    /// parse thread is already gone, but this joins the writer/exit threads and
    /// releases the pty) and paint a one-shot "terminal crashed" banner into the
    /// (recovered) engine so the pane visibly reports the crash instead of
    /// freezing on a stale frame. Idempotent via `banner_drawn`.
    ///
    /// Writing the banner through the recovered engine is safe: `Engine::write`
    /// is pure Rust over memory the `Terminal` owns, so a poisoned-but-recovered
    /// engine cannot be driven into memory-unsafety; the worst case is a cosmetic
    /// glitch on a pane we are already tearing down. This is the "reuse/extend
    /// the child-exited banner path" the task asks for — a child exit closes the
    /// pane, but a *crash* keeps it open with this banner so the user sees why.
    fn settle_dead(&mut self) {
        if self.banner_drawn.get() {
            return;
        }
        self.banner_drawn.set(true);
        if let Some(io) = &mut self.io {
            io.shutdown();
        }

        let reason = self.dead_reason().unwrap_or_else(|| "unknown".to_string());
        // A simple, self-contained banner: reset the screen, move to home, and
        // print a bold red line. Bytes only — no engine methods beyond `write`.
        let banner = format!(
            "\x1b[2J\x1b[H\x1b[1;31m[terminal crashed — {reason}]\x1b[0m\r\n\
             \x1b[0mThis pane's engine thread panicked; its shell has been \
             stopped.\r\nClose this pane to dismiss.\r\n",
        );
        // Lock, recovering the guard whether or not it is poisoned (it always is
        // by the time we get here, but be robust): either way we get a live guard
        // over the engine's owned memory and write the banner.
        let (mut guard, _poisoned) = lock_or_recover(&self.engine);
        guard.write(banner.as_bytes());
    }

    /// Render one frame into this pane's layer. `focused` selects the hollow
    /// (unfocused) vs. solid cursor via `FrameOptions.focused` — the renderer
    /// draws a hollow box for an unfocused pane at no extra cost. `is_split` is
    /// whether this pane's tab has more than one pane; when it does and this pane
    /// is unfocused, the snapshot is dimmed toward the fill color (upstream
    /// `unfocused-split-opacity`, replicated CPU-side like the selection tint).
    /// Apply an OSC 9;4 progress report: derive the display state (or clear on
    /// `Remove`) and (re)arm the 15s auto-clear timer.
    fn set_progress(
        &mut self,
        report: qwertty_term_vt::osc::ProgressReport,
        now: std::time::Instant,
    ) {
        self.progress = crate::progress::ProgressDisplay::from_report(report);
        self.progress_deadline = self.progress.map(|_| now + crate::progress::AUTO_CLEAR);
    }

    /// Clear the progress bar if its auto-clear deadline has passed (upstream's
    /// 15s no-update timeout).
    fn tick_progress_autoclear(&mut self, now: std::time::Instant) {
        if let Some(deadline) = self.progress_deadline
            && now >= deadline
        {
            self.progress = None;
            self.progress_deadline = None;
        }
    }

    /// Sync the progress-bar `CALayer` to the current [`Self::progress`] state: a
    /// bottom strip filled to the progress fraction (full width when
    /// indeterminate), colored by category, drawn over this pane's terminal
    /// content. Hidden when there is no bar. Implicit layer animations are
    /// disabled so it tracks resize/updates instantly.
    fn sync_progress_layer(&mut self) {
        let host = self.view.host_layer().as_layer();
        CATransaction::begin();
        CATransaction::setDisableActions(true);
        match self.progress {
            None => {
                if let Some(layer) = &self.progress_layer {
                    layer.setHidden(true);
                }
            }
            Some(display) => {
                let bounds = host.bounds();
                let width = bounds.size.width;
                let bar_height = 3.0_f64;
                let frac = if display.indeterminate {
                    1.0
                } else {
                    display.fraction
                };
                let (r, g, b) = match display.category {
                    crate::progress::ProgressCategory::Normal => (0.20, 0.52, 1.0),
                    crate::progress::ProgressCategory::Error => (0.90, 0.20, 0.20),
                    crate::progress::ProgressCategory::Paused => (0.95, 0.60, 0.15),
                };
                let alpha = if display.indeterminate { 0.5 } else { 1.0 };
                let color = NSColor::colorWithSRGBRed_green_blue_alpha(r, g, b, alpha);
                let layer = self.progress_layer.get_or_insert_with(|| {
                    let l = CALayer::new();
                    host.addSublayer(&l);
                    l
                });
                layer.setHidden(false);
                // CALayer geometry is bottom-left origin, so y=0 is the pane's
                // bottom edge — where the progress bar sits.
                layer.setFrame(NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(width * frac, bar_height),
                ));
                layer.setBackgroundColor(Some(&color.CGColor()));
            }
        }
        CATransaction::commit();
    }

    fn render(&mut self, focused: bool, is_split: bool) {
        if self.render.is_none() {
            return;
        }
        let (window, range) = {
            let mut engine = self.engine();
            // Resolve the selection range first (immutable borrow), then take
            // the per-frame tracking capture (which mutably clears dirty state).
            let range = engine
                .selection()
                .and_then(|(start, end, rect)| engine.screen_range(start, end, rect));
            let window = engine.snapshot_window_tracking(self.scrollback_offset);
            (window, range)
        };
        self.render_window(window, range, focused, is_split);
    }

    /// Draw one already-captured [`SnapshotWindow`] through this surface's render
    /// engine + font grid, applying the selection / search / unfocused-dim CPU
    /// tint passes, then present it.
    ///
    /// Factored out of [`Surface::render`] so a **display-only tmux pane
    /// surface** (ADR 006 slice 5b-native) can render a `SnapshotWindow`
    /// captured *by reference* from the control surface's Viewer-owned pane
    /// `Terminal` — a terminal this surface does not own — instead of its own
    /// (empty) engine. The normal path passes the surface's own tracking
    /// snapshot + selection range; the tmux path passes the foreign pane's
    /// snapshot and `range: None` (selection/search on tmux panes is slice 5d).
    fn render_window(
        &mut self,
        mut window: qwertty_term_vt::snapshot::SnapshotWindow,
        range: Option<crate::selection::ScreenRange>,
        focused: bool,
        is_split: bool,
    ) {
        if self.render.is_none() {
            return;
        }
        if let Some(range) = range {
            tint_selection(&mut window, range, self.selection_colors);
        }
        // Search-match highlights: a second CPU-side tint pass over the same
        // snapshot window (matches are absolute-screen ranges, so they line up
        // with the window's `window_top`). The current match gets a distinct
        // color. Only when the search bar is open.
        if self.search.is_active() && self.search.count() > 0 {
            crate::selection::tint_matches(
                &mut window,
                self.search.matches(),
                self.search.current_index(),
                self.match_colors,
            );
        }
        // Unfocused-split dimming: the final CPU-side tint pass, applied only to
        // an unfocused pane of a multi-pane tab (upstream's `isSplit &&
        // !isFocusedSurface` gate). Blends every cell toward the fill color at
        // the overlay alpha, replicating upstream's translucent overlay. The
        // focused pane and single-pane tabs are never dimmed.
        if is_split && !focused {
            crate::selection::dim_window(&mut window, self.dim_colors);
        }
        let snapshot = FullSnapshot::from_window(window);
        let render = self.render.as_mut().expect("checked above");
        // A host-side search-highlight change (needle re-run / navigation /
        // open / close) tints existing cells without moving the viewport or
        // dirtying an engine row, so force a full rebuild this frame or the
        // partial path would skip the clean rows and never upload the tint.
        let force_full_rebuild = self.search_highlight_dirty.replace(false);
        let opts = FrameOptions {
            focused,
            hovered_cell: self.hovered_cell,
            force_full_rebuild,
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
                self.last_present_luma = crate::frame_dump::mean_luma(&pixels);
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

    // -- selection gestures ------------------------------------------------

    /// Like [`Surface::cell_at`] but with the position clamped into the grid
    /// first — drags routinely leave the pane (that's what edge-autoscroll is
    /// for), and upstream clamps too (`posToViewport`).
    fn cell_at_clamped(&self, x: f32, y: f32) -> (usize, usize) {
        let max_x = (self.cols * self.font.cell_width as usize).saturating_sub(1) as f32;
        let max_y = (self.rows * self.font.cell_height as usize).saturating_sub(1) as f32;
        let cx = x.clamp(0.0, max_x.max(0.0));
        let cy = y.clamp(0.0, max_y.max(0.0));
        self.cell_at(cx, cy).unwrap_or((0, 0))
    }

    /// The absolute screen point under device-pixel `(x, y)` at this pane's
    /// current scrollback offset, or `None` outside the grid / on an
    /// unwritten pad row.
    fn screen_point_at(&self, x: f32, y: f32) -> Option<(usize, usize)> {
        let cell = self.cell_at(x, y)?;
        self.engine()
            .window_to_screen_point(cell.0, cell.1, self.scrollback_offset)
    }

    /// The [`crate::gesture::Geometry`] for this pane's current grid.
    fn gesture_geometry(&self) -> crate::gesture::Geometry {
        crate::gesture::Geometry {
            columns: self.cols as u32,
            cell_width: self.font.cell_width,
            padding_left: 0,
            screen_height: (self.rows * self.font.cell_height as usize) as u32,
        }
    }

    /// Handle a left-button press for selection: shift-click extends an
    /// existing selection (upstream `Surface.zig:3785-3820`); otherwise the
    /// gesture counts the click and returns the standard single/double/triple
    /// selection, which is applied here (single click with an existing
    /// selection clears it — `Surface.zig:4014-4022`).
    fn selection_press(
        &mut self,
        mods: qwertty_term_input::key_mods::Mods,
        x: f32,
        y: f32,
        interval: std::time::Duration,
    ) {
        let now = std::time::Instant::now();
        let alt_screen = self.engine().alt_screen_active();

        // Shift-click continues the previous gesture instead of starting a
        // new click sequence — but only when a selection exists and we are
        // outside the click-repeat interval (a quick shift-click may be
        // increasing the click count instead).
        if mods.shift && self.gesture.click_count() > 0 {
            let has_selection = self.engine().selection().is_some();
            let within_interval = self
                .gesture
                .click_time()
                .is_some_and(|t| now.duration_since(t) <= interval);
            if has_selection && !within_interval {
                self.selection_drag(x, y, mods.alt);
                return;
            }
        }

        let Some(point) = self.screen_point_at(x, y) else {
            return;
        };
        // Upstream behaviors: [cell, word, ctrl-or-super ? output : line]
        // (`Surface.zig:3977-3981`).
        let behaviors = [
            crate::gesture::Behavior::Cell,
            crate::gesture::Behavior::Word,
            if mods.ctrl || mods.super_ {
                crate::gesture::Behavior::Output
            } else {
                crate::gesture::Behavior::Line
            },
        ];
        let press = crate::gesture::Press {
            time: now,
            point,
            xpos: x as f64,
            ypos: y as f64,
            // Repeat distance: one cell width (`Surface.zig:3974`).
            max_distance: self.font.cell_width as f64,
            repeat_interval: interval,
            alt_screen,
            behaviors,
            boundary_codepoints: &self.word_boundaries,
        };
        // The gesture needs the engine during the press (word/line lookup);
        // take it out so the engine guard's `&self` borrow and the gesture's
        // `&mut` don't overlap.
        let mut gesture = std::mem::take(&mut self.gesture);
        let sel = {
            let engine = self.engine();
            gesture.press(&engine, &press)
        };
        self.gesture = gesture;

        match sel {
            // Press selections (word/line/output) are never rectangular.
            Some((a, b)) => {
                self.engine().select_screen_points(a, b, false);
            }
            None => {
                // A fresh single click clears any existing selection.
                if self.gesture.click_count() == 1 && self.engine().selection().is_some() {
                    self.engine().clear_selection();
                }
            }
        }
    }

    /// Handle a left-button drag for selection at the gesture's granularity.
    /// `rectangle` is whether option is held (macOS rectangle-select state,
    /// upstream `surface_mouse.zig:121-126`). A `None` drag selection clears
    /// (upstream `cursorPosCallback` applies the drag result verbatim).
    fn selection_drag(&mut self, x: f32, y: f32, rectangle: bool) {
        if self.gesture.click_count() == 0 {
            return;
        }
        let alt_screen = self.engine().alt_screen_active();
        if !self.gesture.anchor_valid(alt_screen) {
            return;
        }
        let cell = self.cell_at_clamped(x, y);
        let Some(point) =
            self.engine()
                .window_to_screen_point(cell.0, cell.1, self.scrollback_offset)
        else {
            return;
        };
        let drag = crate::gesture::Drag {
            point,
            xpos: x as f64,
            ypos: y as f64,
            rectangle,
            alt_screen,
            geometry: self.gesture_geometry(),
            boundary_codepoints: &self.word_boundaries,
        };
        let mut gesture = std::mem::take(&mut self.gesture);
        let sel = {
            let engine = self.engine();
            gesture.drag(&engine, &drag)
        };
        self.gesture = gesture;

        match sel {
            Some((a, b)) => {
                // Only cell-granular drags honor the rectangle flag (word/
                // line/output selections are always linear upstream).
                let rect = rectangle && self.gesture.behavior() == crate::gesture::Behavior::Cell;
                self.engine().select_screen_points(a, b, rect);
            }
            None => {
                self.engine().clear_selection();
            }
        }
    }

    /// Handle a left-button release: end the drag phase (keeping the click
    /// count so the next press can double/triple), and update the clipboard
    /// if copy-on-select is on and a selection exists (upstream copies on
    /// release only — `Surface.zig:3860-3872`).
    fn selection_release(&mut self, x: f32, y: f32, copy_on_select: bool) {
        let alt_screen = self.engine().alt_screen_active();
        let point = self.screen_point_at(x, y);
        self.gesture.release(point, alt_screen);
        if copy_on_select && let Some(text) = self.engine().selection_string() {
            crate::clipboard::write(&text);
        }
    }

    /// One selection-autoscroll tick: while a drag is parked past the top or
    /// bottom edge with the button held, scroll the viewport one row in that
    /// direction and continue the drag at the parked pointer position (the
    /// row now under it). Driven by the ~60Hz pace tick, mirroring upstream's
    /// ~15ms `selection_scroll` io timer (`SelectionGesture.autoscrollTick`).
    fn selection_autoscroll_tick(&mut self) {
        if !self.mouse_button_down || self.is_dead() {
            return;
        }
        let Some((xpos, ypos, rectangle)) = self.gesture.last_drag() else {
            return;
        };
        match self.gesture.autoscroll() {
            crate::gesture::Autoscroll::None => return,
            crate::gesture::Autoscroll::Up => {
                let max = self.engine().scrollback_len();
                self.scrollback_offset = (self.scrollback_offset + 1).min(max);
            }
            crate::gesture::Autoscroll::Down => {
                self.scrollback_offset = self.scrollback_offset.saturating_sub(1);
            }
        }
        self.selection_drag(xpos as f32, ypos as f32, rectangle);
    }

    /// Apply a focus change to THIS surface (per-pane focus reporting,
    /// app-hardening). Two effects, both per-SURFACE (previously the tab drove
    /// them per-tab, so a split tab's password poll + mode-1004 reporting were
    /// wrong for the unfocused panes):
    ///
    /// 1. `io.focus(focused)` starts/stops this pane's 200ms termios password
    ///    poll (the hub's `Writer::focus` → `Exec.focusGained` timer flag).
    /// 2. If this pane's program enabled focus reporting (mode 1004,
    ///    `engine.focus_reporting()`), emit `CSI I` on focus-in / `CSI O` on
    ///    focus-out to its pty — the bytes a 1004 app expects. This mirrors
    ///    upstream `Surface.focusCallback` (which sets `io.focused` then, under
    ///    mode 1004, writes `focus_in`/`focus_out`) and the reference spike's
    ///    `Event::WindowFocused` handler. A dead (poisoned) pane is skipped — its
    ///    io is shut down.
    fn set_focus(&mut self, focused: bool) {
        if self.is_dead() {
            return;
        }
        // A display-only tmux pane surface has no pty: focus (and its 1004
        // reporting) is routed to tmux in slice 5d, not here.
        let Some(io) = &self.io else {
            return;
        };
        io.focus(focused);
        // Only emit the 1004 report if the program asked for it. `engine()`
        // degrades a poisoned lock to a dead surface (and returns), so re-check.
        let reporting = self.engine().focus_reporting();
        if self.is_dead() || !reporting {
            return;
        }
        // CSI I = focus in, CSI O = focus out (xterm mode 1004).
        let bytes: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
        self.send_pty(bytes);
    }

    /// Snap this pane's viewport back to the live active area (offset 0) and
    /// clear the wheel accumulator. Called on key input to this pane (upstream
    /// `scroll-to-bottom.keystroke`, default on).
    fn snap_to_bottom(&mut self) {
        self.scrollback_offset = 0;
        self.wheel = crate::scroll::WheelState::default();
    }

    // -- search ----------------------------------------------------------

    /// The pane view's bounds in points (for overlay placement).
    fn view_bounds(&self) -> NSRect {
        self.view.bounds()
    }

    /// Open the search bar on this pane: create the overlay lazily (parented to
    /// this pane's view), show it, focus its field, and mark the search state
    /// active. Idempotent — re-opening just re-focuses the existing overlay.
    fn search_open(
        &mut self,
        mtm: MainThreadMarker,
        controller: Controller,
        tab: TabId,
        surface: SurfaceId,
    ) {
        self.search.open();
        self.search_highlight_dirty.set(true);
        let bounds = self.view_bounds();
        if self.search_overlay.is_none() {
            let overlay =
                crate::search_overlay::SearchOverlay::new(mtm, controller, tab, surface, bounds);
            self.view.addSubview(&overlay);
            self.search_overlay = Some(overlay);
        }
        if let Some(overlay) = &self.search_overlay {
            overlay.open(bounds);
            overlay.set_counter(&self.search.counter_label());
        }
    }

    /// Close the search bar: hide the overlay (returning focus to the terminal),
    /// clear the match state, and snap back to the live viewport.
    fn search_close(&mut self) {
        if let Some(overlay) = &self.search_overlay {
            overlay.close();
        }
        self.search.close();
        // Repaint to drop the now-cleared match highlights.
        self.search_highlight_dirty.set(true);
        // Returning to the live view after a scrolled-to match is the least
        // surprising default (matches scroll-to-bottom on keystroke).
        self.scrollback_offset = 0;
    }

    /// Re-run the search for a new needle (incremental, on the engine under the
    /// lock), update the match set + counter, and scroll the first match into
    /// view. A no-op if the search bar is not open.
    fn search_set_needle(&mut self, needle: &str) {
        if !self.search.is_active() {
            return;
        }
        let matches = self.engine().search_all(needle.as_bytes());
        self.search.set_results(needle.to_string(), matches);
        // The highlight set changed; force a repaint even if no match scrolls the
        // viewport (the common "search for something already on screen" case).
        self.search_highlight_dirty.set(true);
        if let Some(range) = self.search.current_match() {
            self.scroll_match_into_view(range);
        }
        if let Some(overlay) = &self.search_overlay {
            overlay.set_counter(&self.search.counter_label());
        }
    }

    /// Move to the next (`forward`) or previous match, scroll it into view, and
    /// update the counter. A no-op if search is closed or there are no matches.
    fn search_navigate(&mut self, forward: bool) {
        if !self.search.is_active() {
            return;
        }
        let range = if forward {
            self.search.next()
        } else {
            self.search.previous()
        };
        // The current-match tint moved; force a repaint (a same-viewport step,
        // e.g. two matches on one screen, wouldn't otherwise redraw).
        self.search_highlight_dirty.set(true);
        if let Some(range) = range {
            self.scroll_match_into_view(range);
        }
        if let Some(overlay) = &self.search_overlay {
            overlay.set_counter(&self.search.counter_label());
        }
    }

    /// Set this pane's scrollback offset so `range`'s top row sits at the top of
    /// the viewport (clamped to the live-area/top-of-history bounds). Reuses the
    /// same `scrollback_offset` machinery wheel-scroll drives.
    ///
    /// The snapshot maps `window_top = total_rows - (offset + rows)`, i.e.
    /// `window_top = scrollback_len - offset`; to put absolute row `R` at the
    /// top we need `offset = scrollback_len - R`, clamped to `[0,
    /// scrollback_len]`.
    fn scroll_match_into_view(&mut self, range: crate::selection::ScreenRange) {
        let scrollback_len = self.engine().scrollback_len();
        let row = range.top_left.1;
        let offset = scrollback_len.saturating_sub(row);
        self.scrollback_offset = offset.min(scrollback_len);
    }

    /// Move this pane's scrollback viewport per a keybind scroll action
    /// (`scroll_page_up`/`scroll_to_top`/…). Uses the same `scrollback_offset`
    /// the wheel path drives, so the next rendered frame reflects the move.
    fn scroll_viewport(&mut self, to: crate::scroll::ScrollTo) {
        let max = self.engine().scrollback_len();
        let page = self.rows.max(1);
        self.scrollback_offset =
            crate::scroll::scrolled_offset(to, self.scrollback_offset, max, page);
    }

    /// Whether this pane's search bar is currently open (smoke/test).
    fn search_is_active(&self) -> bool {
        self.search.is_active()
    }

    /// This pane's current match count (smoke/test).
    fn search_match_count(&self) -> usize {
        self.search.count()
    }

    /// This pane's current (navigated-to) match index, 0-based (smoke/test).
    fn search_current_index(&self) -> Option<usize> {
        self.search.current_index()
    }

    /// This pane's current search needle (smoke/test).
    fn search_needle(&self) -> String {
        self.search.needle().to_string()
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
        mods: qwertty_term_input::key_mods::Mods,
        mult: crate::scroll::ScrollMultiplier,
        display_max: Option<usize>,
    ) {
        let cell_h = self.font.cell_height as f64;
        let delta = self.wheel.row_delta(yoff, precision, cell_h, mult);
        if delta == 0 {
            return;
        }

        // Display-only tmux pane: no pty, no live mode state of its own — just
        // move the scrollback viewport, clamped to the Viewer pane terminal's
        // real history length. Reporting / alt-scroll / cursor-key paths need a
        // pty and are slice-5d (mouse routing to panes); a pane in alt-screen
        // has 0 scrollback, so this naturally no-ops there.
        if let Some(max) = display_max {
            if let crate::scroll::WheelOutcome::Viewport { rows_up } =
                crate::scroll::decide(delta, false, false, false)
            {
                let cur = self.scrollback_offset as isize;
                self.scrollback_offset = (cur + rows_up).clamp(0, max as isize) as usize;
            }
            return;
        }

        let (reporting_active, alt_screen, alt_scroll, cursor_keys) = {
            let engine = self.engine();
            (
                engine.mouse_event() != qwertty_term_input::mouse_encode::MouseEvent::None,
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
                self.gesture.reset();
                let bytes = arrow_key_bytes(up, cursor_keys);
                for _ in 0..count {
                    self.send_pty(&bytes);
                }
            }
            crate::scroll::WheelOutcome::Report { count, up } => {
                // Upstream clears the selection when reporting is active
                // (a shift-override selection could exist).
                self.engine().clear_selection();
                self.gesture.reset();
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
    fn report_wheel(&mut self, up: bool, mods: qwertty_term_input::key_mods::Mods) {
        use qwertty_term_input::mouse::{Action, Button};
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
            self.send_pty(&bytes);
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
    /// When this tab was created — the 500ms title-fallback grace period is
    /// measured from here (upstream `SurfaceView_AppKit.swift:286-291`: a
    /// timer shows the ghost emoji if no OSC title arrives within 0.5s).
    created: std::time::Instant,
    /// The window title last applied, so the per-tick title poll only calls
    /// `setTitle` on a real change (upstream coalesces rapid changes with a
    /// 75ms timer — `SurfaceView_AppKit.swift:601-618`; the ~16ms poll plus
    /// set-on-change achieves the same no-flicker outcome). `RefCell` because
    /// [`Tab::update_window_title`] takes `&self` from the tick loop.
    last_title: RefCell<String>,
    /// The window subtitle last applied (`window-subtitle`), so the per-tick
    /// poll only calls `setSubtitle` on a real change. Empty when the policy is
    /// `Disabled` or the cwd is unknown.
    last_subtitle: RefCell<String>,
    /// Whether a bell is currently indicated on this tab's title (a pane rang
    /// since the tab was last focused). Drives the 🔔 title prefix (upstream
    /// `bell-features = title`); cleared when the tab becomes key. `Cell` for
    /// the `&self` tick/title accessors.
    bell_ringing: Cell<bool>,
    /// The lazily-created resize-overlay HUD (`cols ⨯ rows`) shown over this
    /// window during a live resize. `None` until the first time it's shown.
    resize_overlay: RefCell<Option<Retained<objc2_app_kit::NSTextField>>>,
    /// When the resize overlay auto-hides if no further resize arrives (upstream
    /// 750ms). `None` when the overlay is hidden. `Cell` for the `&self` tick.
    resize_overlay_deadline: Cell<Option<std::time::Instant>>,
}

impl Tab {
    /// Mint a fresh surface id for this tab.
    fn mint_surface_id(&mut self) -> crate::splits::SurfaceId {
        let id = crate::splits::SurfaceId(self.next_surface);
        self.next_surface += 1;
        id
    }

    /// Show the resize-overlay HUD with `cols ⨯ rows`, positioned per
    /// `resize-overlay-position`, and (re)arm its auto-hide deadline. Lazily
    /// creates the `NSTextField` on first use and adds it over the window's
    /// content view.
    fn show_resize_overlay(
        &self,
        mtm: MainThreadMarker,
        cols: usize,
        rows: usize,
        position: crate::resize_overlay::ResizeOverlayPosition,
        now: std::time::Instant,
        duration: std::time::Duration,
    ) {
        let Some(content) = self.window.contentView() else {
            return;
        };
        let mut slot = self.resize_overlay.borrow_mut();
        let field = slot.get_or_insert_with(|| {
            let f = make_resize_overlay_field(mtm);
            content.addSubview(&f);
            f
        });
        field.setStringValue(&NSString::from_str(&crate::resize_overlay::overlay_text(
            cols, rows,
        )));
        field.sizeToFit();
        // Pad the fit size for a HUD look; center the text within.
        let fit = field.frame().size;
        let hud = (fit.width + 20.0, fit.height + 10.0);
        let container = content.frame().size;
        let (x, y) = position.origin((container.width, container.height), hud);
        field.setFrame(NSRect::new(NSPoint::new(x, y), NSSize::new(hud.0, hud.1)));
        field.setHidden(false);
        self.resize_overlay_deadline.set(Some(now + duration));
    }

    /// Hide the resize overlay if its auto-hide deadline has passed.
    fn tick_resize_overlay(&self, now: std::time::Instant) {
        if let Some(deadline) = self.resize_overlay_deadline.get()
            && now >= deadline
        {
            if let Some(field) = self.resize_overlay.borrow().as_ref() {
                field.setHidden(true);
            }
            self.resize_overlay_deadline.set(None);
        }
    }

    /// The resize overlay's current text while it is shown (deadline in the
    /// future), else `None` (smoke/test).
    fn resize_overlay_text(&self) -> Option<String> {
        self.resize_overlay_deadline.get()?;
        self.resize_overlay
            .borrow()
            .as_ref()
            .filter(|f| !f.isHidden())
            .map(|f| f.stringValue().to_string())
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

        // Position each pane view + reflow its grid. When a pane is zoomed, the
        // layout contains only that one pane; every other pane must be hidden so
        // it doesn't show through the zoomed pane (the zoomed pane covers the
        // whole container). Hiding is toggled per surface each layout so an
        // unzoom re-shows them all.
        for (id, surface) in self.surfaces.iter_mut() {
            if let Some(rect) = layout.panes.get(id) {
                surface.scale = scale;
                surface.rect = *rect;
                let frame = crate::splitview::ns_rect_from_tree(*rect, scale);
                surface.view.setFrame(frame);
                surface.view.setHidden(false);
                surface.reflow();
            } else {
                // Not in the current layout (a non-zoomed pane while another is
                // zoomed): hide it. It keeps its grid/io so an unzoom restores it
                // instantly; it simply isn't drawn.
                surface.view.setHidden(true);
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
    /// title — with native tabs, the window title is also the tab's label.
    ///
    /// Title semantics mirror upstream's macOS surface view: the title is
    /// whatever OSC 0/2 set; if none arrives within a 500ms grace period the
    /// ghost emoji is shown (`SurfaceView_AppKit.swift:286-291` — with shell
    /// integration the shell sets a real title almost immediately, so the
    /// ghost only shows for bare shells). During the grace period the app
    /// name holds the spot (a macOS window needs *some* title before first
    /// draw). `setTitle` is only called when the computed title actually
    /// changes (see [`Tab::last_title`]).
    fn update_window_title(&self, password: bool, forced: Option<&str>) {
        let osc_title = self.focused_surface().and_then(|s| s.engine().title());
        let grace_elapsed = self.created.elapsed() >= std::time::Duration::from_millis(500);
        let title = compose_window_title(
            forced,
            osc_title.as_deref(),
            grace_elapsed,
            self.bell_ringing.get(),
            password,
        );
        if *self.last_title.borrow() == title {
            return;
        }
        self.window.setTitle(&NSString::from_str(&title));
        *self.last_title.borrow_mut() = title;
    }

    /// Reflect `window-subtitle` in the window subtitle (shown under the title,
    /// and in the tab tooltip, on macOS). `WorkingDirectory` tracks the focused
    /// pane's cwd; `Disabled` clears it. Upstream ships this on GTK only
    /// (`Config.zig:2109`); macOS's `NSWindow.subtitle` gives the same surface
    /// natively. Set-on-change, like the title, so the ~16ms poll doesn't churn.
    fn update_window_subtitle(&self, policy: crate::config::WindowSubtitle) {
        let subtitle = match policy {
            crate::config::WindowSubtitle::Disabled => String::new(),
            crate::config::WindowSubtitle::WorkingDirectory => self
                .focused_surface()
                .and_then(|s| s.engine().pwd())
                .unwrap_or_default(),
        };
        if *self.last_subtitle.borrow() == subtitle {
            return;
        }
        self.window.setSubtitle(&NSString::from_str(&subtitle));
        *self.last_subtitle.borrow_mut() = subtitle;
    }

    /// Clear this tab's bell title indicator (called when the tab becomes key /
    /// focused — upstream clears the bell once the user looks at the surface).
    fn clear_bell(&self) {
        self.bell_ringing.set(false);
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
    /// The user's `adjust-*` font-metric nudges, applied to every surface's font
    /// grid at build time (`config.metric_modifiers()`).
    metric_modifiers: qwertty_term_font::metrics::ModifierSet,
    /// A fixed window/tab title override (`title`); when `Some`, it wins over the
    /// program's OSC 0/2 title for every tab (see [`compose_window_title`]).
    forced_title: Option<String>,
    mtm: MainThreadMarker,
    /// The engine startup colors resolved from `config.theme` (palette +
    /// default fg/bg/cursor), applied to every new tab's engine. Falls back
    /// to `qwertty-term-vt`'s built-in default `Colors` if no theme is configured
    /// or it fails to load (a warning is printed to stderr in that case; see
    /// `crate::theme::load_theme`).
    startup_colors: qwertty_term_vt::terminal::Colors,
    /// Selection highlight colors resolved from the same theme (explicit
    /// `selection-background`/`selection-foreground` if the theme set them,
    /// else a plain inverse-video swap).
    selection_colors: SelectionColors,
    /// Whether finishing a mouse-drag selection immediately copies it to the
    /// clipboard (`copy-on-select` config key).
    copy_on_select: bool,
    /// The click-repeat interval for double/triple-click detection: the
    /// `click-repeat-interval` config value if set, else the OS double-click
    /// interval, falling back to upstream's 500ms default
    /// (`click-repeat-interval`, `Config.zig:4673` + `os/mouse.zig`).
    mouse_interval: std::time::Duration,
    /// Word-boundary codepoints for double/triple-click word selection
    /// (`selection-word-chars` config, or the built-in default set). Shared into
    /// each surface at build time. `Arc<[u32]>` so panes share one allocation.
    word_boundaries: std::sync::Arc<[u32]>,
    /// Whether the terminal program may capture shift during mouse reporting
    /// (`mouse-shift-capture`) — combined with each pane's runtime XTSHIFTESCAPE
    /// flag to decide whether shift overrides reporting for selection.
    mouse_shift_capture: crate::config::MouseShiftCapture,
    /// Wheel-scroll multipliers (`mouse-scroll-multiplier` config), clamped to
    /// upstream's valid range.
    scroll_multiplier: crate::scroll::ScrollMultiplier,
    /// User keybindings parsed from `config.keybind` at startup into the ported
    /// `Binding.zig` [`Set`](qwertty_term_input::binding::Set), layered over the
    /// default keymap. Resolved in the key path BEFORE the encoder: chord actions
    /// (tab/split/search) via `perform_keybind_chord`, byte actions (`text:`/
    /// `esc:`/`csi:`) via `crate::keybind::resolve_text_bytes`, and `>`-sequences
    /// via `key_sequence` below.
    keybinds: qwertty_term_input::binding::Set,
    /// In-progress leader-key sequence: the leader triggers pressed so far (e.g.
    /// `[ctrl+a]` after pressing `ctrl+a` of a `ctrl+a>c` binding). `None` when
    /// not mid-sequence. The next key is resolved against this path into
    /// [`Self::keybinds`] (see `handle_key_sequence`).
    key_sequence: Option<Vec<qwertty_term_input::binding::Trigger>>,
    /// Unfocused-split dimming overlay alpha (`1 - unfocused-split-opacity`,
    /// clamped). 0 disables dimming (opacity 1.0). Applied to unfocused panes of
    /// multi-pane tabs only.
    unfocused_dim_alpha: f64,
    /// The configured `unfocused-split-fill` color, if any. When `None`, each
    /// surface dims toward its own terminal background (upstream default).
    unfocused_dim_fill: Option<qwertty_term_vt::color::Rgb>,
    /// The quick-terminal (dropdown) state, lazily created on first toggle.
    /// Its window + single surface live in [`Self::tabs`] under the reserved
    /// [`QUICK_TERMINAL_TAB`] id (so input routing + the pace-tick pump/render
    /// work unchanged), while it stays out of [`Self::registry`] (so tab
    /// navigation, the tab bar, and `tab_count` all exclude it). See
    /// [`Controller::toggle_quick_terminal`].
    quick_terminal: Option<QuickTerminal>,
    /// Quick-terminal config, resolved once at startup (position/size/
    /// animation-duration/autohide).
    quick_terminal_config: QuickTerminalConfig,
    /// Which `bell-features` fire on a terminal BEL (see [`crate::bell`]).
    bell_features: crate::bell::BellFeatures,
    /// What a right-click does (`right-click-action`, default context menu).
    right_click_action: crate::context_menu::RightClickAction,
    /// Whether to hide the mouse cursor while typing (`mouse-hide-while-typing`).
    mouse_hide_while_typing: bool,
    /// Whether hovering a pane focuses it (`focus-follows-mouse`).
    focus_follows_mouse: bool,
    /// What a middle-click does (`middle-click-action`).
    middle_click_action: crate::config::MiddleClickAction,
    /// Paste-protection settings (`clipboard-paste-protection` + `-bracketed-safe`).
    paste_protection: crate::paste::PasteProtection,
    /// Trim trailing whitespace from copied lines (`clipboard-trim-trailing-spaces`).
    clipboard_trim_trailing_spaces: bool,
    /// Clear the selection when the user types (`selection-clear-on-typing`).
    selection_clear_on_typing: bool,
    /// Clear the selection after an explicit copy (`selection-clear-on-copy`);
    /// does not apply to copy-on-select.
    selection_clear_on_copy: bool,
    /// Smoke/test override for the unsafe-paste confirmation: `Some(answer)`
    /// short-circuits the modal alert (which a headless smoke can't drive).
    /// `None` in normal operation. See [`Controller::set_paste_confirm_hook`].
    paste_confirm_hook: Option<bool>,
    /// Whether to quit the app after the last window/surface closes
    /// (`quit-after-last-window-closed`, default false on macOS).
    quit_after_last_window_closed: bool,
    /// Configured initial window size in `(cols, rows)`, applied to the first
    /// window only (`window-width`/`-height`).
    initial_window_cells: Option<(u32, u32)>,
    /// Configured initial window position in `(x, y)` pixels, applied to the
    /// first window only (`window-position-x`/`-y`).
    initial_window_position: Option<(i32, i32)>,
    /// Set once the first window has been created, so the initial-geometry
    /// config applies only to it (not to later Cmd-N windows).
    first_window_placed: Cell<bool>,
    /// Whether apps may post OSC 9 / OSC 777 desktop notifications
    /// (`desktop-notifications`). When false, drained notifications are dropped
    /// before delivery (core-level gate, matching upstream).
    desktop_notifications: bool,
    /// Rate limiter shared across all surfaces (1/sec global + 5s identical
    /// dedup), matching upstream's core notification throttle. `RefCell` so it
    /// can be updated on the pace tick through the shared immutable borrow.
    notification_throttle: RefCell<crate::notify::NotificationThrottle>,
    /// The last desktop notification actually delivered (post-throttle), for
    /// the windowed smoke to observe. `None` until one is delivered.
    last_delivered_notification: RefCell<Option<(String, String)>>,
    /// `notify-on-command-finish` mode (never/unfocused/always). Default
    /// `Never` (the feature is off unless configured).
    notify_on_command_finish: crate::notify::NotifyOnCommandFinish,
    /// Which effects fire on command finish (`bell`/`notify`).
    notify_on_command_finish_action: crate::notify::CommandFinishAction,
    /// Minimum command duration before a finish notifies.
    notify_on_command_finish_after: std::time::Duration,
    /// Whether to show the in-surface OSC 9;4 progress bar (`progress-style`).
    progress_style: bool,
    /// When to confirm before closing a surface with a running process
    /// (`confirm-close-surface`).
    confirm_close_surface: crate::config::ConfirmCloseSurface,
    /// Whether macOS restores windows across quit/relaunch (`window-save-state`);
    /// drives each window's `isRestorable`.
    window_save_state: crate::config::WindowSaveState,
    /// Smoke/test override for the close-confirmation modal: `Some(answer)`
    /// short-circuits the alert (which a headless smoke can't drive). `None` in
    /// normal operation. See [`Controller::set_close_confirm_hook`].
    close_confirm_hook: Option<bool>,
    /// When to show the resize overlay (`resize-overlay`).
    resize_overlay_mode: crate::resize_overlay::ResizeOverlayMode,
    /// Where the resize overlay sits (`resize-overlay-position`).
    resize_overlay_position: crate::resize_overlay::ResizeOverlayPosition,
    /// How long the resize overlay lingers after the last resize
    /// (`resize-overlay-duration`).
    resize_overlay_duration: std::time::Duration,
    /// The window subtitle policy (`window-subtitle`): when
    /// `WorkingDirectory`, each window's `NSWindow.subtitle` tracks the focused
    /// surface's cwd.
    window_subtitle: crate::config::WindowSubtitle,
    /// Where a new tab opens relative to the current one
    /// (`window-new-tab-position`): `Current` (after the active tab) or `End`.
    window_new_tab_position: crate::config::WindowNewTabPosition,
    /// The tab bar visibility policy (`window-show-tab-bar`); drives each new
    /// window's `NSWindowTabbingMode`.
    window_show_tab_bar: crate::config::WindowShowTabBar,
    /// Resize windows in whole-cell increments (`window-step-resize`); sets each
    /// window's `contentResizeIncrements` to the focused cell size.
    window_step_resize: bool,
    /// Whether windows cast a drop shadow (`macos-window-shadow`).
    macos_window_shadow: bool,
    /// The traffic-light button policy (`macos-window-buttons`).
    macos_window_buttons: crate::config::MacWindowButtons,
    /// The window appearance theme (`window-theme`); drives each window's
    /// `NSAppearance` (light/dark, `auto` by background luminosity, or `None`
    /// to follow the system).
    window_theme: crate::config::WindowTheme,
    /// Whether to answer the `CSI 21 t` window-title report query
    /// (`title-report`, default false). Applied to every surface's vt handler
    /// on build + reload (the engine defaults it on for lib parity).
    title_report: bool,
    /// The ENQ (`0x05`) answerback bytes (`enquiry-response`, empty = silent).
    /// Applied to every surface's vt handler on build + reload.
    enquiry_response: Vec<u8>,
    /// The OSC 4/10/11 color-query reply format (`osc-color-report-format`).
    /// Applied to every surface's vt handler on build + reload.
    osc_color_report_format: qwertty_term_vt::stream::OscColorReportFormat,
    /// The per-screen image-storage byte limit (`image-storage-limit`, `0`
    /// disables image protocols). Applied to every surface's terminal on
    /// build + reload.
    image_storage_limit: usize,
    /// The per-surface scrollback byte limit (`scrollback-limit`). Applied at
    /// surface construction; a reload only affects new surfaces (upstream).
    scrollback_limit: usize,
    /// Whether KAM (ANSI mode 2) may suppress keyboard input (`vt-kam-allowed`,
    /// default false). When true and the program has enabled KAM, keyboard
    /// input to the pty is dropped (upstream `Surface.zig:2699`).
    vt_kam_allowed: bool,
    /// tmux control-mode native tabs (ADR 006 slice 5b-native). For each control
    /// surface running `tmux -CC` (keyed by its `(TabId, SurfaceId)`), maps each
    /// tmux **window id** to the native [`TabId`] mirroring it (tmux window →
    /// native tab; its panes are the tab's display-only split surfaces). Entries
    /// are created as windows appear, removed on `RemoveTab`, and the whole
    /// session's tabs are torn down on control-mode `Exit`.
    tmux_tabs: HashMap<(TabId, SurfaceId), HashMap<usize, TabId>>,
    /// Control-surface tabs whose window is currently hidden because their
    /// `tmux -CC` session is live (ADR 006 slice 5d polish, mirroring upstream:
    /// once control mode owns the screen the user should see only the tmux
    /// window tabs, not the raw `tmux -CC` control surface). Hidden when the
    /// session's first native tab is created; the window is restored (and this
    /// entry cleared) on control-mode exit. The control surface keeps being
    /// pumped while hidden — it still drives the in-band control protocol.
    tmux_hidden_controls: std::collections::HashSet<TabId>,
}

/// The subset of VT config toggles applied to a surface's engine (the setters
/// at build + reload, plus the construction-only scrollback limit). Bundled so
/// [`Controller::build_surface`] and [`Controller::reload_config`] apply them
/// identically. Mirrors the per-key seams the `qwertty-term-vt` engine exposes.
#[derive(Clone)]
struct VtToggles {
    title_report: bool,
    enquiry_response: Vec<u8>,
    osc_color_report_format: qwertty_term_vt::stream::OscColorReportFormat,
    image_storage_limit: usize,
    scrollback_limit: usize,
}

impl VtToggles {
    /// Apply the live setters (everything except the construction-only
    /// scrollback limit) to `engine`. Called on the freshly built engine and
    /// on every existing engine during `reload_config`.
    fn apply(&self, engine: &mut Engine) {
        engine.set_title_reporting(self.title_report);
        engine.set_enquiry_response(&self.enquiry_response);
        engine.set_osc_color_report_format(self.osc_color_report_format);
        engine.set_kitty_graphics_size_limit(self.image_storage_limit);
    }
}

/// The reserved [`TabId`] for the quick-terminal surface. Uses the top of the
/// id space so it never collides with a registry-minted tab id.
const QUICK_TERMINAL_TAB: TabId = TabId(u64::MAX);

/// Resolved quick-terminal configuration (see [`crate::config`] +
/// [`crate::quickterm`]).
#[derive(Debug, Clone, Copy)]
struct QuickTerminalConfig {
    position: crate::quickterm::Position,
    size: crate::quickterm::Size,
    animation_duration: f64,
    autohide: bool,
}

/// Live quick-terminal state. The window + surface are in the controller's
/// `tabs` map under [`QUICK_TERMINAL_TAB`]; this tracks only what's specific to
/// the dropdown: whether it's currently shown, and the delegate (kept alive).
struct QuickTerminal {
    /// Whether the dropdown is currently animated in (visible).
    visible: bool,
    /// The window delegate (autohide on resign-key); stored to keep it alive.
    _delegate: Retained<QuickTermDelegate>,
}

/// The controller handle passed to views and menu targets.
#[derive(Clone)]
pub struct Controller(Rc<RefCell<ControllerState>>);

/// Resolve the engine startup [`Colors`](qwertty_term_vt::terminal::Colors) and
/// selection-highlight colors implied by `config`: the theme's palette + fg/bg/
/// cursor, with the user's `cursor-color` override (if set) applied on top of the
/// theme cursor. Shared by [`Controller::new`] and
/// [`Controller::reload_config`] so both derive colors identically. Mirrors the
/// reference spike's `WindowTerminal::new` theme lookup
/// (`crates/spike/src/window/mod.rs`).
/// Compose the window/tab title string. A configured `title` (`forced`)
/// overrides the program-set `osc_title` entirely (upstream `title` semantics:
/// OSC 0/2 changes are ignored while it is set). With neither, the ghost-emoji
/// fallback holds the spot once the 500ms startup grace has elapsed, else the
/// app name. The bell prefix (`bell-features = title`) and password-lock suffix
/// are applied on top of whichever base was chosen.
fn compose_window_title(
    forced: Option<&str>,
    osc_title: Option<&str>,
    grace_elapsed: bool,
    bell: bool,
    password: bool,
) -> String {
    let base = forced
        .or(osc_title)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            if grace_elapsed {
                "👻".to_string()
            } else {
                "qwertty-term".to_string()
            }
        });
    let base = if bell { format!("🔔 {base}") } else { base };
    if password {
        format!("{base} 🔒")
    } else {
        base
    }
}

/// Write `text` to a uniquely-named temp file (`qwertty-term-<kind>-<pid>-<n>.txt`)
/// and return its path, or `None` on an IO error (logged). Backs the
/// `write_*_file` keybind actions. A process-lifetime counter keeps the name
/// unique without a random dependency.
fn write_temp_text_file(text: &str, kind: &str) -> Option<std::path::PathBuf> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "qwertty-term-{kind}-{}-{n}.txt",
        std::process::id()
    ));
    match std::fs::write(&path, text) {
        Ok(()) => Some(path),
        Err(err) => {
            eprintln!(
                "write_temp_text_file: cannot write {}: {err}",
                path.display()
            );
            None
        }
    }
}

fn resolve_colors(
    config: &crate::config::Config,
) -> (qwertty_term_vt::terminal::Colors, SelectionColors) {
    let theme = config.theme.as_deref().and_then(crate::theme::load_theme);
    let mut startup_colors = theme
        .as_ref()
        .map(crate::theme::ThemeColors::to_colors)
        .unwrap_or_default();
    // `cursor-color`/`background`/`foreground` override the theme's values; a
    // running program's OSC 10/11/12 still wins later through the same
    // dynamic-color path.
    if let Some(cursor) = config.cursor_color() {
        startup_colors.cursor.set(cursor);
    }
    if let Some(bg) = config.background() {
        startup_colors.background.set(bg);
    }
    if let Some(fg) = config.foreground() {
        startup_colors.foreground.set(fg);
    }
    // Per-index `palette` overrides sit on top of the theme's palette; OSC 4
    // from the program still overrides at runtime through the dynamic palette.
    for (idx, rgb) in config.palette_overrides() {
        startup_colors.palette.set(idx, rgb);
    }
    // Selection colors: a config `selection-*` overrides the theme's, per
    // channel. Explicit only when both resolve; otherwise invert the cell.
    let sel_bg = config
        .selection_background()
        .or_else(|| theme.as_ref().and_then(|t| t.selection_background));
    let sel_fg = config
        .selection_foreground()
        .or_else(|| theme.as_ref().and_then(|t| t.selection_foreground));
    let selection_colors = match (sel_bg, sel_fg) {
        (Some(bg), Some(fg)) => SelectionColors::Explicit { bg, fg },
        _ => SelectionColors::Inverse,
    };
    (startup_colors, selection_colors)
}

/// The URL at viewport cell `(col, row)` in `snap` (R7 slice 3): the OSC8
/// hyperlink URI if the cell carries one, otherwise a regex-detected URL on
/// that row that covers the column. `None` if the cell isn't over a link.
fn url_at_cell(
    snap: &qwertty_term_vt::snapshot::SnapshotWindow,
    col: usize,
    row: usize,
) -> Option<String> {
    use qwertty_term_vt::page::hyperlink::LinkKey;
    let cells = &snap.window.get(row)?.cells;
    // OSC8 link: the cell carries the URI directly (via its `LinkKey`).
    if let Some(key) = &cells.get(col)?.link {
        let uri = match key {
            LinkKey::Implicit(_, uri) | LinkKey::Explicit(_, uri) => uri,
        };
        return Some(String::from_utf8_lossy(uri).into_owned());
    }
    // Otherwise, regex-detect a URL on the row and return the span under `col`.
    // (Mirrors the renderer's hover detection; the char↔column mapping is small
    // enough to keep local rather than share across the crate boundary.)
    let mut text = String::new();
    let mut col_of_char: Vec<usize> = Vec::new();
    let mut byte_of_char: Vec<usize> = Vec::new();
    for (x, cell) in cells.iter().enumerate() {
        if cell.is_spacer() {
            continue;
        }
        byte_of_char.push(text.len());
        col_of_char.push(x);
        text.push(cell.ch);
        for &c in &cell.combining {
            text.push(c);
        }
    }
    let byte = col_of_char
        .iter()
        .position(|&c| c == col)
        .map(|i| byte_of_char[i])?;
    let span = qwertty_term_renderer::link::url_span_at(&text, byte)?;
    Some(text[span].to_string())
}

/// Open `url` with the system handler (macOS LaunchServices via `open`). The
/// URL is passed as a single argv element (no shell), so it can't inject.
fn open_url(url: &str) {
    let _ = std::process::Command::new("open").arg(url).spawn();
}

impl Controller {
    /// Build a controller from loaded config.
    pub fn new(config: &crate::config::Config, mtm: MainThreadMarker) -> Self {
        let default_font_size = config
            .font_size
            .unwrap_or(crate::font_size::DEFAULT_FONT_SIZE);

        // `window-save-state` drives macOS's `NSQuitAlwaysKeepsWindows` default
        // (read by AppKit at quit): `never` → false, `always` → true, `default`
        // → remove the override so the system "Close windows when quitting an
        // app" setting applies. Port of upstream AppDelegate.applicationWill…
        apply_window_save_state_default(config.window_save_state());

        let (startup_colors, selection_colors) = resolve_colors(config);

        Controller(Rc::new(RefCell::new(ControllerState {
            registry: TabRegistry::new(),
            tabs: HashMap::new(),
            input_config: InputConfig::default(),
            font_family: config.font_family.clone(),
            default_font_size,
            metric_modifiers: config.metric_modifiers(),
            forced_title: config.forced_title().map(str::to_owned),
            mtm,
            startup_colors,
            selection_colors,
            copy_on_select: config.copy_on_select,
            mouse_interval: config
                .click_repeat_interval()
                .unwrap_or_else(crate::gesture::click_interval),
            word_boundaries: config
                .selection_word_chars_codepoints()
                .map(std::sync::Arc::from)
                .unwrap_or_else(|| {
                    std::sync::Arc::from(
                        qwertty_term_vt::screen::DEFAULT_WORD_BOUNDARIES.as_slice(),
                    )
                }),
            mouse_shift_capture: config.mouse_shift_capture(),
            scroll_multiplier: crate::scroll::ScrollMultiplier {
                precision: config.mouse_scroll_multiplier.precision,
                discrete: config.mouse_scroll_multiplier.discrete,
            }
            .clamped(),
            keybinds: crate::keybind::build_set(&config.keybind),
            key_sequence: None,
            // Overlay alpha = 1 - opacity (upstream `SurfaceView.swift` getter),
            // opacity clamped to [0.15, 1.0]. Opacity 1.0 → alpha 0 → no dimming.
            unfocused_dim_alpha: 1.0 - config.unfocused_split_opacity(),
            unfocused_dim_fill: config.unfocused_split_fill(),
            quick_terminal: None,
            quick_terminal_config: QuickTerminalConfig {
                position: config.quick_terminal_position(),
                size: config.quick_terminal_size(),
                animation_duration: config.quick_terminal_animation_duration,
                autohide: config.quick_terminal_autohide,
            },
            bell_features: config.bell_features(),
            right_click_action: config.right_click_action(),
            mouse_hide_while_typing: config.mouse_hide_while_typing,
            focus_follows_mouse: config.focus_follows_mouse,
            middle_click_action: config.middle_click_action(),
            paste_protection: config.paste_protection(),
            clipboard_trim_trailing_spaces: config.clipboard_trim_trailing_spaces,
            selection_clear_on_typing: config.selection_clear_on_typing,
            selection_clear_on_copy: config.selection_clear_on_copy,
            paste_confirm_hook: None,
            quit_after_last_window_closed: config.quit_after_last_window_closed,
            initial_window_cells: config.initial_window_cells(),
            initial_window_position: config.initial_window_position(),
            first_window_placed: Cell::new(false),
            desktop_notifications: config.desktop_notifications,
            notification_throttle: RefCell::new(crate::notify::NotificationThrottle::new()),
            last_delivered_notification: RefCell::new(None),
            notify_on_command_finish: config.notify_on_command_finish(),
            notify_on_command_finish_action: config.notify_on_command_finish_action(),
            notify_on_command_finish_after: config.notify_on_command_finish_after(),
            progress_style: config.progress_style,
            confirm_close_surface: config.confirm_close_surface(),
            close_confirm_hook: None,
            window_save_state: config.window_save_state(),
            resize_overlay_mode: config.resize_overlay(),
            resize_overlay_position: config.resize_overlay_position(),
            resize_overlay_duration: config.resize_overlay_duration(),
            window_subtitle: config.window_subtitle(),
            window_new_tab_position: config.window_new_tab_position(),
            window_show_tab_bar: config.window_show_tab_bar(),
            window_step_resize: config.window_step_resize,
            macos_window_shadow: config.macos_window_shadow,
            macos_window_buttons: config.macos_window_buttons(),
            window_theme: config.window_theme(),
            title_report: config.title_report,
            enquiry_response: config.enquiry_response_bytes().to_vec(),
            osc_color_report_format: config.osc_color_report_format(),
            image_storage_limit: config.image_storage_limit as usize,
            scrollback_limit: config.scrollback_limit,
            vt_kam_allowed: config.vt_kam_allowed,
            tmux_tabs: HashMap::new(),
            tmux_hidden_controls: std::collections::HashSet::new(),
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
        // tmux-managed tab: focus is in a tmux window, so redirect Cmd-T to a
        // tmux `new-window` (Josh's iTerm2-style choice) — it appears as another
        // native tab in the same session via the `%window-add` reconcile.
        // Creating a normal native tab instead would sit outside the session and
        // leave a rogue tab (ADR 006 slice 5e). The new tab arrives async, so
        // there is no id to return synchronously.
        let focused = self.0.borrow().tabs.get(&parent).map(|t| t.tree.focused());
        if let Some(focused) = focused
            && let Some(src) = self.tmux_pane_of(parent, focused)
        {
            self.redirect_tmux_action(src, focused, TmuxNativeAction::NewWindow);
            return None;
        }
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
        tmux_trace!("close_tab({:?})", tab.0);
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

    // -- tmux control mode ------------------------------------------------

    /// Apply the tmux control-mode reconcile events collected during a tick's
    /// per-surface pump (ADR 006 slice 5b-native). Called with **no** controller
    /// borrow held, so it is free to create/remove native tabs.
    ///
    /// Each event carries the control surface `(tab, surface)` running `tmux -CC`
    /// and either a [`ReconcilePlan`](crate::tmux_reconcile::ReconcilePlan) (the
    /// window/pane tree changed) or an `exit` flag (control mode ended). The plan
    /// is applied as Option (a): each tmux **window** becomes a native **tab**
    /// grouped with the control surface's window, and each tmux **pane** becomes
    /// a **display-only split surface** in that tab (rendered from the Viewer's
    /// pane `Terminal` in [`render_tmux_panes`](Self::render_tmux_panes)).
    ///
    /// The native tab creation is driven off `SetSplitTree` (create-or-update)
    /// rather than `CreateTab`: the Reconciler emits a `SetSplitTree` for *every*
    /// present window on each reconcile, and it carries the tree we need to build
    /// the panes, so [`tmux_tabs`](ControllerState::tmux_tabs) membership is the
    /// single source of truth for whether a native tab already exists. `RemoveTab`
    /// closes gone windows; `CreateTab` is implied by the first `SetSplitTree`.
    fn apply_tmux_reconciles(&self, events: Vec<TmuxReconcileEvent>) {
        for event in events {
            let key = (event.tab, event.surface);
            if event.exit {
                // Control mode ended. I1 (never-empty window): restore the
                // hidden `tmux -CC` control tab **before** tearing down the
                // tmux-window tabs, so the window group never passes through a
                // zero-visible-surface state (a zero-surface window self-closes,
                // which is the "close last pane closed the window" bug). Only
                // then close every native tab mirroring this control session.
                tmux_trace!("session EXIT -> tearing down ALL tabs for this control");
                self.restore_control_tab(event.tab);
                let tabs: Vec<TabId> = self
                    .0
                    .borrow_mut()
                    .tmux_tabs
                    .remove(&key)
                    .map(|m| m.into_values().collect())
                    .unwrap_or_default();
                for tab in tabs {
                    self.close_tab(tab);
                }
                continue;
            }
            let control = DisplaySource {
                control_tab: event.tab,
                control_surface: event.surface,
            };
            if let Some(plan) = event.plan {
                self.apply_tmux_plan(key, control, &plan);
            }
            // tmux moved its active pane (e.g. a fresh split makes its new pane
            // active): mirror that into the app's keyboard focus, after any plan
            // above has created the pane's surface. This is the tmux→app half of
            // focus sync; it does NOT send `select-pane` back (no echo loop).
            if let Some(surface) = event.focus {
                self.apply_tmux_focus(surface);
            }
        }
    }

    /// Apply one tmux reconcile plan's tab create/remove/set-tree ops to the
    /// native tabs for control session `key` (ADR 006 slice 5b-native), then hide
    /// the raw control tab once at least one tmux-window tab exists. Split out of
    /// [`apply_tmux_reconciles`](Self::apply_tmux_reconciles) so a focus-only
    /// event (no plan) still reaches the focus-application step.
    fn apply_tmux_plan(
        &self,
        key: (TabId, SurfaceId),
        control: DisplaySource,
        plan: &crate::tmux_reconcile::ReconcilePlan,
    ) {
        tmux_trace!(
            "plan ops={:?} map={:?}",
            plan.ops.len(),
            self.0.borrow().tmux_tabs.get(&key).map(|m| {
                let mut v: Vec<_> = m.iter().map(|(w, t)| (*w, t.0)).collect();
                v.sort();
                v
            })
        );
        // 1. Remove native tabs for windows that disappeared.
        for op in &plan.ops {
            if let crate::tmux_reconcile::ReconcileOp::RemoveTab { window_id } = op {
                tmux_trace!("RemoveTab window={window_id}");
                let native = self
                    .0
                    .borrow_mut()
                    .tmux_tabs
                    .get_mut(&key)
                    .and_then(|m| m.remove(window_id));
                if let Some(tab) = native {
                    self.close_tab(tab);
                }
            }
        }
        // 2. Create-or-update a native tab for every present window.
        for op in &plan.ops {
            if let crate::tmux_reconcile::ReconcileOp::SetSplitTree { window_id, tree } = op {
                let existing = self
                    .0
                    .borrow()
                    .tmux_tabs
                    .get(&key)
                    .and_then(|m| m.get(window_id))
                    .copied();
                match existing {
                    Some(tab) => self.update_tmux_tab(tab, control, tree),
                    None => {
                        // `spawn_tmux_tab` registers the tab itself, before the
                        // window goes live, so it can never be interactable while
                        // unregistered.
                        self.spawn_tmux_tab(key, *window_id, control, tree);
                    }
                }
            }
        }
        // Control-surface visibility follows I1 + I4: while ≥1 tmux-window tab
        // exists, hide the raw `tmux -CC` control tab so the user sees only the
        // tmux windows (I4, upstream behaviour). But the moment a reconcile
        // removes the *last* tmux-window tab (e.g. the last window was killed),
        // restore the control tab so the window group never sits at zero visible
        // surfaces (I1 — never-empty window). `restore_control_tab` is a no-op if
        // the control tab was never hidden (the pre-first-window case), and
        // `hide_control_tab` is a no-op once already hidden.
        // `Some(true)` ≥1 tmux window → hide the control tab (I4). `Some(false)`
        // the map entry exists but is empty → the session had windows and the
        // user just closed the last one: restore the control tab (I1) *and*
        // detach the `tmux -CC` client so it exits instead of lingering (with
        // `detach-on-destroy off` tmux keeps the client attached otherwise —
        // the "closing both tabs doesn't kill tmux -CC" orphan). `None` no entry
        // → pre-first-window startup: restore only, nothing to detach.
        let map_populated = self.0.borrow().tmux_tabs.get(&key).map(|m| !m.is_empty());
        match map_populated {
            Some(true) => self.hide_control_tab(control.control_tab),
            Some(false) => {
                tmux_trace!("map empty -> restore control tab + detach-client");
                self.restore_control_tab(control.control_tab);
                self.detach_tmux_control(control);
            }
            None => self.restore_control_tab(control.control_tab),
        }
    }

    /// Move the app's keyboard focus to the tmux pane `surface` (ADR 006 slice
    /// 5e — the tmux→app half of focus sync). Finds the native tmux-window tab
    /// holding that display-only surface, focuses it in the tree, sends per-pane
    /// focus reporting, and makes its view the window's first responder — WITHOUT
    /// sending a `select-pane` back to tmux (this focus *originated* from tmux,
    /// so echoing it would be redundant control traffic). No-op if the surface is
    /// not a live display pane.
    fn apply_tmux_focus(&self, surface: SurfaceId) {
        let view = {
            let mut state = self.0.borrow_mut();
            // Find the tab whose surface map holds this (unique) display surface.
            let Some((&tab_id, _)) = state
                .tabs
                .iter()
                .find(|(_, t)| t.surfaces.contains_key(&surface))
            else {
                return;
            };
            let Some(t) = state.tabs.get_mut(&tab_id) else {
                return;
            };
            let previous = t.tree.focused();
            if previous == surface {
                return; // already focused; nothing to do.
            }
            if !t.tree.focus(surface) {
                return;
            }
            if let Some(prev) = t.surfaces.get_mut(&previous) {
                prev.set_focus(false);
            }
            if let Some(next) = t.surfaces.get_mut(&surface) {
                next.set_focus(true);
            }
            t.surfaces.get(&surface).map(|s| s.view.clone())
        };
        if let Some(view) = view
            && let Some(window) = view.window()
        {
            window.makeFirstResponder(Some(&view));
        }
    }

    /// Hide the `tmux -CC` control surface's window while its control-mode
    /// session is live (ADR 006 slice 5d polish). The window is ordered out (its
    /// native tab disappears from the group) but the surface keeps being pumped,
    /// so the in-band control protocol continues. Idempotent — tracked in
    /// [`ControllerState::tmux_hidden_controls`].
    fn hide_control_tab(&self, control_tab: TabId) {
        let window = {
            let mut state = self.0.borrow_mut();
            // `tmux_hidden_controls` records our *intent* to keep this control
            // tab hidden; it is NOT a reliable statement about the window. When a
            // tabbed window closes, AppKit surfaces a sibling tab — and the
            // control window is a member of that tab group, so closing a tmux tab
            // can put the raw `tmux -CC` surface back on screen behind our backs.
            // Previously this early-returned on "already hidden" and so never
            // re-hid it: the user was left staring at the control surface (whose
            // grid painting is suppressed, so it shows stale text and no prompt)
            // while the surviving tmux tab was fine underneath. Always re-assert.
            state.tmux_hidden_controls.insert(control_tab);
            state.tabs.get(&control_tab).map(|t| t.window.clone())
        };
        if let Some(window) = window {
            // Only act when it is really on screen, so the common case stays a
            // cheap visibility check with no orderOut/render-clock churn.
            if !window.isVisible() {
                return;
            }
            window.orderOut(None);
            // The render display link was bound to this window's view; a link
            // pauses for an occluded view, so re-point it at a visible window
            // (the tmux tab) or the render loop stalls.
            self.rebind_render_clock();
        }
    }

    /// Restore a previously [hidden](Self::hide_control_tab) control tab's window
    /// on control-mode exit. No-op if it was never hidden.
    fn restore_control_tab(&self, control_tab: TabId) {
        let window = {
            let mut state = self.0.borrow_mut();
            if !state.tmux_hidden_controls.remove(&control_tab) {
                return; // was not hidden
            }
            state.tabs.get(&control_tab).map(|t| t.window.clone())
        };
        if let Some(window) = window {
            window.makeKeyAndOrderFront(None);
            self.rebind_render_clock();
        }
    }

    /// Ask the app delegate to re-point the render display link at a visible
    /// window after a control tab's visibility changed. Reached through the
    /// shared [`AppDelegate`] (the render clock lives there). No-op when the
    /// delegate isn't an [`AppDelegate`] or render is pace-timer driven.
    fn rebind_render_clock(&self) {
        let mtm = self.0.borrow().mtm;
        if let Some(delegate) = NSApplication::sharedApplication(mtm)
            .delegate()
            .and_then(|d| d.downcast::<AppDelegate>().ok())
        {
            delegate.rebind_display_link();
        }
    }

    /// Spawn a native tab that mirrors one tmux window (ADR 006 slice 5b-native):
    /// a window whose content is a [`SplitContainer`] holding one display-only
    /// [`Surface`] per pane in `tree`, grouped as a native tab of the control
    /// surface's window. Returns the new [`TabId`], or `None` if no pane surface
    /// could be built (e.g. no font). The panes are drawn from the Viewer's pane
    /// terminals by [`render_tmux_panes`](Self::render_tmux_panes).
    fn spawn_tmux_tab(
        &self,
        key: (TabId, SurfaceId),
        window_id: usize,
        control: DisplaySource,
        tree: &SplitTree,
    ) -> Option<TabId> {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;
        let scale = 2.0; // provisional; corrected from the real window below.

        let id = self.0.borrow_mut().registry.add();
        let leaves = tree.surfaces();

        // Build a display-only surface per pane (each borrows config internally,
        // so build outside any state borrow).
        let container = crate::splitview::SplitContainer::new(
            mtm,
            controller_ptr,
            id,
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(INITIAL_WIDTH, INITIAL_HEIGHT),
            ),
        );
        let mut surfaces: HashMap<SurfaceId, Surface> = HashMap::new();
        let mut default_bg = (0x18u8, 0x18u8, 0x18u8);
        for leaf in &leaves {
            let Some(surface) = self.build_display_surface(mtm, id, *leaf, scale, control) else {
                continue;
            };
            default_bg = surface.default_bg;
            container.addSubview(&surface.view);
            surfaces.insert(*leaf, surface);
        }
        if surfaces.is_empty() {
            // Nothing to show; roll back the registry entry.
            self.0.borrow_mut().registry.remove(id);
            return None;
        }

        let tabbing_mode = match self.0.borrow().window_show_tab_bar {
            crate::config::WindowShowTabBar::Auto => NSWindowTabbingMode::Automatic,
            crate::config::WindowShowTabBar::Always => NSWindowTabbingMode::Preferred,
            crate::config::WindowShowTabBar::Never => NSWindowTabbingMode::Disallowed,
        };
        let window = make_window(mtm, &container, tabbing_mode);
        set_window_background(&window, default_bg);
        let window_delegate = WindowDelegate::new(mtm, self.clone(), id);
        window.setDelegate(Some(ProtocolObject::from_ref(&*window_delegate)));

        // Correct the scale from the real window and rebuild fonts if it differs.
        let real_scale = window.backingScaleFactor();
        if (real_scale - scale).abs() > f64::EPSILON {
            let family = self.0.borrow().font_family.clone();
            for surface in surfaces.values_mut() {
                surface.scale = real_scale;
                surface.rebuild_font(family.as_deref());
            }
        }

        let next_surface = leaves.iter().map(|s| s.0).max().map(|m| m + 1).unwrap_or(0);
        let mut tab = Tab {
            tree: tree.clone(),
            surfaces,
            window: window.clone(),
            container: container.clone(),
            dividers: Vec::new(),
            _window_delegate: window_delegate,
            next_surface,
            created: std::time::Instant::now(),
            last_title: RefCell::new(String::new()),
            last_subtitle: RefCell::new(String::new()),
            bell_ringing: Cell::new(false),
            resize_overlay: RefCell::new(None),
            resize_overlay_deadline: Cell::new(None),
        };
        tab.relayout(controller_ptr, id, mtm);
        self.0.borrow_mut().tabs.insert(id, tab);
        // Register the tab as tmux-managed BEFORE it becomes live/key below.
        // Every close path (`window_should_close`, `close_tab_confirmed`) decides
        // "is this a tmux window tab?" by looking here; if the window can take a
        // Cmd-W while still unregistered, the close falls through to the ordinary
        // close-window path — which shows the wrong dialog and can tear down the
        // whole session instead of closing one tab.
        self.0
            .borrow_mut()
            .tmux_tabs
            .entry(key)
            .or_default()
            .insert(window_id, id);

        // Group the new tmux-window tab into the control surface's window group
        // so it appears as a native tab (unless tabbing is disallowed).
        if tabbing_mode != NSWindowTabbingMode::Disallowed {
            let parent_window = self
                .0
                .borrow()
                .tabs
                .get(&control.control_tab)
                .map(|t| t.window.clone());
            if let Some(pw) = parent_window {
                pw.addTabbedWindow_ordered(&window, objc2_app_kit::NSWindowOrderingMode::Above);
            }
        }

        window.makeKeyAndOrderFront(None);
        Some(id)
    }

    /// Re-apply a tmux window's split tree to its existing native tab (ADR 006
    /// slice 5b-native): add display-only surfaces for newly-appeared panes,
    /// drop surfaces for panes that vanished, set the tab's tree, and re-lay-out.
    /// Surviving panes keep their surface (the Reconciler keeps a stable
    /// `pane_id → SurfaceId` map), so their content/render state is preserved.
    fn update_tmux_tab(&self, tab_id: TabId, control: DisplaySource, tree: &SplitTree) {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;
        let leaves = tree.surfaces();

        // Diff the leaf set against the tab's current surfaces under a scoped
        // borrow: collect the pane ids to add and the surfaces to drop.
        let (to_add, scale, focus_removed): (Vec<SurfaceId>, f64, bool) = {
            let mut state = self.0.borrow_mut();
            let Some(t) = state.tabs.get_mut(&tab_id) else {
                return;
            };
            let scale = t.window.backingScaleFactor();
            let present: std::collections::HashSet<SurfaceId> =
                t.surfaces.keys().copied().collect();
            let wanted: std::collections::HashSet<SurfaceId> = leaves.iter().copied().collect();
            // Whether the pane that currently holds focus (and thus, for a fronted
            // tmux tab, the window's first responder) is being removed. If so we
            // must hand focus to a survivor below, or the window is left with a
            // dangling first responder and *no pane accepts keyboard* after a pane
            // close (ADR 006 slice 5e teardown fix).
            let focus_removed = !wanted.contains(&t.tree.focused());
            // Remove panes no longer in the layout.
            for gone in present.difference(&wanted) {
                if let Some(dead) = t.surfaces.remove(gone) {
                    dead.view.removeFromSuperview();
                }
            }
            let to_add: Vec<SurfaceId> = wanted.difference(&present).copied().collect();
            (to_add, scale, focus_removed)
        };

        // Build new display surfaces outside the borrow (config reads borrow).
        let mut built: Vec<(SurfaceId, Surface)> = Vec::new();
        for leaf in to_add {
            if let Some(surface) = self.build_display_surface(mtm, tab_id, leaf, scale, control) {
                built.push((leaf, surface));
            }
        }

        // Insert the new surfaces, set the tree, and re-lay-out under a borrow.
        // If the focused pane was removed, hand focus to the new tree's focused
        // survivor (the reconciler's leftmost leaf) and capture its view so it can
        // be made first responder once the borrow drops.
        let refocus_view: Option<Retained<TerminalView>> = {
            let mut state = self.0.borrow_mut();
            let Some(t) = state.tabs.get_mut(&tab_id) else {
                return;
            };
            for (leaf, surface) in built {
                t.container.addSubview(&surface.view);
                t.surfaces.insert(leaf, surface);
            }
            t.tree = tree.clone();
            t.relayout(controller_ptr, tab_id, mtm);
            if focus_removed {
                let survivor = t.tree.focused();
                if let Some(next) = t.surfaces.get_mut(&survivor) {
                    // Focus-IN reporting (mode-1004 + password poll) for the pane
                    // that inherits focus, mirroring the split/close paths.
                    next.set_focus(true);
                    Some(next.view.clone())
                } else {
                    None
                }
            } else {
                None
            }
        };

        // Make the survivor the window's first responder with no borrow held, so
        // keyboard input routes to a live tmux pane after a pane close.
        if let Some(view) = refocus_view
            && let Some(window) = view.window()
        {
            window.makeFirstResponder(Some(&view));
        }
    }

    /// Draw every display-only tmux pane surface from the pane `Terminal` its
    /// control session's Viewer owns (ADR 006 slice 5b-native). Two-phase to
    /// respect the borrow checker: phase 1 reads each display surface's control
    /// session and snapshots its pane terminal into an owned
    /// [`SnapshotWindow`](qwertty_term_vt::snapshot::SnapshotWindow) (immutable
    /// borrow of `state.tabs`, reading one tab's session while identifying
    /// another tab's display surface); phase 2 draws each snapshot through its
    /// own surface's render engine (mutable borrow). Called every render tick —
    /// `%output` updates pane terminals continuously, without a tree change.
    fn render_tmux_panes(&self) {
        struct PaneFrame {
            tab: TabId,
            surface: SurfaceId,
            window: qwertty_term_vt::snapshot::SnapshotWindow,
            range: Option<crate::selection::ScreenRange>,
            focused: bool,
            is_split: bool,
        }

        // Phase 1: snapshot each display pane's foreign terminal (immutable).
        let frames: Vec<PaneFrame> =
            {
                let state = self.0.borrow();
                let mut frames = Vec::new();
                for (tid, tab) in &state.tabs {
                    let is_split = tab.tree.len() > 1;
                    let focused = tab.tree.focused();
                    for (sid, surface) in &tab.surfaces {
                        let Some(src) = surface.display_source() else {
                            continue; // an ordinary pane; already drawn in the tick loop.
                        };
                        // Resolve the control surface's live session and this pane's
                        // Viewer-owned terminal, then snapshot it (non-mutating).
                        // Snapshot at this pane's own scrollback offset so wheel
                        // scrolling into the pane's history is honored (the offset
                        // is advanced by `apply_wheel` against the pane terminal's
                        // real scrollback length — the surface's own engine is
                        // empty). Live view is offset 0.
                        let term = state
                            .tabs
                            .get(&src.control_tab)
                            .and_then(|ct| ct.surfaces.get(&src.control_surface))
                            .and_then(|cs| cs.tmux_session())
                            .and_then(|session| session.pane_terminal(*sid));
                        if let Some(term) = term {
                            let window = term.snapshot_window(surface.scrollback_offset);
                            // Selection tint (ADR 006 slice 5d): the pane terminal
                            // owns any selection built by `tmux_pane_mouse`; convert
                            // its pin-free screen rect for the tint pass.
                            let range = term.selection_screen_rect().map(
                                |(tlx, tly, brx, bry, rectangle)| crate::selection::ScreenRange {
                                    top_left: (tlx, tly),
                                    bottom_right: (brx, bry),
                                    rectangle,
                                },
                            );
                            frames.push(PaneFrame {
                                tab: *tid,
                                surface: *sid,
                                window,
                                range,
                                focused: *sid == focused,
                                is_split,
                            });
                        }
                    }
                }
                frames
            };

        // One-shot diagnostic: confirm this path (which draws tmux panes from
        // the Viewer's terminals) actually runs on the live render loop. Fires
        // the first time >=1 pane is drawn. If you see this line, the display
        // link is rendering tmux panes; if the panes are still blank after it,
        // the problem is downstream (snapshot/draw), not the missing call.
        if !frames.is_empty() {
            use std::sync::atomic::{AtomicBool, Ordering};
            static LOGGED: AtomicBool = AtomicBool::new(false);
            if !LOGGED.swap(true, Ordering::Relaxed) {
                eprintln!(
                    "qwertty-term[tmux]: rendering {} pane(s) from viewer terminals",
                    frames.len()
                );
            }
        }

        // Phase 2: draw each snapshot through its own surface (mutable).
        let mut state = self.0.borrow_mut();
        for f in frames {
            if let Some(tab) = state.tabs.get_mut(&f.tab)
                && let Some(surface) = tab.surfaces.get_mut(&f.surface)
            {
                surface.render_window(f.window, f.range, f.focused, f.is_split);
            }
        }
    }

    // -- quick terminal ---------------------------------------------------

    /// Toggle the quick-terminal dropdown: create it on first use, then animate
    /// it in (if hidden) or out (if visible). The QT is a borderless,
    /// `popUpMenu`-level window hosting one surface, positioned by
    /// `quick-terminal-position`/`-size` and slid in/out over
    /// `quick-terminal-animation-duration` (upstream `QuickTerminalController`).
    pub fn toggle_quick_terminal(&self) {
        // If the QT existed but its shell exited (its tab was closed), forget
        // the stale state so we recreate a fresh dropdown.
        {
            let mut state = self.0.borrow_mut();
            if state.quick_terminal.is_some() && !state.tabs.contains_key(&QUICK_TERMINAL_TAB) {
                state.quick_terminal = None;
            }
        }

        let exists = self.0.borrow().quick_terminal.is_some();
        if !exists {
            if !self.build_quick_terminal() {
                return;
            }
            self.animate_quick_terminal(true);
            return;
        }

        let visible = self
            .0
            .borrow()
            .quick_terminal
            .as_ref()
            .map(|q| q.visible)
            .unwrap_or(false);
        self.animate_quick_terminal(!visible);
    }

    /// Build the quick-terminal window + surface (lazy, first toggle). Returns
    /// whether it was created (false if the surface couldn't be built, e.g. no
    /// PTY). Mirrors [`Self::spawn_tab`] but with a borderless key-capable
    /// window kept out of the tab registry.
    fn build_quick_terminal(&self) -> bool {
        let mtm = self.0.borrow().mtm;
        let scale = 2.0; // provisional; corrected from the real window below.
        let tab = QUICK_TERMINAL_TAB;
        let surface_id = SurfaceId(0);

        let Some(mut surface) = self.build_surface(mtm, tab, surface_id, scale, None) else {
            return false;
        };
        let default_bg = surface.default_bg;

        let controller_ptr: *const Controller = self;
        let container = crate::splitview::SplitContainer::new(
            mtm,
            controller_ptr,
            tab,
            NSRect::new(
                NSPoint::new(0.0, 0.0),
                NSSize::new(INITIAL_WIDTH, INITIAL_HEIGHT),
            ),
        );
        container.addSubview(&surface.view);

        let window = make_quick_terminal_window(mtm, &container);
        set_window_background(&window, default_bg);

        let delegate = QuickTermDelegate::new(mtm, self.clone());
        window.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));

        let mut tree = SplitTree::leaf(surface_id);
        tree.focus(surface_id);

        let real_scale = window.backingScaleFactor();
        surface.scale = real_scale;
        if (real_scale - scale).abs() > f64::EPSILON {
            let family = self.0.borrow().font_family.clone();
            surface.rebuild_font(family.as_deref());
        }

        let mut surfaces = HashMap::new();
        surfaces.insert(surface_id, surface);

        let mut qt_tab = Tab {
            tree,
            surfaces,
            window: window.clone(),
            container: container.clone(),
            dividers: Vec::new(),
            // Reuse the normal delegate slot for a placeholder; the QT's real
            // delegate is the `QuickTermDelegate` stored in `QuickTerminal`.
            _window_delegate: WindowDelegate::new(mtm, self.clone(), tab),
            next_surface: 1,
            created: std::time::Instant::now(),
            last_title: RefCell::new(String::new()),
            last_subtitle: RefCell::new(String::new()),
            bell_ringing: Cell::new(false),
            resize_overlay: RefCell::new(None),
            resize_overlay_deadline: Cell::new(None),
        };

        // Size the window to the configured dropdown size on the target screen,
        // placed at its off-screen initial origin (alpha 0), before layout.
        if let Some((frame, _)) = self.quick_terminal_frames(mtm, &window) {
            window.setFrame_display(frame, false);
        }
        qt_tab.relayout(controller_ptr, tab, mtm);

        {
            let mut state = self.0.borrow_mut();
            state.tabs.insert(tab, qt_tab);
            state.quick_terminal = Some(QuickTerminal {
                visible: false,
                _delegate: delegate,
            });
        }
        true
    }

    /// The `(initial, final)` window frames for the quick terminal on its
    /// target screen, from the configured position + size. `None` if there's
    /// no screen. Both are full `NSRect`s (origin + the configured size).
    fn quick_terminal_frames(
        &self,
        mtm: MainThreadMarker,
        window: &NSWindow,
    ) -> Option<(NSRect, NSRect)> {
        let cfg = self.0.borrow().quick_terminal_config;
        // Prefer the window's current screen; fall back to the main screen.
        let screen = window.screen().or_else(|| NSScreen::mainScreen(mtm))?;
        let vf = screen.visibleFrame();
        let visible = crate::quickterm::Rect {
            x: vf.origin.x,
            y: vf.origin.y,
            width: vf.size.width,
            height: vf.size.height,
        };
        let (w, h) = cfg
            .size
            .calculate(cfg.position, visible.width, visible.height);
        let (ix, iy) = crate::quickterm::initial_origin(cfg.position, &visible, w, h);
        let (fx, fy) = crate::quickterm::final_origin(cfg.position, &visible, w, h);
        let size = NSSize::new(w, h);
        Some((
            NSRect::new(NSPoint::new(ix, iy), size),
            NSRect::new(NSPoint::new(fx, fy), size),
        ))
    }

    /// Animate the quick terminal in (`show = true`) or out. Idempotent: a
    /// no-op if already in the requested state. Grabs everything it needs under
    /// a scoped borrow, then runs the AppKit animation with **no controller
    /// borrow held** (the completion + key-window changes re-enter the
    /// controller — the house re-entrancy rule).
    fn animate_quick_terminal(&self, show: bool) {
        let mtm = self.0.borrow().mtm;
        let (window, duration) = {
            let state = self.0.borrow();
            let Some(qt) = state.quick_terminal.as_ref() else {
                return;
            };
            if qt.visible == show {
                return;
            }
            let Some(t) = state.tabs.get(&QUICK_TERMINAL_TAB) else {
                return;
            };
            (
                t.window.clone(),
                state.quick_terminal_config.animation_duration,
            )
        };

        let Some((initial, final_)) = self.quick_terminal_frames(mtm, &window) else {
            return;
        };

        // Record the new visibility up front (so a resign fired mid-animation
        // sees the intended state).
        if let Some(qt) = self.0.borrow_mut().quick_terminal.as_mut() {
            qt.visible = show;
        }

        // Raise above the menu bar so a top/edge dropdown can render over
        // everything (upstream uses `.popUpMenu`).
        window.setLevel(POPUP_MENU_WINDOW_LEVEL);

        if show {
            // Start off-screen + invisible, order in, then animate to the
            // in-position frame at full alpha.
            window.setFrame_display(initial, false);
            window.setAlphaValue(0.0);
            window.makeKeyAndOrderFront(None);
            // Focus the surface so typing lands in the dropdown.
            if let Some(view) = self.surface_view(QUICK_TERMINAL_TAB, SurfaceId(0)) {
                window.makeFirstResponder(Some(&view));
            }
            run_window_slide(&window, final_, 1.0, duration, None);
        } else {
            // Animate back to the off-screen frame + invisible, then order out.
            let window_out = window.clone();
            run_window_slide(
                &window,
                initial,
                0.0,
                duration,
                Some(Box::new(move || window_out.orderOut(None))),
            );
        }
    }

    /// Called from the QT window delegate's `windowDidResignKey:`. If
    /// `quick-terminal-autohide` is on and the dropdown is visible, animate it
    /// out. No controller borrow is held across the animation.
    pub fn quick_terminal_autohide_on_resign(&self) {
        let (autohide, visible) = {
            let state = self.0.borrow();
            (
                state.quick_terminal_config.autohide,
                state
                    .quick_terminal
                    .as_ref()
                    .map(|q| q.visible)
                    .unwrap_or(false),
            )
        };
        if autohide && visible {
            self.animate_quick_terminal(false);
        }
    }

    /// Whether the quick terminal is currently animated in (smoke/test).
    pub fn quick_terminal_visible(&self) -> bool {
        self.0
            .borrow()
            .quick_terminal
            .as_ref()
            .map(|q| q.visible)
            .unwrap_or(false)
    }

    /// The quick terminal's current window frame as `(x, y, w, h)`
    /// (smoke/test), or `None` if it hasn't been created.
    pub fn quick_terminal_frame(&self) -> Option<(f64, f64, f64, f64)> {
        let state = self.0.borrow();
        let t = state.tabs.get(&QUICK_TERMINAL_TAB)?;
        let f = t.window.frame();
        Some((f.origin.x, f.origin.y, f.size.width, f.size.height))
    }

    /// The quick terminal's target in-position frame `(x, y, w, h)` for the
    /// current config/screen (smoke/test): what the window frame should equal
    /// once fully animated in.
    pub fn quick_terminal_final_frame(&self) -> Option<(f64, f64, f64, f64)> {
        let mtm = self.0.borrow().mtm;
        let window = self
            .0
            .borrow()
            .tabs
            .get(&QUICK_TERMINAL_TAB)?
            .window
            .clone();
        let (_, final_) = self.quick_terminal_frames(mtm, &window)?;
        Some((
            final_.origin.x,
            final_.origin.y,
            final_.size.width,
            final_.size.height,
        ))
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
        let overlay_mode = state.resize_overlay_mode;
        let overlay_position = state.resize_overlay_position;
        let overlay_duration = state.resize_overlay_duration;
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
            // Resize overlay: flash the new `cols ⨯ rows` over the pane. A 500ms
            // startup gate suppresses the window-open resize storm for the
            // default `after-first`; `always` shows immediately.
            use crate::resize_overlay::ResizeOverlayMode;
            if overlay_mode != ResizeOverlayMode::Never {
                let now = std::time::Instant::now();
                let past_startup =
                    now.duration_since(t.created) >= std::time::Duration::from_millis(500);
                if (overlay_mode == ResizeOverlayMode::Always || past_startup)
                    && let Some((cols, rows)) = t.focused_surface().map(|s| (s.cols, s.rows))
                {
                    t.show_resize_overlay(mtm, cols, rows, overlay_position, now, overlay_duration);
                }
            }
        }
        drop(state);
        // If this is a tmux-window tab, tell tmux the new window size by resizing
        // the (hidden) control surface's pty — tmux re-lays-out and emits
        // `%layout-change`, which reflows the pane terminals (bug: a window resize
        // otherwise never reaches tmux; the control surface is in a separate tab).
        self.propagate_tmux_window_resize(tab);
    }

    /// Resize the `tmux -CC` control surface's pty to match the current window
    /// size for a tmux-window tab, so tmux re-lays-out its panes to the new
    /// client dimensions (ADR 006 — native→tmux resize). No-op for an ordinary
    /// tab or when the size is unchanged (the unchanged guard also prevents a
    /// resize↔`%layout-change` feedback loop).
    fn propagate_tmux_window_resize(&self, tab: TabId) {
        // Compute the target control-client grid + locate the control surface.
        let Some((control, cols, rows, cw, ch)) = (|| {
            let state = self.0.borrow();
            let t = state.tabs.get(&tab)?;
            // A tmux-window tab's surfaces are all display panes; grab one's
            // control source. `None` for an ordinary tab.
            let src = t.surfaces.values().find_map(|s| s.display_source())?;
            let surface = t.surfaces.values().next()?;
            let (cw, ch) = (surface.font.cell_width, surface.font.cell_height);
            let scale = t.window.backingScaleFactor();
            let bounds = t.container.bounds();
            let (w, h) = (
                (bounds.size.width * scale) as usize,
                (bounds.size.height * scale) as usize,
            );
            let (cols, rows) = crate::geometry::grid_size(w, h, cw, ch);
            Some((src, cols, rows, cw, ch))
        })() else {
            return;
        };
        let mut state = self.0.borrow_mut();
        if let Some(cs) = state
            .tabs
            .get_mut(&control.control_tab)
            .and_then(|t| t.surfaces.get_mut(&control.control_surface))
        {
            // Keep the control surface's own engine + pty in step.
            if cs.cols != cols || cs.rows != rows {
                cs.cols = cols;
                cs.rows = rows;
                cs.engine().resize(cols, rows);
                if let Some(io) = &cs.io {
                    io.resize(cols as u16, rows as u16, cw, ch);
                }
            }
            // Declare the new grid to tmux. This is the load-bearing half: a
            // control client's size comes from `refresh-client -C`, not the pty
            // winsize, so this is what makes tmux re-lay-out to the window. The
            // Viewer dedups repeats, so a steady window sends nothing.
            let cmds = cs
                .tmux
                .as_mut()
                .map(|sess| sess.set_client_size(cols, rows))
                .unwrap_or_default();
            for cmd in &cmds {
                cs.send_pty(cmd);
            }
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
        let t = state.tabs.get(&tab)?;
        let focused = t.tree.focused();
        let s = t.surfaces.get(&focused)?;
        // A display-only tmux pane surface has an empty own engine; its rendered
        // content lives in the control surface's Viewer-owned pane `Terminal`.
        // Read that back so the type-smoke (and any screen-text probe) sees what
        // the pane actually shows (ADR 006 slice 5d).
        if let Some(src) = s.display_source() {
            return state
                .tabs
                .get(&src.control_tab)
                .and_then(|ct| ct.surfaces.get(&src.control_surface))
                .and_then(|cs| cs.tmux_session())
                .and_then(|sess| sess.pane_terminal(focused))
                .map(|term| term.plain_string());
        }
        Some(s.engine().screen_dump())
    }

    /// The active tab's most recently *presented* frame coverage: the max
    /// per-pixel L1 delta from the theme background in the last frame actually
    /// attached to the CoreAnimation layer (smoke/test only; only populated
    /// when presented-pixel capture is enabled — `QWERTTY_TERM_ASSERT_PRESENT`
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

    /// Any tab window currently visible on screen. Used to rebind the render
    /// display link off a window that was ordered out (a hidden tmux control
    /// tab), so the render loop keeps ticking against a window that presents.
    fn first_visible_window(&self) -> Option<Retained<NSWindow>> {
        let state = self.0.borrow();
        state
            .tabs
            .values()
            .map(|t| t.window.clone())
            .find(|w| w.isVisible())
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

    // -- search dispatch -------------------------------------------------

    /// Open (toggle-on) the search bar on the focused pane of `tab`. If it is
    /// already open, re-focus it (a second Cmd+F is a no-op-ish re-focus, not a
    /// close — matching upstream where `end_search` is a separate binding).
    pub fn search_start(&self, tab: TabId) {
        let mtm = self.0.borrow().mtm;
        let controller = self.clone();
        let mut state = self.0.borrow_mut();
        let Some(t) = state.tabs.get_mut(&tab) else {
            return;
        };
        let surface_id = t.tree.focused();
        if let Some(s) = t.surfaces.get_mut(&surface_id) {
            s.search_open(mtm, controller, tab, surface_id);
        }
    }

    /// Close the search bar on `surface` in `tab` (Escape / Cmd+Shift+F).
    pub fn search_end(&self, tab: TabId, surface: SurfaceId) {
        let mut state = self.0.borrow_mut();
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            s.search_close();
        }
    }

    /// The needle changed in `surface`'s search field (`tab`): re-run the
    /// incremental search and update highlights + counter + scroll.
    pub fn search_set_needle(&self, tab: TabId, surface: SurfaceId, needle: &str) {
        let mut state = self.0.borrow_mut();
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            s.search_set_needle(needle);
        }
    }

    /// Navigate to the next match in `surface`'s search (`tab`).
    pub fn search_navigate_next(&self, tab: TabId, surface: SurfaceId) {
        let mut state = self.0.borrow_mut();
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            s.search_navigate(true);
        }
    }

    /// Navigate to the previous match in `surface`'s search (`tab`).
    pub fn search_navigate_previous(&self, tab: TabId, surface: SurfaceId) {
        let mut state = self.0.borrow_mut();
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            s.search_navigate(false);
        }
    }

    /// Dispatch a resolved [`SearchAction`](crate::searchkeys::SearchAction)
    /// from a key chord to the focused pane of `tab`. `Start`/`End` target the
    /// focused pane; `Next`/`Previous` target it too. Called from the view's
    /// `performKeyEquivalent:`.
    pub fn handle_search_action(&self, tab: TabId, action: crate::searchkeys::SearchAction) {
        use crate::searchkeys::SearchAction;
        let surface = {
            let state = self.0.borrow();
            state.tabs.get(&tab).map(|t| t.tree.focused())
        };
        let Some(surface) = surface else { return };
        match action {
            SearchAction::Start => self.search_start(tab),
            SearchAction::End => self.search_end(tab, surface),
            SearchAction::Next => self.search_navigate_next(tab, surface),
            SearchAction::Previous => self.search_navigate_previous(tab, surface),
        }
    }

    /// Close the focused pane of `tab`, gated by `confirm-close-surface` when a
    /// process is running. The cmd+w / `close_surface` behavior: the last pane's
    /// collapse closes the tab (today's model for single-pane tabs). Shared by
    /// the Close-Tab menu item and the `close_surface`/`close_tab` keybinds.
    fn close_focused_confirmed(&self, tab: TabId) {
        let focused = self.0.borrow().tabs.get(&tab).map(|t| t.tree.focused());
        if let Some(surface) = focused
            && self.confirm_close_surface(tab, surface)
        {
            self.close_surface(tab, surface);
        }
    }

    /// Close a whole *tab* (the Close-Tab menu item / `close_tab` keybind), as
    /// distinct from closing one pane (`close_surface`). For a **tmux-managed
    /// tab** this is redirected to a tmux `kill-window` (ADR 006 slice 5e — gap
    /// 1): tmux owns the layout, so the native tab is removed by the resulting
    /// reconcile, never closed directly (I3). For an ordinary tab it falls back
    /// to closing the focused pane (whose last-pane collapse closes the tab —
    /// today's single-pane-tab model).
    fn close_tab_confirmed(&self, tab: TabId) {
        if let Some((ctrl_tab, ctrl_surface, window_id)) = self.tmux_window_tab(tab) {
            self.redirect_tmux_kill_window(
                DisplaySource {
                    control_tab: ctrl_tab,
                    control_surface: ctrl_surface,
                },
                window_id,
            );
            return;
        }
        if self.is_tmux_managed_tab(tab) {
            // tmux-managed but not resolvable to a window id (should not happen
            // now that registration precedes the tab going live). Do nothing
            // rather than closing it natively: tmux owns the layout (I3), and a
            // native close here desyncs or tears down the session.
            return;
        }
        self.close_focused_confirmed(tab);
    }

    /// Toggle native macOS full-screen for `tab`'s window (the `toggle_fullscreen`
    /// keybind / View menu "Enter Full Screen", NSWindow's own animation).
    fn toggle_fullscreen(&self, tab: TabId) {
        let window = self.0.borrow().tabs.get(&tab).map(|t| t.window.clone());
        if let Some(window) = window {
            window.toggleFullScreen(None);
        }
    }

    /// Read text from the focused pane's engine via `read` (locks the engine
    /// under a shared borrow). `None` if there is no focused surface, or `read`
    /// returns `None` (e.g. an empty selection).
    fn focused_engine_text(
        &self,
        tab: TabId,
        read: impl FnOnce(&Engine) -> Option<String>,
    ) -> Option<String> {
        let state = self.0.borrow();
        let t = state.tabs.get(&tab)?;
        let s = t.surfaces.get(&t.tree.focused())?;
        read(&s.engine())
    }

    /// Write `text` to a `kind` temp file, then copy/paste/open its path per `ws`
    /// (shared by the `write_scrollback_file`/`write_screen_file`/
    /// `write_selection_file` keybind actions). Only plain text is produced
    /// today; the `vt`/`html` formats fall back to plain.
    fn write_screen_action(
        &self,
        tab: TabId,
        ws: qwertty_term_input::binding::action::WriteScreen,
        text: &str,
        kind: &str,
    ) {
        use qwertty_term_input::binding::action::WriteScreenAction;

        let Some(path) = write_temp_text_file(text, kind) else {
            return;
        };
        let path_str = path.to_string_lossy().into_owned();

        match ws.action {
            // Copy the path to the system clipboard.
            WriteScreenAction::Copy => {
                crate::clipboard::write(&path_str);
            }
            // Type the path into the focused pane (so the user can act on it).
            WriteScreenAction::Paste => {
                let mut state = self.0.borrow_mut();
                if let Some(t) = state.tabs.get_mut(&tab) {
                    let sid = t.tree.focused();
                    if let Some(s) = t.surfaces.get_mut(&sid) {
                        s.send_pty(path_str.as_bytes());
                    }
                }
            }
            // Open the file with the default OS handler.
            WriteScreenAction::Open => {
                if let Err(err) = std::process::Command::new("open").arg(&path).spawn() {
                    eprintln!("write_{kind}_file: cannot open {}: {err}", path.display());
                }
            }
        }
    }

    /// Move the focused pane's scrollback viewport per a keybind scroll action
    /// (`scroll_page_up`/`scroll_to_bottom`/…).
    fn scroll_focused_surface(&self, tab: TabId, to: crate::scroll::ScrollTo) {
        let mut state = self.0.borrow_mut();
        let Some(t) = state.tabs.get_mut(&tab) else {
            return;
        };
        let sid = t.tree.focused();
        if let Some(s) = t.surfaces.get_mut(&sid) {
            s.scroll_viewport(to);
        }
    }

    /// Whether the focused pane of the active tab has its search bar open
    /// (smoke/test, and used by the view to gate the Escape chord so a plain
    /// Escape still reaches the PTY when not searching).
    pub fn active_search_is_active(&self) -> bool {
        let state = self.0.borrow();
        let Some(tab) = state.registry.active() else {
            return false;
        };
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.focused_surface())
            .map(|s| s.search_is_active())
            .unwrap_or(false)
    }

    /// The focused pane's search match count (smoke/test).
    pub fn active_search_match_count(&self) -> Option<usize> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.focused_surface())
            .map(|s| s.search_match_count())
    }

    /// The focused pane's current (navigated-to) match index (smoke/test).
    pub fn active_search_current_index(&self) -> Option<usize> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.focused_surface())
            .and_then(|s| s.search_current_index())
    }

    /// The focused pane's current search needle (smoke/test).
    pub fn active_search_needle(&self) -> Option<String> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.focused_surface())
            .map(|s| s.search_needle())
    }

    /// Whether the active window's first responder is a text-field editor — i.e.
    /// the search box is currently being edited (its native `NSTextField` field
    /// editor holds focus). The only editable field in the app is the search
    /// needle, so this is the "typing in the search box" signal. Smoke/test only.
    pub fn active_search_field_is_editing(&self) -> bool {
        let Some(window) = self.active_window() else {
            return false;
        };
        // SAFETY: main-thread AppKit accessors. `firstResponder` is a +0
        // responder or nil; `NSText` is the field-editor superclass.
        unsafe {
            let responder: *mut AnyObject = msg_send![&*window, firstResponder];
            if responder.is_null() {
                return false;
            }
            msg_send![responder, isKindOfClass: objc2::class!(NSText)]
        }
    }

    /// Mark `tab` active (called when its window becomes key).
    pub fn set_active(&self, tab: TabId) {
        self.0.borrow_mut().registry.activate(tab);
    }

    /// A tab's window gained/lost key status (per-pane focus reporting,
    /// app-hardening). Route the focus change to that tab's currently-focused
    /// pane ONLY — the pane the user is looking at — matching upstream
    /// `Surface.focusCallback` semantics where a window losing key sends
    /// focus-out to its focused surface and a window gaining key sends focus-in.
    /// The other panes in the tab are already unfocused and stay that way.
    ///
    /// This is the window-level half; intra-tab pane switches are handled in
    /// [`Self::focus_surface_in_tab`] / [`Self::new_split`] /
    /// [`Self::close_surface`]. Together they make mode-1004 reporting + the
    /// 200ms password poll per-SURFACE rather than the old per-TAB wiring.
    pub fn tab_window_focus(&self, tab: TabId, focused: bool) {
        let mut state = self.0.borrow_mut();
        let Some(t) = state.tabs.get_mut(&tab) else {
            return;
        };
        // The user is now looking at this tab: clear any bell indicator
        // (upstream clears the bell on surface focus).
        if focused {
            t.clear_bell();
        }
        let focused_id = t.tree.focused();
        if let Some(s) = t.surfaces.get_mut(&focused_id) {
            s.set_focus(focused);
        }
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

    /// A specific surface's terminal view (smoke/test): the selection smoke
    /// delivers synthetic mouse events straight to the view's
    /// `mouseDown:`/`mouseDragged:`/`mouseUp:` — dispatching through
    /// `NSApplication::sendEvent` hit-testing can enter AppKit's nested
    /// window-drag tracking loop for points near the titlebar and deadlock a
    /// synthetic event stream (no real mouse-up can ever arrive).
    pub fn surface_view(&self, tab: TabId, surface: SurfaceId) -> Option<Retained<TerminalView>> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.view.clone())
    }

    /// A tab's current NSWindow title (smoke/test) — with native tabs this is
    /// also the tab's label in the tab bar.
    pub fn tab_window_title(&self, tab: TabId) -> Option<String> {
        let state = self.0.borrow();
        state.tabs.get(&tab).map(|t| t.window.title().to_string())
    }

    /// Whether a tab currently shows the bell title indicator (smoke/test).
    pub fn tab_bell_ringing(&self, tab: TabId) -> bool {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .map(|t| t.bell_ringing.get())
            .unwrap_or(false)
    }

    /// Force one pace tick (smoke/test): pump + render every pane, firing the
    /// bell drain/effects. Lets a smoke drive the bell path deterministically
    /// without waiting on the real timer.
    pub fn tick_once(&self) {
        self.tick();
    }

    /// A specific surface's current selection text (smoke/test): the engine's
    /// whitespace-trimmed selection string, `None` when nothing is selected.
    pub fn surface_selection_string(&self, tab: TabId, surface: SurfaceId) -> Option<String> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .and_then(|s| s.engine().selection_string())
    }

    /// The *window*-coordinate point at a fractional cell position of a
    /// surface's grid (smoke/test): `(col + fx, row + fy)` cells, converted
    /// from the pane view's flipped top-left point space to window base
    /// coordinates — the `locationInWindow` a synthetic mouse NSEvent needs to
    /// land on that spot through the real hit-testing path.
    pub fn surface_cell_window_point(
        &self,
        tab: TabId,
        surface: SurfaceId,
        col: f64,
        row: f64,
    ) -> Option<NSPoint> {
        let state = self.0.borrow();
        let s = state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))?;
        // Cell metrics are device pixels; view points are device px / scale.
        let scale = if s.scale > 0.0 { s.scale } else { 2.0 };
        let x_pt = col * s.font.cell_width as f64 / scale;
        let y_pt = row * s.font.cell_height as f64 / scale;
        Some(s.view.convertPoint_toView(NSPoint::new(x_pt, y_pt), None))
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

    /// Smoke/test: resize the active tab's window content by `(dw, dh)` points
    /// and re-sync geometry (as a live window resize would), returning the tab
    /// and its focused pane's new `(cols, rows)`.
    pub fn smoke_resize_active_window(&self, dw: f64, dh: f64) -> Option<(TabId, usize, usize)> {
        let (window, tab) = {
            let state = self.0.borrow();
            let tab = self.active_tab()?;
            let window = state.tabs.get(&tab)?.window.clone();
            (window, tab)
        };
        let size = window.contentView().map(|cv| cv.frame().size)?;
        window.setContentSize(NSSize::new(
            (size.width + dw).max(120.0),
            (size.height + dh).max(80.0),
        ));
        self.resync_tab_geometry(tab);
        let state = self.0.borrow();
        let (cols, rows) = state
            .tabs
            .get(&tab)
            .and_then(|t| t.focused_surface())
            .map(|s| (s.cols, s.rows))?;
        Some((tab, cols, rows))
    }

    /// A tab's currently-shown resize-overlay text (or `None` when hidden) —
    /// the resize smoke reads this to assert the `cols ⨯ rows` HUD shows on
    /// resize and clears after its duration (smoke/test).
    pub fn tab_resize_overlay_text(&self, tab: TabId) -> Option<String> {
        self.0
            .borrow()
            .tabs
            .get(&tab)
            .and_then(|t| t.resize_overlay_text())
    }

    /// A specific surface's current OSC 9;4 progress-bar display state (or
    /// `None` when no bar is shown) — the notify-progress smoke reads this to
    /// assert the parse → drain → gate → auto-clear pipeline (smoke/test).
    pub fn surface_progress(
        &self,
        tab: TabId,
        surface: SurfaceId,
    ) -> Option<crate::progress::ProgressDisplay> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .and_then(|s| s.progress)
    }

    /// A specific surface's presented-frame coverage (max background delta) —
    /// the presented-pixel smoke reads this to confirm each pane rendered ink in
    /// its own rect (smoke/test; needs `QWERTTY_TERM_ASSERT_PRESENT`).
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
            s.send_pty(bytes);
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

    /// Drain a surface's engine reply queue (DSR/DA/CPR/title/color replies
    /// destined for the pty). Smoke/test only — the window-chrome smoke feeds a
    /// query sequence and asserts on (or on the absence of) the reply to verify
    /// a VT config toggle reached the engine.
    pub fn take_surface_reply(&self, tab: TabId, surface: SurfaceId) -> Vec<u8> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.engine().take_output())
            .unwrap_or_default()
    }

    /// Poison a surface's engine lock (smoke/test only) by spawning a thread that
    /// panics while holding it — reproducing the field-observed cascade where the
    /// io-reader/parse thread crashes mid-lock. The controller must survive: the
    /// next [`Controller::tick`] observes the poison, marks the surface dead,
    /// shuts its io down, and banners it, while every other pane keeps working.
    pub fn poison_surface_engine(&self, tab: TabId, surface: SurfaceId) {
        let engine = {
            let state = self.0.borrow();
            state
                .tabs
                .get(&tab)
                .and_then(|t| t.surfaces.get(&surface))
                .map(|s| Arc::clone(&s.engine))
        };
        if let Some(engine) = engine {
            let handle = std::thread::spawn(move || {
                let _guard = engine.lock().expect("engine lock");
                panic!("smoke: simulated parse-thread crash holding the engine lock");
            });
            // Join so the poison is in place before we return (the thread panics).
            let _ = handle.join();
        }
    }

    /// Whether a surface has been marked poison-dead (smoke/test only).
    pub fn surface_is_dead(&self, tab: TabId, surface: SurfaceId) -> Option<bool> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.is_dead())
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

    /// A surface's most-recent presented-frame mean luma (smoke/test only) —
    /// the dimming smoke asserts an unfocused pane's luma sits below its focused
    /// baseline. Needs `QWERTTY_TERM_ASSERT_PRESENT` (capture on).
    pub fn surface_present_luma(&self, tab: TabId, surface: SurfaceId) -> Option<f64> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.last_present_luma)
    }

    /// Whether a surface's view is hidden (smoke/test only) — a zoomed tab hides
    /// every non-zoomed pane.
    pub fn surface_is_hidden(&self, tab: TabId, surface: SurfaceId) -> Option<bool> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.view.isHidden())
    }

    /// The active tab's zoomed surface, if any (smoke/test only).
    pub fn active_zoomed_surface(&self) -> Option<SurfaceId> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        state.tabs.get(&tab).and_then(|t| t.tree.zoomed())
    }

    /// The pixel rect of a surface within its tab (device pixels) — the zoom
    /// smoke asserts a zoomed pane fills the whole container (smoke/test only).
    pub fn surface_rect(&self, tab: TabId, surface: SurfaceId) -> Option<crate::splits::Rect> {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.rect)
    }

    /// The active tab's whole container rect in device pixels (smoke/test only).
    pub fn active_container_rect(&self) -> Option<crate::splits::Rect> {
        let state = self.0.borrow();
        let tab = state.registry.active()?;
        let t = state.tabs.get(&tab)?;
        let scale = t.window.backingScaleFactor();
        let bounds = t.container.bounds();
        Some(crate::splits::Rect::new(
            0.0,
            0.0,
            bounds.size.width * scale,
            bounds.size.height * scale,
        ))
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
    /// Resolve a chord (physical key + mods) against the keybind [`Set`] and, if
    /// it maps to a tab/split/search action this seam handles, perform it.
    /// Returns `true` if the chord was consumed. This is the unified replacement
    /// for the bespoke `tabkeys`/`splitkeys`/`searchkeys` resolve tables: the
    /// lookup is the ported `Set` (`default_set()` + user config), the dispatch
    /// is [`Self::perform_keybind_chord`]. Called from `performKeyEquivalent:`.
    pub fn handle_keybind_chord(
        &self,
        tab: TabId,
        surface: SurfaceId,
        key: qwertty_term_input::key::Key,
        mods: crate::tabkeys::TabMods,
    ) -> bool {
        // Resolve under a short immutable borrow, then dispatch (the handlers
        // take their own borrows). A plain binding yields one action; a `chain=`
        // binding yields several, run in order.
        let actions = {
            let state = self.0.borrow();
            crate::keybind::resolve_actions(&state.keybinds, key, mods)
        };
        match actions.as_slice() {
            [] => false,
            // A single action: return whether it was performed, so menu-scoped
            // actions (which `dispatch_keybind_action` reports as not-performed)
            // fall through to the menu — preserving pre-chain behaviour.
            [single] => self.dispatch_keybind_action(tab, surface, single),
            // A chained binding is explicitly bound to this trigger: run every
            // action and consume the key.
            many => {
                for action in many {
                    self.dispatch_keybind_action(tab, surface, action);
                }
                true
            }
        }
    }

    /// Resolve `key`+`mods` against the keybind set while the search field is
    /// being edited, performing **only** the search-navigation chords (start /
    /// next / previous / end) and reporting whether one fired.
    ///
    /// This is the search-focused counterpart to [`Self::handle_keybind_chord`].
    /// Non-search chords — clipboard, tab, split, font, and leader sequences — are
    /// deliberately *not* performed here: while the field editor holds focus they
    /// must fall through to it so the search box behaves like a standard macOS
    /// text box (its own copy/paste, and the user's system Ctrl-emacs bindings).
    /// `end_search` keeps its self-gate on an open search so a stray Escape is a
    /// no-op rather than a false consume.
    pub fn handle_search_field_chord(
        &self,
        tab: TabId,
        key: qwertty_term_input::key::Key,
        mods: crate::tabkeys::TabMods,
    ) -> bool {
        use crate::searchkeys::SearchAction;
        use qwertty_term_input::binding::Action as A;
        use qwertty_term_input::binding::action::NavigateSearch;

        let actions = {
            let state = self.0.borrow();
            crate::keybind::resolve_actions(&state.keybinds, key, mods)
        };
        // Only a single, exact search chord is honoured while typing.
        match actions.as_slice() {
            [A::StartSearch] => {
                self.handle_search_action(tab, SearchAction::Start);
                true
            }
            [A::NavigateSearch(NavigateSearch::Next)] => {
                self.handle_search_action(tab, SearchAction::Next);
                true
            }
            [A::NavigateSearch(NavigateSearch::Previous)] => {
                self.handle_search_action(tab, SearchAction::Previous);
                true
            }
            [A::EndSearch] if self.active_search_is_active() => {
                self.handle_search_action(tab, SearchAction::End);
                true
            }
            _ => false,
        }
    }

    /// Map a resolved keybind [`Action`](qwertty_term_input::binding::Action) onto
    /// the existing tab/split/search handlers. Returns `true` if performed (and
    /// should be consumed); `false` for actions handled elsewhere (menu, byte
    /// actions, encoder) so `performKeyEquivalent:` falls through. A resolved
    /// tab/split/search chord is consumed regardless of whether it changed state
    /// (e.g. a tab switch with only one tab), matching the previous tables —
    /// except `end_search`, which falls through unless a search is open so a
    /// plain Escape still reaches the pty.
    fn perform_keybind_chord(
        &self,
        tab: TabId,
        action: &qwertty_term_input::binding::Action,
    ) -> bool {
        use crate::searchkeys::SearchAction;
        use qwertty_term_input::binding::Action as A;
        use qwertty_term_input::binding::action::{
            CloseTabMode, NavigateSearch, SplitDirection, SplitFocusDirection, SplitResizeDirection,
        };

        let split_dir = |d: SplitDirection| match d {
            // The bespoke tables had no "auto"; treat it as a right (vertical) split.
            SplitDirection::Right | SplitDirection::Auto => Direction::Right,
            SplitDirection::Down => Direction::Down,
            SplitDirection::Left => Direction::Left,
            SplitDirection::Up => Direction::Up,
        };

        match action {
            A::PreviousTab => {
                self.handle_tab_action(TabAction::PreviousTab);
                true
            }
            A::NextTab => {
                self.handle_tab_action(TabAction::NextTab);
                true
            }
            A::LastTab => {
                self.handle_tab_action(TabAction::LastTab);
                true
            }
            A::GotoTab(n) => {
                self.handle_tab_action(TabAction::GotoTab(*n));
                true
            }

            A::NewSplit(d) => {
                self.handle_split_action(tab, SplitAction::NewSplit(split_dir(*d)));
                true
            }
            A::GotoSplit(d) => {
                let sa = match d {
                    SplitFocusDirection::Previous => {
                        SplitAction::GotoAdjacent(Sequential::Previous)
                    }
                    SplitFocusDirection::Next => SplitAction::GotoAdjacent(Sequential::Next),
                    SplitFocusDirection::Up => SplitAction::GotoSplit(Direction::Up),
                    SplitFocusDirection::Down => SplitAction::GotoSplit(Direction::Down),
                    SplitFocusDirection::Left => SplitAction::GotoSplit(Direction::Left),
                    SplitFocusDirection::Right => SplitAction::GotoSplit(Direction::Right),
                };
                self.handle_split_action(tab, sa);
                true
            }
            A::ResizeSplit(rs) => {
                let dir = match rs.direction {
                    SplitResizeDirection::Up => Direction::Up,
                    SplitResizeDirection::Down => Direction::Down,
                    SplitResizeDirection::Left => Direction::Left,
                    SplitResizeDirection::Right => Direction::Right,
                };
                self.handle_split_action(tab, SplitAction::ResizeSplit(dir));
                true
            }
            A::ToggleSplitZoom => {
                self.handle_split_action(tab, SplitAction::ToggleZoom);
                true
            }
            A::EqualizeSplits => {
                self.handle_split_action(tab, SplitAction::EqualizeSplits);
                true
            }

            A::StartSearch => {
                self.handle_search_action(tab, SearchAction::Start);
                true
            }
            A::NavigateSearch(NavigateSearch::Next) => {
                self.handle_search_action(tab, SearchAction::Next);
                true
            }
            A::NavigateSearch(NavigateSearch::Previous) => {
                self.handle_search_action(tab, SearchAction::Previous);
                true
            }
            // Gate on an active search so a plain Escape still reaches the pty;
            // when no search is open this falls through to the `_` arm below.
            A::EndSearch if self.active_search_is_active() => {
                self.handle_search_action(tab, SearchAction::End);
                true
            }

            // Scrollback viewport moves (default `cmd`/`shift` + Home/PageUp/
            // PageDown/End). Route to the focused pane's `scrollback_offset`.
            A::ScrollToTop => {
                self.scroll_focused_surface(tab, crate::scroll::ScrollTo::Top);
                true
            }
            A::ScrollToBottom => {
                self.scroll_focused_surface(tab, crate::scroll::ScrollTo::Bottom);
                true
            }
            A::ScrollPageUp => {
                self.scroll_focused_surface(tab, crate::scroll::ScrollTo::PageUp);
                true
            }
            A::ScrollPageDown => {
                self.scroll_focused_surface(tab, crate::scroll::ScrollTo::PageDown);
                true
            }
            A::ScrollPageLines(n) => {
                self.scroll_focused_surface(tab, crate::scroll::ScrollTo::Lines(*n as i32));
                true
            }
            A::ScrollPageFractional(f) => {
                self.scroll_focused_surface(tab, crate::scroll::ScrollTo::Fraction(*f));
                true
            }

            // Clipboard (default `cmd+c` / `cmd+v`). Route to the same handlers
            // the Copy/Paste menu items use — so a user rebinding of these
            // actions works and paste-protection is preserved. We only have a
            // system clipboard (no separate primary selection on macOS), so the
            // `CopyToClipboard` mode param is ignored (plain text) and
            // `PasteFromSelection` falls through.
            A::CopyToClipboard(_) => {
                self.copy_selection_from_active();
                true
            }
            A::PasteFromClipboard => {
                self.paste_into_active();
                true
            }

            // Font size (default `cmd+=`/`cmd+-`/`cmd+0`). Our font-size model
            // steps by a fixed increment, so the upstream point delta folds to a
            // single step up/down.
            A::IncreaseFontSize(_) => {
                self.font_size_active(FontStep::Up);
                true
            }
            A::DecreaseFontSize(_) => {
                self.font_size_active(FontStep::Down);
                true
            }
            A::ResetFontSize => {
                self.font_size_active(FontStep::Reset);
                true
            }

            // Window / tab lifecycle. `CloseSurface` closes the focused pane
            // (a tmux pane → `kill-pane`); `CloseTab(this)` closes the whole tab
            // (a tmux tab → `kill-window`, gap 1). For an ordinary tab both
            // collapse to closing the focused pane (the last pane's collapse
            // closes the tab — today's model). `CloseTab(other/right)` isn't
            // supported yet and falls through.
            A::NewWindow => {
                self.new_window();
                true
            }
            A::NewTab => {
                self.new_tab_in(tab);
                true
            }
            A::CloseSurface => {
                self.close_focused_confirmed(tab);
                true
            }
            A::CloseTab(CloseTabMode::This) => {
                self.close_tab_confirmed(tab);
                true
            }
            A::ToggleQuickTerminal => {
                self.toggle_quick_terminal();
                true
            }
            A::ToggleFullscreen => {
                self.toggle_fullscreen(tab);
                true
            }

            // Dump the focused pane's scrollback / viewport / selection to a temp
            // file and copy/paste/open its path.
            A::WriteScrollbackFile(ws) => {
                if let Some(text) = self.focused_engine_text(tab, |e| Some(e.scrollback_string())) {
                    self.write_screen_action(tab, *ws, &text, "scrollback");
                }
                true
            }
            A::WriteScreenFile(ws) => {
                if let Some(text) = self.focused_engine_text(tab, |e| Some(e.screen_dump())) {
                    self.write_screen_action(tab, *ws, &text, "screen");
                }
                true
            }
            A::WriteSelectionFile(ws) => {
                if let Some(text) = self.focused_engine_text(tab, |e| e.selection_string()) {
                    self.write_screen_action(tab, *ws, &text, "selection");
                }
                true
            }

            // Re-read the config from disk and re-apply the runtime-safe settings
            // (default `cmd+shift+,`).
            A::ReloadConfig => {
                self.reload_config();
                true
            }

            // Everything else (menu actions, byte actions, inactive end_search,
            // unhandled) falls through so the menu / keyDown-encoder path handles
            // it.
            _ => false,
        }
    }

    /// Re-read the user config from disk and re-apply the settings that are safe
    /// to change without rebuilding surfaces: the keybind `Set`, copy-on-select,
    /// the scroll multiplier, and the **theme** (palette + default fg/bg/cursor +
    /// selection colors, pushed live into every surface's engine with an
    /// immediate repaint). Bound to the `reload_config` action (default
    /// `cmd+shift+,`).
    ///
    /// Not yet re-applied here (need the font grid / window rebuild —
    /// follow-up slices, see `docs/analysis/config-core.md` §7): fonts (family/
    /// size), cursor style, and window padding. Those take effect on restart or
    /// for new surfaces until wired.
    pub fn reload_config(&self) {
        let config = crate::config::load();

        // Re-resolve the theme + cursor-color override (same derivation as
        // `Controller::new`).
        let (startup_colors, selection_colors) = resolve_colors(&config);

        let mut state = self.0.borrow_mut();
        state.keybinds = crate::keybind::build_set(&config.keybind);
        state.copy_on_select = config.copy_on_select;
        state.scroll_multiplier = crate::scroll::ScrollMultiplier {
            precision: config.mouse_scroll_multiplier.precision,
            discrete: config.mouse_scroll_multiplier.discrete,
        }
        .clamped();

        // Apply the theme colors to the controller defaults (used by future
        // surfaces) and live to every existing surface's engine (which marks the
        // screen dirty so the palette swap repaints immediately).
        state.startup_colors = startup_colors.clone();
        state.selection_colors = selection_colors;
        // Refresh the `adjust-*` metric nudges for future surfaces; existing
        // surfaces pick up any change on their next font rebuild (a font-size
        // change), so stash them per-surface too. (A live re-metric of existing
        // panes needs the font-reload path — still a restart today.)
        let metric_modifiers = config.metric_modifiers();
        state.metric_modifiers = metric_modifiers.clone();
        // The forced-title override re-applies to every tab on the next tick.
        state.forced_title = config.forced_title().map(str::to_owned);
        // Window/tab chrome policies. `window-subtitle` re-applies on the next
        // pace-tick title sync; `window-new-tab-position` affects the next new
        // tab; `window-show-tab-bar` drives the next new window's tabbing mode
        // (existing windows keep their mode until recreated, like other
        // window-creation-time settings).
        state.window_subtitle = config.window_subtitle();
        state.window_new_tab_position = config.window_new_tab_position();
        state.window_show_tab_bar = config.window_show_tab_bar();
        state.window_step_resize = config.window_step_resize;
        state.macos_window_shadow = config.macos_window_shadow;
        state.macos_window_buttons = config.macos_window_buttons();
        state.window_theme = config.window_theme();
        // VT config toggles. The four live setters re-apply to every existing
        // surface's engine below; `scrollback-limit` is construction-only, so a
        // reload only affects new surfaces (upstream `Config.zig:1387`);
        // `vt-kam-allowed` is read per-keystroke, so updating the field suffices.
        state.title_report = config.title_report;
        state.enquiry_response = config.enquiry_response_bytes().to_vec();
        state.osc_color_report_format = config.osc_color_report_format();
        state.image_storage_limit = config.image_storage_limit as usize;
        state.scrollback_limit = config.scrollback_limit;
        state.vt_kam_allowed = config.vt_kam_allowed;
        let vt_toggles = VtToggles {
            title_report: state.title_report,
            enquiry_response: state.enquiry_response.clone(),
            osc_color_report_format: state.osc_color_report_format,
            image_storage_limit: state.image_storage_limit,
            scrollback_limit: state.scrollback_limit,
        };
        // `window-theme` re-applies live to every existing window (appearance is
        // freely settable at runtime).
        let theme = state.window_theme;
        for tab in state.tabs.values() {
            let bg = tab
                .focused_surface()
                .map(|s| s.default_bg)
                .unwrap_or((0x18, 0x18, 0x18));
            apply_window_theme(&tab.window, theme, bg);
        }
        for tab in state.tabs.values_mut() {
            for surface in tab.surfaces.values_mut() {
                surface.selection_colors = selection_colors;
                surface.metric_modifiers = metric_modifiers.clone();
                {
                    let (mut engine, _recovered) = lock_or_recover(&surface.engine);
                    engine.set_colors(startup_colors.clone());
                    vt_toggles.apply(&mut engine);
                }
                // A control surface's tmux panes render their own Viewer-owned
                // terminals, not this engine — recolor those too, or a theme
                // reload leaves the tmux panes at their create-time colors.
                if let Some(sess) = surface.tmux.as_mut() {
                    sess.recolor_panes(&startup_colors);
                }
            }
        }

        eprintln!(
            "qwertty-term: config reloaded (keybinds, copy-on-select, scroll-multiplier, \
             theme/colors; fonts still need a restart until wired)"
        );
    }

    /// Feed a key to the leader-key sequence state machine (`ctrl+a>c`-style
    /// bindings). Returns `true` if the key was consumed by sequence handling:
    /// it started a sequence (a leader key), continued one (a further leader),
    /// completed one (dispatching its action), or aborted one (an unrecognized
    /// key mid-sequence is swallowed). Returns `false` when the key is not
    /// sequence-related, so normal single-key handling proceeds. Called first in
    /// `performKeyEquivalent:`.
    ///
    /// Deferred vs upstream (follow-ups): the idle **timeout** that auto-cancels a
    /// dangling leader, and **flush-on-abort** (upstream replays the buffered keys
    /// to the terminal; we drop them).
    pub fn handle_key_sequence(
        &self,
        tab: TabId,
        surface: SurfaceId,
        key: qwertty_term_input::key::Key,
        mods: crate::tabkeys::TabMods,
    ) -> bool {
        use crate::keybind::SeqStep;

        // Resolve the step against the current path under a short borrow.
        let (step, in_sequence) = {
            let state = self.0.borrow();
            let path = state.key_sequence.as_deref().unwrap_or(&[]);
            (
                crate::keybind::sequence_step(&state.keybinds, path, key, mods),
                state.key_sequence.is_some(),
            )
        };

        match step {
            // A leader key: enter a new sequence (path empty) or descend an
            // existing one. Either way, consume and wait for the next key.
            SeqStep::Descend(trigger) => {
                self.0
                    .borrow_mut()
                    .key_sequence
                    .get_or_insert_with(Vec::new)
                    .push(trigger);
                true
            }
            // Completing key of an in-progress sequence: dispatch its action(s)
            // (a `chain=` leaf runs several, in order) and end the sequence.
            SeqStep::Leaf(actions) if in_sequence => {
                self.0.borrow_mut().key_sequence = None;
                for action in &actions {
                    self.dispatch_keybind_action(tab, surface, action);
                }
                true
            }
            // Unrecognized key mid-sequence: abort (and swallow the key).
            SeqStep::NoMatch if in_sequence => {
                self.0.borrow_mut().key_sequence = None;
                true
            }
            // Not in a sequence and not a leader — a top-level single-key binding
            // (or nothing). Hand off to the normal single-key path, which routes
            // menu-scoped actions to the menu and byte actions via keyDown.
            SeqStep::Leaf(_) | SeqStep::NoMatch => false,
        }
    }

    /// Dispatch an action resolved from a completed leader sequence: byte actions
    /// (`text:`/`esc:`/`csi:`) write their bytes to the focused surface's pty;
    /// chord actions go through [`Self::perform_keybind_chord`]. Menu-scoped
    /// actions are not dispatched from here yet (they return `false`).
    fn dispatch_keybind_action(
        &self,
        tab: TabId,
        surface: SurfaceId,
        action: &qwertty_term_input::binding::Action,
    ) -> bool {
        if let Some(bytes) = crate::keybind::action_bytes(action) {
            let mut state = self.0.borrow_mut();
            if let Some(s) = state
                .tabs
                .get_mut(&tab)
                .and_then(|t| t.surfaces.get_mut(&surface))
            {
                s.snap_to_bottom();
                s.send_pty(&bytes);
            }
            return true;
        }
        self.perform_keybind_chord(tab, action)
    }

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

    /// If the user has a `text:` keybind for `key`+`mods`, send its literal
    /// bytes to `surface`'s pty and return `true` (consuming the key so it never
    /// reaches the encoder). Returns `false` if no binding matches, so the caller
    /// proceeds to normal encoding.
    ///
    /// This is the user-keybind counterpart to [`Self::handle_tab_action`] /
    /// [`Self::handle_split_action`]: those intercept built-in nav chords in
    /// `performKeyEquivalent:`; this intercepts arbitrary user `text:` chords in
    /// the `keyDown:` path, BEFORE `interpretKeyEvents` / the encoder, so e.g.
    /// `shift+enter=text:\x1b\r` sends ESC CR instead of the encoder's plain CR.
    /// Sending bytes snaps the pane's viewport to the live area, same as a
    /// normal keystroke.
    pub fn try_text_keybind_to_surface(
        &self,
        tab: TabId,
        surface: SurfaceId,
        key: qwertty_term_input::key::Key,
        mods: crate::tabkeys::TabMods,
    ) -> bool {
        let mut state = self.0.borrow_mut();
        let Some(bytes) = crate::keybind::resolve_text_bytes(&state.keybinds, key, mods) else {
            return false;
        };
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            s.snap_to_bottom();
            s.send_pty(&bytes);
        }
        true
    }

    /// Encode a raw key event and write it to `surface`'s PTY (within `tab`).
    /// Input isolation: only the focused pane's view is first responder, so this
    /// only ever fires for the pane the user is looking at.
    pub fn encode_key_to_surface(&self, tab: TabId, surface: SurfaceId, raw: &RawKeyEvent) {
        let mut state = self.0.borrow_mut();
        let cfg = state.input_config;
        let clear_on_typing = state.selection_clear_on_typing;
        let vt_kam_allowed = state.vt_kam_allowed;
        // Encode against the target surface. For a display-only tmux pane the
        // encoded bytes can't go to a pty (it has none) — they are returned here
        // and routed to tmux below, after the surface borrow drops.
        let routed: Option<(DisplaySource, Vec<u8>)> = {
            let Some(s) = state
                .tabs
                .get_mut(&tab)
                .and_then(|t| t.surfaces.get_mut(&surface))
            else {
                return;
            };
            // KAM (ANSI mode 2): when `vt-kam-allowed` and the program has
            // enabled `disable_keyboard`, drop the keystroke entirely — the
            // program has asked the terminal to stop accepting keyboard input
            // (upstream `Surface.zig:2699`). Keybindings already ran upstream in
            // `performKeyEquivalent:`, so this only suppresses the encoder path.
            if vt_kam_allowed && s.engine().keyboard_disabled() {
                return;
            }
            let opts = s.engine().key_encode_options();
            let bytes = crate::input::translate::encode_raw(raw, &cfg, opts);
            if bytes.is_empty() {
                return;
            }
            // A key that produced bytes snaps the viewport back to the live
            // area (upstream `scroll-to-bottom.keystroke`, default on). Only
            // scrolled-back panes are affected; this pane is the focused
            // (first-responder) one that received the key.
            s.snap_to_bottom();
            // `selection-clear-on-typing`: a real keystroke drops any
            // selection (upstream clears it on typed input).
            if clear_on_typing {
                s.engine().clear_selection();
                s.gesture.reset();
            }
            match s.display_source() {
                None => {
                    s.send_pty(&bytes);
                    None
                }
                // A display-only tmux pane surface: route below.
                Some(src) => Some((src, bytes)),
            }
        };
        if let Some((src, bytes)) = routed {
            self.route_keys_to_tmux(&mut state, src, surface, &bytes);
        }
    }

    /// Deliver already-encoded key bytes for a display-only tmux pane surface to
    /// its tmux session (ADR 006 slice 5d). The pane has no pty; the control
    /// surface's [`TmuxSession`] turns the bytes into a `send-keys` control
    /// command, which is written to the control pty (control mode is in-band).
    /// `pane_surface` is the display-only surface's own id (the Reconciler's
    /// stable pane→surface key). No-op if the control surface/session is gone.
    fn route_keys_to_tmux(
        &self,
        state: &mut ControllerState,
        src: DisplaySource,
        pane_surface: SurfaceId,
        bytes: &[u8],
    ) {
        if let Some(cs) = state
            .tabs
            .get_mut(&src.control_tab)
            .and_then(|t| t.surfaces.get_mut(&src.control_surface))
        {
            let commands = cs
                .tmux
                .as_mut()
                .map(|sess| sess.send_keys(pane_surface, bytes))
                .unwrap_or_default();
            for cmd in &commands {
                cs.send_pty(cmd);
            }
        }
    }

    /// The display source of `surface` in `tab`, if it is a display-only tmux
    /// pane surface — i.e. `tab` is a **tmux-managed tab** whose split layout the
    /// control session's Viewer owns and reconciles (ADR 006 slice 5e). `Some`
    /// means every native mutation of this tab's tree (split / close / new-tab)
    /// must be redirected to a tmux control command instead, or it fights the
    /// next `%layout-change` reconcile. `None` is an ordinary pane — unchanged.
    fn tmux_pane_of(&self, tab: TabId, surface: SurfaceId) -> Option<DisplaySource> {
        self.0
            .borrow()
            .tabs
            .get(&tab)?
            .surfaces
            .get(&surface)?
            .display_source()
    }

    /// Redirect a native window action on a tmux pane surface to its tmux
    /// control-command equivalent (ADR 006 slice 5e). `src` locates the control
    /// surface running `tmux -CC`; `pane_surface` is the tmux pane surface's own
    /// id (the Reconciler's stable `pane_id → SurfaceId` key). The command is
    /// written to the control pty (control mode is in-band); the resulting
    /// `%layout-change` / `%window-add` / `%window-close` reconcile applies the
    /// native effect (create/remove the split or tab). No-op if the control
    /// surface/session is gone.
    fn redirect_tmux_action(
        &self,
        src: DisplaySource,
        pane_surface: SurfaceId,
        action: TmuxNativeAction,
    ) {
        let mut state = self.0.borrow_mut();
        if let Some(cs) = state
            .tabs
            .get_mut(&src.control_tab)
            .and_then(|t| t.surfaces.get_mut(&src.control_surface))
        {
            let commands = cs
                .tmux
                .as_mut()
                .map(|sess| match action {
                    TmuxNativeAction::Split { horizontal, before } => {
                        sess.split_pane(pane_surface, horizontal, before)
                    }
                    TmuxNativeAction::NewWindow => sess.new_window(),
                    TmuxNativeAction::KillPane => sess.kill_pane(pane_surface),
                    TmuxNativeAction::SelectPane => sess.select_pane(pane_surface),
                })
                .unwrap_or_default();
            for cmd in &commands {
                cs.send_pty(cmd);
            }
        }
    }

    /// Redirect a native **tab** close on a tmux-managed tab to a tmux
    /// `kill-window` control command (ADR 006 slice 5e — gap 1). `control`
    /// locates the `tmux -CC` control surface; `window_id` is the tmux window
    /// the native tab mirrors (its key in [`ControllerState::tmux_tabs`]). The
    /// command is written to the control pty; the native tab is removed by the
    /// follow-up `list-windows` reconcile (I3) — the caller must NOT close the
    /// native tab directly. No-op if the control surface/session is gone.
    fn redirect_tmux_kill_window(&self, control: DisplaySource, window_id: usize) {
        let mut state = self.0.borrow_mut();
        if let Some(cs) = state
            .tabs
            .get_mut(&control.control_tab)
            .and_then(|t| t.surfaces.get_mut(&control.control_surface))
        {
            let commands = cs
                .tmux
                .as_mut()
                .map(|sess| sess.kill_window(window_id))
                .unwrap_or_default();
            for cmd in &commands {
                cs.send_pty(cmd);
            }
        }
    }

    /// Detach the `tmux -CC` client hosted by `control`'s control surface (ADR
    /// 006 slice 5e — orphan teardown / I1). Writes `detach-client` to the
    /// control pty so the client exits and the surface returns to a plain shell,
    /// even under `detach-on-destroy off`. Called from the reconcile when the
    /// last tmux window closes. No-op if the control surface/session is gone.
    fn detach_tmux_control(&self, control: DisplaySource) {
        let mut state = self.0.borrow_mut();
        if let Some(cs) = state
            .tabs
            .get_mut(&control.control_tab)
            .and_then(|t| t.surfaces.get_mut(&control.control_surface))
        {
            let commands = cs
                .tmux
                .as_mut()
                .map(|sess| sess.detach_client())
                .unwrap_or_default();
            for cmd in &commands {
                cs.send_pty(cmd);
            }
        }
    }

    /// If `tab` is a **tmux-window tab** (it mirrors a tmux window under some
    /// live control session), return `(control_tab, control_surface,
    /// window_id)` — everything the window-close delegate needs to redirect a
    /// tab close to `kill-window` (gap 1). `None` for an ordinary tab or the
    /// control tab itself.
    /// Whether `tab` mirrors a tmux window — derived *structurally* from its
    /// surfaces (any display-only tmux pane), not from the
    /// [`tmux_tabs`](ControllerState::tmux_tabs) registry. The close paths use
    /// this as a backstop: a tmux-managed tab must never fall through to the
    /// ordinary close-window path, which shows the wrong dialog and can tear the
    /// whole session down instead of closing one tab. The registry is populated
    /// before a tab goes live, so this should agree with it — but a structural
    /// check cannot be defeated by a future ordering regression.
    fn is_tmux_managed_tab(&self, tab: TabId) -> bool {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .is_some_and(|t| t.surfaces.values().any(|s| s.display_source().is_some()))
    }

    fn tmux_window_tab(&self, tab: TabId) -> Option<(TabId, SurfaceId, usize)> {
        let state = self.0.borrow();
        for (&(ctrl_tab, ctrl_surface), map) in state.tmux_tabs.iter() {
            for (&window_id, &native) in map.iter() {
                if native == tab {
                    return Some((ctrl_tab, ctrl_surface, window_id));
                }
            }
        }
        None
    }

    /// If `tab` hosts a live `tmux -CC` **control surface**, return that
    /// surface's id (gap 4 — window-close teardown). `None` if no surface in the
    /// tab is running control mode.
    fn tmux_control_surface_of(&self, tab: TabId) -> Option<SurfaceId> {
        let state = self.0.borrow();
        let t = state.tabs.get(&tab)?;
        t.surfaces
            .iter()
            .find_map(|(sid, s)| s.tmux_session().is_some().then_some(*sid))
    }

    /// Tear down the tmux session hosted by control surface `(control_tab,
    /// control_surface)` (ADR 006 slice 5e — gap 4). Closes every native
    /// tmux-window tab mirroring the session so none are orphaned when the
    /// control window closes, and clears the hidden-control bookkeeping. The
    /// control surface's own pty close detaches the `tmux -CC` client (tmux
    /// keeps the detached session on its server — a clean detach, not an
    /// orphaned server or a zombie). Idempotent.
    fn teardown_tmux_control(&self, control_tab: TabId, control_surface: SurfaceId) {
        let key = (control_tab, control_surface);
        let tabs: Vec<TabId> = self
            .0
            .borrow_mut()
            .tmux_tabs
            .remove(&key)
            .map(|m| m.into_values().collect())
            .unwrap_or_default();
        for tab in tabs {
            self.close_tab(tab);
        }
        self.0
            .borrow_mut()
            .tmux_hidden_controls
            .remove(&control_tab);
    }

    /// Send already-composed text (IME commit) to `surface`'s pty. Committed
    /// text is user input, so it snaps this pane's viewport to the bottom.
    pub fn send_text_to_surface(&self, tab: TabId, surface: SurfaceId, text: &str) {
        let mut state = self.0.borrow_mut();
        let clear_on_typing = state.selection_clear_on_typing;
        let vt_kam_allowed = state.vt_kam_allowed;
        let routed: Option<DisplaySource> = {
            let Some(s) = state
                .tabs
                .get_mut(&tab)
                .and_then(|t| t.surfaces.get_mut(&surface))
            else {
                return;
            };
            // KAM: committed text is keyboard input; suppress it while the
            // program has disabled the keyboard (see `encode_key_to_surface`).
            if vt_kam_allowed && s.engine().keyboard_disabled() {
                return;
            }
            s.snap_to_bottom();
            // `selection-clear-on-typing`: committed text is typed input.
            if clear_on_typing {
                s.engine().clear_selection();
                s.gesture.reset();
            }
            match s.display_source() {
                None => {
                    s.send_pty(text.as_bytes());
                    None
                }
                // Display-only tmux pane surface: route the text to tmux below.
                Some(src) => Some(src),
            }
        };
        if let Some(src) = routed {
            self.route_keys_to_tmux(&mut state, src, surface, text.as_bytes());
        }
    }

    /// Handle a mouse event on a **display-only tmux pane** (ADR 006 slice 5d):
    /// drive left-button selection directly on the Viewer's pane `Terminal`,
    /// since this surface's own engine is empty. Returns `true` if `surface` is
    /// a display pane and the event was consumed — the caller then skips the
    /// normal engine-selection / mouse-reporting path. `false` for an ordinary
    /// pane. Selection is linear (or rectangular with option held); word/line
    /// double-click and mouse *reporting* into the pane remain future work.
    #[allow(clippy::too_many_arguments)]
    fn tmux_pane_mouse(
        &self,
        tab: TabId,
        surface: SurfaceId,
        action: qwertty_term_input::mouse::Action,
        button: Option<qwertty_term_input::mouse::Button>,
        mods: qwertty_term_input::key_mods::Mods,
        x: f32,
        y: f32,
        pressed: Option<bool>,
        copy_on_select: bool,
    ) -> bool {
        use qwertty_term_input::mouse::{Action, Button};
        enum Op {
            Clear,
            Extend {
                anchor: (usize, usize, usize),
                current: (usize, usize, usize),
                rectangle: bool,
            },
            Release,
        }
        use qwertty_term_input::mouse_encode::{MouseEvent, MouseFormat};
        let mut state = self.0.borrow_mut();

        // Step 0: mouse REPORTING. If the pane's program has mouse tracking on
        // (read off the Viewer's pane terminal — this surface's engine is empty)
        // and shift isn't held (shift forces local selection, matching the normal
        // path), encode the event against the pane's mode and deliver it to the
        // pane as `send-keys` instead of selecting. Covers any button, so a
        // mouse-driven TUI (vim, htop, …) inside a pane receives clicks/drags.
        let Some((src, cols, rows, cell_w, cell_h, button_down)) = (|| {
            let s = state
                .tabs
                .get_mut(&tab)
                .and_then(|t| t.surfaces.get_mut(&surface))?;
            let src = s.display_source()?; // ordinary pane — not handled here.
            if let Some(p) = pressed {
                s.mouse_button_down = p;
            }
            Some((
                src,
                s.cols,
                s.rows,
                s.font.cell_width,
                s.font.cell_height,
                s.mouse_button_down,
            ))
        })() else {
            return false;
        };
        let (event_mode, format) = state
            .tabs
            .get(&src.control_tab)
            .and_then(|ct| ct.surfaces.get(&src.control_surface))
            .and_then(|cs| cs.tmux_session())
            .and_then(|sess| sess.pane_terminal(surface))
            .map(|t| {
                (
                    crate::engine::terminal_mouse_event(t),
                    crate::engine::terminal_mouse_format(t),
                )
            })
            .unwrap_or((MouseEvent::None, MouseFormat::X10));
        if event_mode != MouseEvent::None && !mods.shift {
            let bytes = {
                let Some(s) = state
                    .tabs
                    .get_mut(&tab)
                    .and_then(|t| t.surfaces.get_mut(&surface))
                else {
                    return true;
                };
                let ctx = crate::input::mouse::MouseContext {
                    event_mode,
                    format,
                    screen_width: (cols * cell_w as usize) as f64,
                    screen_height: (rows * cell_h as usize) as f64,
                    cell_width: cell_w as f64,
                    cell_height: cell_h as f64,
                    any_button_pressed: button_down,
                };
                crate::input::mouse::encode(
                    action,
                    button,
                    mods,
                    x,
                    y,
                    &ctx,
                    &mut s.last_mouse_cell,
                )
            };
            if !bytes.is_empty() {
                self.route_keys_to_tmux(&mut state, src, surface, &bytes);
            }
            // Reporting owns the pointer: drop any local selection + drag state.
            if let Some(pt) = state
                .tabs
                .get_mut(&src.control_tab)
                .and_then(|t| t.surfaces.get_mut(&src.control_surface))
                .and_then(|cs| cs.tmux.as_mut())
                .and_then(|sess| sess.pane_terminal_mut(surface))
            {
                pt.clear_selection();
            }
            if let Some(s) = state
                .tabs
                .get_mut(&tab)
                .and_then(|t| t.surfaces.get_mut(&surface))
            {
                s.pane_sel_anchor = None;
            }
            return true;
        }

        // Step 1: on the display pane, update the drag anchor / button state and
        // decide the op. Bail (not handled) for an ordinary pty pane.
        let (src, op) = {
            let Some(s) = state
                .tabs
                .get_mut(&tab)
                .and_then(|t| t.surfaces.get_mut(&surface))
            else {
                return false;
            };
            let Some(src) = s.display_source() else {
                return false; // ordinary pane — not handled here.
            };
            if let Some(p) = pressed {
                s.mouse_button_down = p;
            }
            // Only the left button drives selection; still consume every other
            // button/event on a display pane (no pty to report to; hover/links
            // use the empty engine) so the normal path is fully bypassed.
            if button != Some(Button::Left) {
                return true;
            }
            let (col, row) = s.cell_at_clamped(x, y);
            let cur = (col, row, s.scrollback_offset);
            let op = match action {
                Action::Press => {
                    s.pane_sel_anchor = Some(cur);
                    Op::Clear
                }
                Action::Motion => {
                    if !s.mouse_button_down {
                        return true;
                    }
                    match s.pane_sel_anchor {
                        Some(anchor) => Op::Extend {
                            anchor,
                            current: cur,
                            rectangle: mods.alt,
                        },
                        None => return true,
                    }
                }
                Action::Release => {
                    s.pane_sel_anchor = None;
                    Op::Release
                }
            };
            (src, op)
        };
        // Step 2: apply the op to the Viewer's pane terminal (a separate
        // surface, so this is a sequential — not aliasing — borrow).
        let Some(pt) = state
            .tabs
            .get_mut(&src.control_tab)
            .and_then(|t| t.surfaces.get_mut(&src.control_surface))
            .and_then(|cs| cs.tmux.as_mut())
            .and_then(|sess| sess.pane_terminal_mut(surface))
        else {
            return true;
        };
        match op {
            Op::Clear => pt.clear_selection(),
            Op::Extend {
                anchor,
                current,
                rectangle,
            } => {
                if let (Some(a), Some(b)) = (
                    pt.window_to_screen_point(anchor.0, anchor.1, anchor.2),
                    pt.window_to_screen_point(current.0, current.1, current.2),
                ) {
                    pt.select_screen_points(a, b, rectangle);
                }
            }
            Op::Release => {
                if copy_on_select && let Some(text) = pt.selection_text(true) {
                    crate::clipboard::write(&text);
                }
            }
        }
        true
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
        action: qwertty_term_input::mouse::Action,
        button: Option<qwertty_term_input::mouse::Button>,
        mods: qwertty_term_input::key_mods::Mods,
        x: f32,
        y: f32,
        pressed: Option<bool>,
    ) {
        let (copy_on_select, mouse_interval, mouse_shift_capture) = {
            let s = self.0.borrow();
            (s.copy_on_select, s.mouse_interval, s.mouse_shift_capture)
        };
        // A display-only tmux pane routes selection to the Viewer's pane
        // terminal (its own engine is empty); handle it there and bypass the
        // normal engine-selection / mouse-reporting path below.
        if self.tmux_pane_mouse(
            tab,
            surface,
            action,
            button,
            mods,
            x,
            y,
            pressed,
            copy_on_select,
        ) {
            return;
        }
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

        // R7: track which cell the mouse is over so the renderer can underline
        // a hovered OSC8 link. On any motion, update `hovered_cell` (`None` when
        // outside the grid); the render pace tick reads it and forces a rebuild
        // when it changes. Independent of mouse reporting / selection.
        if action == qwertty_term_input::mouse::Action::Motion {
            let hovered = s.cell_at(x, y);
            s.hovered_cell = hovered;
        }

        // R7 slice 3: cmd+click opens the link under the pointer (OSC8 or a
        // regex-detected URL) and consumes the event, so it doesn't also start a
        // selection or send a mouse report. Matches the hover-underline affordance.
        if action == qwertty_term_input::mouse::Action::Press
            && button == Some(qwertty_term_input::mouse::Button::Left)
            && mods.super_
            && let Some((col, row)) = s.cell_at(x, y)
        {
            let url = {
                let engine = s.engine();
                let snap = engine.snapshot_window(s.scrollback_offset);
                url_at_cell(&snap, col, row)
            };
            if let Some(url) = url {
                open_url(&url);
                return;
            }
        }

        if button == Some(qwertty_term_input::mouse::Button::Left) {
            let reporting_active =
                s.engine().mouse_event() != qwertty_term_input::mouse_encode::MouseEvent::None;
            // Shift overrides mouse reporting for selection, unless
            // `mouse-shift-capture` (combined with the program's XTSHIFTESCAPE
            // request) lets the program capture shift instead (upstream
            // `Surface.zig:3788-3790` + `mouseShiftCapture`).
            let shift_captured = mouse_shift_capture.captures(s.engine().mouse_shift_capture());
            let selection_allowed = !reporting_active || (mods.shift && !shift_captured);
            if selection_allowed {
                match action {
                    qwertty_term_input::mouse::Action::Press => {
                        s.selection_press(mods, x, y, mouse_interval);
                    }
                    qwertty_term_input::mouse::Action::Motion => {
                        if s.mouse_button_down {
                            // Rectangle select: option held (macOS —
                            // `surface_mouse.zig:121-126`).
                            s.selection_drag(x, y, mods.alt);
                        }
                    }
                    qwertty_term_input::mouse::Action::Release => {
                        s.selection_release(x, y, copy_on_select);
                    }
                }
            } else if action == qwertty_term_input::mouse::Action::Press {
                // Mouse reporting captured the press: the program owns the
                // pointer, so clear the selection and the click sequence
                // (upstream `Surface.zig:3908-3915`).
                s.engine().clear_selection();
                s.gesture.reset();
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
            s.send_pty(&bytes);
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
        mods: qwertty_term_input::key_mods::Mods,
    ) {
        let mult = self.0.borrow().scroll_multiplier;
        // A display-only tmux pane's own engine is empty, so it can't clamp the
        // scrollback viewport — the real history lives in the Viewer's pane
        // `Terminal`. Resolve its scrollback length (read borrow) and hand it to
        // `apply_wheel` as the clamp bound; `None` for an ordinary pty pane.
        let display_max = {
            let state = self.0.borrow();
            state
                .tabs
                .get(&tab)
                .and_then(|t| t.surfaces.get(&surface))
                .and_then(|s| s.display_source())
                .and_then(|src| {
                    state
                        .tabs
                        .get(&src.control_tab)
                        .and_then(|ct| ct.surfaces.get(&src.control_surface))
                        .and_then(|cs| cs.tmux_session())
                        .and_then(|sess| sess.pane_terminal(surface))
                        .map(|term| term.scrollback_len())
                })
        };
        let mut state = self.0.borrow_mut();
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            s.apply_wheel(yoff, precision, mods, mult, display_max);
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
        let (view, transitioned) = {
            let mut state = self.0.borrow_mut();
            let Some(t) = state.tabs.get_mut(&tab) else {
                return;
            };
            // Per-pane focus reporting (app-hardening): remember the outgoing
            // pane so we can send it focus-OUT and the incoming pane focus-IN.
            let previous = t.tree.focused();
            if !t.tree.focus(surface) {
                return;
            }
            let transitioned = previous != surface;
            // A real transition (different pane) drives the per-surface focus
            // change: the newly-focused pane gets `focus(true)` (mode-1004 CSI I
            // + password poll on), the previous one `focus(false)` (CSI O +
            // poll off). Same-pane re-focus is a no-op for reporting.
            if transitioned {
                if let Some(prev) = t.surfaces.get_mut(&previous) {
                    prev.set_focus(false);
                }
                if let Some(next) = t.surfaces.get_mut(&surface) {
                    next.set_focus(true);
                }
            }
            (t.focused_surface().map(|s| s.view.clone()), transitioned)
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
        // app→tmux focus sync (ADR 006 slice 5e): a real focus transition onto a
        // tmux pane makes it tmux's active pane, so bare `split-window` and the
        // active-pane indicator operate on the pane the user is actually in.
        if transitioned && let Some(src) = self.tmux_pane_of(tab, surface) {
            self.redirect_tmux_action(src, surface, TmuxNativeAction::SelectPane);
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
                    self.close_tab_confirmed(active);
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
            MenuAction::ToggleQuickTerminal => {
                self.toggle_quick_terminal();
            }
            MenuAction::Quit => {
                let mtm = self.0.borrow().mtm;
                NSApplication::sharedApplication(mtm).terminate(None);
            }
        }
    }

    /// Copy the *focused* pane's current selection to the system clipboard
    /// (Cmd-C). No-op if there is no selection. Reuses the hardened per-surface
    /// path (honors `clipboard-trim-trailing-spaces`).
    fn copy_selection_from_active(&self) {
        let Some(tab) = self.active_tab() else { return };
        let Some(surface) = self.0.borrow().tabs.get(&tab).map(|t| t.tree.focused()) else {
            return;
        };
        self.copy_surface_selection(tab, surface);
    }

    /// Paste the clipboard into the *focused* pane's PTY (Cmd-V). Reuses the
    /// hardened per-surface path (honors `clipboard-paste-protection`).
    fn paste_into_active(&self) {
        let Some(tab) = self.active_tab() else { return };
        let Some(surface) = self.0.borrow().tabs.get(&tab).map(|t| t.tree.focused()) else {
            return;
        };
        self.paste_into_surface(tab, surface);
    }

    // -- right-click / mouse behaviors ------------------------------------

    /// Whether the app should quit after the last window/surface closes
    /// (`quit-after-last-window-closed`).
    pub fn quit_after_last_window_closed(&self) -> bool {
        self.0.borrow().quit_after_last_window_closed
    }

    /// Whether the active tab's window is marked restorable — the observable
    /// effect of `window-save-state` (smoke/test).
    pub fn active_window_is_restorable(&self) -> Option<bool> {
        let state = self.0.borrow();
        let tab = self.active_tab()?;
        Some(state.tabs.get(&tab)?.window.isRestorable())
    }

    /// The configured initial window size in cells (`window-width`/`-height`),
    /// or `None` when unset (smoke/test).
    pub fn configured_initial_cells(&self) -> Option<(u32, u32)> {
        self.0.borrow().initial_window_cells
    }

    /// Deliver one desktop notification (already gated + throttled). macOS
    /// notifications proper (`UNUserNotificationCenter`) require a signed app
    /// bundle + runtime authorization, which this CLI-launched binary does not
    /// have (see ADR 0003). Until the app ships as a bundle we use the
    /// unbundled-safe fallback: bounce the Dock (informational attention) and
    /// log the title/body. The delivered `(title, body)` is recorded for the
    /// windowed smoke to observe.
    fn deliver_notification(&self, title: &str, body: &str) {
        let mtm = {
            let state = self.0.borrow();
            *state.last_delivered_notification.borrow_mut() =
                Some((title.to_owned(), body.to_owned()));
            state.mtm
        };
        if title.is_empty() {
            eprintln!("qwertty-term: notification: {body}");
        } else {
            eprintln!("qwertty-term: notification: {title} — {body}");
        }
        // Dock attention (no-op while the app is already frontmost); the real
        // OS notification lands here once bundled (ADR 0003).
        NSApplication::sharedApplication(mtm)
            .requestUserAttention(objc2_app_kit::NSRequestUserAttentionType::InformationalRequest);
    }

    /// The last desktop notification actually delivered (post-throttle), for
    /// the windowed smoke to assert on (smoke/test).
    pub fn last_delivered_notification(&self) -> Option<(String, String)> {
        self.0.borrow().last_delivered_notification.borrow().clone()
    }

    /// Apply the configured initial geometry (`window-width`/`-height` cells +
    /// `window-position-x`/`-y`) to `window`, but only for the *first* window
    /// (a latch ensures later Cmd-N windows use the default size). `cell_w`/
    /// `cell_h` are the surface's device-pixel cell metrics; the content is
    /// sized to `cols × rows` cells in points (device px / scale). A no-op when
    /// no geometry is configured.
    fn apply_initial_window_geometry(&self, window: &NSWindow, cell_w: u32, cell_h: u32) {
        let (cells, position) = {
            let state = self.0.borrow();
            if state.first_window_placed.get() {
                return;
            }
            state.first_window_placed.set(true);
            (state.initial_window_cells, state.initial_window_position)
        };
        let scale = window.backingScaleFactor().max(1.0);
        if let Some((cols, rows)) = cells {
            // Cell metrics are device pixels; content-size is in points.
            let w = cols as f64 * cell_w as f64 / scale;
            let h = rows as f64 * cell_h as f64 / scale;
            window.setContentSize(NSSize::new(w, h));
        }
        if let Some((x, y)) = position {
            // `window-position-*` is pixels from the visible screen's top-left;
            // AppKit's `setFrameTopLeftPoint` is in bottom-left screen space, so
            // convert y against the screen's full height.
            let screen_h = window
                .screen()
                .map(|s| s.frame().size.height)
                .unwrap_or(0.0);
            let top_left = NSPoint::new(x as f64, screen_h - y as f64);
            window.setFrameTopLeftPoint(top_left);
        }
    }

    /// The configured `right-click-action`.
    pub fn right_click_action(&self) -> crate::context_menu::RightClickAction {
        self.0.borrow().right_click_action
    }

    /// Override the unsafe-paste confirmation answer (smoke/test): `Some(true)`
    /// = confirm, `Some(false)` = decline, `None` = show the real modal.
    pub fn set_paste_confirm_hook(&self, answer: Option<bool>) {
        self.0.borrow_mut().paste_confirm_hook = answer;
    }

    /// Whether pasting `text` into `surface` would be gated as unsafe under the
    /// current `clipboard-paste-protection` config (smoke/test): folds in the
    /// pane's live bracketed-paste mode.
    pub fn paste_is_unsafe(&self, tab: TabId, surface: SurfaceId, text: &str) -> bool {
        let state = self.0.borrow();
        let bracketed = state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.engine().bracketed_paste())
            .unwrap_or(false);
        crate::paste::is_unsafe(text, bracketed, state.paste_protection)
    }

    /// Paste `text` into `surface` through the full hardened paste path
    /// (bracketed detection + `clipboard-paste-protection` + confirm hook),
    /// without going through `NSPasteboard`. Shared by the clipboard-smoke hook
    /// and the middle-click primary paste.
    fn paste_text(&self, tab: TabId, surface: SurfaceId, text: &str) {
        let bracketed = {
            let state = self.0.borrow();
            let Some(s) = state.tabs.get(&tab).and_then(|t| t.surfaces.get(&surface)) else {
                return;
            };
            match s.display_source() {
                // Display pane: its own engine is empty; read the live mode off
                // the Viewer's pane terminal (fed by `%output`).
                Some(src) => state
                    .tabs
                    .get(&src.control_tab)
                    .and_then(|ct| ct.surfaces.get(&src.control_surface))
                    .and_then(|cs| cs.tmux_session())
                    .and_then(|sess| sess.pane_terminal(surface))
                    .map(|t| t.modes.get(qwertty_term_vt::modes::Mode::BracketedPaste))
                    .unwrap_or(false),
                None => s.engine().bracketed_paste(),
            }
        };
        let protection = self.0.borrow().paste_protection;
        if crate::paste::is_unsafe(text, bracketed, protection) && !self.confirm_unsafe_paste(text)
        {
            return;
        }
        self.write_paste(tab, surface, text, bracketed);
    }

    /// Paste `text` into `surface` as if from the clipboard (smoke/test).
    pub fn paste_text_for_test(&self, tab: TabId, surface: SurfaceId, text: &str) {
        self.paste_text(tab, surface, text);
    }

    /// Whether hovering a pane focuses it (`focus-follows-mouse`).
    pub fn focus_follows_mouse(&self) -> bool {
        self.0.borrow().focus_follows_mouse
    }

    /// Handle a middle-click on `surface` per `middle-click-action`:
    /// `primary-paste` pastes the current selection into the pane (through the
    /// hardened paste path); `ignore` does nothing.
    pub fn middle_click(&self, tab: TabId, surface: SurfaceId) {
        use crate::config::MiddleClickAction as MCA;
        if self.0.borrow().middle_click_action != MCA::PrimaryPaste {
            return;
        }
        if let Some(text) = self.surface_selection_string(tab, surface) {
            self.paste_text(tab, surface, &text);
        }
    }

    /// Select `n` cells on row 0 of `surface` (smoke/test), so tests can create
    /// a selection without driving mouse events.
    pub fn smoke_select_row0(&self, tab: TabId, surface: SurfaceId, n: usize) {
        let mut state = self.0.borrow_mut();
        if let Some(s) = state
            .tabs
            .get_mut(&tab)
            .and_then(|t| t.surfaces.get_mut(&surface))
        {
            let mut engine = s.engine();
            engine.select_screen_points((0, 0), (n.saturating_sub(1), 0), false);
        }
    }

    /// The context-menu item titles for a surface (smoke/test): actionable
    /// items by title, separators as `"---"`. Built from the same pure model
    /// the view uses, so it mirrors what a right-click would show.
    pub fn context_menu_titles(&self, tab: TabId, surface: SurfaceId) -> Vec<String> {
        let has_selection = self.surface_has_selection(tab, surface);
        crate::context_menu::context_items(has_selection)
            .into_iter()
            .map(|item| match item {
                crate::context_menu::ContextItem::Separator => "---".to_string(),
                crate::context_menu::ContextItem::Action(a) => a.title().to_string(),
            })
            .collect()
    }

    /// Whether `mouse-hide-while-typing` is enabled.
    pub fn mouse_hide_while_typing(&self) -> bool {
        self.0.borrow().mouse_hide_while_typing
    }

    /// Whether `surface` in `tab` currently has a text selection (gates the
    /// context menu's Copy item).
    pub fn surface_has_selection(&self, tab: TabId, surface: SurfaceId) -> bool {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.engine().selection().is_some())
            .unwrap_or(false)
    }

    /// Whether `surface` in `tab` has mouse reporting active — when it does, a
    /// right-click belongs to the program, so we suppress the context menu
    /// (matching upstream `mouseCaptured`).
    pub fn surface_reporting_active(&self, tab: TabId, surface: SurfaceId) -> bool {
        let state = self.0.borrow();
        state
            .tabs
            .get(&tab)
            .and_then(|t| t.surfaces.get(&surface))
            .map(|s| s.engine().mouse_event() != qwertty_term_input::mouse_encode::MouseEvent::None)
            .unwrap_or(false)
    }

    /// Perform a context-menu [`ContextAction`](crate::context_menu::ContextAction)
    /// against the right-clicked pane: copy its selection, paste into it, split
    /// it, or close it. Focuses the pane first so split/close act on it.
    pub fn context_menu_action(
        &self,
        tab: TabId,
        surface: SurfaceId,
        action: crate::context_menu::ContextAction,
    ) {
        use crate::context_menu::ContextAction as CA;
        // Focus the target pane so splits/close operate on it (and a following
        // paste goes to the right pty).
        self.focus_surface_in_tab(tab, surface);
        match action {
            CA::Copy => self.copy_surface_selection(tab, surface),
            CA::Paste => self.paste_into_surface(tab, surface),
            CA::SplitRight | CA::SplitLeft | CA::SplitDown | CA::SplitUp => {
                if let Some(dir) = action.split_direction() {
                    self.handle_split_action(tab, SplitAction::NewSplit(dir));
                }
            }
            CA::ClosePane => {
                // Gated by `confirm-close-surface` when a process is running.
                if self.confirm_close_surface(tab, surface) {
                    self.close_surface(tab, surface);
                }
            }
        }
    }

    /// The direct (non-menu) right-click actions: `paste` / `copy` /
    /// `copy-or-paste`. `ignore` and `context-menu` are handled elsewhere.
    pub fn right_click_direct(&self, tab: TabId, surface: SurfaceId) {
        use crate::context_menu::RightClickAction as RCA;
        match self.right_click_action() {
            RCA::Paste => self.paste_into_surface(tab, surface),
            RCA::Copy => self.copy_surface_selection(tab, surface),
            RCA::CopyOrPaste => {
                if self.surface_has_selection(tab, surface) {
                    self.copy_surface_selection(tab, surface);
                } else {
                    self.paste_into_surface(tab, surface);
                }
            }
            RCA::ContextMenu | RCA::Ignore => {}
        }
    }

    /// Copy `surface`'s selection to the system clipboard (no-op without one),
    /// honoring `clipboard-trim-trailing-spaces`.
    fn copy_surface_selection(&self, tab: TabId, surface: SurfaceId) {
        let (trim, clear_on_copy) = {
            let state = self.0.borrow();
            (
                state.clipboard_trim_trailing_spaces,
                state.selection_clear_on_copy,
            )
        };
        let state = self.0.borrow();
        let Some(s) = state.tabs.get(&tab).and_then(|t| t.surfaces.get(&surface)) else {
            return;
        };
        // Read + copy under a scoped engine lock, then release it before the
        // clear (`engine()` returns a `MutexGuard` — re-locking it while the
        // first guard is live would deadlock).
        let copied = {
            if let Some(text) = s.engine().selection_string_opt(trim) {
                crate::clipboard::write(&text);
                true
            } else {
                false
            }
        };
        // `selection-clear-on-copy`: drop the selection after an explicit copy
        // (this path is the `copy_to_clipboard` action only, never
        // copy-on-select — that goes through `selection_release`).
        if copied && clear_on_copy {
            s.engine().clear_selection();
        }
    }

    /// Paste the system clipboard into `surface`'s pty (bracketed if the
    /// program enabled bracketed paste). Honors `clipboard-paste-protection`:
    /// an unsafe paste (newline / bracketed-end sequence) prompts for
    /// confirmation first (a modal alert), and only proceeds if the user
    /// confirms.
    fn paste_into_surface(&self, tab: TabId, surface: SurfaceId) {
        let Some(text) = crate::clipboard::read() else {
            return;
        };
        // Decide safety under a scoped borrow (read the pane's bracketed mode +
        // the config), then release it before any modal alert / io write.
        let (bracketed, protection) = {
            let state = self.0.borrow();
            let bracketed = state
                .tabs
                .get(&tab)
                .and_then(|t| t.surfaces.get(&surface))
                .map(|s| s.engine().bracketed_paste())
                .unwrap_or(false);
            (bracketed, state.paste_protection)
        };
        if crate::paste::is_unsafe(&text, bracketed, protection)
            && !self.confirm_unsafe_paste(&text)
        {
            // User declined the unsafe paste.
            return;
        }
        self.write_paste(tab, surface, &text, bracketed);
    }

    /// Write `text` to `surface`'s pty as a paste (bracketed if `bracketed`).
    /// Split out so the confirmed and direct paths share the encoding.
    fn write_paste(&self, tab: TabId, surface: SurfaceId, text: &str, bracketed: bool) {
        let payload = if bracketed {
            let mut p = Vec::with_capacity(text.len() + 12);
            p.extend_from_slice(b"\x1b[200~");
            p.extend_from_slice(text.as_bytes());
            p.extend_from_slice(b"\x1b[201~");
            p
        } else {
            text.as_bytes().to_vec()
        };
        // A display-only tmux pane has no pty, so `send_pty` would silently drop
        // the paste; route it to tmux as `send-keys`, exactly like keyboard input.
        let src = {
            let state = self.0.borrow();
            match state.tabs.get(&tab).and_then(|t| t.surfaces.get(&surface)) {
                Some(s) => s.display_source(),
                None => return,
            }
        };
        let mut state = self.0.borrow_mut();
        match src {
            Some(src) => self.route_keys_to_tmux(&mut state, src, surface, &payload),
            None => {
                if let Some(s) = state.tabs.get(&tab).and_then(|t| t.surfaces.get(&surface)) {
                    s.send_pty(&payload);
                }
            }
        }
    }

    /// Show a modal confirmation for an unsafe paste; returns whether the user
    /// chose to paste anyway. No controller borrow is held across the modal
    /// (`runModal` spins its own run loop, which can re-enter the controller).
    fn confirm_unsafe_paste(&self, text: &str) -> bool {
        let mtm = self.0.borrow().mtm;
        // The smoke can't drive a modal; a test hook lets it answer
        // deterministically (see `set_paste_confirm_hook`).
        if let Some(answer) = self.0.borrow().paste_confirm_hook {
            return answer;
        }
        let alert = objc2_app_kit::NSAlert::new(mtm);
        let lines = text.lines().count().max(1);
        alert.setMessageText(&NSString::from_str("Paste multiple lines?"));
        alert.setInformativeText(&NSString::from_str(&format!(
            "The clipboard contains {lines} lines of text. Pasting it may run \
             commands. Paste anyway?"
        )));
        alert.addButtonWithTitle(&NSString::from_str("Paste"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        // NSAlertFirstButtonReturn == 1000 ("Paste").
        let response = alert.runModal();
        response == objc2_app_kit::NSAlertFirstButtonReturn
    }

    /// Smoke/test hook: force the next close-confirmation modal to a fixed
    /// answer (`Some(true)` = Close, `Some(false)` = Cancel) instead of showing
    /// the alert. `None` restores normal modal behavior.
    pub fn set_close_confirm_hook(&self, answer: Option<bool>) {
        self.0.borrow_mut().close_confirm_hook = answer;
    }

    /// Whether closing `surface` needs a confirmation per `confirm-close-surface`
    /// and the surface's shell-integration prompt state. A dead (exited) surface
    /// never confirms; `always` confirms even at a prompt; `true` (`OnRunning`)
    /// confirms only when the cursor is not at a prompt (a command is running,
    /// or there's no shell integration to say otherwise).
    fn surface_needs_confirm_close(&self, tab: TabId, surface: SurfaceId) -> bool {
        use crate::config::ConfirmCloseSurface as C;
        let state = self.0.borrow();
        let mode = state.confirm_close_surface;
        if mode == C::Never {
            return false;
        }
        let Some(s) = state.tabs.get(&tab).and_then(|t| t.surfaces.get(&surface)) else {
            return false;
        };
        if s.is_dead() {
            return false;
        }
        // A display pane's own engine is empty, so read the prompt state off the
        // Viewer's pane terminal (fed by `%output`) — otherwise a tmux pane sitting
        // at a shell prompt is wrongly treated as "running" and always confirms.
        let at_prompt = match s.display_source() {
            Some(src) => state
                .tabs
                .get(&src.control_tab)
                .and_then(|ct| ct.surfaces.get(&src.control_surface))
                .and_then(|cs| cs.tmux_session())
                .and_then(|sess| sess.pane_terminal(surface))
                .map(|t| t.cursor_is_at_prompt())
                .unwrap_or(false),
            None => s.engine().cursor_is_at_prompt(),
        };
        match mode {
            C::Never => false,
            C::Always => true,
            C::OnRunning => !at_prompt,
        }
    }

    /// Whether closing the whole tab/window needs confirmation — true if *any*
    /// of its panes has a running process (upstream OR-reduces the per-surface
    /// predicate for window/tab closes).
    pub fn tab_needs_confirm_close(&self, tab: TabId) -> bool {
        let surfaces: Vec<SurfaceId> = {
            let state = self.0.borrow();
            match state.tabs.get(&tab) {
                Some(t) => t.surfaces.keys().copied().collect(),
                None => return false,
            }
        };
        surfaces
            .into_iter()
            .any(|sid| self.surface_needs_confirm_close(tab, sid))
    }

    /// Run the close-confirmation modal (or the smoke hook). Returns whether the
    /// user chose to close.
    fn run_close_confirm_alert(&self, message: &str, informative: &str) -> bool {
        if let Some(answer) = self.0.borrow().close_confirm_hook {
            return answer;
        }
        let mtm = self.0.borrow().mtm;
        let alert = objc2_app_kit::NSAlert::new(mtm);
        alert.setMessageText(&NSString::from_str(message));
        alert.setInformativeText(&NSString::from_str(informative));
        alert.addButtonWithTitle(&NSString::from_str("Close"));
        alert.addButtonWithTitle(&NSString::from_str("Cancel"));
        alert.runModal() == objc2_app_kit::NSAlertFirstButtonReturn
    }

    /// Confirm closing a single surface (split/pane), if it needs it. Returns
    /// whether to proceed with the close.
    fn confirm_close_surface(&self, tab: TabId, surface: SurfaceId) -> bool {
        if !self.surface_needs_confirm_close(tab, surface) {
            return true;
        }
        self.run_close_confirm_alert(
            "Close Terminal?",
            "The terminal still has a running process. If you close the terminal \
             the process will be killed.",
        )
    }

    /// Smoke/test: run the full close-confirmation decision for a surface
    /// (needs-confirm predicate + the modal, short-circuited by the close hook)
    /// **without** actually closing it. Returns whether the close would proceed.
    pub fn would_confirm_close_surface(&self, tab: TabId, surface: SurfaceId) -> bool {
        self.confirm_close_surface(tab, surface)
    }

    /// Confirm closing a whole window (all its panes/tabs), if any pane has a
    /// running process. Returns whether to proceed. Called from
    /// `windowShouldClose:`.
    fn confirm_close_window(&self, tab: TabId) -> bool {
        if !self.tab_needs_confirm_close(tab) {
            return true;
        }
        self.run_close_confirm_alert(
            "Close Window?",
            "This window still has running processes. Closing it will terminate them.",
        )
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
        let mut any_changed = false;
        if let Some(t) = state.tabs.get_mut(&tab) {
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
        drop(state);
        // A font-size change resizes the grid (bigger/smaller cells → fewer/more
        // cols/rows fit the window). For a tmux-window tab, propagate the new
        // grid to tmux so it re-lays-out and the pane terminals reflow — same
        // omission (and fix) as a window resize.
        if any_changed {
            self.propagate_tmux_window_resize(tab);
        }
    }

    /// Pump + render every pane of every live tab. Called each pace tick. A
    /// tab is closed when its *last* pane's shell exits; individual pane exits
    /// collapse the split (handled here via `close_surface`).
    /// The combined pace tick (io-service + render). Used by the fallback
    /// `NSTimer` when there is no `CADisplayLink` (GUI smokes, no screen-backed
    /// window). Behaviorally identical to the historical single tick.
    pub fn tick(&self) {
        self.tick_impl(true);
    }

    /// io-service only — pump each pane (drain the engine's reply bytes to the
    /// pty, service lifecycle/bell/notifications/progress/titles) but do NOT
    /// render. Driven by the steady service timer so it runs regardless of
    /// window visibility: a `CADisplayLink` pauses for occluded/background
    /// windows, but a background terminal must still answer pty queries (DSR/DA)
    /// and service its io. See [`AppDelegate::start_display_link`].
    pub fn tick_service(&self) {
        self.tick_impl(false);
    }

    /// Render/present every pane. Driven by the `CADisplayLink` (vsync). Reads
    /// the engine state that [`Controller::tick_service`] and the io-reader
    /// thread maintain, and presents it at the display refresh — so high-fps
    /// content is subsampled evenly (smooth) instead of on a drifting timer.
    pub fn tick_render(&self) {
        {
            let mut state = self.0.borrow_mut();
            for tab in state.tabs.values_mut() {
                let focused = tab.tree.focused();
                let is_split = tab.tree.len() > 1;
                for (sid, surface) in tab.surfaces.iter_mut() {
                    // Display-only tmux panes have no engine of their own; they
                    // are drawn from the Viewer's pane terminals in
                    // `render_tmux_panes` below. Rendering them here would just
                    // present their empty engine, so skip them.
                    if surface.display_source().is_some() {
                        surface.sync_progress_layer();
                        continue;
                    }
                    surface.render(*sid == focused, is_split);
                    surface.sync_progress_layer();
                }
            }
        }
        // Draw the display-only tmux pane surfaces from their bound
        // Viewer-owned pane terminals. `tick_impl(true)` (the fallback pace
        // timer) already did this; the display-link render path was missing it,
        // which left tmux panes blank on machines where the display link drives
        // rendering (i.e. normally). ADR 006 slice 5b-native.
        self.render_tmux_panes();
    }

    fn tick_impl(&self, render_panes: bool) {
        // Collect (tab, surface) pairs whose shell exited, plus per-tab focused
        // title/password state, under one borrow. Render every pane.
        let bell_features = self.0.borrow().bell_features;
        let desktop_notifications = self.0.borrow().desktop_notifications;
        let progress_style = self.0.borrow().progress_style;
        let now_tick = std::time::Instant::now();
        let notify_mode = self.0.borrow().notify_on_command_finish;
        // Whether the app is frontmost — the `unfocused` gate needs real user
        // focus (key window + active app), not just a tab's focused pane.
        let app_active = {
            let mtm = self.0.borrow().mtm;
            NSApplication::sharedApplication(mtm).isActive()
        };
        let (exited, bell_rang, notifications, command_finishes, tmux_events) = {
            let mut state = self.0.borrow_mut();
            let mut dead: Vec<(TabId, SurfaceId)> = Vec::new();
            // Whether any pane rang this tick — drives the once-per-tick
            // audible/attention effects below (the title indicator is per-tab).
            let mut any_bell = false;
            // OSC 9/777 desktop notifications observed this tick (across all
            // panes); throttled + delivered after the borrow drops.
            let mut notifications: Vec<(String, String)> = Vec::new();
            // Commands that finished this tick (OSC 133), with per-surface
            // focus, decided + delivered after the borrow drops. `(tab,
            // exit_code, elapsed, focused)`.
            let mut command_finishes: Vec<(TabId, Option<i32>, std::time::Duration, bool)> =
                Vec::new();
            // tmux control-mode reconcile events observed this tick (ADR 006
            // slice 5b-native): `(tab, control-surface, plan, exit)`.
            // Collected here and applied to native tabs after the per-surface
            // borrow drops (creating/removing tabs needs `&mut state.tabs`,
            // which is borrowed by this loop).
            let mut tmux_events: Vec<TmuxReconcileEvent> = Vec::new();
            // The forced-title override (config `title`); cloned out before the
            // per-tab loop so it doesn't alias the `state.tabs` mutable borrow.
            let forced_title = state.forced_title.clone();
            // `window-subtitle` policy, read before the per-tab loop.
            let subtitle_policy = state.window_subtitle;
            for (tid, tab) in state.tabs.iter_mut() {
                let focused = tab.tree.focused();
                // Whether this tab has more than one pane — the gate for
                // unfocused-split dimming (upstream `isSplit`). A zoomed tab
                // renders only one pane (the zoomed one), which is focused, so
                // no dimming applies while zoomed regardless.
                let is_split = tab.tree.len() > 1;
                let mut password_focused = false;
                for (sid, surface) in tab.surfaces.iter_mut() {
                    let result = surface.pump();
                    // tmux control-mode: a tree change (plan) or an exit is
                    // recorded for post-borrow application to native tabs.
                    if result.tmux_plan.is_some() || result.tmux_exit || result.tmux_focus.is_some()
                    {
                        tmux_events.push(TmuxReconcileEvent {
                            tab: *tid,
                            surface: *sid,
                            plan: result.tmux_plan,
                            exit: result.tmux_exit,
                            focus: result.tmux_focus,
                        });
                    }
                    if result.exited {
                        dead.push((*tid, *sid));
                    } else {
                        // Poison-dead surface: settle it (shut io down + paint the
                        // crash banner) once, then keep rendering so the banner
                        // shows. It is deliberately NOT pushed onto `dead` — a
                        // crashed pane stays open (unlike a clean child exit) so
                        // the user sees why, and other panes/tabs are unaffected.
                        if surface.is_dead() {
                            surface.settle_dead();
                        }
                        // Selection edge-autoscroll: while a drag is parked
                        // past the pane's top/bottom edge, each tick scrolls
                        // one row and extends the selection (upstream's
                        // ~15ms `selection_scroll` io timer).
                        surface.selection_autoscroll_tick();
                        // OSC 9;4 progress bar: apply a fresh report (when
                        // `progress-style` is on), expire a stale one, then sync
                        // the overlay layer after the frame is drawn.
                        if progress_style && let Some(report) = result.progress_report {
                            surface.set_progress(report, now_tick);
                        }
                        surface.tick_progress_autoclear(now_tick);
                        // Render only on the combined tick; when a display link
                        // drives presentation, `tick_render` does this at vsync.
                        // A display-only tmux pane surface renders a foreign pane
                        // `Terminal` it doesn't own, so it can't be drawn from
                        // this per-surface borrow (the owner is another tab's
                        // control surface); it's drawn afterwards in
                        // `render_tmux_panes`, once this borrow drops.
                        if render_panes && surface.display.is_none() {
                            surface.render(*sid == focused, is_split);
                            surface.sync_progress_layer();
                        }
                    }
                    // A BEL from any pane marks this tab's title (until the tab
                    // is next focused) and triggers the once-per-tick audible/
                    // attention effects.
                    if result.bell && bell_features.any_active() {
                        if bell_features.title {
                            tab.bell_ringing.set(true);
                        }
                        any_bell = true;
                    }
                    // Collect any OSC 9/777 notification (dropped here when
                    // `desktop-notifications` is off — the core-level gate).
                    if desktop_notifications && let Some(n) = result.notification {
                        notifications.push(n);
                    }
                    // Collect a finished command (OSC 133) for command-finish
                    // notification, unless the feature is off.
                    if notify_mode != crate::notify::NotifyOnCommandFinish::Never
                        && let Some((exit_code, elapsed)) = result.command_finished
                    {
                        let focused_here =
                            app_active && tab.window.isKeyWindow() && *sid == focused;
                        command_finishes.push((*tid, exit_code, elapsed, focused_here));
                    }
                    if *sid == focused
                        && let Some(active) = result.password
                    {
                        password_focused = active;
                    }
                }
                tab.update_window_title(password_focused, forced_title.as_deref());
                tab.update_window_subtitle(subtitle_policy);
                // Expire the resize overlay once its lifetime elapses.
                tab.tick_resize_overlay(now_tick);
            }
            (dead, any_bell, notifications, command_finishes, tmux_events)
        };
        // Apply tmux control-mode reconcile events with the per-surface borrow
        // released (creating/removing native tabs needs `&mut state.tabs`).
        if !tmux_events.is_empty() {
            self.apply_tmux_reconciles(tmux_events);
        }
        // Draw every display-only tmux pane surface from its (foreign) pane
        // `Terminal`. Done every render tick, not just on a tree change: `%output`
        // updates pane terminals continuously without a reconcile. Runs with no
        // controller borrow held (it takes its own two-phase borrow to read one
        // tab's control session and draw into another).
        if render_panes {
            self.render_tmux_panes();
        }
        // Fire the once-per-tick audible/attention bell effects with no
        // controller borrow held (these are AppKit calls). The per-tab title
        // indicator was already set inside the borrow above.
        if bell_rang {
            if bell_features.system {
                // System alert sound (respects the user's alert-volume; a
                // no-op when the system alert sound is muted).
                objc2_app_kit::NSBeep();
            }
            if bell_features.attention {
                // Bounce the Dock icon until the app is activated (macOS
                // ignores this while the app is already active/frontmost).
                let mtm = self.0.borrow().mtm;
                NSApplication::sharedApplication(mtm).requestUserAttention(
                    objc2_app_kit::NSRequestUserAttentionType::InformationalRequest,
                );
            }
        }
        // Rate-limit the collected desktop notifications (1/sec global + 5s
        // identical dedup, matching upstream's core throttle) under a short
        // borrow, then deliver the admitted ones with no borrow held.
        if !notifications.is_empty() {
            let admitted: Vec<(String, String)> = {
                let state = self.0.borrow();
                let mut throttle = state.notification_throttle.borrow_mut();
                let now = std::time::Instant::now();
                notifications
                    .into_iter()
                    .filter(|(title, body)| {
                        throttle.admit(
                            &crate::notify::Notification::new(title.clone(), body.clone()),
                            now,
                        )
                    })
                    .collect()
            };
            for (title, body) in admitted {
                self.deliver_notification(&title, &body);
            }
        }
        // Command-finish notifications (OSC 133): apply the mode/threshold gate,
        // then fire the configured effect(s) with no borrow held.
        for (tid, exit_code, elapsed, focused) in command_finishes {
            let (action, after) = {
                let s = self.0.borrow();
                (
                    s.notify_on_command_finish_action,
                    s.notify_on_command_finish_after,
                )
            };
            if !crate::notify::should_notify_command_finish(notify_mode, focused, elapsed, after) {
                continue;
            }
            if action.bell {
                // Mark the tab title + beep + dock attention (the bell path).
                let mtm = {
                    let s = self.0.borrow();
                    if let Some(t) = s.tabs.get(&tid) {
                        t.bell_ringing.set(true);
                    }
                    s.mtm
                };
                objc2_app_kit::NSBeep();
                NSApplication::sharedApplication(mtm).requestUserAttention(
                    objc2_app_kit::NSRequestUserAttentionType::InformationalRequest,
                );
            }
            if action.notify {
                let notif = crate::notify::command_finish_notification(exit_code, elapsed);
                let admit = {
                    let s = self.0.borrow();
                    let mut throttle = s.notification_throttle.borrow_mut();
                    throttle.admit(&notif, std::time::Instant::now())
                };
                if admit {
                    self.deliver_notification(&notif.title, &notif.body);
                }
            }
        }
        for (tab, surface) in exited {
            self.close_surface(tab, surface);
        }
        // Quit when the last tab's last pane exits — but only if
        // `quit-after-last-window-closed` is set (default false on macOS: the
        // app stays running with no windows, standard macOS behavior).
        if self.tab_count() == 0 && self.0.borrow().quit_after_last_window_closed {
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
        self.build_surface_with(mtm, tab, surface, scale, cwd, None)
    }

    /// Build a **display-only** tmux pane surface (ADR 006 slice 5b-native): a
    /// [`Surface`] with **no pty and no shell** (`io: None`) that renders the
    /// pane `Terminal` owned by `control`'s [`TmuxSession`] Viewer, sourced by
    /// reference each frame in [`Controller::render_tmux_panes`]. Its own engine
    /// is created (to satisfy the struct + any incidental `engine()` read) but
    /// stays empty and is never fed. `surface` must be the
    /// [`Reconciler`](crate::tmux_reconcile::Reconciler)-minted id for this tmux
    /// pane, so [`TmuxSession::pane_terminal`](crate::tmux_session::TmuxSession::pane_terminal)
    /// resolves it back to the right pane.
    fn build_display_surface(
        &self,
        mtm: MainThreadMarker,
        tab: TabId,
        surface: SurfaceId,
        scale: f64,
        control: DisplaySource,
    ) -> Option<Surface> {
        self.build_surface_with(mtm, tab, surface, scale, None, Some(control))
    }

    /// Shared body of [`build_surface`](Self::build_surface) and
    /// [`build_display_surface`](Self::build_display_surface). When `display` is
    /// `Some`, no pty is spawned (`io: None`) and the surface renders a foreign
    /// tmux pane terminal; otherwise it spawns a shell in `cwd` and renders its
    /// own engine, exactly as before.
    fn build_surface_with(
        &self,
        mtm: MainThreadMarker,
        tab: TabId,
        surface: SurfaceId,
        scale: f64,
        cwd: Option<&std::path::Path>,
        display: Option<DisplaySource>,
    ) -> Option<Surface> {
        let (
            family,
            default_size,
            startup_colors,
            selection_colors,
            dim_alpha,
            dim_fill,
            mods,
            word_boundaries,
            vt_toggles,
        ) = {
            let s = self.0.borrow();
            (
                s.font_family.clone(),
                s.default_font_size,
                s.startup_colors.clone(),
                s.selection_colors,
                s.unfocused_dim_alpha,
                s.unfocused_dim_fill,
                s.metric_modifiers.clone(),
                s.word_boundaries.clone(),
                VtToggles {
                    title_report: s.title_report,
                    enquiry_response: s.enquiry_response.clone(),
                    osc_color_report_format: s.osc_color_report_format,
                    image_storage_limit: s.image_storage_limit,
                    scrollback_limit: s.scrollback_limit,
                },
            )
        };

        let font_size = FontSize::new(default_size);
        let fg = font::build(family.as_deref(), (font_size.get() as f64) * scale, &mods).ok()?;
        let (cw, ch) = (fg.cell_width, fg.cell_height);
        let init_w = (INITIAL_WIDTH * scale) as usize;
        let init_h = (INITIAL_HEIGHT * scale) as usize;
        let (cols, rows) = geometry::grid_size(init_w, init_h, cw, ch);

        let default_bg = startup_colors
            .background
            .get()
            .map(|c| (c.r, c.g, c.b))
            .unwrap_or((0x18, 0x18, 0x18));

        // Keep a copy of the configured palette for this surface's tmux session
        // (if it ever runs `tmux -CC`); the original is moved into the engine.
        let tmux_colors = startup_colors.clone();
        let mut engine_inner =
            Engine::with_options(cols, rows, startup_colors, vt_toggles.scrollback_limit);
        vt_toggles.apply(&mut engine_inner);
        let engine = Arc::new(Mutex::new(engine_inner));
        // A display-only tmux pane surface has no pty: its bytes arrive via
        // `%output` through the control surface's Viewer, which owns the
        // `Terminal` this surface renders. Everything else (font, render engine,
        // view) is identical to a normal pane.
        let io = match display {
            None => Some(
                TabIo::spawn(Arc::clone(&engine), cols as u16, rows as u16, cw, ch, cwd).ok()?,
            ),
            Some(_) => None,
        };
        let render = RenderEngine::new(cw, ch).ok();

        let frame_dump = crate::frame_dump::FrameDump::from_env(tab.0, surface.0);
        let capture_present =
            frame_dump.is_some() || std::env::var_os("QWERTTY_TERM_ASSERT_PRESENT").is_some();

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
            metric_modifiers: mods,
            view,
            cols,
            rows,
            scale,
            rect: crate::splits::Rect::new(0.0, 0.0, 0.0, 0.0),
            last_mouse_cell: None,
            mouse_button_down: false,
            hovered_cell: None,
            gesture: crate::gesture::SelectionGesture::new(),
            word_boundaries,
            selection_colors,
            default_bg,
            frame_dump,
            last_present_delta: 0,
            last_present_luma: 0.0,
            capture_present,
            scrollback_offset: 0,
            wheel: crate::scroll::WheelState::default(),
            pane_sel_anchor: None,
            dead: Cell::new(false),
            dead_reason: RefCell::new(None),
            banner_drawn: Cell::new(false),
            search: crate::search::SearchState::default(),
            search_highlight_dirty: Cell::new(false),
            search_overlay: None,
            match_colors: crate::selection::MatchColors::default(),
            dim_colors: crate::selection::DimColors {
                // Dim toward the configured fill, else this surface's own
                // terminal background (upstream `unfocused-split-fill ??
                // background`).
                fill: dim_fill.unwrap_or_else(|| {
                    qwertty_term_vt::color::Rgb::new(default_bg.0, default_bg.1, default_bg.2)
                }),
                overlay_alpha: dim_alpha,
                // Resolve `Default` fg/bg the way the renderer paints them so the
                // CPU dim matches the presented pixels: fg falls back to the
                // renderer's `FrameOptions::default_fg` (0xd8d8d8), bg to this
                // surface's terminal background.
                default_fg_fallback: qwertty_term_vt::color::Rgb::new(0xd8, 0xd8, 0xd8),
                default_bg_fallback: qwertty_term_vt::color::Rgb::new(
                    default_bg.0,
                    default_bg.1,
                    default_bg.2,
                ),
            },
            command_started_at: None,
            progress: None,
            progress_deadline: None,
            progress_layer: None,
            tmux: None,
            startup_colors: tmux_colors,
            display,
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

        // `window-show-tab-bar` → the new window's tabbing mode.
        let tabbing_mode = match self.0.borrow().window_show_tab_bar {
            crate::config::WindowShowTabBar::Auto => NSWindowTabbingMode::Automatic,
            crate::config::WindowShowTabBar::Always => NSWindowTabbingMode::Preferred,
            crate::config::WindowShowTabBar::Never => NSWindowTabbingMode::Disallowed,
        };
        let window = make_window(mtm, &container, tabbing_mode);
        // Paint the window background the terminal colour so any sub-cell
        // remainder strip is seamless (see the R5 dark-band fix).
        set_window_background(&window, default_bg);
        // Window chrome config (`macos-window-shadow`, `macos-window-buttons`,
        // `window-step-resize`) — applied once at creation from the surface's
        // cell metrics. Step-resize increments are in points (device px / scale).
        {
            let (shadow, buttons, step_resize, theme) = {
                let state = self.0.borrow();
                (
                    state.macos_window_shadow,
                    state.macos_window_buttons,
                    state.window_step_resize,
                    state.window_theme,
                )
            };
            // `window-theme`: light/dark/auto(-by-luminosity) NSAppearance, or
            // follow the system.
            apply_window_theme(&window, theme, default_bg);
            // `macos-window-shadow`: default true; only `false` changes anything
            // (upstream `TerminalWindow.swift:476`).
            window.setHasShadow(shadow);
            // `macos-window-buttons = hidden`: hide close/miniaturize/zoom
            // (upstream `TerminalWindow.swift:570`).
            if buttons == crate::config::MacWindowButtons::Hidden {
                hide_window_buttons(&window);
            }
            // `window-step-resize`: resize in whole-cell increments (upstream
            // `BaseTerminalController.swift:884`). Skip zero cell sizes (Stage
            // Manager can momentarily report a zero-size window).
            if step_resize {
                let scale = window.backingScaleFactor().max(1.0);
                let inc_w = surface.font.cell_width as f64 / scale;
                let inc_h = surface.font.cell_height as f64 / scale;
                if inc_w > 0.0 && inc_h > 0.0 {
                    window.setContentResizeIncrements(NSSize::new(inc_w, inc_h));
                }
            }
        }
        // `window-save-state`: mark the window (non-)restorable so macOS's native
        // window restoration honors the config. `never` opts every window out.
        let restorable = self.0.borrow().window_save_state != crate::config::WindowSaveState::Never;
        window.setRestorable(restorable);
        if restorable {
            // Give the window a stable identifier + name the app delegate as its
            // restoration class so AppKit re-provides it on relaunch (the delegate
            // hands back the launch window and refills content via the window
            // delegate's `didDecodeRestorableState`). Content is encoded there.
            window.setIdentifier(Some(&NSString::from_str(WINDOW_RESTORATION_IDENTIFIER)));
            unsafe {
                window.setRestorationClass(Some(<AppDelegate as objc2::ClassType>::class()));
            }
        }

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

        // Apply configured initial geometry to the very first window only
        // (later Cmd-N windows keep the default size). Cell metrics are the
        // surface's device-pixel font cell; the helper converts to points.
        self.apply_initial_window_geometry(
            &window,
            surface.font.cell_width,
            surface.font.cell_height,
        );

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
            created: std::time::Instant::now(),
            last_title: RefCell::new(String::new()),
            last_subtitle: RefCell::new(String::new()),
            bell_ringing: Cell::new(false),
            resize_overlay: RefCell::new(None),
            resize_overlay_deadline: Cell::new(None),
        };
        // Lay out the single pane to the container's (AppKit-sized) bounds.
        tab.relayout(controller_ptr, id, mtm);

        self.0.borrow_mut().tabs.insert(id, tab);

        // Native tabbing: add to the parent's window group if requested. A
        // `.disallowed` window (`window-show-tab-bar = never`) never joins a
        // group — it stays a standalone window (upstream
        // `TerminalController.swift:453` gates on `tabbingMode != .disallowed`).
        if let Some(parent) = tab_group_parent
            && tabbing_mode != NSWindowTabbingMode::Disallowed
        {
            let (parent_window, position) = {
                let state = self.0.borrow();
                (
                    state.tabs.get(&parent).map(|t| t.window.clone()),
                    state.window_new_tab_position,
                )
            };
            if let Some(pw) = parent_window {
                // `window-new-tab-position`: `End` groups against the *last*
                // window in the parent's tab group so the new tab lands after
                // every existing tab; `Current` groups against the parent so it
                // lands right after the active tab (upstream
                // `TerminalController.swift:456`). If the parent has no group
                // yet, both fall back to the parent window itself.
                let anchor = match position {
                    crate::config::WindowNewTabPosition::End => pw
                        .tabGroup()
                        .and_then(|g| g.windows().iter().last())
                        .unwrap_or_else(|| pw.clone()),
                    crate::config::WindowNewTabPosition::Current => pw.clone(),
                };
                anchor.addTabbedWindow_ordered(&window, objc2_app_kit::NSWindowOrderingMode::Above);
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
            SplitAction::ToggleZoom => self.toggle_split_zoom(tab),
            SplitAction::ResizeSplit(dir) => self.resize_split(tab, dir),
            SplitAction::EqualizeSplits => self.equalize_splits(tab),
        }
    }

    /// Split the focused pane of `tab` in `direction`, spawning a new surface
    /// (its own shell, inheriting the focused pane's pwd) at a 50/50 ratio, then
    /// re-lay-out. The new pane becomes focused (first responder).
    pub fn new_split(&self, tab: TabId, direction: Direction) {
        let (anchor, cwd) = {
            let state = self.0.borrow();
            let Some(t) = state.tabs.get(&tab) else {
                return;
            };
            let anchor = t.tree.focused();
            let cwd = t
                .focused_surface()
                .and_then(|s| s.engine().pwd())
                .and_then(|p| tabs::inherit_pwd(Some(&p)));
            (anchor, cwd)
        };
        // tmux-managed tab: the Viewer owns this tab's layout. Redirect the
        // split to a tmux `split-window` (the resulting `%layout-change`
        // reconcile creates+renders the new pane) instead of spawning a rogue
        // native pty pane that would fight the reconcile (ADR 006 slice 5e).
        if let Some(src) = self.tmux_pane_of(tab, anchor) {
            self.redirect_tmux_action(
                src,
                anchor,
                TmuxNativeAction::Split {
                    horizontal: direction.axis() == crate::splits::Axis::Horizontal,
                    before: direction.new_is_first(),
                },
            );
            return;
        }
        self.spawn_split(tab, anchor, direction, cwd);
    }

    /// Split `anchor`'s pane of `tab` in `direction`, spawning a new surface
    /// whose shell starts in `cwd`, at a 50/50 ratio; re-lay-out and focus the
    /// new pane. Returns the new surface id, or `None` if the tab/anchor is gone
    /// or the surface fails to build. This is the shared core of interactive
    /// [`new_split`](Self::new_split) and session restore
    /// ([`rebuild_session_tree`](Self::rebuild_session_tree)).
    fn spawn_split(
        &self,
        tab: TabId,
        anchor: SurfaceId,
        direction: Direction,
        cwd: Option<PathBuf>,
    ) -> Option<SurfaceId> {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;

        // Focus the anchor (so `split` targets its leaf) + mint a fresh id.
        let (surface_id, scale) = {
            let mut state = self.0.borrow_mut();
            let t = state.tabs.get_mut(&tab)?;
            if !t.tree.focus(anchor) {
                return None;
            }
            let scale = t.window.backingScaleFactor();
            (t.mint_surface_id(), scale)
        };

        // Build the new surface outside the borrow (spawning a shell is heavy).
        let surface = self.build_surface(mtm, tab, surface_id, scale, cwd.as_deref())?;
        let view = surface.view.clone();

        // Insert into the tree + container + surface map, then re-lay-out.
        {
            let mut state = self.0.borrow_mut();
            let t = state.tabs.get_mut(&tab)?;
            // The pane that was focused before this split loses focus to the new
            // pane (per-pane focus reporting): send it focus-OUT.
            let previous = t.tree.focused();
            t.container.addSubview(&surface.view);
            t.surfaces.insert(surface_id, surface);
            t.tree.split(surface_id, direction);
            t.relayout(controller_ptr, tab, mtm);
            // `split` focuses the new pane. Route focus-out to the old pane and
            // focus-in to the new one (mode-1004 + password poll, per surface).
            if previous != surface_id
                && let Some(prev) = t.surfaces.get_mut(&previous)
            {
                prev.set_focus(false);
            }
            if let Some(next) = t.surfaces.get_mut(&surface_id) {
                next.set_focus(true);
            }
        }

        // Focus the new pane (first responder) outside the borrow.
        if let Some(window) = view.window() {
            window.makeFirstResponder(Some(&view));
        }
        Some(surface_id)
    }

    /// Capture `tab`'s restorable session: its split tree with each pane's
    /// working directory (OSC 7). Serializable via [`crate::session`] for
    /// `window-save-state`. Returns `None` if the tab is gone.
    pub fn capture_window_session(&self, tab: TabId) -> Option<crate::session::WindowSession> {
        let state = self.0.borrow();
        let t = state.tabs.get(&tab)?;
        Some(crate::session::WindowSession::new(node_to_session(
            t.tree.root(),
            t,
        )))
    }

    /// Restore a captured [`WindowSession`](crate::session::WindowSession) into a
    /// new tab: spawn the tab in the tree's leftmost cwd, rebuild the full split
    /// structure (one shell per leaf, in its saved cwd), and re-apply each
    /// split's ratio. Returns the new tab id, or `None` if the tab can't spawn.
    ///
    /// The OS side — handing this session to macOS's `NSWindowRestoration`
    /// `NSCoder` so a genuine quit+relaunch replays it — is the remaining
    /// slice-2b wiring; this method is what that (and the in-app path) drives.
    pub fn restore_window_session(&self, session: &crate::session::WindowSession) -> Option<TabId> {
        let cwd = leftmost_cwd(&session.tree).and_then(|c| tabs::inherit_pwd(Some(&c)));
        let tab = self.spawn_tab(cwd, None)?;
        // The tab's single leaf is the tree's leftmost leaf; grow the rest.
        let anchor = {
            let state = self.0.borrow();
            state.tabs.get(&tab)?.tree.focused()
        };
        self.rebuild_session_tree(tab, &session.tree, anchor);
        self.apply_session_ratios(tab, &session.tree, &mut Vec::new());
        Some(tab)
    }

    /// Recursively grow `tab`'s split tree to match `node`. `anchor` is the
    /// surface already occupying `node`'s whole slot (its leftmost leaf, spawned
    /// by the parent). For a split, `anchor` stays in the first slot and a new
    /// surface (its own shell in the second subtree's leftmost cwd) fills the
    /// second, then both subtrees recurse. Splits at 0.5; ratios are applied
    /// afterward by [`apply_session_ratios`](Self::apply_session_ratios).
    fn rebuild_session_tree(
        &self,
        tab: TabId,
        node: &crate::session::SessionNode,
        anchor: SurfaceId,
    ) {
        use crate::session::{SessionAxis, SessionNode};
        let SessionNode::Split {
            axis,
            first,
            second,
            ..
        } = node
        else {
            return; // Leaf: `anchor` already represents it.
        };
        // The direction that keeps `anchor` in the first slot and puts the new
        // surface second — matching how capture reads first/second.
        let direction = match axis {
            SessionAxis::Horizontal => Direction::Right,
            SessionAxis::Vertical => Direction::Down,
        };
        let cwd = leftmost_cwd(second).and_then(|c| tabs::inherit_pwd(Some(&c)));
        let Some(new) = self.spawn_split(tab, anchor, direction, cwd) else {
            return; // Surface failed to build; leave the partial tree as-is.
        };
        self.rebuild_session_tree(tab, first, anchor);
        self.rebuild_session_tree(tab, second, new);
    }

    /// Walk the rebuilt tree in lockstep with `node`, setting each split's ratio.
    /// The live tree's first/second orientation matches the session's (see
    /// [`rebuild_session_tree`](Self::rebuild_session_tree)), so the `false=first
    /// / true=second` `path` addresses the same split in both.
    fn apply_session_ratios(
        &self,
        tab: TabId,
        node: &crate::session::SessionNode,
        path: &mut Vec<bool>,
    ) {
        use crate::session::SessionNode;
        let SessionNode::Split {
            ratio,
            first,
            second,
            ..
        } = node
        else {
            return;
        };
        {
            let mut state = self.0.borrow_mut();
            if let Some(t) = state.tabs.get_mut(&tab) {
                t.tree.set_ratio(path, *ratio);
            }
        }
        path.push(false);
        self.apply_session_ratios(tab, first, path);
        path.pop();
        path.push(true);
        self.apply_session_ratios(tab, second, path);
        path.pop();
    }

    /// Encode `tab`'s restorable [`WindowSession`](crate::session::WindowSession)
    /// into `coder` (the window-restoration `NSCoder` macOS hands us from
    /// `window:willEncodeRestorableState:`) as a JSON `NSString` under
    /// [`SESSION_CODER_KEY`]. No-op if the tab is gone. The counterpart is
    /// [`decode_session_from`](Self::decode_session_from); the pair is exercised
    /// end-to-end (through a real `NSKeyedArchiver`) by the session smoke.
    pub fn encode_session_into(&self, coder: &objc2_foundation::NSCoder, tab: TabId) {
        let Some(session) = self.capture_window_session(tab) else {
            return;
        };
        let json = NSString::from_str(&session.to_json());
        let key = NSString::from_str(SESSION_CODER_KEY);
        // SAFETY: `json` is an `NSString` (secure-codable); the coder retains a
        // copy under `key`. Nothing here outlives the call.
        unsafe { coder.encodeObject_forKey(Some(&json), &key) };
    }

    /// Decode a [`WindowSession`](crate::session::WindowSession) previously
    /// archived by [`encode_session_into`](Self::encode_session_into) from
    /// `coder`, or `None` if the key is absent or the payload doesn't parse.
    /// Secure decode restricted to `NSString`, so a hostile archive can't
    /// instantiate an unexpected class.
    pub fn decode_session_from(
        coder: &objc2_foundation::NSCoder,
    ) -> Option<crate::session::WindowSession> {
        let key = NSString::from_str(SESSION_CODER_KEY);
        // SAFETY: we constrain the decoded class to `NSString` and only read it.
        let obj = unsafe {
            coder.decodeObjectOfClass_forKey(<NSString as objc2::ClassType>::class(), &key)
        }?;
        let s = obj.downcast::<NSString>().ok()?;
        crate::session::WindowSession::from_json(&s.to_string())
    }

    /// Rebuild `session`'s split content into `tab`'s existing (single-pane)
    /// tree: grow the current pane into the full tree and apply each ratio. Used
    /// by the window-restoration `didDecodeRestorableState` path — the OS has
    /// already recreated (or, for us, is reusing) the window, and we refill its
    /// content. The tab's current pane becomes the tree's leftmost leaf.
    pub fn restore_content_into_tab(&self, tab: TabId, session: &crate::session::WindowSession) {
        let anchor = {
            let state = self.0.borrow();
            match state.tabs.get(&tab) {
                Some(t) => t.tree.focused(),
                None => return,
            }
        };
        self.rebuild_session_tree(tab, &session.tree, anchor);
        self.apply_session_ratios(tab, &session.tree, &mut Vec::new());
    }

    /// The active tab and its window — the window macOS restoration reuses for a
    /// restored session (handing back the launch window instead of creating a
    /// duplicate). `None` if there is no active tab.
    pub fn active_window_and_tab(&self) -> Option<(Retained<NSWindow>, TabId)> {
        let tab = self.active_tab()?;
        let state = self.0.borrow();
        Some((state.tabs.get(&tab)?.window.clone(), tab))
    }

    /// The resolved click-repeat interval (the `click-repeat-interval` config
    /// value if set, else the OS/default) — the observable proof the config key
    /// is wired into double/triple-click detection (smoke/test).
    pub fn mouse_interval(&self) -> std::time::Duration {
        self.0.borrow().mouse_interval
    }

    /// The active window's restoration `identifier` (the observable proof that
    /// the restorable-window wiring set it), or `None` if unset (smoke/test).
    pub fn active_window_identifier(&self) -> Option<String> {
        let tab = self.active_tab()?;
        let state = self.0.borrow();
        let id = state.tabs.get(&tab)?.window.identifier()?;
        Some(id.to_string())
    }

    /// Move focus to the spatially-adjacent pane of `tab` in `direction`, if one
    /// exists. No-op otherwise (mirrors upstream's performable check).
    pub fn goto_split(&self, tab: TabId, direction: Direction) {
        // Directional navigation unzooms (upstream `ghosttyDidFocusSplit` with
        // `split-preserve-zoom` off, its default — we don't expose that config).
        // Unzoom first so the spatial neighbour is computed against the real
        // split layout (a zoomed layout has only one pane → no neighbours).
        let (target, was_zoomed) = {
            let mut state = self.0.borrow_mut();
            let Some(t) = state.tabs.get_mut(&tab) else {
                return;
            };
            let was_zoomed = t.tree.is_zoomed();
            if was_zoomed {
                t.tree.unzoom();
            }
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
            (t.tree.neighbor(direction, &layout), was_zoomed)
        };
        // If we unzoomed, re-lay-out to reveal the restored split (upstream
        // unzooms on directional nav regardless of whether a neighbour exists).
        if was_zoomed {
            let mtm = self.0.borrow().mtm;
            let controller_ptr: *const Controller = self;
            let mut state = self.0.borrow_mut();
            if let Some(t) = state.tabs.get_mut(&tab) {
                t.relayout(controller_ptr, tab, mtm);
            }
        }
        if let Some(target) = target {
            self.focus_surface_in_tab(tab, target);
        }
    }

    /// Move focus to the previous / next pane of `tab` in flatten order (wraps).
    pub fn goto_adjacent(&self, tab: TabId, seq: Sequential) {
        // Like directional nav, prev/next unzooms (upstream default).
        let (target, was_zoomed) = {
            let mut state = self.0.borrow_mut();
            let Some(t) = state.tabs.get_mut(&tab) else {
                return;
            };
            let was_zoomed = t.tree.is_zoomed();
            if was_zoomed {
                t.tree.unzoom();
            }
            (t.tree.adjacent(seq), was_zoomed)
        };
        if was_zoomed {
            let mtm = self.0.borrow().mtm;
            let controller_ptr: *const Controller = self;
            let mut state = self.0.borrow_mut();
            if let Some(t) = state.tabs.get_mut(&tab) {
                t.relayout(controller_ptr, tab, mtm);
            }
        }
        if let Some(target) = target {
            self.focus_surface_in_tab(tab, target);
        }
    }

    /// Toggle zoom on `tab`'s focused pane (upstream `toggle_split_zoom`): the
    /// focused pane fills the whole content area (all others hidden), or, if it
    /// was already zoomed, restores the split layout. A single-pane tab can't
    /// zoom (no-op). Re-lays-out so the zoomed pane reflows to the full container
    /// and the hidden panes are removed from view; focus stays on the same pane.
    pub fn toggle_split_zoom(&self, tab: TabId) {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;
        let mut state = self.0.borrow_mut();
        let Some(t) = state.tabs.get_mut(&tab) else {
            return;
        };
        t.tree.toggle_zoom();
        t.relayout(controller_ptr, tab, mtm);
    }

    /// Resize `tab`'s focused split in `direction` by the fixed step (upstream
    /// `resize_split`, 10pt): move the nearest ancestor split of the matching
    /// axis, then re-lay-out both adjacent panes (engine + PTY resize). Resets
    /// zoom. No-op if there's no matching-axis split to resize.
    pub fn resize_split(&self, tab: TabId, direction: Direction) {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;
        let mut state = self.0.borrow_mut();
        let Some(t) = state.tabs.get_mut(&tab) else {
            return;
        };
        // tmux owns the layout (I3): mutating the native tree on a tmux tab would
        // desync and snap back. The divider *drag* routes to `resize-pane`;
        // keyboard split-resize is a no-op here until it's wired to a relative
        // `resize-pane -L/-R/-U/-D` (follow-up).
        if t.surfaces.values().any(|s| s.display_source().is_some()) {
            return;
        }
        let scale = t.window.backingScaleFactor();
        let bounds = t.container.bounds();
        let container_px = crate::splits::Rect::new(
            0.0,
            0.0,
            bounds.size.width * scale,
            bounds.size.height * scale,
        );
        let divider_px = (DIVIDER_THICKNESS_PT * scale).round();
        // The step is in points; scale to device pixels to match the tree's
        // pixel-space geometry (same convention as `drag_divider`).
        let step_px = crate::splitkeys::RESIZE_STEP_PT * scale;
        t.tree
            .resize_dir(direction, step_px, container_px, divider_px);
        t.relayout(controller_ptr, tab, mtm);
    }

    /// Equalize `tab`'s splits (upstream `equalize_splits`): every split's ratio
    /// becomes its children's leaf-count weight, then re-lay-out. Preserves zoom.
    pub fn equalize_splits(&self, tab: TabId) {
        let mtm = self.0.borrow().mtm;
        let controller_ptr: *const Controller = self;
        let mut state = self.0.borrow_mut();
        let Some(t) = state.tabs.get_mut(&tab) else {
            return;
        };
        // I3: tmux owns a tmux tab's layout; equalizing the native tree would
        // desync. No-op here (a tmux `select-layout even-*` wiring is a follow-up).
        if t.surfaces.values().any(|s| s.display_source().is_some()) {
            return;
        }
        t.tree.equalize();
        t.relayout(controller_ptr, tab, mtm);
    }

    /// Close a surface (pane) within `tab`: collapse its parent split so the
    /// sibling absorbs the space, drop the pane's view + IO, re-lay-out, and move
    /// focus to the sibling. If it was the tab's last pane, close the whole tab
    /// (today's behaviour). Called on `cmd+w` (focused pane) and on a pane's
    /// shell exit.
    pub fn close_surface(&self, tab: TabId, surface: SurfaceId) {
        // tmux-managed tab: redirect the pane close to a tmux `kill-pane`. tmux's
        // `%layout-change` (or `%window-close` for the last pane) reconcile then
        // removes the native surface/tab — closing it natively here would fight
        // that reconcile (ADR 006 slice 5e).
        if let Some(src) = self.tmux_pane_of(tab, surface) {
            self.redirect_tmux_action(src, surface, TmuxNativeAction::KillPane);
            return;
        }

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
            // Per-pane focus reporting: note who was focused before the close so
            // we only send focus-IN to the sibling if focus actually MOVED to it
            // (closing an unfocused, e.g. crashed/exited, pane leaves focus put).
            let previous_focus = t.tree.focused();
            match t.tree.close(surface) {
                None => Outcome::CloseTab,
                Some(new_focus) => {
                    // Remove the pane bundle (drops its view + joins io threads).
                    // The closed pane's io is torn down, so no focus-out for it.
                    if let Some(dead) = t.surfaces.remove(&surface) {
                        dead.view.removeFromSuperview();
                    } else {
                        return; // already gone
                    }
                    t.relayout(controller_ptr, tab, mtm);
                    // If focus moved to the sibling (the closed pane was the
                    // focused one), send that sibling focus-IN (mode-1004 + poll).
                    if previous_focus == surface
                        && new_focus != surface
                        && let Some(next) = t.surfaces.get_mut(&new_focus)
                    {
                        next.set_focus(true);
                    }
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
        let ratio = crate::splits::clamp_ratio((coord_px - origin) / span);

        // On a tmux-window tab, tmux owns the layout (I3): translate the drag
        // into a `resize-pane` and let the `%layout-change` reconcile move the
        // native divider — do NOT mutate the tree directly (it would desync and
        // snap back, and the pane terminals wouldn't reflow). `None` = ordinary
        // tab; do the native ratio drag.
        let tmux_resize = if let Some(src) = t.surfaces.values().find_map(|s| s.display_source()) {
            let first_extent = ratio * span;
            let (cw, ch) = t
                .surfaces
                .values()
                .next()
                .map(|s| {
                    (
                        s.font.cell_width.max(1) as f64,
                        s.font.cell_height.max(1) as f64,
                    )
                })
                .unwrap_or((1.0, 1.0));
            // Probe the centre of the first (before-side) child to pick a pane in
            // that row/column; tmux resizes its shared border. Target the child's
            // new cell extent along the split axis.
            let (probe_x, probe_y, width, height) = match axis {
                crate::splits::Axis::Horizontal => (
                    origin + first_extent * 0.5,
                    split_rect.y + split_rect.h * 0.5,
                    Some((first_extent / cw).max(1.0) as usize),
                    None,
                ),
                crate::splits::Axis::Vertical => (
                    split_rect.x + split_rect.w * 0.5,
                    origin + first_extent * 0.5,
                    None,
                    Some((first_extent / ch).max(1.0) as usize),
                ),
            };
            let layout = t.tree.layout(container_px, divider_px);
            t.tree
                .hit_test(probe_x, probe_y, &layout)
                .map(|pane_surface| (src, pane_surface, width, height))
        } else {
            t.tree.set_ratio(path, ratio);
            t.relayout(controller_ptr, tab, mtm);
            None
        };
        drop(state);
        if let Some((src, pane_surface, width, height)) = tmux_resize {
            self.redirect_tmux_resize_pane(src, pane_surface, width, height);
        }
    }

    /// tmux lifecycle smoke (`QWERTTY_TERM_SMOKE_TMUXLIFE`). Drives the app's
    /// *own* actions against a real `tmux -CC` — the same entry points Cmd-T and
    /// a tab close use — dumping observable state between steps.
    ///
    /// This exists because the tmux lifecycle bugs only reproduce through real
    /// GUI interaction: the model-level test (`tests/tmux_real.rs`) drives the
    /// Viewer/session directly and so cannot see the native tab layer, and an
    /// external `tmux new-window` doesn't exercise the app's own command queue
    /// and focus sync the way Cmd-T does. Steps are timer-chained because every
    /// tmux action round-trips through the control pty asynchronously.
    fn run_tmuxlife_smoke(&self) -> bool {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static STEP: AtomicUsize = AtomicUsize::new(0);
        let step = STEP.fetch_add(1, Ordering::Relaxed);
        match step {
            0 => {
                self.dump_tmux_state("1: after `tmux -CC` settled");
                // Cmd-T equivalent, through the very same entry point.
                if let Some(active) = self.active_tab() {
                    eprintln!("TMUXLIFE action: new_tab_in({}) [Cmd-T]", active.0);
                    self.new_tab_in(active);
                } else {
                    eprintln!("TMUXLIFE FAIL: no active tab for Cmd-T");
                }
            }
            1 => {
                self.dump_tmux_state("2: after Cmd-T");
                // Close the most recently created tmux-managed tab — what the
                // user does when they close the tab Cmd-T just opened.
                let target = {
                    let state = self.0.borrow();
                    let mut managed: Vec<TabId> = state
                        .tabs
                        .iter()
                        .filter(|(_, t)| t.surfaces.values().any(|s| s.display_source().is_some()))
                        .map(|(id, _)| *id)
                        .collect();
                    managed.sort_by_key(|t| t.0);
                    managed.last().copied()
                };
                match target {
                    Some(tab) => {
                        eprintln!(
                            "TMUXLIFE action: close_tab_confirmed({}) [close 2nd tab]",
                            tab.0
                        );
                        self.close_tab_confirmed(tab);
                    }
                    None => eprintln!("TMUXLIFE FAIL: no tmux-managed tab to close"),
                }
            }
            _ => {
                self.dump_tmux_state("3: after closing the second tab");
                // Assert the invariants that were actually broken in the field.
                let (tmux_tabs, control_visible, survivor_has_text) = {
                    let state = self.0.borrow();
                    let managed: Vec<&Tab> = state
                        .tabs
                        .values()
                        .filter(|t| t.surfaces.values().any(|s| s.display_source().is_some()))
                        .collect();
                    let control_visible = state.tabs.values().any(|t| {
                        !t.surfaces.values().any(|s| s.display_source().is_some())
                            && t.surfaces.values().any(|s| s.tmux_session().is_some())
                            && t.window.isVisible()
                    });
                    let has_text = managed.iter().any(|t| {
                        t.surfaces.keys().any(|sid| {
                            state
                                .tabs
                                .values()
                                .filter_map(|c| c.surfaces.values().find_map(|s| s.tmux_session()))
                                .any(|sess| {
                                    sess.pane_terminal(*sid)
                                        .is_some_and(|pt| !pt.plain_string().trim().is_empty())
                                })
                        })
                    });
                    (managed.len(), control_visible, has_text)
                };
                if tmux_tabs != 1 {
                    tmuxlife_fail(format!(
                        "expected exactly 1 surviving tmux tab after closing the \
                         second, found {tmux_tabs} — closing one tab must not tear \
                         down the others"
                    ));
                }
                if control_visible {
                    tmuxlife_fail(
                        "the raw `tmux -CC` control tab is on screen while a tmux \
                         window tab still exists. AppKit surfaces a sibling tab when \
                         one closes, so the control tab must be re-hidden — otherwise \
                         the user is left looking at the control surface (grid \
                         painting suppressed: stale text, no prompt) instead of their \
                         shell"
                            .to_string(),
                    );
                }
                if !survivor_has_text {
                    tmuxlife_fail(
                        "the surviving tmux tab's pane terminal is empty — it should \
                         still show the shell prompt"
                            .to_string(),
                    );
                }
                eprintln!("TMUXLIFE ok: 1 tmux tab survived, control tab hidden, pane has content");
                std::process::exit(0);
            }
        }
        // Chain the next step once tmux has round-tripped.
        true
    }

    /// Print the observable tmux/tab state: every tab, whether it mirrors a tmux
    /// window, and the *visible text* of each pane — for a display pane that is
    /// the Viewer-owned pane `Terminal` it actually renders, not its own (empty)
    /// engine. Used by the tmux lifecycle smoke so a headless run can see what a
    /// human would see (e.g. "is there a shell prompt in the surviving tab?"),
    /// rather than inferring it from tab counts.
    pub fn dump_tmux_state(&self, label: &str) {
        let state = self.0.borrow();
        let active = state.registry.active();
        eprintln!(
            "=== TMUXSTATE {label} === tabs={} active={:?}",
            state.tabs.len(),
            active.map(|t| t.0)
        );
        let mut ids: Vec<TabId> = state.tabs.keys().copied().collect();
        ids.sort_by_key(|t| t.0);
        for tid in ids {
            let Some(t) = state.tabs.get(&tid) else {
                continue;
            };
            let managed = t.surfaces.values().any(|s| s.display_source().is_some());
            let visible = t.window.isVisible();
            eprintln!(
                "  tab {} tmux_managed={} visible={} surfaces={}",
                tid.0,
                managed,
                visible,
                t.surfaces.len()
            );
            let mut sids: Vec<SurfaceId> = t.surfaces.keys().copied().collect();
            sids.sort_by_key(|s| s.0);
            for sid in sids {
                let Some(surface) = t.surfaces.get(&sid) else {
                    continue;
                };
                // A display pane renders a foreign terminal; read *that*, since
                // its own engine is intentionally empty.
                let text = match surface.display_source() {
                    Some(src) => state
                        .tabs
                        .get(&src.control_tab)
                        .and_then(|ct| ct.surfaces.get(&src.control_surface))
                        .and_then(|cs| cs.tmux_session())
                        .and_then(|sess| sess.pane_terminal(sid))
                        .map(|t| t.plain_string())
                        .unwrap_or_else(|| "<no pane terminal>".to_string()),
                    None => surface.engine().plain_string(),
                };
                let last: Vec<&str> = text
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .rev()
                    .take(2)
                    .collect();
                let tail: Vec<String> = last
                    .into_iter()
                    .rev()
                    .map(|l| l.trim_end().to_string())
                    .collect();
                eprintln!(
                    "    surface {} display={} tail={:?}",
                    sid.0,
                    surface.display_source().is_some(),
                    tail
                );
            }
        }
    }

    /// Redirect a native split-divider drag on a tmux tab to a tmux `resize-pane`
    /// (ADR 006 — I3: tmux owns the layout). `pane_surface` is a display pane on
    /// the before-side of the divider; `width`/`height` its target cell extent.
    /// The native divider is moved by the follow-up `%layout-change` reconcile;
    /// the caller must NOT mutate the native tree. No-op if the control surface
    /// is gone.
    fn redirect_tmux_resize_pane(
        &self,
        control: DisplaySource,
        pane_surface: SurfaceId,
        width: Option<usize>,
        height: Option<usize>,
    ) {
        let mut state = self.0.borrow_mut();
        if let Some(cs) = state
            .tabs
            .get_mut(&control.control_tab)
            .and_then(|t| t.surfaces.get_mut(&control.control_surface))
        {
            let commands = cs
                .tmux
                .as_mut()
                .map(|sess| sess.resize_pane(pane_surface, width, height))
                .unwrap_or_default();
            for cmd in &commands {
                cs.send_pty(cmd);
            }
        }
    }
}

/// Which way a font-size step goes.
enum FontStep {
    Up,
    Down,
    Reset,
}

/// Map a live split-tree node to its serializable session form, resolving each
/// leaf's working directory from its surface's engine (OSC 7).
/// The `NSCoder` key our window-session JSON is archived under in the macOS
/// window-restoration state. A namespaced constant so a future key can't
/// silently collide with AppKit's own restorable-state keys.
const SESSION_CODER_KEY: &str = "term.qwertty.windowSession";

/// The restoration `identifier` set on every restorable window, matched by the
/// app delegate's `restoreWindowWithIdentifier:` on relaunch.
const WINDOW_RESTORATION_IDENTIFIER: &str = "term.qwertty.mainWindow";

fn node_to_session(node: &crate::splits::Node, t: &Tab) -> crate::session::SessionNode {
    use crate::session::{SessionAxis, SessionNode};
    use crate::splits::{Axis, Node};
    match node {
        Node::Leaf(sid) => SessionNode::Leaf {
            cwd: t.surfaces.get(sid).and_then(|s| s.engine().pwd()),
        },
        Node::Split(split) => SessionNode::Split {
            axis: match split.axis {
                Axis::Horizontal => SessionAxis::Horizontal,
                Axis::Vertical => SessionAxis::Vertical,
            },
            ratio: split.ratio,
            first: Box::new(node_to_session(&split.first, t)),
            second: Box::new(node_to_session(&split.second, t)),
        },
    }
}

/// The working directory of a session tree's leftmost leaf — the cwd a
/// single-pane restore spawns the shell in.
fn leftmost_cwd(node: &crate::session::SessionNode) -> Option<String> {
    use crate::session::SessionNode;
    match node {
        SessionNode::Leaf { cwd } => cwd.clone(),
        SessionNode::Split { first, .. } => leftmost_cwd(first),
    }
}

/// Apply `window-save-state` to the `NSQuitAlwaysKeepsWindows` standard user
/// default that AppKit reads at quit to decide whether to persist/restore
/// windows: `never` → false, `always` → true, `default` → remove the override
/// (system setting applies). Port of upstream `AppDelegate`'s equivalent.
fn apply_window_save_state_default(state: crate::config::WindowSaveState) {
    use crate::config::WindowSaveState as W;
    let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
    let key = NSString::from_str("NSQuitAlwaysKeepsWindows");
    match state {
        W::Never => defaults.setBool_forKey(false, &key),
        W::Always => defaults.setBool_forKey(true, &key),
        W::Default => defaults.removeObjectForKey(&key),
    }
}

/// Build the resize-overlay HUD label: a rounded, semi-transparent dark badge
/// with centered white text, hidden until first shown. Non-interactive.
fn make_resize_overlay_field(mtm: MainThreadMarker) -> Retained<objc2_app_kit::NSTextField> {
    let field = objc2_app_kit::NSTextField::new(mtm);
    field.setEditable(false);
    field.setSelectable(false);
    field.setBezeled(false);
    field.setBordered(false);
    field.setDrawsBackground(true);
    field.setBackgroundColor(Some(&NSColor::colorWithSRGBRed_green_blue_alpha(
        0.0, 0.0, 0.0, 0.72,
    )));
    field.setTextColor(Some(&NSColor::whiteColor()));
    field.setAlignment(objc2_app_kit::NSTextAlignment::Center);
    field.setHidden(true);
    field.setWantsLayer(true);
    if let Some(layer) = field.layer() {
        layer.setCornerRadius(5.0);
        layer.setMasksToBounds(true);
    }
    field
}

/// Build an `NSWindow` sized to the initial content, tabbing-enabled, hosting
/// `content_view` (the tab's split container) as its content view. `tabbing_mode`
/// comes from `window-show-tab-bar` (`Auto`→`.automatic`, `Always`→`.preferred`,
/// `Never`→`.disallowed`).
fn make_window(
    mtm: MainThreadMarker,
    content_view: &NSView,
    tabbing_mode: NSWindowTabbingMode,
) -> Retained<NSWindow> {
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
        window.setTitle(&NSString::from_str("qwertty-term"));
        // Native tabbing: `.automatic` (the AppKit default) lets a lone window
        // stay tab-bar-free, matching macOS convention (and real Ghostty) —
        // the tab bar only appears once a window has 2+ tabs. This does not
        // affect Cmd-T: `new_tab_in` always calls `addTabbedWindow:ordered:`
        // explicitly (below), which groups windows into the same tabbed
        // window regardless of `tabbingMode`; that mode only governs the
        // *implicit* behavior AppKit applies to windows opened without an
        // explicit group (e.g. Cmd-N's `new_window`). The default `.automatic`
        // keeps a lone window tab-bar-free. `window-show-tab-bar = always`
        // (`.preferred`) forces the tab bar always-on even for a single window
        // (what used to be the unconditional default — the reported "empty dark
        // strip" bug when it wasn't opt-in); `never` (`.disallowed`) suppresses
        // native tabbing entirely so new tabs open as windows.
        window.setTabbingMode(tabbing_mode);
        window.setContentView(Some(content_view));
        window.setReleasedWhenClosed(false);
    }
    window
}

/// Apply the `window-theme` appearance to `window`. `bg` is the terminal
/// background (0–255 sRGB), used only for the `auto` luminosity decision.
/// Mirrors upstream `NSAppearance+Extension.swift`:
///
/// - `Light` → aqua; `Dark` → darkAqua.
/// - `Auto` → aqua when the background is light (luminance > 0.5), else darkAqua.
/// - `System` / `Ghostty` → `None` (clear any override; follow the system).
fn apply_window_theme(window: &NSWindow, theme: crate::config::WindowTheme, bg: (u8, u8, u8)) {
    use crate::config::WindowTheme as T;
    use objc2_app_kit::NSAppearance;
    use objc2_app_kit::NSAppearanceCustomization;
    use objc2_app_kit::NSAppearanceNameAqua;
    use objc2_app_kit::NSAppearanceNameDarkAqua;

    // Which built-in appearance to force, or `None` to follow the system.
    // SAFETY: reading the framework's `NSAppearanceName` string statics.
    let name = unsafe {
        match theme {
            T::Light => Some(NSAppearanceNameAqua),
            T::Dark => Some(NSAppearanceNameDarkAqua),
            T::Auto if background_is_light(bg) => Some(NSAppearanceNameAqua),
            T::Auto => Some(NSAppearanceNameDarkAqua),
            // `System` and (on macOS) `Ghostty` follow the system appearance.
            T::System | T::Ghostty => None,
        }
    };
    let appearance = name.and_then(NSAppearance::appearanceNamed);
    window.setAppearance(appearance.as_deref());
}

/// Whether a background color reads as "light" — perceived luminance > 0.5,
/// matching upstream `OSColor.isLightColor` / `.luminance`
/// (`OSColor+Extension.swift`: `0.299r + 0.587g + 0.114b`).
fn background_is_light((r, g, b): (u8, u8, u8)) -> bool {
    let lum = 0.299 * r as f64 + 0.587 * g as f64 + 0.114 * b as f64;
    lum / 255.0 > 0.5
}

/// Hide the three standard traffic-light window buttons (close/miniaturize/
/// zoom) for `macos-window-buttons = hidden` (upstream
/// `TerminalWindow.swift:570-573`).
fn hide_window_buttons(window: &NSWindow) {
    use objc2_app_kit::NSWindowButton;
    for button in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        if let Some(b) = window.standardWindowButton(button) {
            b.setHidden(true);
        }
    }
}

/// The `NSPopUpMenuWindowLevel` — high enough to render over the menu bar and
/// off-screen, which upstream found is the level a dropdown needs
/// (`QuickTerminalController.swift:445`, `window.level = .popUpMenu`). The
/// AppKit constant isn't surfaced by objc2, so use its documented raw value.
const POPUP_MENU_WINDOW_LEVEL: NSWindowLevel = 101;

/// Build the borderless, key-capable window that hosts the quick-terminal
/// surface. Borderless so it reads as a dropdown (no titlebar/controls);
/// [`QuickTerminalWindow`] overrides `canBecomeKeyWindow` so typing works;
/// `Resizable` in the mask so `setFrame` animates the size cleanly. Never
/// added to a native tab group.
fn make_quick_terminal_window(mtm: MainThreadMarker, content_view: &NSView) -> Retained<NSWindow> {
    let content = NSRect::new(
        NSPoint::new(0.0, 0.0),
        NSSize::new(INITIAL_WIDTH, INITIAL_HEIGHT),
    );
    // Borderless + resizable/titled-less. A borderless window can still be
    // resized programmatically via setFrame (needed for the slide animation).
    let style = NSWindowStyleMask::Borderless | NSWindowStyleMask::Resizable;

    // `.set_ivars(())` converts the `Allocated` into the `PartialInit` receiver
    // objc2 requires for a super-init of a defined class (the subclass has no
    // ivars, so `()`).
    let alloc = QuickTerminalWindow::alloc(mtm).set_ivars(());
    // SAFETY: standard designated NSWindow initializer via the subclass.
    let window: Retained<QuickTerminalWindow> = unsafe {
        msg_send![
            super(alloc),
            initWithContentRect: content,
            styleMask: style,
            backing: NSBackingStoreType::Buffered,
            defer: false
        ]
    };
    let window: Retained<NSWindow> = window.into_super();
    unsafe {
        window.setReleasedWhenClosed(false);
        // Never participates in native tabbing.
        window.setTabbingMode(NSWindowTabbingMode::Disallowed);
        window.setContentView(Some(content_view));
    }
    window
}

/// Slide `window` to `frame` and fade to `alpha` over `duration` seconds,
/// ease-in (upstream `QuickTerminalController` animates at
/// `quick-terminal-animation-duration` with `.easeIn`). If `completion` is
/// given it runs on the main thread when the animation ends (used to
/// `orderOut` the dropdown after it slides away). No controller borrow is held
/// across this call — the completion re-enters AppKit safely.
fn run_window_slide(
    window: &NSWindow,
    frame: NSRect,
    alpha: f64,
    duration: f64,
    completion: Option<Box<dyn Fn()>>,
) {
    let animator = window.animator();
    let changes = block2::RcBlock::new(move |ctx: core::ptr::NonNull<NSAnimationContext>| {
        // SAFETY: `ctx` is the live animation context AppKit passes in.
        let ctx = unsafe { ctx.as_ref() };
        ctx.setDuration(duration);
        let ease_in = unsafe {
            objc2_quartz_core::CAMediaTimingFunction::functionWithName(
                objc2_quartz_core::kCAMediaTimingFunctionEaseIn,
            )
        };
        ctx.setTimingFunction(Some(&ease_in));
        // The animator proxy makes these property sets animate.
        animator.setFrame_display(frame, true);
        animator.setAlphaValue(alpha);
    });
    // The completion (if any) is already a `Box<dyn Fn()>`, whose `IntoBlock`
    // `Dyn` is `dyn Fn()`, so `RcBlock::new` yields the `RcBlock<dyn Fn()>` the
    // AppKit binding expects directly.
    let completion_block = completion.map(block2::RcBlock::new);
    NSAnimationContext::runAnimationGroup_completionHandler(&changes, completion_block.as_deref());
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
    #[name = "QwerttyTermWindowDelegate"]
    #[ivars = WindowDelegateIvars]
    #[thread_kind = MainThreadOnly]
    pub struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        /// The window's close button (or Cmd-W routed to the window) was hit.
        ///
        /// Three cases (ADR 006 tmux lifecycle):
        /// - **tmux-window tab** (gap 1): redirect the close to a tmux
        ///   `kill-window` and **veto** the native close (`false`) — tmux owns
        ///   the layout, so the native tab is removed by the resulting
        ///   `%window-close` / `list-windows` reconcile, not here (I3).
        /// - **tmux control tab** (gap 4): closing the `tmux -CC` host window
        ///   ends the session; tear down its mirrored tmux tabs (so none are
        ///   orphaned), then proceed with the close (the pty close detaches the
        ///   client cleanly).
        /// - **ordinary tab**: gate on `confirm-close-surface`. Returning
        ///   `false` vetoes the close (the user cancelled); `true` proceeds.
        #[unsafe(method(windowShouldClose:))]
        fn window_should_close(&self, _sender: &NSObject) -> bool {
            let ivars = self.ivars();
            let controller = &ivars.controller;
            let tab = ivars.tab;
            // Gap 1: a tmux-window tab close becomes `kill-window`; veto the
            // native close so the reconcile removes the tab.
            if let Some((ctrl_tab, ctrl_surface, window_id)) = controller.tmux_window_tab(tab) {
                controller.redirect_tmux_kill_window(
                    DisplaySource {
                        control_tab: ctrl_tab,
                        control_surface: ctrl_surface,
                    },
                    window_id,
                );
                false
            } else if controller.is_tmux_managed_tab(tab) {
                // A tmux-window tab we could not resolve to a window id: veto the
                // close instead of falling through to the window path below,
                // which would show the close-*window* dialog and can tear down
                // every mirrored tab.
                false
            } else {
                // Gap 4: closing the control window ends the session — tear the
                // mirrored tmux tabs down before the window goes.
                if let Some(ctrl_surface) = controller.tmux_control_surface_of(tab) {
                    controller.teardown_tmux_control(tab, ctrl_surface);
                }
                controller.confirm_close_window(tab)
            }
        }

        /// The window (tab) became key: mark its tab active in the controller and
        /// route focus-IN to its focused pane (per-pane mode-1004 reporting +
        /// password poll). A focused-pane 1004 app sees `CSI I` on window focus.
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            let ivars = self.ivars();
            ivars.controller.set_active(ivars.tab);
            ivars.controller.tab_window_focus(ivars.tab, true);
        }

        /// The window (tab) resigned key (another window/app took focus): route
        /// focus-OUT to this tab's focused pane only (`CSI O` under mode 1004,
        /// password poll off). The other panes are already unfocused. This is the
        /// window half of upstream `Surface.focusCallback(false)`.
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            let ivars = self.ivars();
            ivars.controller.tab_window_focus(ivars.tab, false);
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

        /// `window-save-state`: encode this window's restorable session (its
        /// split tree + per-pane cwd) into the state coder. macOS calls this when
        /// it snapshots restorable state (window-save-state must not be `never`,
        /// which leaves the window non-restorable so this never fires).
        #[unsafe(method(window:willEncodeRestorableState:))]
        fn window_will_encode_restorable_state(&self, _window: &NSWindow, state: &NSCoder) {
            let ivars = self.ivars();
            ivars.controller.encode_session_into(state, ivars.tab);
        }

        /// `window-save-state`: decode a session previously encoded above and
        /// rebuild its split content into this window's tab. Fires when macOS
        /// hands a restored window its saved state on relaunch.
        #[unsafe(method(window:didDecodeRestorableState:))]
        fn window_did_decode_restorable_state(&self, _window: &NSWindow, state: &NSCoder) {
            let ivars = self.ivars();
            if let Some(session) = Controller::decode_session_from(state) {
                ivars
                    .controller
                    .restore_content_into_tab(ivars.tab, &session);
            }
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
// Quick terminal (dropdown) — window subclass + delegate
// ---------------------------------------------------------------------------

define_class!(
    // SAFETY: NSWindow subclass overriding only `canBecomeKeyWindow`/
    // `canBecomeMainWindow` (a borderless window returns NO by default, which
    // would keep keystrokes from ever reaching the dropdown). No ivars, no
    // unsafe Drop.
    #[unsafe(super(NSWindow))]
    #[name = "QwerttyTermQuickTerminalWindow"]
    #[thread_kind = MainThreadOnly]
    pub struct QuickTerminalWindow;

    impl QuickTerminalWindow {
        /// A borderless window must opt in to key status, else the terminal
        /// view never becomes first responder and no typing works.
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key_window(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main_window(&self) -> bool {
            true
        }
    }
);

/// Ivars for the quick-terminal window delegate: just the controller (the QT
/// tab id is the fixed [`QUICK_TERMINAL_TAB`]).
pub struct QuickTermDelegateIvars {
    controller: Controller,
}

define_class!(
    // SAFETY: NSObject subclass implementing NSWindowDelegate; no unsafe Drop.
    #[unsafe(super(NSObject))]
    #[name = "QwerttyTermQuickTermDelegate"]
    #[ivars = QuickTermDelegateIvars]
    #[thread_kind = MainThreadOnly]
    pub struct QuickTermDelegate;

    unsafe impl NSObjectProtocol for QuickTermDelegate {}

    unsafe impl NSWindowDelegate for QuickTermDelegate {
        /// The dropdown became key: route focus-IN to its surface (mode-1004
        /// reporting + password poll), matching a normal tab's window. It is
        /// deliberately NOT marked the *active* registry tab (the QT isn't in
        /// the registry).
        #[unsafe(method(windowDidBecomeKey:))]
        fn window_did_become_key(&self, _notification: &NSNotification) {
            self.ivars()
                .controller
                .tab_window_focus(QUICK_TERMINAL_TAB, true);
        }

        /// The dropdown resigned key (focus moved elsewhere): route focus-OUT,
        /// and — when `quick-terminal-autohide` is on — animate it back out of
        /// view (upstream `quick-terminal-autohide`, default true on macOS).
        #[unsafe(method(windowDidResignKey:))]
        fn window_did_resign_key(&self, _notification: &NSNotification) {
            let controller = self.ivars().controller.clone();
            controller.tab_window_focus(QUICK_TERMINAL_TAB, false);
            controller.quick_terminal_autohide_on_resign();
        }

        /// Reflow on resize / backing-scale change, same as a normal window.
        #[unsafe(method(windowDidResize:))]
        fn window_did_resize(&self, _notification: &NSNotification) {
            self.ivars()
                .controller
                .resync_tab_geometry(QUICK_TERMINAL_TAB);
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn window_did_change_backing_properties(&self, _notification: &NSNotification) {
            self.ivars()
                .controller
                .resync_tab_geometry(QUICK_TERMINAL_TAB);
        }
    }
);

impl QuickTermDelegate {
    fn new(mtm: MainThreadMarker, controller: Controller) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(QuickTermDelegateIvars { controller });
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
    /// Tab-strip geometry smoke (`QWERTTY_TERM_SMOKE_GEOMETRY`): dump + assert the
    /// window geometry across the 1-tab → 2-tab → 1-tab transition, then exit.
    /// See [`AppDelegate::run_geometry_smoke`].
    smoke_geometry: bool,
    /// Tab-navigation keybind smoke (`QWERTTY_TERM_SMOKE_TABKEYS`): open 3 tabs,
    /// drive the built-in tab chords, and assert the active-tab index after each
    /// (plus the pty-encoding regression: tab chords send nothing, plain Tab /
    /// Shift+Tab still encode). See [`AppDelegate::run_tabkeys_smoke`].
    smoke_tabkeys: bool,
    /// Splits smoke (`QWERTTY_TERM_SMOKE_SPLITS`): split right then down (3 panes),
    /// assert 3 live shells with isolated input, directional focus walk, divider
    /// resize, and close-collapse. See [`AppDelegate::run_splits_smoke`].
    smoke_splits: bool,
    /// Splits smoke phase state carried from phase 1 to phase 2: the tab under
    /// test and each pane's `(SurfaceId, unique marker)`.
    splits_state: RefCell<Option<SplitsSmokeState>>,
    /// Keybind smoke (`QWERTTY_TERM_SMOKE_KEYBIND`): drive the seeded shift+enter
    /// `text:` binding + a plain enter through the real key path and assert the
    /// pty round-trip. See [`AppDelegate::run_keybind_smoke`].
    smoke_keybind: bool,
    /// Focus-reporting smoke (`QWERTTY_TERM_SMOKE_FOCUS`): two `cat -v` panes with
    /// mode 1004 on; focus-switch and assert focus-in/out bytes reach the right
    /// ptys. See [`AppDelegate::run_focus_smoke`].
    smoke_focus: bool,
    /// Focus smoke phase state: the tab + the two panes' `(SurfaceId)` ids.
    focus_state: RefCell<Option<(TabId, SurfaceId, SurfaceId)>>,
    /// Search smoke (`QWERTTY_TERM_SMOKE_SEARCH`): fill scrollback with 3
    /// markers, Cmd+F, type the needle, assert the counter reads 3, navigate,
    /// and assert Escape restores PTY input. See [`AppDelegate::run_search_smoke`].
    smoke_search: bool,
    /// Search smoke phase state: the tab + focused surface under test.
    search_state: RefCell<Option<(TabId, SurfaceId)>>,
    /// Selection smoke (`QWERTTY_TERM_SMOKE_SELECTION`): drive synthetic mouse
    /// gestures (double/triple click, drag, shift-extend, edge autoscroll)
    /// through the real event path and assert the selection text. See
    /// [`AppDelegate::run_selection_smoke`].
    smoke_selection: bool,
    /// Title smoke (`QWERTTY_TERM_SMOKE_TITLE`): feed OSC 2 titles into two
    /// tabs and assert per-tab window/tab titles + the ghost-emoji fallback.
    /// See [`AppDelegate::run_title_smoke`].
    smoke_title: bool,
    /// Quick-terminal smoke (`QWERTTY_TERM_SMOKE_QUICKTERM`): toggle the
    /// dropdown in/out and assert visibility, geometry, and a live shell. See
    /// [`AppDelegate::run_quickterm_smoke`].
    smoke_quickterm: bool,
    /// Bell smoke (`QWERTTY_TERM_SMOKE_BELL`): feed a BEL and assert the tab's
    /// 🔔 title indicator appears then clears on refocus. See
    /// [`AppDelegate::run_bell_smoke`].
    smoke_bell: bool,
    /// Mouse smoke (`QWERTTY_TERM_SMOKE_MOUSE`): assert the right-click context
    /// menu items + Split/Close actions. See [`AppDelegate::run_mouse_smoke`].
    smoke_mouse: bool,
    /// Clipboard smoke (`QWERTTY_TERM_SMOKE_CLIPBOARD`): paste-protection +
    /// selection-clear-on-typing. See [`AppDelegate::run_clipboard_smoke`].
    smoke_clipboard: bool,
    /// Window-state smoke (`QWERTTY_TERM_SMOKE_WINDOWSTATE`): assert the first
    /// window honors configured initial geometry. See
    /// [`AppDelegate::run_windowstate_smoke`].
    smoke_windowstate: bool,
    /// Notify smoke (`QWERTTY_TERM_SMOKE_NOTIFY`): assert OSC 9/777 desktop
    /// notifications reach the delivery seam. See
    /// [`AppDelegate::run_notify_smoke`].
    smoke_notify: bool,
    /// Command-finish smoke (`QWERTTY_TERM_SMOKE_NOTIFYCMD`): assert OSC 133
    /// command-finish notifications fire. See
    /// [`AppDelegate::run_notifycmd_smoke`].
    smoke_notifycmd: bool,
    /// Progress smoke (`QWERTTY_TERM_SMOKE_PROGRESS`): assert OSC 9;4 progress-
    /// bar state tracking. See [`AppDelegate::run_progress_smoke`].
    smoke_progress: bool,
    /// Confirm-close smoke (`QWERTTY_TERM_SMOKE_CONFIRMCLOSE`): assert
    /// confirm-close-surface gating. See [`AppDelegate::run_confirmclose_smoke`].
    smoke_confirmclose: bool,
    /// Resize smoke (`QWERTTY_TERM_SMOKE_RESIZE`): assert the resize overlay HUD.
    /// See [`AppDelegate::run_resize_smoke`].
    smoke_resize: bool,
    /// Mouse-2 smoke (`QWERTTY_TERM_SMOKE_MOUSE2`): middle-click paste +
    /// focus-follows-mouse. See [`AppDelegate::run_mouse2_smoke`].
    smoke_mouse2: bool,
    /// Save-state smoke (`QWERTTY_TERM_SMOKE_SAVESTATE`): assert window-save-state
    /// wiring. See [`AppDelegate::run_savestate_smoke`].
    smoke_savestate: bool,
    /// Session smoke (`QWERTTY_TERM_SMOKE_SESSION`): capture/round-trip/restore
    /// the window-session tree. See [`AppDelegate::run_session_smoke`].
    smoke_session: bool,
    /// Word-chars smoke (`QWERTTY_TERM_SMOKE_WORDCHARS`): assert
    /// `selection-word-chars` + `click-repeat-interval` config wiring. See
    /// [`AppDelegate::run_wordchars_smoke`].
    smoke_wordchars: bool,
    /// Mouse-shift smoke (`QWERTTY_TERM_SMOKE_MOUSESHIFT`): assert
    /// `mouse-shift-capture` gates the shift-over-reporting selection override.
    /// See [`AppDelegate::run_mouseshift_smoke`].
    smoke_mouseshift: bool,
    /// Clear-copy smoke (`QWERTTY_TERM_SMOKE_CLEARCOPY`): assert
    /// `selection-clear-on-copy` clears on explicit copy but not copy-on-select.
    /// See [`AppDelegate::run_clearcopy_smoke`].
    smoke_clearcopy: bool,
    /// Window-chrome smoke (`QWERTTY_TERM_SMOKE_WINDOWCHROME`): assert
    /// `window-show-tab-bar` (tabbing mode), `window-subtitle` (NSWindow
    /// subtitle from cwd), and `window-new-tab-position` (new tab lands at the
    /// group end). See [`AppDelegate::run_windowchrome_smoke`].
    smoke_windowchrome: bool,
    /// The vsync-synced present clock (a `CADisplayLink` bound to the first
    /// window's display) when running interactively. Retained here so it isn't
    /// deallocated; `None` when the fallback combined `NSTimer` tick is used
    /// (GUI smokes, or no screen-backed window). See
    /// [`AppDelegate::start_display_link`].
    display_link: RefCell<Option<Retained<CADisplayLink>>>,
}

/// Phase-1→phase-2 handoff for the splits smoke: the tab under test and each
/// pane's `(SurfaceId, unique marker)`.
type SplitsSmokeState = (TabId, Vec<(SurfaceId, String)>);

define_class!(
    // SAFETY: NSObject subclass; implements NSApplicationDelegate + a menu action
    // selector. No unsafe Drop.
    #[unsafe(super(NSObject))]
    #[name = "QwerttyTermAppDelegate"]
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

            // Present pacing. Interactive runs split the tick: io-service on a
            // steady ~60Hz timer (background-safe: a background terminal must
            // keep answering pty queries), and render on a vsync-synced
            // CADisplayLink (smooth present, no judder on high-fps content). The
            // GUI smokes keep the deterministic combined ~60Hz NSTimer tick, as
            // does the fallback when there's no screen-backed window. (The
            // headless --offscreen-smoke never reaches this delegate.)
            let gui_smoke = self.ivars().smoke_geometry
                || self.ivars().smoke_tabkeys
                || self.ivars().smoke_splits
                || self.ivars().smoke_keybind
                || self.ivars().smoke_focus
                || !self.ivars().smoke_type.borrow().is_empty();
            if !gui_smoke && self.start_display_link() {
                    self.start_service_timer();
            } else {
                self.start_pace_timer();
            }

            // Geometry smoke: dump + assert window geometry across the
            // 1-tab→2-tab→1-tab transition, then exit. Takes precedence over the
            // other smokes (it exits itself). Then synthetic-input, then the
            // plain auto-exit.
            // tmux lifecycle smoke (`QWERTTY_TERM_SMOKE_TMUXLIFE`): drive the
            // app's *own* Cmd-T / close-tab actions against a real `tmux -CC`
            // and dump observable state at each step. Read from the env directly
            // rather than threading another bool through `run`'s signature.
            if std::env::var_os("QWERTTY_TERM_SMOKE_TMUXLIFE").is_some() {
                self.schedule_selector(3.0, sel!(ghosttyTmuxLifeSmoke:));
            }
            let has_geometry = self.ivars().smoke_geometry;
            let has_tabkeys = self.ivars().smoke_tabkeys;
            let has_splits = self.ivars().smoke_splits;
            let has_keybind = self.ivars().smoke_keybind;
            let has_focus = self.ivars().smoke_focus;
            let has_search = self.ivars().smoke_search;
            let has_selection = self.ivars().smoke_selection;
            let has_title = self.ivars().smoke_title;
            let has_quickterm = self.ivars().smoke_quickterm;
            let has_bell = self.ivars().smoke_bell;
            let has_mouse = self.ivars().smoke_mouse;
            let has_clipboard = self.ivars().smoke_clipboard;
            let has_windowstate = self.ivars().smoke_windowstate;
            let has_notify = self.ivars().smoke_notify;
            let has_notifycmd = self.ivars().smoke_notifycmd;
            let has_progress = self.ivars().smoke_progress;
            let has_confirmclose = self.ivars().smoke_confirmclose;
            let has_resize = self.ivars().smoke_resize;
            let has_mouse2 = self.ivars().smoke_mouse2;
            let has_savestate = self.ivars().smoke_savestate;
            let has_session = self.ivars().smoke_session;
            let has_wordchars = self.ivars().smoke_wordchars;
            let has_mouseshift = self.ivars().smoke_mouseshift;
            let has_clearcopy = self.ivars().smoke_clearcopy;
            let has_windowchrome = self.ivars().smoke_windowchrome;
            let has_type = !self.ivars().smoke_type.borrow().is_empty();
            if has_splits {
                self.schedule_splits_smoke();
            } else if has_tabkeys {
                self.schedule_tabkeys_smoke();
            } else if has_keybind {
                self.schedule_keybind_smoke();
            } else if has_focus {
                self.schedule_focus_smoke();
            } else if has_search {
                self.schedule_search_smoke();
            } else if has_selection {
                self.schedule_selection_smoke();
            } else if has_title {
                self.schedule_title_smoke();
            } else if has_quickterm {
                self.schedule_quickterm_smoke();
            } else if has_bell {
                self.schedule_bell_smoke();
            } else if has_mouse {
                self.schedule_mouse_smoke();
            } else if has_clipboard {
                self.schedule_clipboard_smoke();
            } else if has_windowstate {
                self.schedule_windowstate_smoke();
            } else if has_notify {
                self.schedule_notify_smoke();
            } else if has_notifycmd {
                self.schedule_notifycmd_smoke();
            } else if has_progress {
                self.schedule_progress_smoke();
            } else if has_confirmclose {
                self.schedule_confirmclose_smoke();
            } else if has_resize {
                self.schedule_resize_smoke();
            } else if has_mouse2 {
                self.schedule_mouse2_smoke();
            } else if has_savestate {
                self.schedule_savestate_smoke();
            } else if has_session {
                self.schedule_session_smoke();
            } else if has_wordchars {
                self.schedule_wordchars_smoke();
            } else if has_mouseshift {
                self.schedule_mouseshift_smoke();
            } else if has_clearcopy {
                self.schedule_clearcopy_smoke();
            } else if has_windowchrome {
                self.schedule_windowchrome_smoke();
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
            // Background (smoke) runs drive the app with synthetic events, so
            // skip claiming focus — otherwise every smoke yanks the developer's
            // keyboard away mid-run.
            if !crate::app::background_mode() {
                app.activate();
                #[allow(deprecated)]
                app.activateIgnoringOtherApps(true);
            }
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn should_terminate_after_last(&self, _app: &NSApplication) -> bool {
            // Honor `quit-after-last-window-closed` (default false on macOS).
            self.ivars().controller.quit_after_last_window_closed()
        }

        /// Opt into secure-coded window restoration (required on macOS 12+, else
        /// AppKit logs and disables restoration). Our restorable state is a single
        /// secure-coded `NSString` (JSON), decoded via `decodeObjectOfClass:`.
        #[unsafe(method(applicationSupportsSecureRestorableState:))]
        fn supports_secure_restorable_state(&self, _app: &NSApplication) -> bool {
            true
        }
    }

    // `window-save-state`: the app delegate is each restorable window's
    // `restorationClass`. On relaunch AppKit calls this to re-provide the window;
    // rather than build a second window (the launch path already made one), we
    // reuse the active window and let its delegate's `didDecodeRestorableState`
    // refill the split content from `state`.
    unsafe impl NSWindowRestoration for AppDelegate {
        #[unsafe(method(restoreWindowWithIdentifier:state:completionHandler:))]
        fn restore_window(
            _identifier: &NSString,
            _state: &NSCoder,
            completion_handler: &block2::DynBlock<
                dyn Fn(*mut NSWindow, *mut objc2_foundation::NSError),
            >,
        ) {
            // AppKit drives restoration on the main thread.
            let Some(mtm) = MainThreadMarker::new() else {
                completion_handler.call((std::ptr::null_mut(), std::ptr::null_mut()));
                return;
            };
            // Reach the controller through the shared app delegate; hand back the
            // active (launch) window so AppKit associates the saved state with it
            // and then invokes `window:didDecodeRestorableState:` to rebuild.
            let window = NSApplication::sharedApplication(mtm)
                .delegate()
                .and_then(|d| d.downcast::<AppDelegate>().ok())
                .and_then(|d| d.ivars().controller.active_window_and_tab())
                .map(|(w, _tab)| w);
            match window {
                Some(w) => completion_handler
                    .call((Retained::as_ptr(&w) as *mut NSWindow, std::ptr::null_mut())),
                None => {
                    completion_handler.call((std::ptr::null_mut(), std::ptr::null_mut()))
                }
            }
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

        /// Pace-timer callback (fallback): pump + render every tab.
        #[unsafe(method(ghosttyPaceTick:))]
        fn pace_tick(&self, _timer: &AnyObject) {
            self.ivars().controller.tick();
        }

        /// Service-timer callback: io-service only (no render). Paired with the
        /// display link, which renders. Runs regardless of window visibility.
        #[unsafe(method(ghosttyServiceTick:))]
        fn service_tick(&self, _timer: &AnyObject) {
            self.ivars().controller.tick_service();
        }

        /// Display-link callback: render/present every tab at vsync.
        #[unsafe(method(ghosttyRenderTick:))]
        fn render_tick(&self, _link: &AnyObject) {
            self.ivars().controller.tick_render();
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
        #[unsafe(method(ghosttyTmuxLifeSmoke:))]
        fn tmuxlife_smoke(&self, _timer: &AnyObject) {
            if self.ivars().controller.run_tmuxlife_smoke() {
                self.schedule_selector(2.5, sel!(ghosttyTmuxLifeSmoke:));
            }
        }

        #[unsafe(method(ghosttySplitsSmoke:))]
        fn splits_smoke(&self, _timer: &AnyObject) {
            self.run_splits_smoke();
        }

        /// Splits smoke phase 2: assert isolation + resize + close-collapse, exit.
        #[unsafe(method(ghosttySplitsSmokeCheck:))]
        fn splits_smoke_check(&self, _timer: &AnyObject) {
            self.finish_splits_smoke();
        }

        /// Keybind smoke phase 1: drive shift+enter (the seeded `text:` binding)
        /// then a plain enter through the real window key path.
        #[unsafe(method(ghosttyKeybindSmoke:))]
        fn keybind_smoke(&self, _timer: &AnyObject) {
            self.run_keybind_smoke();
        }

        /// Keybind smoke phase 2: assert the pty round-trip and exit 0/1.
        #[unsafe(method(ghosttyKeybindSmokeCheck:))]
        fn keybind_smoke_check(&self, _timer: &AnyObject) {
            self.finish_keybind_smoke();
        }

        /// Focus smoke phase 1: split into 2 `cat -v` panes, enable mode 1004.
        #[unsafe(method(ghosttyFocusSmoke:))]
        fn focus_smoke(&self, _timer: &AnyObject) {
            self.run_focus_smoke();
        }

        /// Focus smoke phase 2: focus-switch, assert focus-in/out bytes, exit.
        #[unsafe(method(ghosttyFocusSmokeCheck:))]
        fn focus_smoke_check(&self, _timer: &AnyObject) {
            self.finish_focus_smoke();
        }

        /// Search smoke phase 1: fill scrollback with markers, Cmd+F, type the
        /// needle.
        #[unsafe(method(ghosttySearchSmoke:))]
        fn search_smoke(&self, _timer: &AnyObject) {
            self.run_search_smoke();
        }

        /// Search smoke phase 2: assert counter + navigation + escape-restores-
        /// input, exit.
        #[unsafe(method(ghosttySearchSmokeCheck:))]
        fn search_smoke_check(&self, _timer: &AnyObject) {
            self.finish_search_smoke();
        }

        /// Selection smoke: drive the synthetic mouse gestures and exit 0/1.
        #[unsafe(method(ghosttySelectionSmoke:))]
        fn selection_smoke(&self, _timer: &AnyObject) {
            self.run_selection_smoke();
        }

        /// Title smoke: feed OSC 2 titles per tab, assert, and exit 0/1.
        #[unsafe(method(ghosttyTitleSmoke:))]
        fn title_smoke(&self, _timer: &AnyObject) {
            self.run_title_smoke();
        }

        /// Quick-terminal smoke: toggle the dropdown in/out, assert, exit 0/1.
        #[unsafe(method(ghosttyQuickTermSmoke:))]
        fn quickterm_smoke(&self, _timer: &AnyObject) {
            self.run_quickterm_smoke();
        }

        /// Bell smoke: feed a BEL, assert the title indicator, exit 0/1.
        #[unsafe(method(ghosttyBellSmoke:))]
        fn bell_smoke(&self, _timer: &AnyObject) {
            self.run_bell_smoke();
        }

        /// Mouse smoke: assert the context menu + split/close, exit 0/1.
        #[unsafe(method(ghosttyMouseSmoke:))]
        fn mouse_smoke(&self, _timer: &AnyObject) {
            self.run_mouse_smoke();
        }

        /// Clipboard smoke: paste-protection + selection-clear, exit 0/1.
        #[unsafe(method(ghosttyClipboardSmoke:))]
        fn clipboard_smoke(&self, _timer: &AnyObject) {
            self.run_clipboard_smoke();
        }

        /// Window-state smoke: assert initial geometry, exit 0/1.
        #[unsafe(method(ghosttyWindowStateSmoke:))]
        fn windowstate_smoke(&self, _timer: &AnyObject) {
            self.run_windowstate_smoke();
        }

        /// Notify smoke: assert OSC 9/777 desktop notifications deliver, exit 0/1.
        #[unsafe(method(ghosttyNotifySmoke:))]
        fn notify_smoke(&self, _timer: &AnyObject) {
            self.run_notify_smoke();
        }

        /// Command-finish smoke: assert OSC 133 command-finish notifies, exit 0/1.
        #[unsafe(method(ghosttyNotifyCmdSmoke:))]
        fn notifycmd_smoke(&self, _timer: &AnyObject) {
            self.run_notifycmd_smoke();
        }

        /// Progress smoke: assert OSC 9;4 progress-bar state tracking, exit 0/1.
        #[unsafe(method(ghosttyProgressSmoke:))]
        fn progress_smoke(&self, _timer: &AnyObject) {
            self.run_progress_smoke();
        }

        /// Confirm-close smoke: assert confirm-close-surface gating, exit 0/1.
        #[unsafe(method(ghosttyConfirmCloseSmoke:))]
        fn confirmclose_smoke(&self, _timer: &AnyObject) {
            self.run_confirmclose_smoke();
        }

        /// Resize smoke: assert the resize overlay HUD, exit 0/1.
        #[unsafe(method(ghosttyResizeSmoke:))]
        fn resize_smoke(&self, _timer: &AnyObject) {
            self.run_resize_smoke();
        }

        /// Mouse-2 smoke: middle-click paste + focus-follows-mouse, exit 0/1.
        #[unsafe(method(ghosttyMouse2Smoke:))]
        fn mouse2_smoke(&self, _timer: &AnyObject) {
            self.run_mouse2_smoke();
        }

        /// Save-state smoke: assert window-save-state wiring, exit 0/1.
        #[unsafe(method(ghosttySaveStateSmoke:))]
        fn savestate_smoke(&self, _timer: &AnyObject) {
            self.run_savestate_smoke();
        }

        /// Session smoke: capture/round-trip/restore the window-session tree,
        /// exit 0/1.
        #[unsafe(method(ghosttySessionSmoke:))]
        fn session_smoke(&self, _timer: &AnyObject) {
            self.run_session_smoke();
        }

        /// Word-chars smoke: assert selection-word-chars + click-repeat-interval
        /// config wiring, exit 0/1.
        #[unsafe(method(ghosttyWordCharsSmoke:))]
        fn wordchars_smoke(&self, _timer: &AnyObject) {
            self.run_wordchars_smoke();
        }

        /// Mouse-shift smoke: assert mouse-shift-capture gating, exit 0/1.
        #[unsafe(method(ghosttyMouseShiftSmoke:))]
        fn mouseshift_smoke(&self, _timer: &AnyObject) {
            self.run_mouseshift_smoke();
        }

        /// Clear-copy smoke: assert selection-clear-on-copy, exit 0/1.
        #[unsafe(method(ghosttyClearCopySmoke:))]
        fn clearcopy_smoke(&self, _timer: &AnyObject) {
            self.run_clearcopy_smoke();
        }

        /// Window-chrome smoke: assert window-show-tab-bar / window-subtitle /
        /// window-new-tab-position wiring, exit 0/1.
        #[unsafe(method(ghosttyWindowChromeSmoke:))]
        fn windowchrome_smoke(&self, _timer: &AnyObject) {
            self.run_windowchrome_smoke();
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
        smoke_keybind: bool,
        smoke_focus: bool,
        smoke_search: bool,
        smoke_selection: bool,
        smoke_title: bool,
        smoke_quickterm: bool,
        smoke_bell: bool,
        smoke_mouse: bool,
        smoke_clipboard: bool,
        smoke_windowstate: bool,
        smoke_notify: bool,
        smoke_notifycmd: bool,
        smoke_progress: bool,
        smoke_confirmclose: bool,
        smoke_resize: bool,
        smoke_mouse2: bool,
        smoke_savestate: bool,
        smoke_session: bool,
        smoke_wordchars: bool,
        smoke_mouseshift: bool,
        smoke_clearcopy: bool,
        smoke_windowchrome: bool,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars {
            controller,
            smoke_ms,
            smoke_type: RefCell::new(smoke_type),
            smoke_geometry,
            smoke_tabkeys,
            smoke_splits,
            splits_state: RefCell::new(None),
            smoke_keybind,
            smoke_focus,
            focus_state: RefCell::new(None),
            smoke_search,
            search_state: RefCell::new(None),
            smoke_selection,
            smoke_title,
            smoke_quickterm,
            smoke_bell,
            smoke_mouse,
            smoke_clipboard,
            smoke_windowstate,
            smoke_notify,
            smoke_notifycmd,
            smoke_progress,
            smoke_confirmclose,
            smoke_resize,
            smoke_mouse2,
            smoke_savestate,
            smoke_session,
            smoke_wordchars,
            smoke_mouseshift,
            smoke_clearcopy,
            smoke_windowchrome,
            display_link: RefCell::new(None),
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
        if std::env::var_os("QWERTTY_TERM_ASSERT_PRESENT").is_some() {
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
        let consumed_ctrl_tab = Self::perform_on_view(&view, mtm, KEYCODE_TAB, TAB_MOD_CTRL, "");
        if !consumed_ctrl_tab {
            fail("ctrl+tab was NOT consumed by performKeyEquivalent (would reach the pty)".into());
        }
        let consumed_ctrl_shift_tab =
            Self::perform_on_view(&view, mtm, KEYCODE_TAB, TAB_MOD_CTRL_SHIFT, "");
        if !consumed_ctrl_shift_tab {
            fail("ctrl+shift+tab was NOT consumed by performKeyEquivalent".into());
        }
        let consumed_cmd_3 = Self::perform_on_view(&view, mtm, KEYCODE_3, TAB_MOD_CMD, "");
        if !consumed_cmd_3 {
            fail("cmd+3 was NOT consumed by performKeyEquivalent".into());
        }

        // Plain Tab, Shift+Tab, and Ctrl+I must NOT be consumed — they fall
        // through to keyDown → the encoder. (performKeyEquivalent returns false.)
        let consumed_plain_tab = Self::perform_on_view(&view, mtm, KEYCODE_TAB, TAB_MOD_NONE, "");
        if consumed_plain_tab {
            fail(
                "plain Tab was WRONGLY consumed by performKeyEquivalent (won't reach the pty)"
                    .into(),
            );
        }
        let consumed_shift_tab = Self::perform_on_view(&view, mtm, KEYCODE_TAB, TAB_MOD_SHIFT, "");
        if consumed_shift_tab {
            fail("Shift+Tab was WRONGLY consumed (CSI Z won't reach the pty)".into());
        }
        let consumed_ctrl_i = Self::perform_on_view(&view, mtm, KEYCODE_I, TAB_MOD_CTRL, "");
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
        Self::send_key_equiv_chars(controller, mtm, keycode, mods, "");
    }

    /// Like [`Self::send_key_equiv`] but carries `chars` as the event's
    /// `characters` / `charactersIgnoringModifiers`. The search field's editing
    /// chords (Cmd+A/C/X/V/Z) resolve on the produced character, so this variant
    /// is needed to exercise that path (e.g. Cmd+V with `chars = "v"`).
    fn send_key_equiv_chars(
        controller: &Controller,
        mtm: MainThreadMarker,
        keycode: u16,
        mods: NSEventModifierFlags,
        chars: &str,
    ) {
        if let Some(view) = controller.active_view() {
            let _ = Self::perform_on_view(&view, mtm, keycode, mods, chars);
        }
    }

    /// Build a synthetic keyDown `NSEvent` for `keycode`+`mods` (carrying `chars`
    /// as its characters) and send it to `view.performKeyEquivalent:`. Returns
    /// whether the view consumed it.
    fn perform_on_view(
        view: &TerminalView,
        _mtm: MainThreadMarker,
        keycode: u16,
        mods: NSEventModifierFlags,
        chars: &str,
    ) -> bool {
        let chars = NSString::from_str(chars);
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
                characters: &*chars,
                charactersIgnoringModifiers: &*chars,
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
        // Only when capture is enabled (QWERTTY_TERM_ASSERT_PRESENT). ---
        if std::env::var_os("QWERTTY_TERM_ASSERT_PRESENT").is_some() {
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

        // --- Slice 2: equalize, resize chords, zoom, and unfocused dimming.
        // Tree here is `left | (top_right / bottom_right)`; the root split (path
        // []) was just dragged to 0.25. ---

        // Equalize: the root's right child is a *cross-axis* (vertical) split, so
        // it counts as weight 1 → root ratio returns to 0.5 (leaf 0 vs. the
        // vertical subtree). The nested vertical split equalizes to 0.5 too.
        let (eq_left_before, _) = controller
            .surface_grid(tab, left)
            .unwrap_or_else(|| fail("no grid for left before equalize".into()));
        controller.equalize_splits(tab);
        let (eq_left_after, _) = controller
            .surface_grid(tab, left)
            .unwrap_or_else(|| fail("no grid for left after equalize".into()));
        // Left was shrunk to 25% by the drag; equalize restores it to ~50%, so
        // its column count must grow back.
        if eq_left_after <= eq_left_before {
            fail(format!(
                "equalize did not restore the left pane toward 50% \
                 (cols {eq_left_before} -> {eq_left_after})"
            ));
        }
        // After equalize the left and the right column should be near-equal width
        // (root ratio 0.5). Assert the left is within ~15% of the right column's
        // width by comparing columns (top_right spans the right column's width).
        let (left_cols, _) = controller.surface_grid(tab, left).unwrap();
        let (right_cols, _) = controller.surface_grid(tab, top_right).unwrap();
        let ratio = left_cols as f64 / (left_cols + right_cols) as f64;
        if !(0.35..=0.65).contains(&ratio) {
            fail(format!(
                "after equalize the split should be ~50/50, saw left/right col \
                 ratio {ratio:.2} ({left_cols} vs {right_cols})"
            ));
        }

        // Resize chord: shrink the left pane with a `resize_split` Left step
        // (cmd+ctrl+shift+left). It moves the root horizontal split; the left
        // pane's columns must drop and the right column's must grow (WINCH both).
        let (rc_left_before, _) = controller.surface_grid(tab, left).unwrap();
        let (rc_right_before, _) = controller.surface_grid(tab, top_right).unwrap();
        controller.resize_split(tab, Direction::Left);
        let (rc_left_after, _) = controller.surface_grid(tab, left).unwrap();
        let (rc_right_after, _) = controller.surface_grid(tab, top_right).unwrap();
        if rc_left_after >= rc_left_before {
            fail(format!(
                "resize_split Left did not shrink the left pane \
                 ({rc_left_before} -> {rc_left_after})"
            ));
        }
        if rc_right_after <= rc_right_before {
            fail(format!(
                "resize_split Left did not grow the right column \
                 ({rc_right_before} -> {rc_right_after})"
            ));
        }
        // Re-equalize so the geometry is symmetric for the zoom/dimming checks.
        controller.equalize_splits(tab);

        // Zoom: focus the middle pane (top_right) and toggle zoom. It must fill
        // the whole container; the other panes hide; the tree records the zoom.
        controller.focus_surface_in_tab(tab, top_right);
        controller.toggle_split_zoom(tab);
        if controller.active_zoomed_surface() != Some(top_right) {
            fail("toggle_split_zoom did not record the middle pane as zoomed".into());
        }
        let container = controller
            .active_container_rect()
            .unwrap_or_else(|| fail("no container rect while zoomed".into()));
        let zoomed_rect = controller
            .surface_rect(tab, top_right)
            .unwrap_or_else(|| fail("no rect for zoomed pane".into()));
        // The zoomed pane fills the container (within 1px of each dimension).
        if (zoomed_rect.w - container.w).abs() > 1.0 || (zoomed_rect.h - container.h).abs() > 1.0 {
            fail(format!(
                "zoomed pane should fill the container {container:?}, saw {zoomed_rect:?}"
            ));
        }
        // The other two panes are hidden while zoomed.
        for sid in [left, bottom_right] {
            if controller.surface_is_hidden(tab, sid) != Some(true) {
                fail(format!(
                    "pane {sid:?} should be hidden while another is zoomed"
                ));
            }
        }
        if controller.surface_is_hidden(tab, top_right) != Some(false) {
            fail("the zoomed pane must be visible".into());
        }

        // Unzoom via a second toggle → frames restored exactly (all panes shown).
        let left_rect_before_zoom = controller.surface_rect(tab, left);
        controller.toggle_split_zoom(tab);
        if controller.active_zoomed_surface().is_some() {
            fail("second toggle_split_zoom did not unzoom".into());
        }
        for sid in [left, top_right, bottom_right] {
            if controller.surface_is_hidden(tab, sid) != Some(false) {
                fail(format!("pane {sid:?} should be visible after unzoom"));
            }
        }
        // The left pane's rect returns to what it was before zooming (the split
        // layout is preserved through zoom/unzoom).
        if let (Some(before), Some(after)) =
            (left_rect_before_zoom, controller.surface_rect(tab, left))
            && ((before.w - after.w).abs() > 1.0 || (before.h - after.h).abs() > 1.0)
        {
            fail(format!(
                "unzoom did not restore the split layout: left rect {before:?} -> {after:?}"
            ));
        }

        // Zoom + a new split unzooms-then-splits (upstream `inserting` resets
        // zoom). Zoom the left pane, then create a split; the tree must not be
        // zoomed afterward and the pane count grows.
        controller.focus_surface_in_tab(tab, left);
        controller.toggle_split_zoom(tab);
        if controller.active_zoomed_surface() != Some(left) {
            fail("failed to zoom the left pane".into());
        }
        controller.new_split(tab, Direction::Right);
        if controller.active_zoomed_surface().is_some() {
            fail("creating a split should have unzoomed (upstream inserting resets zoom)".into());
        }
        if controller.active_surface_count() != Some(4) {
            fail(format!(
                "zoom + new_split should yield 4 panes, saw {:?}",
                controller.active_surface_count()
            ));
        }
        // Close the pane we just added to return to the 3-pane layout for the
        // dimming + remaining checks. The new pane is the focused one.
        let new_pane = controller
            .active_focused_surface()
            .unwrap_or_else(|| fail("no focused pane after zoom+split".into()));
        controller.close_surface(tab, new_pane);
        if controller.active_surface_count() != Some(3) {
            fail(format!(
                "expected 3 panes after closing the extra, saw {:?}",
                controller.active_surface_count()
            ));
        }

        // --- Unfocused dimming: with capture on, a pane's presented mean luma
        // drops when it is UNFOCUSED (multi-pane tab) and returns when focused,
        // and a pane in a single-pane tab is never dimmed. Only when capture is
        // enabled (readback). ---
        if std::env::var_os("QWERTTY_TERM_ASSERT_PRESENT").is_some() {
            // Re-feed a screenful of bright text into the left pane's engine and
            // sample the *peak* presented luma over a few ticks: the running shell
            // may redraw/scroll and the parse→snapshot→present→readback pipeline
            // lags a frame, so the ink's brightness shows up on the peak frame,
            // not necessarily the last. This makes the measurement robust to shell
            // interference and readback lag while still isolating the dim effect
            // (focused = undimmed, so its peak is brighter than the unfocused,
            // dimmed peak of the same ink).
            let bright: String = std::iter::repeat_n("#", 32 * 40)
                .collect::<String>()
                .as_bytes()
                .chunks(32)
                .map(|c| format!("{}\r\n", std::str::from_utf8(c).unwrap()))
                .collect();
            let peak_luma = |c: &Controller| -> f64 {
                c.feed_surface_output(tab, left, bright.as_bytes());
                let mut peak = 0.0f64;
                for _ in 0..8 {
                    c.tick();
                    if let Some(l) = c.surface_present_luma(tab, left) {
                        peak = peak.max(l);
                    }
                }
                peak
            };

            // Focus the left pane → renders undimmed. Peak luma is the bright ink.
            controller.focus_surface_in_tab(tab, left);
            let focused_luma = peak_luma(controller);

            // Focus a different pane so `left` becomes unfocused → dimmed. The
            // same ink now dims toward the dark background at overlay alpha 0.3.
            controller.focus_surface_in_tab(tab, bottom_right);
            let dimmed_luma = peak_luma(controller);

            if dimmed_luma + 2.0 >= focused_luma {
                fail(format!(
                    "unfocused-split dimming did not reduce the left pane's mean luma \
                     (focused {focused_luma:.2}, unfocused {dimmed_luma:.2})"
                ));
            }

            // Re-focus the left pane → dimming lifts, luma returns to the bright
            // baseline (within a small tolerance).
            controller.focus_surface_in_tab(tab, left);
            let refocused_luma = peak_luma(controller);
            if refocused_luma + 2.0 < focused_luma {
                fail(format!(
                    "re-focusing the left pane did not restore its brightness \
                     (was {focused_luma:.2}, now {refocused_luma:.2})"
                ));
            }
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
                qwertty_term_input::key_mods::Mods::default(),
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

        // --- Poison resilience (app-hardening): crash ONE pane's engine (the
        // field-observed cascade — the parse thread panics holding the lock) and
        // assert the whole app survives: that pane is marked dead + banners while
        // every other pane keeps rendering and stays alive. ---
        {
            // All three panes are healthy before the crash.
            for sid in [left, top_right, bottom_right] {
                if controller.surface_is_dead(tab, sid) != Some(false) {
                    fail(format!(
                        "pane {sid:?} should be healthy before the poison test"
                    ));
                }
            }
            // Poison the top-right pane's engine lock.
            controller.poison_surface_engine(tab, top_right);
            // Drive one pace tick: it must NOT panic (pre-fix, the first
            // `engine.lock().unwrap()` here took the whole app down). The tick
            // observes the poison, marks the pane dead, shuts its io down, and
            // paints the crash banner.
            controller.tick();

            if controller.surface_is_dead(tab, top_right) != Some(true) {
                fail("the poisoned pane was not marked dead after a tick".into());
            }
            // Crucially, the app survived and the OTHER panes are still healthy
            // and still present (the crash degraded to one dead SURFACE, not a
            // dead app). The tab is unchanged (dead panes stay open, not closed).
            if controller.surface_is_dead(tab, left) != Some(false)
                || controller.surface_is_dead(tab, bottom_right) != Some(false)
            {
                fail("a crash in one pane wrongly marked a sibling pane dead".into());
            }
            if controller.active_surface_count() != Some(3) {
                fail(format!(
                    "the crashed pane must stay open (banner), not close: expected 3 \
                     panes, saw {:?}",
                    controller.active_surface_count()
                ));
            }
            // The dead pane's engine now shows the crash banner (proof it settled
            // and rendered a final state rather than freezing/crashing).
            let banner_screen = controller
                .surface_screen_text(tab, top_right)
                .unwrap_or_default();
            if !banner_screen.contains("terminal crashed") {
                fail(format!(
                    "the dead pane should show the 'terminal crashed' banner, saw:\n{banner_screen}"
                ));
            }
            // A second tick still doesn't panic and doesn't re-close anything.
            controller.tick();
            if controller.active_surface_count() != Some(3) {
                fail("a second tick after the crash disturbed the pane count".into());
            }
        }

        // --- Close-collapse: close the middle pane (top-right). The tree
        // collapses so the sibling (bottom-right) absorbs the right column →
        // 2 panes remain. (This also cleanly closes the crashed pane, proving a
        // dead pane is still closable.) ---
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
             a divider drag resizes both adjacent panes' grids, equalize restores \
             even ratios, a resize_split chord shrinks/grows adjacent panes (WINCH), \
             zoom fills the container + hides the rest and unzoom restores the \
             layout exactly, zoom+new_split unzooms-then-splits, unfocused panes dim \
             (mean luma drops, restored on refocus), wheel-scrolling one pane back \
             leaves the others pinned to the live area (per-pane scrollback \
             isolation), poisoning one pane's engine marks only that pane dead \
             (crash banner) while the app + sibling panes survive, closing the \
             middle pane collapses to 2 with the sibling absorbing the space, and \
             closing every pane closes the tab."
        );
        std::process::exit(0);
    }

    /// Schedule the keybind smoke: let the shell draw its prompt, then run phase 1.
    fn schedule_keybind_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyKeybindSmoke:));
    }

    /// Keybind smoke phase 1. The controller was seeded with
    /// `shift+enter=text:zzKBMARKERzz` (see `main::run_window`). Deliver a
    /// synthetic **Shift+Return** keyDown through the real window key path: it
    /// must hit `TerminalView::keyDown:` → `try_handle_text_keybind` → the
    /// controller's keybind table BEFORE the encoder, and send the literal marker
    /// bytes to the focused pane's pty (so the shell echoes the marker). Then a
    /// plain **Return** (no shift) must NOT match the binding and instead encode
    /// to CR, submitting the line (so the marker runs → "command not found"
    /// appears, proving plain enter still sends `\r`). Phase 2 asserts the screen.
    fn run_keybind_smoke(&self) {
        let mtm = self.mtm();
        let app = NSApplication::sharedApplication(mtm);
        let controller = &self.ivars().controller;

        let win_num: isize = app
            .keyWindow()
            .or_else(|| controller.active_window())
            .map(|w| w.windowNumber())
            .unwrap_or(0);

        // Shift+Return: the seeded `text:` binding fires (marker bytes → pty).
        Self::send_keydown(
            &app,
            win_num,
            KEYCODE_RETURN,
            "\r",
            NSEventModifierFlags::Shift,
        );
        // Plain Return: not bound → encoder sends CR → the marker line is
        // submitted and runs.
        Self::send_keydown(
            &app,
            win_num,
            KEYCODE_RETURN,
            "\r",
            NSEventModifierFlags::empty(),
        );

        // Give the shell time to echo the marker + run it, then check.
        self.schedule_selector(1.0, sel!(ghosttyKeybindSmokeCheck:));
    }

    /// Keybind smoke phase 2: the focused pane's screen must contain the marker
    /// (proof shift+enter routed the `text:` bytes to the pty via the keybind
    /// path). Because plain Return then submitted it, the marker appears at least
    /// once (its echo). Exits 0/1.
    fn finish_keybind_smoke(&self) {
        let controller = &self.ivars().controller;
        let screen = controller.active_screen_text().unwrap_or_default();
        const MARKER: &str = "zzKBMARKERzz";
        let count = screen.matches(MARKER).count();
        if count < 1 {
            eprintln!(
                "FAIL: keybind smoke — the shift+enter `text:` binding did not reach \
                 the pty: marker '{MARKER}' absent from the focused pane. The keybind \
                 path (keyDown → try_handle_text_keybind → controller) is broken.\n\
                 ----- screen -----\n{screen}\n------------------"
            );
            std::process::exit(1);
        }
        println!(
            "OK: keybind smoke — shift+enter fired the seeded `text:` binding, \
             sending its literal bytes to the focused pane's pty (marker '{MARKER}' \
             found {count}x), and a plain enter fell through to the encoder (CR) to \
             submit the line. The maintainer's exact `\\x1b\\r` bytes are unit-tested \
             in keybind.rs."
        );
        std::process::exit(0);
    }

    /// Build + dispatch a synthetic keyDown `NSEvent` through the app responder
    /// chain (`app.sendEvent`), the exact path a hardware keystroke takes to
    /// `TerminalView::keyDown:`. `chars` is the event's characters string.
    fn send_keydown(
        app: &NSApplication,
        win_num: isize,
        keycode: u16,
        chars: &str,
        mods: NSEventModifierFlags,
    ) {
        let ns_chars = NSString::from_str(chars);
        // SAFETY: standard keyDown NSEvent constructor; nil context; main-thread
        // dispatch through the app like a real event.
        unsafe {
            let cls = objc2::class!(NSEvent);
            let event: Option<Retained<objc2_app_kit::NSEvent>> = msg_send![
                cls,
                keyEventWithType: NSEventType::KeyDown,
                location: NSPoint::new(0.0, 0.0),
                modifierFlags: mods,
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

    /// Schedule the focus-reporting smoke: let the first shell draw its prompt,
    /// then run phase 1.
    fn schedule_focus_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyFocusSmoke:));
    }

    /// Focus-reporting smoke phase 1 (per-pane mode-1004, app-hardening). Split
    /// into two panes, run `cat -v` in each (so any bytes we send to a pane's pty
    /// are echoed back with control chars made visible — ESC shows as `^[`), and
    /// enable focus reporting (mode 1004) in BOTH engines by feeding the SM
    /// sequence straight into each engine (so `focus_reporting()` is true without
    /// needing the child to emit it). Phase 2 focus-switches and asserts the
    /// focus-in/out bytes land at the right ptys.
    fn run_focus_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = focus_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at focus smoke start".into());
        };
        // Split right → 2 panes; the new (right) pane is focused.
        controller.new_split(tab, Direction::Right);
        let (tab_id, surfaces) = controller
            .active_surfaces()
            .unwrap_or_else(|| fail("no surfaces after split".into()));
        if surfaces.len() != 2 {
            fail(format!("expected 2 panes, saw {}", surfaces.len()));
        }
        let (left, right) = (surfaces[0], surfaces[1]);

        // Run `cat -v` in each pane so sent bytes echo back visibly (^[ for ESC).
        controller.write_to_surface(tab_id, left, b"exec cat -v\n");
        controller.write_to_surface(tab_id, right, b"exec cat -v\n");

        // Enable mode 1004 in BOTH engines (feed the SM sequence directly so it
        // sets the engine mode deterministically, without the child's help).
        controller.feed_surface_output(tab_id, left, b"\x1b[?1004h");
        controller.feed_surface_output(tab_id, right, b"\x1b[?1004h");

        *self.ivars().focus_state.borrow_mut() = Some((tab_id, left, right));

        // Give `cat -v` a beat to take over each pty, then focus-switch + assert.
        self.schedule_selector(0.8, sel!(ghosttyFocusSmokeCheck:));
    }

    /// Focus-reporting smoke phase 2. The right pane is focused (mode 1004 now on
    /// in both). Switch focus to the LEFT pane: that must send the RIGHT pane
    /// focus-OUT (`CSI O`) and the LEFT pane focus-IN (`CSI I`), each delivered to
    /// that pane's OWN pty (proof reporting is per-SURFACE, not per-tab). With
    /// `cat -v` echoing, the left pane's screen shows `^[[I` and the right's shows
    /// `^[[O`. Then switch back and assert the reverse. Exits 0/1.
    fn finish_focus_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = focus_fail;
        let (tab, left, right) = self
            .ivars()
            .focus_state
            .borrow()
            .unwrap_or_else(|| fail("focus smoke phase-2 state missing".into()));

        // Sanity: right is focused coming out of the split.
        if controller.active_focused_surface() != Some(right) {
            fail("the newly-split (right) pane should be focused before the switch".into());
        }

        // Switch focus right → left: right gets CSI O, left gets CSI I.
        controller.focus_surface_in_tab(tab, left);
        // Let cat -v echo the bytes back and the engine render them.
        Self::spin(0.4);

        let left_screen = controller
            .surface_screen_text(tab, left)
            .unwrap_or_default();
        let right_screen = controller
            .surface_screen_text(tab, right)
            .unwrap_or_default();
        // `cat -v` renders ESC as `^[`, so CSI I = `^[[I`, CSI O = `^[[O`.
        if !left_screen.contains("^[[I") {
            fail(format!(
                "focus-IN (CSI I → '^[[I') did not reach the newly-focused LEFT pane's \
                 pty.\n--- left screen ---\n{left_screen}"
            ));
        }
        if !right_screen.contains("^[[O") {
            fail(format!(
                "focus-OUT (CSI O → '^[[O') did not reach the un-focused RIGHT pane's \
                 pty.\n--- right screen ---\n{right_screen}"
            ));
        }
        // Cross-check: the focus-in went to LEFT only (right didn't get a spurious
        // CSI I from this switch beyond what it may already show), and left got
        // its CSI I. The decisive per-surface property is that each pane received
        // the correct direction — asserted above.

        // Switch back left → right: left gets CSI O, right gets CSI I.
        controller.focus_surface_in_tab(tab, right);
        Self::spin(0.4);
        let left_screen2 = controller
            .surface_screen_text(tab, left)
            .unwrap_or_default();
        let right_screen2 = controller
            .surface_screen_text(tab, right)
            .unwrap_or_default();
        if !right_screen2.contains("^[[I") {
            fail(format!(
                "focus-IN did not reach the RIGHT pane after switching back.\n\
                 --- right screen ---\n{right_screen2}"
            ));
        }
        if !left_screen2.contains("^[[O") {
            fail(format!(
                "focus-OUT did not reach the LEFT pane after switching away.\n\
                 --- left screen ---\n{left_screen2}"
            ));
        }

        println!(
            "OK: focus smoke — with mode 1004 on in two panes, switching pane focus \
             delivers CSI I (focus-in) to the newly-focused pane's pty and CSI O \
             (focus-out) to the previously-focused pane's pty, per SURFACE. Switching \
             back delivers the reverse. Per-pane focus reporting works."
        );
        std::process::exit(0);
    }

    /// Schedule the search smoke: give the shell a beat, then run phase 1.
    fn schedule_search_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttySearchSmoke:));
    }

    /// Search smoke phase 1. Fill the focused pane's scrollback with known
    /// content (many filler lines plus 3 lines carrying a distinctive marker,
    /// pushing the first markers up into history), then drive **Cmd+F** through
    /// the real `performKeyEquivalent:` path (opening the overlay), and set the
    /// needle to the marker (the exact call the overlay's field delegate makes
    /// on each keystroke). Assert the counter reads 3. Phase 2 navigates and
    /// checks that Escape restores PTY input.
    fn run_search_smoke(&self) {
        let controller = &self.ivars().controller;
        let mtm = self.mtm();
        let fail = search_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at search smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at search smoke start".into());
        };

        // Fill the scrollback deterministically by feeding engine output
        // directly (no async pty round-trip): 60 filler lines interleaved with
        // exactly 3 marker lines, so the first markers land in history and the
        // whole-scrollback search must reach them.
        let marker = "NEEDLEZZ";
        let mut content = String::new();
        for i in 0..60u32 {
            if i == 5 || i == 30 || i == 55 {
                content.push_str(marker);
                content.push_str(" line\r\n");
            } else {
                content.push_str("filler filler filler\r\n");
            }
        }
        controller.feed_surface_output(tab, surface, content.as_bytes());

        // Cmd+F through the real interception path → opens the overlay.
        Self::send_key_equiv(controller, mtm, KEYCODE_F, TAB_MOD_CMD);
        if !controller.active_search_is_active() {
            fail("Cmd+F did not open the search bar (search state not active)".into());
        }

        // Type the needle (the field-delegate path).
        controller.search_set_needle(tab, surface, marker);

        let count = controller.active_search_match_count().unwrap_or(0);
        if count != 3 {
            fail(format!(
                "expected 3 matches for '{marker}' over the scrollback, saw {count}"
            ));
        }
        // The first match is current (index 0) and the viewport scrolled to it.
        if controller.active_search_current_index() != Some(0) {
            fail(format!(
                "first search result should be current (index 0), saw {:?}",
                controller.active_search_current_index()
            ));
        }

        *self.ivars().search_state.borrow_mut() = Some((tab, surface));
        self.schedule_selector(0.3, sel!(ghosttySearchSmokeCheck:));
    }

    /// Search smoke phase 2: navigate next/next/prev asserting the current-match
    /// index (and thus the viewport offset — each match is on a distinct row, so
    /// the offset lands on that row) advances, then Escape and assert typed bytes
    /// reach the pty again (input restored). Exits 0/1.
    fn finish_search_smoke(&self) {
        let controller = &self.ivars().controller;
        let mtm = self.mtm();
        let fail = search_fail;
        let (tab, surface) = self
            .ivars()
            .search_state
            .borrow()
            .unwrap_or_else(|| fail("search smoke phase-2 state missing".into()));

        // Record each match's scroll offset as we navigate — every match is on a
        // different scrollback row, so the offset must change on each step.
        let off0 = controller
            .surface_scrollback_offset(tab, surface)
            .unwrap_or(0);

        // next → index 1 (the middle marker).
        controller.search_navigate_next(tab, surface);
        if controller.active_search_current_index() != Some(1) {
            fail(format!(
                "after next, expected current index 1, saw {:?}",
                controller.active_search_current_index()
            ));
        }
        let off1 = controller
            .surface_scrollback_offset(tab, surface)
            .unwrap_or(0);

        // next → index 2 (the last marker; near the live area → smaller offset).
        controller.search_navigate_next(tab, surface);
        if controller.active_search_current_index() != Some(2) {
            fail(format!(
                "after next, expected current index 2, saw {:?}",
                controller.active_search_current_index()
            ));
        }
        let off2 = controller
            .surface_scrollback_offset(tab, surface)
            .unwrap_or(0);

        // prev → back to index 1, and the offset must return to off1.
        controller.search_navigate_previous(tab, surface);
        if controller.active_search_current_index() != Some(1) {
            fail(format!(
                "after prev, expected current index 1, saw {:?}",
                controller.active_search_current_index()
            ));
        }
        let off1b = controller
            .surface_scrollback_offset(tab, surface)
            .unwrap_or(0);

        // The offsets must be distinct across rows and deterministic per match:
        // earlier markers are further up history (larger offset).
        if !(off0 > off1 && off1 > off2) {
            fail(format!(
                "viewport offsets should decrease as we navigate toward newer \
                 matches (off0={off0}, off1={off1}, off2={off2})"
            ));
        }
        if off1b != off1 {
            fail(format!(
                "navigating back to a match must land on the same viewport offset \
                 (off1={off1}, off1b={off1b})"
            ));
        }

        // The search field behaves like a standard text box: while it is being
        // edited, Cmd+V must paste into the FIELD, not the shell. (Before the
        // field-editor routing this pasted the clipboard into the pty — a real
        // hazard.) The overlay made the field first responder when Cmd+F opened
        // it, so assert that, then drive Cmd+V through the real key-equiv path.
        if !controller.active_search_field_is_editing() {
            fail(
                "search field is not first responder after opening — cannot verify \
                 field-editor key routing (the overlay should have focused it)"
                    .into(),
            );
        }
        // A distinctive clipboard payload that does NOT occur in the scrollback,
        // so if it (wrongly) reached the pty it would be unmistakable on screen.
        let paste_guard = "PASTEGUARDZZ";
        if !crate::clipboard::write(paste_guard) {
            fail("could not seed the clipboard for the paste-routing check".into());
        }
        Self::send_key_equiv_chars(controller, mtm, KEYCODE_V, TAB_MOD_CMD, "v");
        // Let any (erroneous) pty paste echo at the prompt.
        Self::spin(0.5);
        let screen = controller
            .surface_screen_text(tab, surface)
            .unwrap_or_default();
        if screen.contains(paste_guard) {
            fail(format!(
                "Cmd+V while editing the search field pasted into the shell (marker \
                 '{paste_guard}' appeared on screen) — the field-editor routing did \
                 not intercept it.\n--- screen ---\n{screen}"
            ));
        }
        // ...and it DID land in the field: the paste fires the field delegate's
        // controlTextDidChange, so the needle now carries the pasted text.
        if controller.active_search_needle().as_deref() != Some(paste_guard) {
            fail(format!(
                "Cmd+V while editing the search field did not paste into the field \
                 (needle is {:?}, expected {paste_guard:?})",
                controller.active_search_needle()
            ));
        }

        // Escape restores PTY input: send Escape (gated on search active) via the
        // real key-equiv path → closes the bar. Then type a marker through the
        // real keyDown path and assert it reaches the pty (echoes back).
        Self::send_key_equiv(controller, mtm, KEYCODE_ESCAPE, TAB_MOD_NONE);
        if controller.active_search_is_active() {
            fail("Escape did not close the search bar".into());
        }

        // Type a distinctive marker string through the real window key path (the
        // focused pane's view should be first responder again after close).
        let app = NSApplication::sharedApplication(mtm);
        let win_num: isize = app
            .keyWindow()
            .or_else(|| controller.active_window())
            .map(|w| w.windowNumber())
            .unwrap_or(0);
        for ch in "echo AFTERSEARCH".chars() {
            let (keycode, chars) = synth_key_for_char(ch);
            Self::send_keydown(
                &app,
                win_num,
                keycode,
                &chars,
                NSEventModifierFlags::empty(),
            );
        }
        Self::send_keydown(
            &app,
            win_num,
            KEYCODE_RETURN,
            "\r",
            NSEventModifierFlags::empty(),
        );

        // Let the shell echo + run it.
        Self::spin(1.0);
        let screen = controller
            .surface_screen_text(tab, surface)
            .unwrap_or_default();
        if !screen.contains("AFTERSEARCH") {
            fail(format!(
                "after Escape, typed input did not reach the pty (marker \
                 'AFTERSEARCH' absent) — closing search did not restore terminal \
                 first-responder focus.\n--- screen ---\n{screen}"
            ));
        }

        println!(
            "OK: search smoke — Cmd+F opened the overlay, typing the needle found \
             all 3 markers across scrollback (counter 1/3), next/next/prev moved the \
             current match and scrolled the viewport to each match's row (offsets \
             {off0} → {off1} → {off2}, back to {off1b}), and Escape closed the bar, \
             restoring PTY input (typed text reached the shell)."
        );
        std::process::exit(0);
    }

    /// Schedule the selection smoke: let the first shell draw its prompt, then
    /// run the gesture sequence.
    fn schedule_selection_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttySelectionSmoke:));
    }

    /// Deliver one synthetic mouse event to the pane view's real
    /// `mouseDown:`/`mouseDragged:`/`mouseUp:` handler. `location` is in
    /// window base coordinates (see [`Controller::surface_cell_window_point`])
    /// — the view converts via `convertPoint:fromView:` exactly as it does
    /// for a real event. Delivery is direct (not `sendEvent` hit-testing,
    /// which can enter AppKit's nested window-drag tracking loop for a
    /// synthetic down near the titlebar and deadlock). `clickCount` is fixed
    /// at 1: the gesture layer does its own time+distance click counting
    /// (upstream `SelectionGesture` parity), so double/triple clicks are just
    /// successive down/up pairs.
    fn send_mouse(
        view: &crate::view::TerminalView,
        win_num: isize,
        ty: NSEventType,
        location: NSPoint,
        mods: NSEventModifierFlags,
    ) {
        // SAFETY: standard mouse NSEvent constructor; nil context; delivered
        // on the main thread to the view's responder methods.
        unsafe {
            let cls = objc2::class!(NSEvent);
            let event: Option<Retained<objc2_app_kit::NSEvent>> = msg_send![
                cls,
                mouseEventWithType: ty,
                location: location,
                modifierFlags: mods,
                timestamp: 0.0_f64,
                windowNumber: win_num,
                context: std::ptr::null::<AnyObject>(),
                eventNumber: 0_isize,
                clickCount: 1_isize,
                pressure: 1.0_f32,
            ];
            let Some(event) = event else { return };
            match ty {
                NSEventType::LeftMouseDown => {
                    let _: () = msg_send![view, mouseDown: &*event];
                }
                NSEventType::LeftMouseDragged => {
                    let _: () = msg_send![view, mouseDragged: &*event];
                }
                _ => {
                    let _: () = msg_send![view, mouseUp: &*event];
                }
            }
        }
    }

    /// One synthetic left click (down + up) at `location`.
    fn send_click(
        view: &crate::view::TerminalView,
        win_num: isize,
        location: NSPoint,
        mods: NSEventModifierFlags,
    ) {
        Self::send_mouse(view, win_num, NSEventType::LeftMouseDown, location, mods);
        Self::send_mouse(view, win_num, NSEventType::LeftMouseUp, location, mods);
    }

    /// Selection smoke: feed a deterministic screen, then drive the mouse
    /// gestures through the real window event path and assert the engine's
    /// selection text after each:
    ///
    /// 1. double-click on `beta-gamma` selects the word (`-` is not in the
    ///    upstream boundary set — `selection_codepoints.zig`);
    /// 2. triple-click selects the whole (trimmed) line;
    /// 3. a fresh single click clears the selection;
    /// 4. press–drag–release selects by cell with the 60% threshold;
    /// 5. a shift-click past the old end extends the selection
    ///    (`Surface.zig:3785`);
    /// 6. a drag parked at the top edge autoscrolls the viewport into
    ///    scrollback, extending the selection with history content, and stops
    ///    on release.
    ///
    /// Exits 0/1.
    fn run_selection_smoke(&self) {
        let controller = &self.ivars().controller;
        let mtm = self.mtm();
        let fail = selection_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at selection smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at selection smoke start".into());
        };

        let app = NSApplication::sharedApplication(mtm);
        let win_num: isize = app
            .keyWindow()
            .or_else(|| controller.active_window())
            .map(|w| w.windowNumber())
            .unwrap_or(0);
        let view = controller
            .surface_view(tab, surface)
            .unwrap_or_else(|| fail("no view for the focused surface".into()));
        let view = &*view;

        // Deterministic screen: clear + home, then known rows (fed straight
        // into the engine — no pty round-trip).
        controller.feed_surface_output(tab, surface, b"\x1b[2J\x1b[H");
        controller.feed_surface_output(
            tab,
            surface,
            b"alpha beta-gamma delta\r\n\r\nthird line here\r\n",
        );
        Self::spin(0.1);

        // Scenario separator: anything longer than the click-repeat interval
        // starts a fresh click sequence.
        let gap = (crate::gesture::click_interval() + std::time::Duration::from_millis(250))
            .as_secs_f64();
        let pt = |col: f64, row: f64| {
            controller
                .surface_cell_window_point(tab, surface, col, row)
                .unwrap_or_else(|| fail("cell → window point mapping failed".into()))
        };
        let none = NSEventModifierFlags::empty();
        let selection = || controller.surface_selection_string(tab, surface);

        // 1. Double-click the middle of "beta-gamma" (row 0, cols 6..15).
        let word_pt = pt(10.5, 0.5);
        Self::send_click(view, win_num, word_pt, none);
        Self::send_click(view, win_num, word_pt, none);
        let sel = selection();
        if sel.as_deref() != Some("beta-gamma") {
            fail(format!(
                "double-click should select the word 'beta-gamma' (hyphen is \
                 not a boundary), got {sel:?}"
            ));
        }

        // 2. Triple-click selects the whole trimmed line.
        Self::spin(gap);
        Self::send_click(view, win_num, word_pt, none);
        Self::send_click(view, win_num, word_pt, none);
        Self::send_click(view, win_num, word_pt, none);
        let sel = selection();
        if sel.as_deref() != Some("alpha beta-gamma delta") {
            fail(format!(
                "triple-click should select the line 'alpha beta-gamma delta', \
                 got {sel:?}"
            ));
        }

        // 3. A fresh single click clears the selection.
        Self::spin(gap);
        Self::send_click(view, win_num, pt(12.5, 0.5), none);
        let sel = selection();
        if sel.is_some() {
            fail(format!("a fresh single click should clear, got {sel:?}"));
        }

        // 4. Cell drag: press in the left part of col 0, drag to the right
        //    part of col 4, release → "alpha" (both cells inside the 60%
        //    threshold rule).
        Self::spin(gap);
        let from = pt(0.2, 0.5);
        let to = pt(4.8, 0.5);
        Self::send_mouse(view, win_num, NSEventType::LeftMouseDown, from, none);
        Self::send_mouse(view, win_num, NSEventType::LeftMouseDragged, to, none);
        Self::send_mouse(view, win_num, NSEventType::LeftMouseUp, to, none);
        let sel = selection();
        if sel.as_deref() != Some("alpha") {
            fail(format!("cell drag should select 'alpha', got {sel:?}"));
        }

        // 5. Shift-click at the end of "delta" (col 21) extends the existing
        //    selection from the old anchor.
        Self::spin(gap);
        Self::send_click(view, win_num, pt(21.8, 0.5), NSEventModifierFlags::Shift);
        let sel = selection();
        if sel.as_deref() != Some("alpha beta-gamma delta") {
            fail(format!(
                "shift-click should extend to 'alpha beta-gamma delta', got {sel:?}"
            ));
        }

        // 6. Edge autoscroll: fill scrollback with numbered markers, press
        //    mid-screen, park the drag at the very top edge (ypos 0 ≤ the 1px
        //    autoscroll buffer), and let the pace ticks scroll + extend.
        let (_, rows) = controller
            .surface_grid(tab, surface)
            .unwrap_or_else(|| fail("no grid for autoscroll scenario".into()));
        let mut content = String::new();
        for i in 0..80u32 {
            content.push_str(&format!("SCROLLMARK-{i:03}\r\n"));
        }
        controller.feed_surface_output(tab, surface, content.as_bytes());
        Self::spin(gap.max(0.2));
        let press_pt = pt(2.5, 5.5);
        let park_pt = pt(2.5, 0.0);
        Self::send_mouse(view, win_num, NSEventType::LeftMouseDown, press_pt, none);
        Self::send_mouse(view, win_num, NSEventType::LeftMouseDragged, park_pt, none);
        Self::spin(0.6); // ~36 pace ticks at 60Hz, one row each
        let offset = controller
            .surface_scrollback_offset(tab, surface)
            .unwrap_or(0);
        if offset == 0 {
            fail("a drag parked at the top edge did not autoscroll the viewport".into());
        }
        let sel = selection().unwrap_or_default();
        let first = sel.lines().next().unwrap_or("");
        // The selection's top line starts at the parked drag *column* (a
        // backward cell selection's end point), so it can begin mid-marker
        // (e.g. "ROLLMARK-021" from column 2) — match the marker tail and
        // parse the trailing index.
        if !first.contains("ROLLMARK-") {
            fail(format!(
                "autoscrolled selection's top line should be a marker, got {first:?}"
            ));
        }
        let digits: String = {
            let tail: Vec<char> = first
                .trim_end()
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            tail.into_iter().rev().collect()
        };
        let first_idx: u32 = digits.parse().unwrap_or_else(|_| {
            fail(format!(
                "autoscrolled selection's top marker index unparsable, got {first:?}"
            ))
        });
        // Markers visible at press time start around 80 - rows; the selection
        // must have reached strictly above that (history content).
        let top_visible_at_press = 80u32.saturating_sub(rows as u32);
        if first_idx >= top_visible_at_press {
            fail(format!(
                "autoscroll should extend the selection into scrollback (top \
                 selected marker {first_idx}, viewport top at press ~{top_visible_at_press})"
            ));
        }
        // Release stops the autoscroll: the offset must hold steady.
        Self::send_mouse(view, win_num, NSEventType::LeftMouseUp, park_pt, none);
        let off_a = controller
            .surface_scrollback_offset(tab, surface)
            .unwrap_or(0);
        Self::spin(0.25);
        let off_b = controller
            .surface_scrollback_offset(tab, surface)
            .unwrap_or(0);
        if off_a != off_b {
            fail(format!(
                "autoscroll should stop on release (offset {off_a} → {off_b})"
            ));
        }

        println!(
            "OK: selection smoke — double-click selected the word, triple-click \
             the line, a fresh click cleared, press-drag-release selected by cell \
             ('alpha'), shift-click extended to the full line, and an edge-parked \
             drag autoscrolled {offset} rows into scrollback (top selected marker \
             {first_idx}), stopping on release."
        );
        std::process::exit(0);
    }

    fn schedule_wordchars_smoke(&self) {
        self.schedule_selector(0.5, sel!(ghosttyWordCharsSmoke:));
    }

    /// `selection-word-chars` + `click-repeat-interval` config smoke. Launched
    /// with `selection-word-chars = " -"` (hyphen is a boundary) and
    /// `click-repeat-interval = 1234`: double-clicking "beta-gamma" then selects
    /// only "beta" — the inverse of the default-config selection smoke, where the
    /// hyphen is *not* a boundary and the word is "beta-gamma" — and the resolved
    /// click interval matches the config. Exits 0/1.
    fn run_wordchars_smoke(&self) {
        let mtm = self.mtm();
        let controller = &self.ivars().controller;
        let fail = wordchars_fail;

        // 1. click-repeat-interval flowed into the resolved mouse interval.
        if controller.mouse_interval() != std::time::Duration::from_millis(1234) {
            fail(format!(
                "click-repeat-interval=1234 should set the mouse interval; got {:?}",
                controller.mouse_interval()
            ));
        }

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at wordchars smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at wordchars smoke start".into());
        };
        let app = NSApplication::sharedApplication(mtm);
        let win_num: isize = app
            .keyWindow()
            .or_else(|| controller.active_window())
            .map(|w| w.windowNumber())
            .unwrap_or(0);
        let view = controller
            .surface_view(tab, surface)
            .unwrap_or_else(|| fail("no view for the focused surface".into()));
        let view = &*view;

        controller.feed_surface_output(tab, surface, b"\x1b[2J\x1b[H");
        controller.feed_surface_output(tab, surface, b"alpha beta-gamma delta\r\n");
        Self::spin(0.1);

        let pt = |col: f64, row: f64| {
            controller
                .surface_cell_window_point(tab, surface, col, row)
                .unwrap_or_else(|| fail("cell → window point mapping failed".into()))
        };
        let none = NSEventModifierFlags::empty();

        // 2. Double-click the middle of "beta" (cols 6..9). With `-` now a
        //    boundary, the word stops at the hyphen → "beta".
        let word_pt = pt(7.5, 0.5);
        Self::send_click(view, win_num, word_pt, none);
        Self::send_click(view, win_num, word_pt, none);
        let sel = controller.surface_selection_string(tab, surface);
        if sel.as_deref() != Some("beta") {
            fail(format!(
                "with selection-word-chars including '-', double-click should select \
                 'beta' (hyphen is a boundary); got {sel:?}"
            ));
        }

        println!(
            "OK: wordchars smoke — click-repeat-interval set the mouse interval to \
             1234ms, and selection-word-chars made '-' a boundary so a double-click \
             selected 'beta' rather than 'beta-gamma'."
        );
        std::process::exit(0);
    }

    fn schedule_mouseshift_smoke(&self) {
        self.schedule_selector(0.5, sel!(ghosttyMouseShiftSmoke:));
    }

    /// `mouse-shift-capture` config smoke. Launched with
    /// `mouse-shift-capture = always`: with a program that has mouse reporting on
    /// (`CSI ?1000h`), a shift-drag does *not* select (shift is captured by the
    /// program instead of overriding reporting) — whereas the default config
    /// would select. A control drag with reporting off still selects, proving the
    /// selection machinery is live and it's the config gating shift. Exits 0/1.
    fn run_mouseshift_smoke(&self) {
        let mtm = self.mtm();
        let controller = &self.ivars().controller;
        let fail = mouseshift_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at mouse-shift smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at mouse-shift smoke start".into());
        };
        let app = NSApplication::sharedApplication(mtm);
        let win_num: isize = app
            .keyWindow()
            .or_else(|| controller.active_window())
            .map(|w| w.windowNumber())
            .unwrap_or(0);
        let view = controller
            .surface_view(tab, surface)
            .unwrap_or_else(|| fail("no view for the focused surface".into()));
        let view = &*view;

        let pt = |col: f64, row: f64| {
            controller
                .surface_cell_window_point(tab, surface, col, row)
                .unwrap_or_else(|| fail("cell → window point mapping failed".into()))
        };
        let shift = NSEventModifierFlags::Shift;
        let none = NSEventModifierFlags::empty();
        let drag = |from: NSPoint, to: NSPoint, mods: NSEventModifierFlags| {
            Self::send_mouse(view, win_num, NSEventType::LeftMouseDown, from, mods);
            Self::send_mouse(view, win_num, NSEventType::LeftMouseDragged, to, mods);
            Self::send_mouse(view, win_num, NSEventType::LeftMouseUp, to, mods);
        };

        // 1. Enable mouse reporting + lay down a word to drag over.
        controller.feed_surface_output(tab, surface, b"\x1b[?1000h\x1b[2J\x1b[HSELECTME");
        Self::spin(0.15);

        // With mouse-shift-capture=always + reporting on, a shift-drag is captured
        // by the program → no selection is made.
        drag(pt(0.2, 0.5), pt(7.8, 0.5), shift);
        if controller.surface_has_selection(tab, surface) {
            fail(
                "with mouse-shift-capture=always a shift-drag under reporting should \
                  NOT select (shift is captured by the program)"
                    .into(),
            );
        }

        // 2. Control: disable reporting; a plain drag now selects, proving the
        //    machinery works and it was the config gating shift above.
        controller.feed_surface_output(tab, surface, b"\x1b[?1000l");
        Self::spin(0.1);
        drag(pt(0.2, 0.5), pt(7.8, 0.5), none);
        if !controller.surface_has_selection(tab, surface) {
            fail("a plain drag with reporting off should select (control)".into());
        }

        println!(
            "OK: mouse-shift smoke — mouse-shift-capture=always let the program \
             capture a shift-drag under mouse reporting (no selection), while a \
             plain drag with reporting off still selected."
        );
        std::process::exit(0);
    }

    fn schedule_clearcopy_smoke(&self) {
        self.schedule_selector(0.5, sel!(ghosttyClearCopySmoke:));
    }

    /// `selection-clear-on-copy` config smoke. Launched with `copy-on-select =
    /// true` + `selection-clear-on-copy = true`: a drag selection (copied via
    /// copy-on-select) stays visible — clear-on-copy excludes copy-on-select —
    /// but an explicit Copy (`copy_to_clipboard` / menu) then clears it. Exits
    /// 0/1.
    fn run_clearcopy_smoke(&self) {
        let mtm = self.mtm();
        let controller = &self.ivars().controller;
        let fail = clearcopy_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at clear-copy smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at clear-copy smoke start".into());
        };
        let app = NSApplication::sharedApplication(mtm);
        let win_num: isize = app
            .keyWindow()
            .or_else(|| controller.active_window())
            .map(|w| w.windowNumber())
            .unwrap_or(0);
        let view = controller
            .surface_view(tab, surface)
            .unwrap_or_else(|| fail("no view for the focused surface".into()));
        let view = &*view;
        let pt = |col: f64, row: f64| {
            controller
                .surface_cell_window_point(tab, surface, col, row)
                .unwrap_or_else(|| fail("cell → window point mapping failed".into()))
        };
        let none = NSEventModifierFlags::empty();

        controller.feed_surface_output(tab, surface, b"\x1b[2J\x1b[HSELECTME");
        Self::spin(0.15);

        // 1. Copy-on-select drag: selects + copies, but the selection stays
        //    (clear-on-copy does not apply to copy-on-select).
        Self::send_mouse(
            view,
            win_num,
            NSEventType::LeftMouseDown,
            pt(0.2, 0.5),
            none,
        );
        Self::send_mouse(
            view,
            win_num,
            NSEventType::LeftMouseDragged,
            pt(7.8, 0.5),
            none,
        );
        Self::send_mouse(view, win_num, NSEventType::LeftMouseUp, pt(7.8, 0.5), none);
        if !controller.surface_has_selection(tab, surface) {
            fail("copy-on-select drag should leave the selection visible".into());
        }

        // 2. Explicit Copy (the `copy_to_clipboard` action) clears it.
        controller.handle_action(crate::menu::MenuAction::Copy);
        Self::spin(0.1);
        if controller.surface_has_selection(tab, surface) {
            fail(
                "selection-clear-on-copy should clear the selection after an \
                  explicit copy"
                    .into(),
            );
        }

        println!(
            "OK: clear-copy smoke — copy-on-select kept the selection, and an \
             explicit copy cleared it (selection-clear-on-copy=true)."
        );
        std::process::exit(0);
    }

    /// Schedule the window-chrome smoke: let the first shell settle, then run.
    fn schedule_windowchrome_smoke(&self) {
        self.schedule_selector(0.6, sel!(ghosttyWindowChromeSmoke:));
    }

    /// Window-chrome smoke. Launch with `--window-show-tab-bar=always
    /// --window-subtitle=working-directory --window-new-tab-position=end
    /// --window-step-resize=true --macos-window-shadow=false
    /// --macos-window-buttons=hidden --window-theme=dark`. Asserts, in one
    /// window session:
    ///
    /// 1. the first window's tabbing mode is `.preferred` (`show-tab-bar=always`
    ///    forces the tab bar on even with a single tab);
    /// 2. `macos-window-shadow=false` → the window casts no shadow;
    /// 3. `macos-window-buttons=hidden` → the traffic-light buttons are hidden;
    /// 4. `window-step-resize=true` → the content resize increments are the cell
    ///    size (both > 1 point, not the 1×1 pixel default);
    /// 5. `window-theme=dark` → the window's appearance is darkAqua;
    /// 6. after an OSC 7 cwd report, the window subtitle tracks that directory
    ///    (`window-subtitle=working-directory`);
    /// 7. a new tab opened while an *earlier* tab is active still lands at the
    ///    END of the group (`window-new-tab-position=end`) — the assertion that
    ///    distinguishes `end` from `current`.
    ///
    /// Exits 0/1.
    fn run_windowchrome_smoke(&self) {
        let mtm = self.mtm();
        let controller = &self.ivars().controller;
        let fail = windowchrome_fail;

        let Some(tab_a) = controller.active_tab() else {
            fail("no active tab at window-chrome smoke start".into());
        };
        let Some(surface_a) = controller.active_focused_surface() else {
            fail("no focused surface at window-chrome smoke start".into());
        };
        let Some(win_a) = controller.active_window() else {
            fail("no active window at window-chrome smoke start".into());
        };

        // 1. window-show-tab-bar=always → the window's tabbing mode is .preferred.
        let mode = win_a.tabbingMode();
        if mode != NSWindowTabbingMode::Preferred {
            fail(format!(
                "window-show-tab-bar=always should set tabbingMode=.preferred, got {mode:?}"
            ));
        }

        // 2. macos-window-shadow=false → the window casts no shadow.
        if win_a.hasShadow() {
            fail("macos-window-shadow=false should clear NSWindow.hasShadow".into());
        }

        // 3. macos-window-buttons=hidden → all three standard buttons are hidden.
        for (button, name) in [
            (objc2_app_kit::NSWindowButton::CloseButton, "close"),
            (
                objc2_app_kit::NSWindowButton::MiniaturizeButton,
                "miniaturize",
            ),
            (objc2_app_kit::NSWindowButton::ZoomButton, "zoom"),
        ] {
            match win_a.standardWindowButton(button) {
                Some(b) if b.isHidden() => {}
                Some(_) => fail(format!(
                    "macos-window-buttons=hidden should hide the {name} button"
                )),
                None => {}
            }
        }

        // 4. window-step-resize=true → content resize increments are the cell
        //    size (both > 1 point, distinct from the 1×1 pixel default).
        let inc = win_a.contentResizeIncrements();
        if inc.width <= 1.0 || inc.height <= 1.0 {
            fail(format!(
                "window-step-resize=true should set cell-sized resize increments, got \
                 {}×{}",
                inc.width, inc.height
            ));
        }

        // 5. window-theme=dark → the window's appearance is darkAqua.
        {
            use objc2_app_kit::NSAppearanceCustomization;
            let appearance_name = win_a
                .appearance()
                .map(|a| a.name().to_string())
                .unwrap_or_default();
            if !appearance_name.to_lowercase().contains("dark") {
                fail(format!(
                    "window-theme=dark should set a darkAqua appearance, got {appearance_name:?}"
                ));
            }
        }

        // 6. window-subtitle=working-directory → feed OSC 7 and assert the
        //    subtitle tracks the reported cwd (applied on the next pace tick).
        let dir = "/private/tmp/qwertty-chrome-smoke";
        controller.feed_surface_output(
            tab_a,
            surface_a,
            format!("\x1b]7;file://localhost{dir}\x07").as_bytes(),
        );
        Self::spin(0.25);
        let subtitle = win_a.subtitle().to_string();
        if subtitle != dir {
            fail(format!(
                "window-subtitle=working-directory should set the subtitle to the \
                 cwd {dir:?}, got {subtitle:?}"
            ));
        }

        // 7. window-new-tab-position=end: open tab B, refocus A, then open tab
        //    C. With `end`, C joins after the LAST tab (B), so the group order
        //    is [A, B, C] and C's window is last. (`current` would insert C
        //    right after A, making B last instead.)
        if controller.new_tab_in(tab_a).is_none() {
            fail("failed to open tab B".into());
        }
        Self::spin(0.15);
        // Refocus tab A (cmd+1 selects the first tab in the group).
        Self::send_key_equiv(controller, mtm, KEYCODE_1, TAB_MOD_CMD);
        Self::spin(0.1);
        if controller.new_tab_in(tab_a).is_none() {
            fail("failed to open tab C".into());
        }
        Self::spin(0.15);
        let Some(win_c) = controller.active_window() else {
            fail("no active window after opening tab C".into());
        };
        let group = win_a
            .tabGroup()
            .unwrap_or_else(|| fail("tab A has no tab group after opening tabs".into()));
        let count = group.windows().count();
        if count != 3 {
            fail(format!("expected 3 tabs in the group, saw {count}"));
        }
        let last = group
            .windows()
            .iter()
            .last()
            .unwrap_or_else(|| fail("empty tab group".into()));
        if last.windowNumber() != win_c.windowNumber() {
            fail(format!(
                "window-new-tab-position=end should place the new tab last; last \
                 group window #{} != new tab window #{}",
                last.windowNumber(),
                win_c.windowNumber()
            ));
        }

        // 8. VT config toggle wiring (`title-report`): the smoke runs with the
        //    default config (title-report unset → false), so the app must have
        //    called `set_title_reporting(false)` on the surface's engine,
        //    overriding the engine's libghostty-vt parity default of *true*.
        //    Feed `CSI 21 t` (window-title report) and assert the engine emits
        //    NO reply — if the wiring were missing, the default-true engine
        //    would answer `ESC ] l … ST` (upstream `Surface.zig:983`).
        //    Drain first to clear any startup replies, then query.
        let _ = controller.take_surface_reply(tab_a, surface_a);
        controller.feed_surface_output(tab_a, surface_a, b"\x1b[21t");
        let reply = controller.take_surface_reply(tab_a, surface_a);
        if !reply.is_empty() {
            fail(format!(
                "title-report defaults false, so CSI 21 t must produce no reply; the \
                 app did not override the engine's parity default (got {reply:?})"
            ));
        }

        println!(
            "OK: window-chrome smoke — tabbingMode=.preferred (window-show-tab-bar=\
             always), no window shadow (macos-window-shadow=false), buttons hidden \
             (macos-window-buttons=hidden), cell-sized resize increments \
             (window-step-resize=true), darkAqua appearance (window-theme=dark), the \
             subtitle tracked the cwd (window-subtitle=working-directory), a new \
             tab landed at the group end (window-new-tab-position=end), and CSI 21 t \
             was suppressed (title-report=false wired to the engine)."
        );
        std::process::exit(0);
    }

    /// Schedule the title smoke: let the first shell settle, then run.
    fn schedule_title_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyTitleSmoke:));
    }

    /// Tab-title smoke: per-tab live titles from OSC 0/2, the update-on-change
    /// path, per-tab isolation, and the ghost-emoji fallback:
    ///
    /// 1. feed `OSC 2 ; TITLE-ALPHA BEL` into tab A's focused engine → its
    ///    window title (= native tab label) reads `TITLE-ALPHA`;
    /// 2. open tab B, feed `TITLE-BETA` → B reads `TITLE-BETA`, A still
    ///    `TITLE-ALPHA` (per-tab isolation);
    /// 3. change A's title again → A updates (set-on-change didn't wedge);
    /// 4. clear B's title (`OSC 2 ; BEL`) and wait out the 500ms grace → B
    ///    falls back to the ghost emoji (upstream
    ///    `SurfaceView_AppKit.swift:286-291`), A untouched.
    ///
    /// Exits 0/1.
    fn run_title_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = title_fail;

        let Some(tab_a) = controller.active_tab() else {
            fail("no active tab at title smoke start".into());
        };
        let Some(surface_a) = controller.active_focused_surface() else {
            fail("no focused surface at title smoke start".into());
        };
        let title_of = |tab: TabId| controller.tab_window_title(tab).unwrap_or_default();

        // 1. Live title from OSC 2 (BEL-terminated), polled by the pace tick.
        controller.feed_surface_output(tab_a, surface_a, b"\x1b]2;TITLE-ALPHA\x07");
        Self::spin(0.1);
        if title_of(tab_a) != "TITLE-ALPHA" {
            fail(format!(
                "OSC 2 title did not reach tab A's window title (got {:?})",
                title_of(tab_a)
            ));
        }

        // 2. A second tab owns its own title.
        let Some(tab_b) = controller.new_tab_in(tab_a) else {
            fail("could not open a second tab".into());
        };
        let Some(surface_b) = controller.active_focused_surface() else {
            fail("no focused surface in tab B".into());
        };
        controller.feed_surface_output(tab_b, surface_b, b"\x1b]2;TITLE-BETA\x07");
        Self::spin(0.1);
        if title_of(tab_b) != "TITLE-BETA" {
            fail(format!(
                "OSC 2 title did not reach tab B's window title (got {:?})",
                title_of(tab_b)
            ));
        }
        if title_of(tab_a) != "TITLE-ALPHA" {
            fail(format!(
                "tab B's title leaked into tab A (A now {:?})",
                title_of(tab_a)
            ));
        }

        // 3. A title *change* propagates (set-on-change caching can't wedge).
        controller.feed_surface_output(tab_a, surface_a, b"\x1b]2;TITLE-ALPHA-2\x07");
        Self::spin(0.1);
        if title_of(tab_a) != "TITLE-ALPHA-2" {
            fail(format!(
                "changed OSC 2 title did not propagate to tab A (got {:?})",
                title_of(tab_a)
            ));
        }

        // 4. Clearing the title falls back to the ghost emoji once the 500ms
        //    grace (from tab creation, long since elapsed for B) applies.
        controller.feed_surface_output(tab_b, surface_b, b"\x1b]2;\x07");
        Self::spin(0.65);
        if title_of(tab_b) != "\u{1F47B}" {
            fail(format!(
                "cleared title should fall back to the ghost emoji (got {:?})",
                title_of(tab_b)
            ));
        }
        if title_of(tab_a) != "TITLE-ALPHA-2" {
            fail(format!(
                "fallback on tab B disturbed tab A (A now {:?})",
                title_of(tab_a)
            ));
        }

        println!(
            "OK: title smoke — OSC 2 set each tab's window/tab title live \
             (TITLE-ALPHA / TITLE-BETA, per-tab isolated), a title change \
             propagated, and a cleared title fell back to the ghost emoji \
             after the 500ms grace without disturbing the other tab."
        );
        std::process::exit(0);
    }

    /// Schedule the quick-terminal smoke: let the first window settle, then run.
    fn schedule_quickterm_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyQuickTermSmoke:));
    }

    /// Quick-terminal smoke: exercise the dropdown end to end.
    ///
    /// 1. Toggle in → assert it becomes visible, its window frame lands at the
    ///    configured screen edge (== the computed final frame), and its shell
    ///    echoes typed input (a live PTY behind the dropdown).
    /// 2. Toggle out → assert it's no longer visible.
    /// 3. Toggle in again → assert it re-shows (the reuse path, not a fresh
    ///    build), preserving its shell.
    ///
    /// Exits 0/1.
    fn run_quickterm_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = quickterm_fail;
        let qt_tab = QUICK_TERMINAL_TAB;
        let qt_surface = SurfaceId(0);

        // 1. Toggle in.
        controller.toggle_quick_terminal();
        Self::spin(0.6); // animation (default 0.2s) + settle
        if !controller.quick_terminal_visible() {
            fail("toggle did not make the quick terminal visible".into());
        }

        // The window frame should have animated to the configured final frame.
        let frame = controller
            .quick_terminal_frame()
            .unwrap_or_else(|| fail("quick terminal has no window frame".into()));
        let target = controller
            .quick_terminal_final_frame()
            .unwrap_or_else(|| fail("quick terminal has no computed final frame".into()));
        let close = |a: f64, b: f64| (a - b).abs() <= 1.5;
        if !(close(frame.0, target.0)
            && close(frame.1, target.1)
            && close(frame.2, target.2)
            && close(frame.3, target.3))
        {
            fail(format!(
                "quick terminal window frame {frame:?} did not reach the configured \
                 position {target:?}"
            ));
        }
        // Default position is `top`: the window's top edge sits at the screen's
        // visible top and it spans the full visible width — sanity-check the
        // shape so a wrong-axis regression is caught, not just "some frame".
        if frame.2 < 200.0 || frame.3 < 100.0 {
            fail(format!(
                "quick terminal frame looks degenerate (w={}, h={})",
                frame.2, frame.3
            ));
        }

        // The dropdown hosts a live shell: type a command and assert the echo.
        controller.write_to_surface(qt_tab, qt_surface, b"echo QTLIVEMARKER\r");
        Self::spin(1.0);
        let screen = controller
            .surface_screen_text(qt_tab, qt_surface)
            .unwrap_or_default();
        if !screen.contains("QTLIVEMARKER") {
            fail(format!(
                "quick terminal shell did not echo typed input (marker absent).\n\
                 --- screen ---\n{screen}"
            ));
        }

        // 2. Toggle out.
        controller.toggle_quick_terminal();
        Self::spin(0.6);
        if controller.quick_terminal_visible() {
            fail("second toggle did not hide the quick terminal".into());
        }

        // 3. Toggle back in (reuse path): still the same surface, re-shown.
        controller.toggle_quick_terminal();
        Self::spin(0.6);
        if !controller.quick_terminal_visible() {
            fail("third toggle did not re-show the quick terminal".into());
        }
        // The reused shell still holds its scrollback (the earlier marker).
        let screen = controller
            .surface_screen_text(qt_tab, qt_surface)
            .unwrap_or_default();
        if !screen.contains("QTLIVEMARKER") {
            fail("re-shown quick terminal lost its shell/scrollback (marker gone)".into());
        }

        println!(
            "OK: quick-terminal smoke — toggle showed the dropdown at the configured \
             top edge (frame {frame:?}), its shell echoed typed input, toggling hid \
             it, and toggling again re-showed the same live surface."
        );
        std::process::exit(0);
    }

    /// Schedule the bell smoke: let the first window settle, then run.
    fn schedule_bell_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyBellSmoke:));
    }

    /// Bell smoke: feed a BEL into the focused pane's engine and assert the
    /// tab's title shows the 🔔 indicator (default `bell-features` includes
    /// `title`), then that refocusing the window clears it. Exits 0/1.
    fn run_bell_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = bell_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at bell smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at bell smoke start".into());
        };

        // No bell yet.
        if controller.tab_bell_ringing(tab) {
            fail("bell indicator was set before any BEL".into());
        }

        // Feed a BEL straight into the engine, then tick so the pump drains it
        // and marks the tab. Drive the tick directly (deterministic) plus a
        // short spin so the title `setTitle` lands.
        controller.feed_surface_output(tab, surface, b"\x07");
        controller.tick_once();
        Self::spin(0.1);

        if !controller.tab_bell_ringing(tab) {
            fail("a BEL did not set the tab bell indicator".into());
        }
        let title = controller.tab_window_title(tab).unwrap_or_default();
        if !title.starts_with("🔔") {
            fail(format!(
                "bell did not add the 🔔 title indicator (title {title:?})"
            ));
        }

        // Refocusing the window (windowDidBecomeKey) clears the bell.
        controller.tab_window_focus(tab, true);
        controller.tick_once();
        Self::spin(0.1);
        if controller.tab_bell_ringing(tab) {
            fail("refocusing the window did not clear the bell indicator".into());
        }
        let title = controller.tab_window_title(tab).unwrap_or_default();
        if title.starts_with("🔔") {
            fail(format!(
                "bell 🔔 title indicator persisted after refocus (title {title:?})"
            ));
        }

        println!(
            "OK: bell smoke — a BEL set the tab's 🔔 title indicator (default \
             bell-features attention+title), and refocusing the window cleared it."
        );
        std::process::exit(0);
    }

    /// Schedule the mouse smoke: let the first window settle, then run.
    fn schedule_mouse_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyMouseSmoke:));
    }

    /// Mouse-behaviors smoke: assert the right-click context menu's contents
    /// for the focused pane, then that its Split Right / Close Pane items
    /// actually split and collapse the tab. Exits 0/1.
    fn run_mouse_smoke(&self) {
        use crate::context_menu::ContextAction;
        let controller = &self.ivars().controller;
        let fail = mouse_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at mouse smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at mouse smoke start".into());
        };

        // Right-click shows the context menu by default.
        if controller.right_click_action() != crate::context_menu::RightClickAction::ContextMenu {
            fail("default right-click-action should be context-menu".into());
        }

        // Menu contents with no selection: Paste, split ×4, Close (no Copy).
        let titles = controller.context_menu_titles(tab, surface);
        let expected = vec![
            "Paste",
            "---",
            "Split Right",
            "Split Left",
            "Split Down",
            "Split Up",
            "---",
            "Close Pane",
        ];
        if titles != expected {
            fail(format!(
                "context menu (no selection) items {titles:?} != expected {expected:?}"
            ));
        }

        // One pane to start.
        if controller.active_surface_count() != Some(1) {
            fail(format!(
                "expected 1 pane at start, saw {:?}",
                controller.active_surface_count()
            ));
        }

        // Invoke "Split Right" from the context menu → 2 panes.
        controller.context_menu_action(tab, surface, ContextAction::SplitRight);
        Self::spin(0.3);
        if controller.active_surface_count() != Some(2) {
            fail(format!(
                "Split Right from the context menu did not create a 2nd pane (saw {:?})",
                controller.active_surface_count()
            ));
        }

        // The new pane is focused; "Close Pane" collapses back to 1.
        let Some(new_surface) = controller.active_focused_surface() else {
            fail("no focused surface after split".into());
        };
        controller.context_menu_action(tab, new_surface, ContextAction::ClosePane);
        Self::spin(0.3);
        if controller.active_surface_count() != Some(1) {
            fail(format!(
                "Close Pane from the context menu did not collapse the split (saw {:?})",
                controller.active_surface_count()
            ));
        }

        println!(
            "OK: mouse smoke — right-click context menu showed Paste/splits/Close (Copy \
             gated on selection), and its Split Right / Close Pane items split then \
             collapsed the tab."
        );
        std::process::exit(0);
    }

    /// Schedule the clipboard smoke: let the child settle, then run.
    fn schedule_clipboard_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyClipboardSmoke:));
    }

    /// Clipboard-hardening smoke: paste-protection classification + gating, and
    /// selection-clear-on-typing. Runs against a `cat` child (set via
    /// `QWERTTY_TERM_COMMAND`) so pastes echo deterministically and no shell
    /// enables bracketed paste. Exits 0/1.
    fn run_clipboard_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = clipboard_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at clipboard smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at clipboard smoke start".into());
        };

        // 1. Classification: a multiline paste is unsafe; a single line is safe.
        if !controller.paste_is_unsafe(tab, surface, "rm -rf /\nyes") {
            fail("a multiline paste should be classified unsafe".into());
        }
        if controller.paste_is_unsafe(tab, surface, "ls -la") {
            fail("a single-line paste should be classified safe".into());
        }

        // 2. Gating: decline an unsafe paste → it never reaches the pty (cat
        //    would otherwise echo it). Then confirm one → it does.
        controller.set_paste_confirm_hook(Some(false));
        controller.paste_text_for_test(tab, surface, "DECLINEDMARK\nrest");
        Self::spin(0.6);
        let screen = controller
            .surface_screen_text(tab, surface)
            .unwrap_or_default();
        if screen.contains("DECLINEDMARK") {
            fail(format!(
                "a declined unsafe paste still reached the pty (marker present).\n\
                 --- screen ---\n{screen}"
            ));
        }

        controller.set_paste_confirm_hook(Some(true));
        controller.paste_text_for_test(tab, surface, "CONFIRMEDMARK\nrest");
        Self::spin(0.8);
        let screen = controller
            .surface_screen_text(tab, surface)
            .unwrap_or_default();
        if !screen.contains("CONFIRMEDMARK") {
            fail(format!(
                "a confirmed unsafe paste did not reach the pty (marker absent).\n\
                 --- screen ---\n{screen}"
            ));
        }

        // 3. selection-clear-on-typing: make a selection, then type → cleared.
        controller.feed_surface_output(tab, surface, b"\r\nSELECTME\r\n");
        Self::spin(0.2);
        controller.smoke_select_row0(tab, surface, 4);
        if !controller.surface_has_selection(tab, surface) {
            fail("failed to create a selection for the clear-on-typing check".into());
        }
        // Typed text (the IME-commit path) clears the selection.
        controller.send_text_to_surface(tab, surface, "x");
        if controller.surface_has_selection(tab, surface) {
            fail("typing did not clear the selection (selection-clear-on-typing)".into());
        }

        println!(
            "OK: clipboard smoke — paste-protection classified a multiline paste unsafe \
             (declined paste never reached the pty; confirmed one did), and typing \
             cleared the selection."
        );
        std::process::exit(0);
    }

    fn schedule_windowstate_smoke(&self) {
        // Give AppKit time to place + first-layout the window before probing.
        self.schedule_selector(0.7, sel!(ghosttyWindowStateSmoke:));
    }

    /// Window-state smoke: with `window-width`/`window-height` configured (the
    /// harness writes a temp config), assert the first window's live grid
    /// equals the requested cell count — proving the initial geometry override
    /// took effect end-to-end (config → Controller → `setContentSize`). Exits
    /// 0/1.
    fn run_windowstate_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = windowstate_fail;

        let Some((cfg_cols, cfg_rows)) = controller.configured_initial_cells() else {
            fail(
                "no initial window geometry configured — the smoke harness must set \
                 window-width/window-height in a temp QWERTTY_TERM_CONFIG_DIR."
                    .into(),
            );
        };
        let Some(tab) = controller.active_tab() else {
            fail("no active tab at window-state smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at window-state smoke start".into());
        };

        // Let the window settle (place + backing-scale correction + relayout).
        Self::spin(0.4);

        let Some((cols, rows)) = controller.surface_grid(tab, surface) else {
            fail("could not read the surface grid".into());
        };

        // The grid is derived from the content area / cell size; if the initial
        // geometry took effect the fit rounds back to the configured cells. Allow
        // a ±1 cell slack for sub-pixel content-rect rounding.
        let (want_c, want_r) = (cfg_cols as i64, cfg_rows as i64);
        let (got_c, got_r) = (cols as i64, rows as i64);
        if (got_c - want_c).abs() > 1 || (got_r - want_r).abs() > 1 {
            fail(format!(
                "the first window did not honor the configured initial geometry: \
                 requested {want_c}x{want_r} cells but the live grid is {got_c}x{got_r} \
                 (more than 1 cell off). The window-width/-height override did not \
                 reach setContentSize on the first window."
            ));
        }

        println!(
            "OK: window-state smoke — the first window honored the configured initial \
             geometry (requested {want_c}x{want_r} cells; live grid {got_c}x{got_r})."
        );
        std::process::exit(0);
    }

    fn schedule_notify_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyNotifySmoke:));
    }

    /// Desktop-notification smoke: feed OSC 9 and OSC 777 to the focused
    /// surface and assert each is parsed, drained, gated, throttled, and
    /// delivered (observed via `last_delivered_notification`). Real OS delivery
    /// needs a bundle (ADR 0003); this asserts the end-to-end plumbing up to the
    /// delivery seam. Exits 0/1.
    fn run_notify_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = notify_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at notify smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at notify smoke start".into());
        };

        // 1. OSC 9: the whole payload is the body; title is empty.
        controller.feed_surface_output(tab, surface, b"\x1b]9;build finished\x07");
        Self::spin(0.4);
        match controller.last_delivered_notification() {
            Some((title, body)) if title.is_empty() && body == "build finished" => {}
            other => fail(format!(
                "OSC 9 notification did not reach the delivery seam as (\"\", \
                 \"build finished\"); got {other:?}"
            )),
        }

        // 2. OSC 777;notify;Title;Body after the 1s global throttle window.
        Self::spin(1.1);
        controller.feed_surface_output(
            tab,
            surface,
            b"\x1b]777;notify;Deploy;the release is live\x07",
        );
        Self::spin(0.4);
        match controller.last_delivered_notification() {
            Some((title, body)) if title == "Deploy" && body == "the release is live" => {}
            other => fail(format!(
                "OSC 777 notification did not reach the delivery seam as \
                 (\"Deploy\", \"the release is live\"); got {other:?}"
            )),
        }

        println!(
            "OK: notify smoke — OSC 9 and OSC 777 desktop notifications parsed, drained, \
             gated, throttled, and delivered to the notification seam."
        );
        std::process::exit(0);
    }

    fn schedule_notifycmd_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyNotifyCmdSmoke:));
    }

    /// Command-finish smoke (`notify-on-command-finish`): feed OSC 133 `C`/`D`
    /// marks and assert the app times the command and delivers the right
    /// finish notification (title by exit status). The harness configures
    /// `always` + `notify` action + `after = 0` so focus/threshold don't gate.
    /// Exits 0/1.
    fn run_notifycmd_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = notifycmd_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at notifycmd smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at notifycmd smoke start".into());
        };

        // A successful command: C … (time passes) … D;0.
        controller.feed_surface_output(tab, surface, b"\x1b]133;C\x07");
        Self::spin(0.3);
        controller.feed_surface_output(tab, surface, b"\x1b]133;D;0\x07");
        Self::spin(0.4);
        match controller.last_delivered_notification() {
            Some((title, body)) if title == "Command Succeeded" && body.contains("code 0") => {}
            other => fail(format!(
                "a finished (exit 0) command did not deliver a \"Command Succeeded\" \
                 notification; got {other:?}"
            )),
        }

        // A failed command after the 1s throttle window: C … D;3.
        Self::spin(1.1);
        controller.feed_surface_output(tab, surface, b"\x1b]133;C\x07");
        Self::spin(0.3);
        controller.feed_surface_output(tab, surface, b"\x1b]133;D;3\x07");
        Self::spin(0.4);
        match controller.last_delivered_notification() {
            Some((title, body)) if title == "Command Failed" && body.contains("code 3") => {}
            other => fail(format!(
                "a finished (exit 3) command did not deliver a \"Command Failed\" \
                 notification; got {other:?}"
            )),
        }

        println!(
            "OK: notifycmd smoke — OSC 133 command boundaries timed a command and \
             delivered the exit-status finish notification (succeeded + failed)."
        );
        std::process::exit(0);
    }

    fn schedule_progress_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyProgressSmoke:));
    }

    /// Progress-bar smoke (OSC 9;4): feed set/error/remove reports and assert
    /// the app parses, drains, gates (`progress-style`), and tracks the derived
    /// display state (fraction + category), clearing on `remove`. Exits 0/1.
    fn run_progress_smoke(&self) {
        use crate::progress::ProgressCategory;
        let controller = &self.ivars().controller;
        let fail = progress_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at progress smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at progress smoke start".into());
        };

        // 1. Set 50% → a determinate normal bar at 0.5.
        controller.feed_surface_output(tab, surface, b"\x1b]9;4;1;50\x07");
        Self::spin(0.2);
        match controller.surface_progress(tab, surface) {
            Some(d)
                if d.category == ProgressCategory::Normal
                    && !d.indeterminate
                    && (d.fraction - 0.5).abs() < 1e-6 => {}
            other => fail(format!(
                "set 50% did not yield a 0.5 normal bar; got {other:?}"
            )),
        }

        // 2. Error 90% → a red bar at 0.9.
        controller.feed_surface_output(tab, surface, b"\x1b]9;4;2;90\x07");
        Self::spin(0.2);
        match controller.surface_progress(tab, surface) {
            Some(d) if d.category == ProgressCategory::Error && (d.fraction - 0.9).abs() < 1e-6 => {
            }
            other => fail(format!(
                "error 90% did not yield a 0.9 error bar; got {other:?}"
            )),
        }

        // 3. Remove → the bar clears.
        controller.feed_surface_output(tab, surface, b"\x1b]9;4;0\x07");
        Self::spin(0.2);
        if let Some(d) = controller.surface_progress(tab, surface) {
            fail(format!("remove did not clear the progress bar; got {d:?}"));
        }

        println!(
            "OK: progress smoke — OSC 9;4 set/error/remove parsed, drained, gated, and \
             tracked as a progress-bar display state (0.5 normal → 0.9 error → cleared)."
        );
        std::process::exit(0);
    }

    fn schedule_confirmclose_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyConfirmCloseSmoke:));
    }

    /// Confirm-close smoke: drive the OSC 133 prompt state and assert
    /// `confirm-close-surface` (default `true`) needs confirmation only when a
    /// process is running (or shell integration is absent), and that the modal
    /// answer gates the close. Exits 0/1.
    fn run_confirmclose_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = confirmclose_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at confirm-close smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at confirm-close smoke start".into());
        };

        // 1. No shell integration yet → cursor is in the `output` state, so the
        //    default (`true`) errs toward confirming.
        if !controller.tab_needs_confirm_close(tab) {
            fail("with no shell integration the default should need confirmation".into());
        }

        // 2. Mark a prompt (OSC 133 A then B) → at a prompt → no confirmation.
        controller.feed_surface_output(tab, surface, b"\x1b]133;A\x07\x1b]133;B\x07");
        Self::spin(0.2);
        if controller.tab_needs_confirm_close(tab) {
            fail("at a shell prompt, closing should not need confirmation".into());
        }

        // 3. A command starts producing output (OSC 133 C) → running → confirm.
        controller.feed_surface_output(tab, surface, b"\x1b]133;C\x07");
        Self::spin(0.2);
        if !controller.tab_needs_confirm_close(tab) {
            fail("a running command should need close confirmation".into());
        }

        // 4. The modal answer gates the close (non-destructive check): "Cancel"
        //    vetoes, "Close" proceeds.
        controller.set_close_confirm_hook(Some(false));
        if controller.would_confirm_close_surface(tab, surface) {
            fail("cancelling the confirmation should veto the close".into());
        }
        controller.set_close_confirm_hook(Some(true));
        if !controller.would_confirm_close_surface(tab, surface) {
            fail("confirming should let the close proceed".into());
        }

        println!(
            "OK: confirm-close smoke — confirm-close-surface needs confirmation only when a \
             process is running (or shell integration is absent), and the modal answer \
             gates the close."
        );
        std::process::exit(0);
    }

    fn schedule_resize_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyResizeSmoke:));
    }

    /// Resize-overlay smoke: with `resize-overlay = always` + a short duration
    /// configured, resize the window and assert the `cols ⨯ rows` HUD shows the
    /// new grid, then auto-clears after its duration. Exits 0/1.
    fn run_resize_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = resize_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at resize smoke start".into());
        };

        // No overlay before any resize.
        if controller.tab_resize_overlay_text(tab).is_some() {
            fail("the overlay should not be shown before any resize".into());
        }

        // Resize the window; the HUD should show the new grid.
        let Some((_tab, cols, rows)) = controller.smoke_resize_active_window(-80.0, -60.0) else {
            fail("failed to resize the active window".into());
        };
        Self::spin(0.1);
        let want = crate::resize_overlay::overlay_text(cols, rows);
        match controller.tab_resize_overlay_text(tab) {
            Some(text) if text == want => {}
            other => fail(format!(
                "after a resize the overlay should read {want:?}; got {other:?}"
            )),
        }

        // After the (short, 400ms configured) duration elapses, it clears.
        Self::spin(0.7);
        if let Some(text) = controller.tab_resize_overlay_text(tab) {
            fail(format!(
                "the overlay should auto-clear after its duration; still showing {text:?}"
            ));
        }

        println!(
            "OK: resize smoke — the resize overlay showed the new cols ⨯ rows grid and \
             auto-cleared after its duration."
        );
        std::process::exit(0);
    }

    fn schedule_mouse2_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttyMouse2Smoke:));
    }

    /// Mouse slice-2 smoke: middle-click primary-paste + focus-follows-mouse.
    /// The harness configures `focus-follows-mouse = true` and uses a `cat`
    /// child so a paste echoes deterministically. Exits 0/1.
    fn run_mouse2_smoke(&self) {
        use crate::context_menu::ContextAction;
        let controller = &self.ivars().controller;
        let fail = mouse2_fail;

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at mouse2 smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at mouse2 smoke start".into());
        };

        // 1. middle-click primary-paste: show a marker on screen, select it,
        //    then middle-click — which pastes the selection to the pty, so the
        //    `cat` child echoes it and the marker appears a second time. Let the
        //    login banner settle, then clear + home so the marker is at row 0
        //    (where `smoke_select_row0` selects).
        Self::spin(0.3);
        controller.feed_surface_output(tab, surface, b"\x1b[2J\x1b[HMARK123456");
        Self::spin(0.2);
        controller.smoke_select_row0(tab, surface, 10);
        if !controller.surface_has_selection(tab, surface) {
            fail("failed to create a selection for the primary-paste check".into());
        }
        controller.middle_click(tab, surface);
        Self::spin(0.6);
        let screen = controller
            .surface_screen_text(tab, surface)
            .unwrap_or_default();
        if screen.matches("MARK123456").count() < 2 {
            fail(format!(
                "middle-click did not primary-paste the selection (marker should appear \
                 twice).\n--- screen ---\n{screen}"
            ));
        }

        // 2. focus-follows-mouse: split right (the new pane takes focus), then
        //    simulate the mouse entering the original pane — with the config on,
        //    that focuses it (exactly what the view's `mouseEntered:` does).
        if !controller.focus_follows_mouse() {
            fail("focus-follows-mouse should be enabled by the smoke config".into());
        }
        let original = surface;
        controller.context_menu_action(tab, original, ContextAction::SplitRight);
        Self::spin(0.4);
        let new_pane = controller.active_focused_surface();
        if new_pane == Some(original) || new_pane.is_none() {
            fail("Split Right did not create + focus a new pane".into());
        }
        // The mouseEntered handler is `if focus_follows_mouse() { focus(..) }`.
        if controller.focus_follows_mouse() {
            controller.focus_surface_in_tab(tab, original);
        }
        if controller.active_focused_surface() != Some(original) {
            fail("entering the original pane with focus-follows-mouse did not focus it".into());
        }

        println!(
            "OK: mouse2 smoke — middle-click primary-pasted the selection, and \
             focus-follows-mouse focused the hovered pane."
        );
        std::process::exit(0);
    }

    fn schedule_savestate_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttySaveStateSmoke:));
    }

    /// Window-save-state smoke: with `window-save-state = never` configured,
    /// assert the window is marked non-restorable and the
    /// `NSQuitAlwaysKeepsWindows` user default was set to false. Exits 0/1.
    fn run_savestate_smoke(&self) {
        let controller = &self.ivars().controller;
        let fail = savestate_fail;

        // 1. The window reflects the config (never → non-restorable).
        match controller.active_window_is_restorable() {
            Some(false) => {}
            other => fail(format!(
                "with window-save-state=never the window should be non-restorable; got {other:?}"
            )),
        }

        // 2. The NSQuitAlwaysKeepsWindows default was set (present) and false.
        let defaults = objc2_foundation::NSUserDefaults::standardUserDefaults();
        let key = NSString::from_str("NSQuitAlwaysKeepsWindows");
        let present = defaults.objectForKey(&key).is_some();
        if !present || defaults.boolForKey(&key) {
            fail("NSQuitAlwaysKeepsWindows should be present and false for never".into());
        }

        println!(
            "OK: savestate smoke — window-save-state=never marked the window non-restorable \
             and set NSQuitAlwaysKeepsWindows=false."
        );
        std::process::exit(0);
    }

    fn schedule_session_smoke(&self) {
        self.schedule_selector(0.7, sel!(ghosttySessionSmoke:));
    }

    /// Window-session smoke (window-save-state slice 2 core): capture a live
    /// tab's tree + per-pane cwd, round-trip the JSON, capture a split, and
    /// restore a single-pane session into a fresh tab. Exits 0/1.
    fn run_session_smoke(&self) {
        use crate::context_menu::ContextAction;
        use crate::session::{SessionAxis, SessionNode, WindowSession};
        let controller = &self.ivars().controller;
        let fail = session_fail;

        // Structural equality ignoring cwd (restored shells run `sleep`, so they
        // never emit OSC 7 — only the tree shape + ratios round-trip live).
        fn shape_eq(a: &SessionNode, b: &SessionNode) -> bool {
            match (a, b) {
                (SessionNode::Leaf { .. }, SessionNode::Leaf { .. }) => true,
                (
                    SessionNode::Split {
                        axis: aax,
                        ratio: ar,
                        first: af,
                        second: asec,
                    },
                    SessionNode::Split {
                        axis: bax,
                        ratio: br,
                        first: bf,
                        second: bsec,
                    },
                ) => {
                    aax == bax && (ar - br).abs() < 0.02 && shape_eq(af, bf) && shape_eq(asec, bsec)
                }
                _ => false,
            }
        }

        let Some(tab) = controller.active_tab() else {
            fail("no active tab at session smoke start".into());
        };
        let Some(surface) = controller.active_focused_surface() else {
            fail("no focused surface at session smoke start".into());
        };

        // 0. OS restoration wiring: with the default config the window is
        //    restorable and carries the restoration identifier the app delegate's
        //    restorationClass matches on relaunch.
        if controller.active_window_is_restorable() != Some(true) {
            fail("default-config window should be restorable".into());
        }
        match controller.active_window_identifier().as_deref() {
            Some(WINDOW_RESTORATION_IDENTIFIER) => {}
            other => fail(format!(
                "restorable window should carry the restoration identifier; got {other:?}"
            )),
        }

        // 1. Set a cwd via OSC 7, then capture — the single leaf carries it.
        controller.feed_surface_output(tab, surface, b"\x1b]7;file:///tmp\x07");
        Self::spin(0.2);
        let Some(captured) = controller.capture_window_session(tab) else {
            fail("capture_window_session returned None".into());
        };
        match &captured.tree {
            SessionNode::Leaf { cwd } if cwd.as_deref() == Some("/tmp") => {}
            other => fail(format!("captured leaf cwd should be /tmp; got {other:?}")),
        }

        // 2. JSON round-trips.
        let json = captured.to_json();
        match WindowSession::from_json(&json) {
            Some(back) if back == captured => {}
            other => fail(format!("session JSON did not round-trip; got {other:?}")),
        }

        // 3. A split is captured as a two-leaf tree.
        controller.context_menu_action(tab, surface, ContextAction::SplitRight);
        Self::spin(0.3);
        let Some(split_session) = controller.capture_window_session(tab) else {
            fail("capture after split returned None".into());
        };
        if split_session.tree.leaf_count() != 2 {
            fail(format!(
                "a split should capture two leaves; got {}",
                split_session.tree.leaf_count()
            ));
        }

        // 4. Restore a single-pane session → a fresh, distinct tab is spawned.
        let single = WindowSession::new(SessionNode::Leaf {
            cwd: Some("/tmp".into()),
        });
        let before = controller.tab_count();
        let Some(new_tab) = controller.restore_window_session(&single) else {
            fail("restore_window_session returned None".into());
        };
        Self::spin(0.3);
        if new_tab == tab || controller.tab_count() != before + 1 {
            fail("restore did not spawn a distinct new tab".into());
        }

        // 5. Restore a multi-pane session (0 | (1 / 2) with non-even ratios) and
        //    re-capture the rebuilt tab: the structure + ratios must match.
        let multi = WindowSession::new(SessionNode::Split {
            axis: SessionAxis::Horizontal,
            ratio: 0.7,
            first: Box::new(SessionNode::Leaf { cwd: None }),
            second: Box::new(SessionNode::Split {
                axis: SessionAxis::Vertical,
                ratio: 0.3,
                first: Box::new(SessionNode::Leaf { cwd: None }),
                second: Box::new(SessionNode::Leaf { cwd: None }),
            }),
        });
        let Some(rebuilt_tab) = controller.restore_window_session(&multi) else {
            fail("restore of a multi-pane session returned None".into());
        };
        Self::spin(0.5);
        let Some(rebuilt) = controller.capture_window_session(rebuilt_tab) else {
            fail("capture of the rebuilt tab returned None".into());
        };
        if rebuilt.tree.leaf_count() != 3 {
            fail(format!(
                "rebuilt tree should have three panes; got {}",
                rebuilt.tree.leaf_count()
            ));
        }
        if !shape_eq(&rebuilt.tree, &multi.tree) {
            fail(format!(
                "rebuilt tree shape/ratios did not match the session; got {:?}",
                rebuilt.tree
            ));
        }

        // 6. Persist `tab`'s live session through a real `NSKeyedArchiver` and
        //    recover it from an `NSKeyedUnarchiver` — the exact Cocoa coder path
        //    macOS drives for window-restoration state. Proves the NSCoder codec
        //    (`encode_session_into` / `decode_session_from`) round-trips.
        use objc2::AllocAnyThread;
        use objc2_foundation::{NSKeyedArchiver, NSKeyedUnarchiver};
        let live = controller
            .capture_window_session(tab)
            .unwrap_or_else(|| fail("capture of the coder-source tab returned None".into()));
        let archiver = NSKeyedArchiver::initRequiringSecureCoding(NSKeyedArchiver::alloc(), true);
        controller.encode_session_into(&archiver, tab);
        archiver.finishEncoding();
        let data = archiver.encodedData();
        let unarchiver = match unsafe {
            NSKeyedUnarchiver::initForReadingFromData_error(NSKeyedUnarchiver::alloc(), &data)
        } {
            Ok(u) => u,
            Err(e) => fail(format!("NSKeyedUnarchiver init failed: {e:?}")),
        };
        let Some(decoded) = Controller::decode_session_from(&unarchiver) else {
            fail("decode_session_from recovered no session from the archive".into());
        };
        if decoded != live {
            fail(format!(
                "coder round-trip changed the session; encoded {live:?}, decoded {decoded:?}"
            ));
        }

        println!(
            "OK: session smoke — captured a tab's tree + cwd, round-tripped the JSON, \
             captured a split as two leaves, restored a single-pane session into a fresh \
             tab, rebuilt a 3-pane session (structure + ratios) into another, and \
             round-tripped a live session through a real NSKeyedArchiver/NSCoder."
        );
        std::process::exit(0);
    }

    /// Pump the run loop for `secs` so scheduled io + pace ticks make progress
    /// (the smoke drives assertions synchronously between focus switches, but the
    /// pty round-trip + render happen on the run loop). Runs the default run loop
    /// in short slices.
    fn spin(secs: f64) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs_f64(secs);
        while std::time::Instant::now() < deadline {
            let rl = objc2_foundation::NSRunLoop::currentRunLoop();
            let until = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(0.02);
            rl.runUntilDate(&until);
        }
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

    /// Start the ~60 Hz service timer (io-service only, no render), paired with
    /// the display link. Repeating, on the main run loop.
    fn start_service_timer(&self) {
        let interval = 1.0 / 60.0;
        let target: &AnyObject = self.as_ref();
        // SAFETY: the delegate outlives the timer; the selector is implemented
        // on this class; main-thread call.
        unsafe {
            let _ = objc2_foundation::NSTimer::scheduledTimerWithTimeInterval_target_selector_userInfo_repeats(
                interval,
                target,
                sel!(ghosttyServiceTick:),
                None,
                true,
            );
        }
    }

    /// Start a `CADisplayLink` render clock, synced to the first window's
    /// display refresh, firing `ghosttyRenderTick:`. Returns `false` (caller
    /// should fall back to the combined `NSTimer` pace tick) when there's no
    /// screen-backed content view to bind to.
    ///
    /// An `NSTimer` isn't phase-locked to the display, so on a 120 Hz ProMotion
    /// panel its presents land at drifting, uneven times relative to vsync and
    /// high-fps content (e.g. a full-screen animation) judders. A display link
    /// is a vsync callback, so presents land on refresh boundaries and the
    /// subsampling of fast content stays even and smooth. `NSView.displayLink`
    /// (macOS 14+) gives a main-thread `CADisplayLink`, so the AppKit/Metal
    /// present needs no cross-thread marshaling (unlike classic `CVDisplayLink`).
    /// Render is split onto the link while io-service stays on the steady
    /// [`start_service_timer`](Self::start_service_timer) — the link pauses for
    /// occluded windows, but a background terminal must still service its io.
    fn start_display_link(&self) -> bool {
        let Some(window) = self.ivars().controller.active_window() else {
            return false;
        };
        self.bind_display_link(&window)
    }

    /// Bind (or rebind) the render [`CADisplayLink`] to `window`'s content view,
    /// replacing any current link. Factored out of [`start_display_link`] so the
    /// link can be re-pointed at a different window when the one it was bound to
    /// is ordered out (a display link pauses for its occluded view). Returns
    /// `false` when the window has no screen-backed content view.
    fn bind_display_link(&self, window: &NSWindow) -> bool {
        let Some(view) = window.contentView() else {
            return false;
        };
        // Drop the previous link (if any) before creating the new one.
        if let Some(old) = self.ivars().display_link.borrow_mut().take() {
            old.invalidate();
        }
        // SAFETY: main thread; `self` (the target) outlives the link, retained
        // in `display_link` below; `ghosttyRenderTick:` is implemented on this
        // class. The link auto-associates with the view's display and re-tracks
        // if the view moves to another screen.
        let link = unsafe { view.displayLinkWithTarget_selector(self, sel!(ghosttyRenderTick:)) };
        // SAFETY: main run loop; common modes keep it firing during modal /
        // event-tracking loops (window resize, menus).
        unsafe {
            let run_loop = NSRunLoop::mainRunLoop();
            link.addToRunLoop_forMode(&run_loop, NSRunLoopCommonModes);
        }
        *self.ivars().display_link.borrow_mut() = Some(link);
        true
    }

    /// Re-point the render display link at a currently-visible window. Called
    /// when a tmux control tab is hidden/restored: the link was bound to that
    /// window's view and a `CADisplayLink` pauses for an occluded view, which
    /// would otherwise stall the whole render loop. No-op when render is driven
    /// by the pace `NSTimer` (no link exists — GUI smokes / no screen-backed
    /// window). Falls back to the pace timer if no visible window remains.
    pub(crate) fn rebind_display_link(&self) {
        if self.ivars().display_link.borrow().is_none() {
            return;
        }
        match self.ivars().controller.first_visible_window() {
            Some(window) if self.bind_display_link(&window) => {}
            _ => {
                if let Some(old) = self.ivars().display_link.borrow_mut().take() {
                    old.invalidate();
                }
                self.start_pace_timer();
            }
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

/// Print a focus smoke failure and exit non-zero.
fn focus_fail(msg: String) -> ! {
    eprintln!("FAIL: focus smoke — {msg}");
    std::process::exit(1);
}

/// Print a search smoke failure and exit non-zero.
fn search_fail(msg: String) -> ! {
    eprintln!("FAIL: search smoke — {msg}");
    std::process::exit(1);
}

/// Print a selection-smoke failure and exit non-zero. Free `!`-returning fn so
/// it works inside `Option::unwrap_or_else` closures (which need the never
/// type).
fn selection_fail(msg: String) -> ! {
    eprintln!("FAIL: selection smoke — {msg}");
    std::process::exit(1);
}

/// Print a title-smoke failure and exit non-zero.
fn title_fail(msg: String) -> ! {
    eprintln!("FAIL: title smoke — {msg}");
    std::process::exit(1);
}

/// Print a quick-terminal-smoke failure and exit non-zero.
fn quickterm_fail(msg: String) -> ! {
    eprintln!("FAIL: quick-terminal smoke — {msg}");
    std::process::exit(1);
}

/// Print a bell-smoke failure and exit non-zero.
fn bell_fail(msg: String) -> ! {
    eprintln!("FAIL: bell smoke — {msg}");
    std::process::exit(1);
}

/// Print a mouse-smoke failure and exit non-zero.
/// Print a tmux-lifecycle-smoke failure and exit non-zero.
fn tmuxlife_fail(msg: String) -> ! {
    eprintln!("FAIL: tmux lifecycle smoke — {msg}");
    std::process::exit(1);
}

fn mouse_fail(msg: String) -> ! {
    eprintln!("FAIL: mouse smoke — {msg}");
    std::process::exit(1);
}

/// Print a clipboard-smoke failure and exit non-zero.
fn clipboard_fail(msg: String) -> ! {
    eprintln!("FAIL: clipboard smoke — {msg}");
    std::process::exit(1);
}

/// Print a window-state smoke failure and exit non-zero.
fn windowstate_fail(msg: String) -> ! {
    eprintln!("FAIL: window-state smoke — {msg}");
    std::process::exit(1);
}

/// Print a notify-smoke failure and exit non-zero.
fn notify_fail(msg: String) -> ! {
    eprintln!("FAIL: notify smoke — {msg}");
    std::process::exit(1);
}

/// Print a command-finish smoke failure and exit non-zero.
fn notifycmd_fail(msg: String) -> ! {
    eprintln!("FAIL: notifycmd smoke — {msg}");
    std::process::exit(1);
}

/// Print a progress-bar smoke failure and exit non-zero.
fn progress_fail(msg: String) -> ! {
    eprintln!("FAIL: progress smoke — {msg}");
    std::process::exit(1);
}

/// Print a confirm-close smoke failure and exit non-zero.
fn confirmclose_fail(msg: String) -> ! {
    eprintln!("FAIL: confirm-close smoke — {msg}");
    std::process::exit(1);
}

/// Print a resize-overlay smoke failure and exit non-zero.
fn resize_fail(msg: String) -> ! {
    eprintln!("FAIL: resize smoke — {msg}");
    std::process::exit(1);
}

/// Print a mouse-2 smoke failure and exit non-zero.
fn mouse2_fail(msg: String) -> ! {
    eprintln!("FAIL: mouse2 smoke — {msg}");
    std::process::exit(1);
}

/// Print a save-state smoke failure and exit non-zero.
fn savestate_fail(msg: String) -> ! {
    eprintln!("FAIL: savestate smoke — {msg}");
    std::process::exit(1);
}

/// Print a session smoke failure and exit non-zero.
fn session_fail(msg: String) -> ! {
    eprintln!("FAIL: session smoke — {msg}");
    std::process::exit(1);
}

/// Print a wordchars smoke failure and exit non-zero.
fn wordchars_fail(msg: String) -> ! {
    eprintln!("FAIL: wordchars smoke — {msg}");
    std::process::exit(1);
}

/// Print a mouse-shift smoke failure and exit non-zero.
fn mouseshift_fail(msg: String) -> ! {
    eprintln!("FAIL: mouse-shift smoke — {msg}");
    std::process::exit(1);
}

/// Print a clear-copy smoke failure and exit non-zero.
fn clearcopy_fail(msg: String) -> ! {
    eprintln!("FAIL: clear-copy smoke — {msg}");
    std::process::exit(1);
}

/// Print a window-chrome smoke failure and exit non-zero.
fn windowchrome_fail(msg: String) -> ! {
    eprintln!("FAIL: window-chrome smoke — {msg}");
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
const KEYCODE_RETURN: u16 = 0x24; // kVK_Return
const KEYCODE_F: u16 = 0x03; // kVK_ANSI_F
const KEYCODE_V: u16 = 0x09; // kVK_ANSI_V
const KEYCODE_ESCAPE: u16 = 0x35; // kVK_Escape
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
        qwertty_term_input::key_encode::Options::default(),
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
/// directional focus, divider resize, and close-collapse, exit); `smoke_keybind`
/// instead runs the keybind smoke (seed the maintainer's shift+enter `text:`
/// binding, drive shift+enter + plain enter through the real key path, assert
/// the pty round-trip, exit); `smoke_focus` instead runs the per-pane
/// focus-reporting smoke (two `cat -v` panes with mode 1004 on; focus-switch and
/// assert focus-in/out bytes reach the right ptys, exit). Returns after the run
/// loop exits.
#[allow(clippy::too_many_arguments)]
pub fn run(
    config: &crate::config::Config,
    smoke_ms: u64,
    smoke_type: String,
    smoke_geometry: bool,
    smoke_tabkeys: bool,
    smoke_splits: bool,
    smoke_keybind: bool,
    smoke_focus: bool,
    smoke_search: bool,
    smoke_selection: bool,
    smoke_title: bool,
    smoke_quickterm: bool,
    smoke_bell: bool,
    smoke_mouse: bool,
    smoke_clipboard: bool,
    smoke_windowstate: bool,
    smoke_notify: bool,
    smoke_notifycmd: bool,
    smoke_progress: bool,
    smoke_confirmclose: bool,
    smoke_resize: bool,
    smoke_mouse2: bool,
    smoke_savestate: bool,
    smoke_session: bool,
    smoke_wordchars: bool,
    smoke_mouseshift: bool,
    smoke_clearcopy: bool,
    smoke_windowchrome: bool,
) {
    let mtm = MainThreadMarker::new().expect("run() must be called on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    // Accessory keeps the app out of the Dock and off the foreground, so a smoke
    // run renders on the windowserver without becoming the active application.
    app.setActivationPolicy(if background_mode() {
        NSApplicationActivationPolicy::Accessory
    } else {
        NSApplicationActivationPolicy::Regular
    });

    let controller = Controller::new(config, mtm);
    let delegate = AppDelegate::new(
        mtm,
        controller,
        smoke_ms,
        smoke_type,
        smoke_geometry,
        smoke_tabkeys,
        smoke_splits,
        smoke_keybind,
        smoke_focus,
        smoke_search,
        smoke_selection,
        smoke_title,
        smoke_quickterm,
        smoke_bell,
        smoke_mouse,
        smoke_clipboard,
        smoke_windowstate,
        smoke_notify,
        smoke_notifycmd,
        smoke_progress,
        smoke_confirmclose,
        smoke_resize,
        smoke_mouse2,
        smoke_savestate,
        smoke_session,
        smoke_wordchars,
        smoke_mouseshift,
        smoke_clearcopy,
        smoke_windowchrome,
    );
    let object = ProtocolObject::from_ref(&*delegate);
    app.setDelegate(Some(object));

    app.run();
}

#[cfg(test)]
mod tests {
    use super::arrow_key_bytes;
    use super::lock_or_recover;
    use super::url_at_cell;
    use crate::engine::Engine;
    use std::sync::{Arc, Mutex};

    #[test]
    fn url_at_cell_resolves_osc8_and_regex_urls() {
        use qwertty_term_vt::stream::{Stream, TerminalHandler};
        use qwertty_term_vt::terminal::{Options, Terminal};

        // Row 0: an OSC8 link over "ab" → then a plain "http://x.io" (regex).
        let term = Terminal::new(Options {
            cols: 40,
            rows: 2,
            ..Default::default()
        });
        let mut stream = Stream::new(TerminalHandler::new(term));
        stream.feed(b"\x1b]8;;https://osc8.test\x1b\\ab\x1b]8;;\x1b\\ http://x.io");
        let snap = stream.handler.terminal.snapshot_window(0);

        // OSC8 cells (col 0-1) resolve to the OSC8 URI (not the visible text).
        assert_eq!(
            url_at_cell(&snap, 0, 0).as_deref(),
            Some("https://osc8.test")
        );
        assert_eq!(
            url_at_cell(&snap, 1, 0).as_deref(),
            Some("https://osc8.test")
        );

        // The plain URL starts at col 3 ("ab" + space). Any column inside it
        // resolves to the detected URL span text.
        let http_col = 3 + "http:".len(); // a column inside "http://x.io"
        assert_eq!(
            url_at_cell(&snap, http_col, 0).as_deref(),
            Some("http://x.io")
        );

        // The separating space (col 2) is over neither link.
        assert_eq!(url_at_cell(&snap, 2, 0), None);
    }

    #[test]
    fn arrow_key_bytes_match_upstream_alternate_scroll_sequences() {
        // Normal mode (DECCKM off): CSI A / CSI B.
        assert_eq!(arrow_key_bytes(true, false), b"\x1b[A");
        assert_eq!(arrow_key_bytes(false, false), b"\x1b[B");
        // Application mode (DECCKM on): SS3 A / SS3 B.
        assert_eq!(arrow_key_bytes(true, true), b"\x1bOA");
        assert_eq!(arrow_key_bytes(false, true), b"\x1bOB");
    }

    #[test]
    fn write_temp_text_file_round_trips_and_is_unique() {
        let a = super::write_temp_text_file("hello scrollback\n", "test").expect("write a");
        let b = super::write_temp_text_file("second", "test").expect("write b");
        assert_ne!(a, b, "each call gets a unique path");
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "hello scrollback\n");
        assert_eq!(std::fs::read_to_string(&b).unwrap(), "second");
        let _ = std::fs::remove_file(&a);
        let _ = std::fs::remove_file(&b);
    }

    #[test]
    fn compose_window_title_forces_over_osc_and_applies_decorations() {
        use super::compose_window_title;

        // A configured `title` wins over the program's OSC title.
        assert_eq!(
            compose_window_title(Some("Build"), Some("vim"), true, false, false),
            "Build"
        );
        // Without a forced title, the OSC title is used.
        assert_eq!(
            compose_window_title(None, Some("vim"), true, false, false),
            "vim"
        );
        // Neither, after the grace → ghost; before the grace → app name.
        assert_eq!(compose_window_title(None, None, true, false, false), "👻");
        assert_eq!(
            compose_window_title(None, None, false, false, false),
            "qwertty-term"
        );
        // Bell prefix + password suffix decorate the chosen base (even a forced one).
        assert_eq!(
            compose_window_title(Some("Build"), None, true, true, true),
            "🔔 Build 🔒"
        );
        // An empty OSC title is ignored (falls through to the fallback).
        assert_eq!(
            compose_window_title(None, Some(""), true, false, false),
            "👻"
        );
    }

    /// `cursor-color` seeds the engine's startup cursor color, overriding the
    /// theme's (here: no theme, so it overrides the default UNSET). A later OSC 12
    /// from the running program still wins through the same dynamic-color path.
    #[test]
    fn cursor_color_config_seeds_startup_colors() {
        let mut config = crate::config::Config::default();
        // No theme → default colors; without the override the cursor is UNSET.
        assert!(super::resolve_colors(&config).0.cursor.get().is_none());

        config.cursor_color = Some("#ff8800".to_string());
        let (colors, _) = super::resolve_colors(&config);
        assert_eq!(
            colors.cursor.get(),
            Some(qwertty_term_vt::color::Rgb::new(0xff, 0x88, 0x00))
        );
    }

    /// `background`/`foreground` seed the startup colors, and a pair of
    /// `selection-*` overrides produces explicit selection colors.
    #[test]
    fn color_config_seeds_startup_and_selection_colors() {
        use qwertty_term_vt::color::Rgb;
        let mut config = crate::config::Config::default();
        // No theme, no overrides → bg/fg UNSET and selection inverts.
        let (colors, selection) = super::resolve_colors(&config);
        assert!(colors.background.get().is_none());
        assert!(colors.foreground.get().is_none());
        assert!(matches!(selection, super::SelectionColors::Inverse));

        config.background = Some("#101010".to_string());
        config.foreground = Some("#eeeeee".to_string());
        config.selection_background = Some("#334455".to_string());
        config.selection_foreground = Some("#ffffff".to_string());
        let (colors, selection) = super::resolve_colors(&config);
        assert_eq!(colors.background.get(), Some(Rgb::new(0x10, 0x10, 0x10)));
        assert_eq!(colors.foreground.get(), Some(Rgb::new(0xee, 0xee, 0xee)));
        match selection {
            super::SelectionColors::Explicit { bg, fg } => {
                assert_eq!(bg, Rgb::new(0x33, 0x44, 0x55));
                assert_eq!(fg, Rgb::new(0xff, 0xff, 0xff));
            }
            super::SelectionColors::Inverse => panic!("expected explicit selection colors"),
        }
    }

    /// `palette` entries override specific indices of the startup palette.
    #[test]
    fn palette_config_overrides_startup_palette_entries() {
        use qwertty_term_vt::color::Rgb;
        let config = crate::config::Config {
            palette: vec!["0=#1e1e2e".to_string(), "15=#ffffff".to_string()],
            ..Default::default()
        };
        let (colors, _) = super::resolve_colors(&config);
        assert_eq!(colors.palette.current[0], Rgb::new(0x1e, 0x1e, 0x2e));
        assert_eq!(colors.palette.current[15], Rgb::new(0xff, 0xff, 0xff));
    }

    /// Poison resilience (app-hardening): reproduce the field-observed cascade —
    /// a thread panics while holding one surface's engine lock, poisoning it —
    /// and assert (1) recovering that lock does NOT panic (the app survives), (2)
    /// the poison is observed so the owning surface can be marked dead, (3) the
    /// recovered engine's state is still readable for a final render, and (4) a
    /// *second* surface's engine (a separate `Arc<Mutex<Engine>>`) is entirely
    /// unaffected — it locks cleanly and keeps working.
    ///
    /// This exercises the exact mechanism `Surface::engine` uses
    /// (`lock_or_recover` → `PoisonError::into_inner` + a dead flag). The full
    /// `Controller`/`Surface` path additionally needs AppKit (`TerminalView`) and
    /// a live pty, so the controller-survives-and-banners assertion is covered by
    /// the windowed splits smoke; this unit test pins the resilience primitive.
    #[test]
    fn poisoned_engine_lock_degrades_to_dead_surface_not_dead_app() {
        // Two independent surfaces' engines (the real per-surface sharing shape).
        let crashed: Arc<Mutex<Engine>> = Arc::new(Mutex::new(Engine::new(20, 3)));
        let survivor: Arc<Mutex<Engine>> = Arc::new(Mutex::new(Engine::new(20, 3)));

        // Seed distinct content, then poison ONLY the first engine's lock by
        // panicking a spawned thread while it holds the guard (exactly what the
        // io-reader/parse thread did in the field).
        {
            let mut e = crashed.lock().unwrap();
            e.write(b"crashed-pane");
        }
        {
            let mut e = survivor.lock().unwrap();
            e.write(b"live-pane");
        }
        let poisoner = {
            let crashed = Arc::clone(&crashed);
            std::thread::spawn(move || {
                let _guard = crashed.lock().unwrap();
                panic!("simulated parse-thread crash while holding the engine lock");
            })
        };
        // The spawned thread panics; joining it returns Err but must not unwind
        // into us — the main thread stays alive, mirroring the app surviving.
        assert!(
            poisoner.join().is_err(),
            "poisoner thread should have panicked"
        );

        // (1)+(2): recovering the poisoned lock does not panic and reports the
        // poison, so the owning surface would be marked dead.
        let (crashed_guard, was_poisoned) = lock_or_recover(&crashed);
        assert!(
            was_poisoned,
            "the crashed surface's lock must be observed as poisoned (→ mark dead)"
        );
        // (3): its engine state is still readable for the final render / banner.
        assert!(
            crashed_guard.screen_dump().contains("crashed-pane"),
            "the recovered engine must still be readable for a final render"
        );
        drop(crashed_guard);

        // (4): the OTHER surface's engine is untouched — it locks cleanly (not
        // poisoned) and keeps working, proving the crash did not cascade.
        let (mut survivor_guard, survivor_poisoned) = lock_or_recover(&survivor);
        assert!(
            !survivor_poisoned,
            "an unrelated surface's lock must remain healthy after another's crash"
        );
        assert!(survivor_guard.screen_dump().contains("live-pane"));
        // And it can still accept new output — a live, working pane. Use a fresh
        // line so the short 20-col grid doesn't wrap the marker.
        survivor_guard.write(b"\r\nok");
        assert!(survivor_guard.screen_dump().contains("ok"));
    }
}
