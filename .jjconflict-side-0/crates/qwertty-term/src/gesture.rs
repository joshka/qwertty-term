//! Selection gestures: press / drag / release with single-, double-, and
//! triple-click behaviors. Port of upstream `src/terminal/SelectionGesture.zig`
//! (at `2da015cd6`), adapted to this app's engine boundary.
//!
//! The state machine is faithful to upstream: repeated presses within
//! `Press::repeat_interval` and `Press::max_distance` increment the click
//! count up to three; `Press::behaviors` maps click counts to behaviors
//! (default cell / word / line); drags use the behavior chosen by the press;
//! the 60%-of-cell-width threshold rule decides whether the clicked and
//! dragged cells are included in a cell-granular selection
//! (`SelectionGesture.zig` `dragSelection`); dragging within 1px of the top/
//! bottom edge requests viewport autoscroll (`autoscroll_buffer`).
//!
//! **Anchor representation (documented deviation, mechanism only):** upstream
//! tracks the initial click as a pagelist-*tracked* [`Pin`] so the anchor
//! follows content through scrollback pruning and resize reflow mid-drag,
//! validated against the screen's key + generation. This port stores the
//! anchor as an absolute *screen* coordinate (`Tag::Screen` space) plus the
//! active-screen key at press time, re-resolved to a pin per event by the
//! engine accessors ([`crate::engine::Engine::select_screen_points`] &c.).
//! Appending output does not move absolute screen coordinates, so the common
//! select-while-output-scrolls case behaves like upstream; the anchor only
//! drifts if history is *pruned* or the window is *resized* (reflow) while
//! the button is held — a transient mis-anchor that self-corrects on the
//! next press, never unsoundness. A vt-side tracked-anchor accessor can
//! replace this later without changing the state machine.
//!
//! [`Pin`]: qwertty_term_vt::pagelist::Pin

use std::time::{Duration, Instant};

use crate::engine::Engine;

/// An absolute screen coordinate (`Tag::Screen` space: scrollback + active
/// area), the coordinate system the whole gesture works in.
pub type ScreenPoint = (usize, usize);

/// `(start, end)` selection endpoints in anchor → active order.
pub type Bounds = (ScreenPoint, ScreenPoint);

/// The selection behavior for a click and its subsequent drag. Port of
/// `SelectionGesture.Behavior`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Behavior {
    /// Cell-granular drag selection; press returns `None` to clear.
    Cell,
    /// Word selection on press, word-granular drag.
    Word,
    /// Line selection on press, line-granular drag.
    Line,
    /// Semantic command-output selection on press and drag.
    Output,
}

/// The viewport autoscroll direction requested by the active drag. Port of
/// `SelectionGesture.Autoscroll`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Autoscroll {
    None,
    Up,
    Down,
}

/// Standard behaviors for single/double/triple clicks. Port of
/// `default_behaviors`.
pub const DEFAULT_BEHAVIORS: [Behavior; 3] = [Behavior::Cell, Behavior::Word, Behavior::Line];

/// Distance from the top/bottom surface edge, in pixels, where dragging
/// requests autoscroll (upstream keeps a 1px buffer so fullscreen-edge drags
/// still trigger). Port of `autoscroll_buffer`.
const AUTOSCROLL_BUFFER: f64 = 1.0;

/// A press event. Port of `SelectionGesture.Press` (`time` is always
/// available on macOS, so upstream's "no monotonic clock" null-time path is
/// dropped).
pub struct Press<'a> {
    /// When the press occurred (monotonic).
    pub time: Instant,
    /// The screen cell under the click.
    pub point: ScreenPoint,
    /// Click position relative to the pane, device pixels, top-left origin.
    pub xpos: f64,
    pub ypos: f64,
    /// Maximum distance from the previous click to count as a repeat
    /// (upstream passes the cell width — `Surface.zig:3974`).
    pub max_distance: f64,
    /// Maximum interval for a press to count as a repeat (`click-repeat-
    /// interval`, default the OS double-click interval or 500ms).
    pub repeat_interval: Duration,
    /// Whether the alternate screen is active (anchor validation across
    /// screen switches, standing in for upstream's screen key+generation).
    pub alt_screen: bool,
    /// Behaviors for single/double/triple clicks (upstream passes
    /// `[cell, word, ctrl-or-super ? output : line]` — `Surface.zig:3977`).
    pub behaviors: [Behavior; 3],
    /// Word-boundary codepoints for word selection.
    pub boundary_codepoints: &'a [u32],
}

/// Display geometry for threshold + autoscroll math. Port of
/// `SelectionGesture.Drag.Geometry` (same integer types; the threshold math
/// replicates upstream's u32 truncation semantics exactly).
#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    /// Columns in the rendered grid.
    pub columns: u32,
    /// Width of one cell in surface (device) pixels.
    pub cell_width: u32,
    /// Left padding before the grid, in surface pixels (0 — no padding is
    /// wired in this app yet).
    pub padding_left: u32,
    /// Height of the rendered surface in surface pixels.
    pub screen_height: u32,
}

/// A drag event. Port of `SelectionGesture.Drag`.
pub struct Drag<'a> {
    /// The screen cell under the pointer (clamped into the grid by the
    /// caller when the pointer is outside the pane).
    pub point: ScreenPoint,
    /// Pointer position relative to the pane, device pixels (may lie outside
    /// the pane during an out-of-bounds drag).
    pub xpos: f64,
    pub ypos: f64,
    /// True for a rectangular selection (option held, on macOS —
    /// `surface_mouse.zig:121`).
    pub rectangle: bool,
    /// Whether the alternate screen is active (anchor validation).
    pub alt_screen: bool,
    /// Geometry for the threshold + autoscroll calculations.
    pub geometry: Geometry,
    /// Word-boundary codepoints for word-granular drags.
    pub boundary_codepoints: &'a [u32],
}

/// Gesture state for one pane's pointer stream. Port of the
/// `SelectionGesture` struct fields.
#[derive(Debug)]
pub struct SelectionGesture {
    /// The screen cell of the initial left click (upstream: tracked pin).
    anchor: Option<ScreenPoint>,
    /// Whether the anchor was pressed on the alternate screen.
    anchor_alt_screen: bool,
    /// Click count for double/triple clicks (0 = no active gesture).
    click_count: u8,
    /// When the last counted click happened.
    click_time: Option<Instant>,
    /// The behavior chosen by the active press.
    behavior: Behavior,
    /// The press position (surface pixels; distance detection for repeats
    /// and the cell-fraction threshold for drags).
    click_xpos: f64,
    click_ypos: f64,
    /// True once the active gesture moved off the pressed cell.
    dragged: bool,
    /// The autoscroll direction derived from the latest drag position.
    autoscroll: Autoscroll,
    /// The latest drag position + rectangle flag, kept so autoscroll ticks
    /// can continue the drag (upstream re-reads the OS cursor position; the
    /// app records the last `mouseDragged:` instead).
    last_drag: Option<(f64, f64, bool)>,
}

impl Default for SelectionGesture {
    fn default() -> Self {
        Self::new()
    }
}

impl SelectionGesture {
    /// A fresh gesture (port of `init`).
    pub fn new() -> Self {
        SelectionGesture {
            anchor: None,
            anchor_alt_screen: false,
            click_count: 0,
            click_time: None,
            behavior: Behavior::Cell,
            click_xpos: 0.0,
            click_ypos: 0.0,
            dragged: false,
            autoscroll: Autoscroll::None,
            last_drag: None,
        }
    }

    /// Reset all gesture state (cancellation/abandonment: mouse reporting
    /// taking over, surface teardown, …). `release` is the ordinary
    /// button-up path and deliberately keeps the click count/time so the
    /// next press can become a double-click; `reset` clears the sequence.
    /// Port of `reset`.
    pub fn reset(&mut self) {
        self.click_count = 0;
        self.click_time = None;
        self.behavior = Behavior::Cell;
        self.dragged = false;
        self.autoscroll = Autoscroll::None;
        self.anchor = None;
        self.last_drag = None;
    }

    /// The current click count (0 = no active gesture).
    pub fn click_count(&self) -> u8 {
        self.click_count
    }

    /// The behavior chosen by the active press (cell for an idle gesture).
    pub fn behavior(&self) -> Behavior {
        self.behavior
    }

    /// When the last counted click happened.
    pub fn click_time(&self) -> Option<Instant> {
        self.click_time
    }

    /// True once the active gesture moved off the pressed cell.
    pub fn dragged(&self) -> bool {
        self.dragged
    }

    /// The autoscroll direction requested by the latest drag.
    pub fn autoscroll(&self) -> Autoscroll {
        self.autoscroll
    }

    /// The latest drag position `(xpos, ypos, rectangle)`, for autoscroll
    /// ticks.
    pub fn last_drag(&self) -> Option<(f64, f64, bool)> {
        self.last_drag
    }

    /// The anchor is valid for the currently-active screen (port of
    /// `validatedLeftClickPin`'s key check; generation is not modeled — see
    /// the module docs).
    pub fn anchor_valid(&self, alt_screen: bool) -> bool {
        self.anchor.is_some() && self.anchor_alt_screen == alt_screen
    }

    /// Record a press and return the standard selection for the resulting
    /// click count (`None` for a single click — the caller clears any
    /// existing selection). Port of `press`.
    pub fn press(&mut self, engine: &Engine, p: &Press<'_>) -> Option<Bounds> {
        if self.click_count > 0 && self.press_repeat(p) {
            return self.press_selection(engine, p);
        }
        // Initial click, or the repeat failed (too late / too far / other
        // screen).
        self.press_initial(p);
        self.press_selection(engine, p)
    }

    /// Try to continue the click sequence. On failure the sequence is
    /// cleared (the caller then starts a fresh single click). Port of
    /// `pressRepeat` (the `errdefer` reset included).
    fn press_repeat(&mut self, p: &Press<'_>) -> bool {
        let ok = (|| {
            let prev = self.click_time?;
            if p.time.duration_since(prev) > p.repeat_interval {
                return None;
            }
            let distance =
                ((p.xpos - self.click_xpos).powi(2) + (p.ypos - self.click_ypos).powi(2)).sqrt();
            if distance > p.max_distance {
                return None;
            }
            // A prior click on another screen (alt vs primary) can't
            // continue the sequence.
            if self.anchor_alt_screen != p.alt_screen {
                return None;
            }
            Some(())
        })()
        .is_some();

        if !ok {
            self.click_count = 0;
            self.behavior = Behavior::Cell;
            self.anchor = None;
            return false;
        }

        self.click_time = Some(p.time);
        self.dragged = false;
        self.autoscroll = Autoscroll::None;
        self.click_count = (self.click_count + 1).min(3);
        self.behavior = p.behaviors[self.click_count as usize - 1];
        true
    }

    /// Start a fresh single-click gesture anchored at `p`. Port of
    /// `pressInitial`.
    fn press_initial(&mut self, p: &Press<'_>) {
        self.anchor = Some(p.point);
        self.anchor_alt_screen = p.alt_screen;
        self.click_count = 1;
        self.behavior = p.behaviors[0];
        self.click_xpos = p.xpos;
        self.click_ypos = p.ypos;
        self.click_time = Some(p.time);
        self.dragged = false;
        self.autoscroll = Autoscroll::None;
        self.last_drag = None;
    }

    /// The standard selection for the current behavior at the *pressed*
    /// point (upstream selects under the current press's pin, keeping the
    /// original anchor for drags). Port of `pressSelection`.
    fn press_selection(&self, engine: &Engine, p: &Press<'_>) -> Option<Bounds> {
        let (x, y) = p.point;
        match self.behavior {
            Behavior::Cell => None,
            Behavior::Word => engine.select_word_bounds(x, y, p.boundary_codepoints),
            Behavior::Line => engine.select_line_bounds(x, y, true),
            Behavior::Output => engine.select_output_bounds(x, y),
        }
    }

    /// Record a drag and return the selection it produces (`None` = no
    /// selection: the caller clears). Also updates `dragged`, `autoscroll`,
    /// and the stored drag position. The caller gates on `click_count() > 0`
    /// and `anchor_valid()` (upstream `cursorPosCallback` does the same
    /// before calling `drag`). Port of `drag`.
    pub fn drag(&mut self, engine: &Engine, d: &Drag<'_>) -> Option<Bounds> {
        if self.click_count == 0 {
            return None;
        }
        let anchor = self.anchor?;
        if self.anchor_alt_screen != d.alt_screen {
            return None;
        }
        if d.point != anchor {
            self.dragged = true;
        }
        self.last_drag = Some((d.xpos, d.ypos, d.rectangle));

        // Autoscroll: above the top buffer → up; below the bottom → down.
        let max_y = d.geometry.screen_height as f64;
        self.autoscroll = if d.ypos <= AUTOSCROLL_BUFFER {
            Autoscroll::Up
        } else if d.ypos > max_y - AUTOSCROLL_BUFFER {
            Autoscroll::Down
        } else {
            Autoscroll::None
        };

        let selection = match self.behavior {
            Behavior::Cell => drag_selection(
                engine,
                anchor,
                d.point,
                // Zig `@intFromFloat(@max(0, x))`: clamp then truncate.
                self.click_xpos.max(0.0) as u32,
                d.xpos.max(0.0) as u32,
                d.rectangle,
                &d.geometry,
            ),
            Behavior::Word => drag_selection_word(engine, anchor, d.point, d.boundary_codepoints),
            Behavior::Line => drag_selection_line(engine, anchor, d.point),
            Behavior::Output => drag_selection_output(engine, anchor, d.point),
        };

        // A same-cell cell-drag can still cross the within-cell threshold
        // into a real selection; treat that as a drag so click-only actions
        // (links) don't also fire.
        if self.behavior == Behavior::Cell && selection.is_some() {
            self.dragged = true;
        }

        selection
    }

    /// Record a release: stop autoscroll and update `dragged`, but keep the
    /// click count/time so the next nearby press can become a double/triple
    /// click. `point` is the cell under the release, if it mapped to one.
    /// Port of `release`.
    pub fn release(&mut self, point: Option<ScreenPoint>, alt_screen: bool) {
        if self.click_count == 0 {
            return;
        }
        match point {
            Some(p) if self.anchor_valid(alt_screen) => {
                if Some(p) != self.anchor {
                    self.dragged = true;
                }
            }
            // No cell under the release, or the anchor is stale:
            // conservatively treat the click as dragged so click-only
            // actions don't fire against the wrong content.
            _ => self.dragged = true,
        }
        self.autoscroll = Autoscroll::None;
    }
}

/// Whether screen point `a` reads before `b` (row-major). Stands in for
/// upstream `Pin.before` in the drag math.
fn point_before(a: ScreenPoint, b: ScreenPoint) -> bool {
    (a.1, a.0) < (b.1, b.0)
}

/// One cell to the left of `p`, wrapping to the previous row's last column
/// (`None` at the top-left corner — upstream `Pin.leftWrap(1)` returning
/// null at the start of the pagelist).
fn left_wrap(p: ScreenPoint, columns: u32) -> Option<ScreenPoint> {
    if p.0 > 0 {
        Some((p.0 - 1, p.1))
    } else if p.1 > 0 {
        Some((columns as usize - 1, p.1 - 1))
    } else {
        None
    }
}

/// One cell to the right of `p`, wrapping to the next row's first column
/// (`None` past the last written row — upstream `Pin.rightWrap(1)` returning
/// null at the end of the pagelist).
fn right_wrap(engine: &Engine, p: ScreenPoint, columns: u32) -> Option<ScreenPoint> {
    if p.0 + 1 < columns as usize {
        Some((p.0 + 1, p.1))
    } else if engine.screen_cell_exists(0, p.1 + 1) {
        Some((0, p.1 + 1))
    } else {
        None
    }
}

/// The cell-granular drag selection with the 60%-of-cell-width inclusion
/// threshold. Port of `dragSelection` (`SelectionGesture.zig:687-825`),
/// including its exact u32 truncation/`-|`/`%` arithmetic (see the
/// numeric-semantics porting rule) and the compound no-selection check.
fn drag_selection(
    engine: &Engine,
    click_point: ScreenPoint,
    drag_point: ScreenPoint,
    click_x: u32,
    drag_x: u32,
    rectangle: bool,
    geometry: &Geometry,
) -> Option<Bounds> {
    // 60% of the cell width, chosen empirically upstream.
    let threshold_point: u32 = ((geometry.cell_width as f64) * 0.6).round() as u32;

    let max_x = geometry.columns * geometry.cell_width - 1;
    let drag_x_frac = max_x.min(drag_x.saturating_sub(geometry.padding_left)) % geometry.cell_width;
    let click_x_frac =
        max_x.min(click_x.saturating_sub(geometry.padding_left)) % geometry.cell_width;

    let same_pin = drag_point == click_point;

    // Whether the selection's end point is before its start point.
    let end_before_start = if same_pin {
        drag_x_frac < click_x_frac
    } else if rectangle {
        match drag_point.0.cmp(&click_point.0) {
            std::cmp::Ordering::Equal => drag_x_frac < click_x_frac,
            std::cmp::Ordering::Less => true,
            std::cmp::Ordering::Greater => false,
        }
    } else {
        point_before(drag_point, click_point)
    };

    let include_click_cell = if end_before_start {
        click_x_frac >= threshold_point
    } else {
        click_x_frac < threshold_point
    };
    let include_drag_cell = if end_before_start {
        drag_x_frac < threshold_point
    } else {
        drag_x_frac >= threshold_point
    };

    // The excluded click cell is replaced by its neighbor toward the drag
    // (wrapping for normal selections, clamping within the row for
    // rectangles); likewise for the drag cell toward the click.
    let columns = geometry.columns;
    let start_point = if include_click_cell {
        click_point
    } else if end_before_start {
        if rectangle {
            (click_point.0.saturating_sub(1), click_point.1)
        } else {
            left_wrap(click_point, columns).unwrap_or(click_point)
        }
    } else if rectangle {
        ((click_point.0 + 1).min(columns as usize - 1), click_point.1)
    } else {
        right_wrap(engine, click_point, columns).unwrap_or(click_point)
    };

    let end_point = if include_drag_cell {
        drag_point
    } else if end_before_start {
        if rectangle {
            ((drag_point.0 + 1).min(columns as usize - 1), drag_point.1)
        } else {
            right_wrap(engine, drag_point, columns).unwrap_or(drag_point)
        }
    } else if rectangle {
        (drag_point.0.saturating_sub(1), drag_point.1)
    } else {
        left_wrap(drag_point, columns).unwrap_or(drag_point)
    };

    // No selection when exclusion collapses the range onto excluded cells
    // (ported verbatim, including the rectangle column comparisons).
    if (!include_click_cell && same_pin)
        || (!include_click_cell && rectangle && click_point.0 == drag_point.0)
        || (!include_click_cell && end_point == click_point)
        || (!include_click_cell && rectangle && end_point.0 == click_point.0)
        || (!include_drag_cell && start_point == drag_point)
        || (!include_drag_cell && rectangle && start_point.0 == drag_point.0)
    {
        return None;
    }

    Some((start_point, end_point))
}

/// Word-granular drag for a double-click gesture: the selection spans from
/// the word nearest the click anchor to the word nearest the pointer. Port
/// of `dragSelectionWord`.
fn drag_selection_word(
    engine: &Engine,
    click_point: ScreenPoint,
    drag_point: ScreenPoint,
    boundary_codepoints: &[u32],
) -> Option<Bounds> {
    let word_start =
        engine.select_word_between_bounds(click_point, drag_point, boundary_codepoints)?;
    let word_current =
        engine.select_word_between_bounds(drag_point, click_point, boundary_codepoints)?;
    Some(if point_before(drag_point, click_point) {
        (word_current.0, word_start.1)
    } else {
        (word_start.0, word_current.1)
    })
}

/// Line-granular drag for a triple-click gesture. Port of
/// `dragSelectionLine` (including the untrimmed retry for an all-blank
/// clicked line).
fn drag_selection_line(
    engine: &Engine,
    click_point: ScreenPoint,
    drag_point: ScreenPoint,
) -> Option<Bounds> {
    let line = engine.select_line_bounds(drag_point.0, drag_point.1, true)?;
    let mut sel = engine
        .select_line_bounds(click_point.0, click_point.1, true)
        .or_else(|| engine.select_line_bounds(click_point.0, click_point.1, false))?;
    if point_before(drag_point, click_point) {
        sel.0 = line.0;
    } else {
        sel.1 = line.1;
    }
    Some(sel)
}

/// Output-granular drag for a ctrl/cmd-triple-click gesture: expand from the
/// output block under the click to the block under the pointer (keep the
/// original block if the pointer isn't on output). Port of
/// `dragSelectionOutput`.
fn drag_selection_output(
    engine: &Engine,
    click_point: ScreenPoint,
    drag_point: ScreenPoint,
) -> Option<Bounds> {
    let mut sel = engine.select_output_bounds(click_point.0, click_point.1)?;
    let Some(current) = engine.select_output_bounds(drag_point.0, drag_point.1) else {
        return Some(sel);
    };
    if point_before(drag_point, click_point) {
        sel.0 = current.0;
    } else {
        sel.1 = current.1;
    }
    Some(sel)
}

/// The system double-click interval in milliseconds, if the OS reports one.
/// Port of `os/mouse.zig` `clickInterval` (upstream falls back to 500ms when
/// unavailable — `Config.zig:4673`).
#[cfg(target_os = "macos")]
pub fn os_click_interval() -> Option<Duration> {
    let secs = objc2_app_kit::NSEvent::doubleClickInterval();
    if secs.is_finite() && secs > 0.0 {
        Some(Duration::from_secs_f64(secs))
    } else {
        None
    }
}

/// The click-repeat interval to use: the OS double-click interval, or
/// upstream's 500ms default.
#[cfg(target_os = "macos")]
pub fn click_interval() -> Duration {
    os_click_interval().unwrap_or(Duration::from_millis(500))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 10-px-wide-cell geometry over a 20×10 grid, 200px tall.
    fn geo() -> Geometry {
        Geometry {
            columns: 20,
            cell_width: 10,
            padding_left: 0,
            screen_height: 200,
        }
    }

    const BOUNDARY: &[u32] = &qwertty_term_vt::screen::DEFAULT_WORD_BOUNDARIES;

    /// A press at screen cell `point`, with the pointer at pixel
    /// `(xpos, ypos)`, `t` milliseconds after `base`.
    fn press_at(
        base: Instant,
        ms: u64,
        point: ScreenPoint,
        xpos: f64,
        ypos: f64,
    ) -> Press<'static> {
        Press {
            time: base + Duration::from_millis(ms),
            point,
            xpos,
            ypos,
            max_distance: 10.0,
            repeat_interval: Duration::from_millis(500),
            alt_screen: false,
            behaviors: DEFAULT_BEHAVIORS,
            boundary_codepoints: BOUNDARY,
        }
    }

    fn drag_to(point: ScreenPoint, xpos: f64, ypos: f64) -> Drag<'static> {
        Drag {
            point,
            xpos,
            ypos,
            rectangle: false,
            alt_screen: false,
            geometry: geo(),
            boundary_codepoints: BOUNDARY,
        }
    }

    /// An engine with `hello beta-gamma` on row 0 and `third line here` on
    /// row 2.
    fn engine() -> Engine {
        let mut e = Engine::new(20, 10);
        e.write(b"hello beta-gamma\r\n\r\nthird line here");
        e
    }

    // ---- click counting --------------------------------------------------

    #[test]
    fn single_press_is_cell_behavior_and_returns_none() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        let sel = g.press(&e, &press_at(base, 0, (2, 0), 25.0, 5.0));
        assert_eq!(sel, None);
        assert_eq!(g.click_count(), 1);
    }

    #[test]
    fn double_press_selects_word_triple_selects_line() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        // Click on "beta-gamma" (cols 6..15 of row 0).
        let p = |ms| press_at(base, ms, (8, 0), 85.0, 5.0);
        assert_eq!(g.press(&e, &p(0)), None);
        // Second press 100ms later: word selection.
        let sel = g.press(&e, &p(100));
        assert_eq!(g.click_count(), 2);
        assert_eq!(sel, Some(((6, 0), (15, 0))));
        // Third press: line selection ("hello beta-gamma").
        let sel = g.press(&e, &p(200));
        assert_eq!(g.click_count(), 3);
        assert_eq!(sel, Some(((0, 0), (15, 0))));
        // Fourth press stays at 3 (line again).
        let sel = g.press(&e, &p(300));
        assert_eq!(g.click_count(), 3);
        assert_eq!(sel, Some(((0, 0), (15, 0))));
    }

    #[test]
    fn slow_second_press_resets_to_single() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        g.press(&e, &press_at(base, 0, (8, 0), 85.0, 5.0));
        // 600ms later: outside the 500ms interval.
        let sel = g.press(&e, &press_at(base, 600, (8, 0), 85.0, 5.0));
        assert_eq!(g.click_count(), 1);
        assert_eq!(sel, None);
    }

    #[test]
    fn distant_second_press_resets_to_single() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        g.press(&e, &press_at(base, 0, (8, 0), 85.0, 5.0));
        // 100ms later but 30px away (max_distance is 10).
        let sel = g.press(&e, &press_at(base, 100, (11, 0), 115.0, 5.0));
        assert_eq!(g.click_count(), 1);
        assert_eq!(sel, None);
    }

    #[test]
    fn screen_switch_resets_sequence() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        g.press(&e, &press_at(base, 0, (8, 0), 85.0, 5.0));
        let mut p = press_at(base, 100, (8, 0), 85.0, 5.0);
        p.alt_screen = true;
        g.press(&e, &p);
        assert_eq!(g.click_count(), 1);
    }

    // ---- cell drag (threshold) --------------------------------------------

    #[test]
    fn cell_drag_left_to_right_includes_cells_past_threshold() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        // Press in the left part of cell (2,0) (x=22 → frac 2 < threshold 6):
        // the clicked cell is included.
        g.press(&e, &press_at(base, 0, (2, 0), 22.0, 5.0));
        // Drag to the right part of cell (5,0) (x=58 → frac 8 ≥ 6): included.
        let sel = g.drag(&e, &drag_to((5, 0), 58.0, 5.0));
        assert_eq!(sel, Some(((2, 0), (5, 0))));
        assert!(g.dragged());
    }

    #[test]
    fn cell_drag_within_threshold_of_same_cell_is_no_selection() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        // Press and micro-drag inside the same cell, both left of the
        // threshold: no selection.
        g.press(&e, &press_at(base, 0, (2, 0), 21.0, 5.0));
        let sel = g.drag(&e, &drag_to((2, 0), 23.0, 5.0));
        assert_eq!(sel, None);
    }

    #[test]
    fn cell_drag_excluded_click_cell_starts_at_neighbor() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        // Press in the *right* part of cell (2,0) (x=28 → frac 8 ≥ 6): the
        // clicked cell is excluded going left-to-right; start is (3,0).
        g.press(&e, &press_at(base, 0, (2, 0), 28.0, 5.0));
        let sel = g.drag(&e, &drag_to((5, 0), 58.0, 5.0));
        assert_eq!(sel, Some(((3, 0), (5, 0))));
    }

    #[test]
    fn cell_drag_right_to_left_selects_backwards() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        // Press in the right part of cell (5,0) (frac 8 ≥ 6 → included for a
        // backwards selection), drag left to the left part of (2,0) (frac
        // 2 < 6 → included).
        g.press(&e, &press_at(base, 0, (5, 0), 58.0, 5.0));
        let sel = g.drag(&e, &drag_to((2, 0), 22.0, 5.0));
        assert_eq!(sel, Some(((5, 0), (2, 0))));
    }

    // ---- word / line drags -------------------------------------------------

    #[test]
    fn word_drag_spans_words_in_both_directions() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        // Double-click "hello" (cols 0..4).
        let p = |ms| press_at(base, ms, (2, 0), 25.0, 5.0);
        g.press(&e, &p(0));
        let sel = g.press(&e, &p(100));
        assert_eq!(sel, Some(((0, 0), (4, 0))));
        // Drag forward into "beta-gamma": start of "hello" → end of it.
        let sel = g.drag(&e, &drag_to((8, 0), 85.0, 5.0));
        assert_eq!(sel, Some(((0, 0), (15, 0))));
        // Drag down to "third line here" row: extends to its words.
        let sel = g.drag(&e, &drag_to((4, 2), 45.0, 25.0));
        assert_eq!(sel, Some(((0, 0), (4, 2))));
    }

    #[test]
    fn line_drag_extends_by_lines() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        let p = |ms| press_at(base, ms, (2, 0), 25.0, 5.0);
        g.press(&e, &p(0));
        g.press(&e, &p(100));
        let sel = g.press(&e, &p(200));
        assert_eq!(g.click_count(), 3);
        assert_eq!(sel, Some(((0, 0), (15, 0))));
        // Drag to row 2: the selection extends to that line's end.
        let sel = g.drag(&e, &drag_to((0, 2), 5.0, 25.0));
        assert_eq!(sel, Some(((0, 0), (14, 2))));
    }

    // ---- autoscroll / release / reset ---------------------------------------

    #[test]
    fn drag_at_edges_requests_autoscroll() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        g.press(&e, &press_at(base, 0, (2, 0), 25.0, 5.0));
        g.drag(&e, &drag_to((2, 0), 25.0, 0.5));
        assert_eq!(g.autoscroll(), Autoscroll::Up);
        g.drag(&e, &drag_to((2, 9), 25.0, 199.5));
        assert_eq!(g.autoscroll(), Autoscroll::Down);
        g.drag(&e, &drag_to((2, 5), 25.0, 100.0));
        assert_eq!(g.autoscroll(), Autoscroll::None);
    }

    #[test]
    fn release_keeps_click_state_but_stops_autoscroll() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        g.press(&e, &press_at(base, 0, (2, 0), 25.0, 5.0));
        g.drag(&e, &drag_to((5, 0), 58.0, 0.5));
        assert_eq!(g.autoscroll(), Autoscroll::Up);
        g.release(Some((5, 0)), false);
        assert_eq!(g.autoscroll(), Autoscroll::None);
        assert_eq!(g.click_count(), 1);
        assert!(g.dragged());
    }

    #[test]
    fn release_without_cell_marks_dragged() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        g.press(&e, &press_at(base, 0, (2, 0), 25.0, 5.0));
        g.release(None, false);
        assert!(g.dragged());
    }

    #[test]
    fn reset_clears_sequence() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        g.press(&e, &press_at(base, 0, (2, 0), 25.0, 5.0));
        g.reset();
        assert_eq!(g.click_count(), 0);
        // The next press is a fresh single click even if quick and nearby.
        let sel = g.press(&e, &press_at(base, 50, (2, 0), 25.0, 5.0));
        assert_eq!(sel, None);
        assert_eq!(g.click_count(), 1);
    }

    #[test]
    fn drag_on_other_screen_is_noop() {
        let e = engine();
        let mut g = SelectionGesture::new();
        let base = Instant::now();
        g.press(&e, &press_at(base, 0, (2, 0), 25.0, 5.0));
        let mut d = drag_to((5, 0), 58.0, 5.0);
        d.alt_screen = true;
        assert_eq!(g.drag(&e, &d), None);
        assert!(!g.anchor_valid(true));
        assert!(g.anchor_valid(false));
    }
}
