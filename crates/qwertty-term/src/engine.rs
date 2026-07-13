//! Terminal engine wrapper over `qwertty-term-vt`.
//!
//! A thin adapter around `qwertty_term_vt`'s [`Stream`] + [`Terminal`], exposing only
//! what the AppKit host needs: feed PTY bytes in, drain reply bytes out, take a
//! windowed render snapshot, read the input-affecting modes / kitty flags for
//! the key encoder, read the OSC 7 working directory (for new-tab inheritance),
//! and resize.
//!
//! This mirrors the reference `crates/spike/src/engine.rs` (read-only spike
//! material) — same call sites into `qwertty-term-vt` — but is an independent copy so
//! the app doesn't path-depend on `qwertty-term-spike` (which pulls in eframe). The
//! subset here is what R5 actually exercises.

use qwertty_term_input::key_encode::{KittyFlags, Options as EncodeOptions};
use qwertty_term_input::mouse_encode::{MouseEvent, MouseFormat};
use qwertty_term_vt::modes::Mode;
use qwertty_term_vt::pagelist::Pin;
use qwertty_term_vt::point::Point;
use qwertty_term_vt::screen::selection::Selection;
use qwertty_term_vt::search::PageListSearch;
use qwertty_term_vt::snapshot::SnapshotWindow;
use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Colors, Options, Terminal};

/// The terminal engine, backed by `qwertty-term-vt`.
pub struct Engine {
    stream: Stream<TerminalHandler>,
}

// SAFETY (M2 chunk E, `docs/analysis/termio-hub.md` §3.3): `qwertty-term-vt`'s
// `Terminal`/`Screen`/`PageList` are not auto-`Send` because the pagelist
// threads raw pointers (`*mut Row`, `*mut Cell`, …) through its bump-allocated
// pages. Those pointers reference ONLY memory transitively owned by the
// `Terminal` itself — there is no thread-local, process-global, or externally
// shared state behind them, and they move atomically with the `Terminal` when
// it moves. The app shares the engine as `Arc<Mutex<Engine>>`: the mutex
// serializes every access (the termio parse thread applies output; the main
// pace tick renders + drains replies), so the engine is only ever touched by
// one thread at a time and never observed mid-mutation across threads. Under
// that exclusive-access discipline moving the engine between threads is sound.
// This assertion is scoped to the app's wrapper, not upstream's `Terminal`.
unsafe impl Send for Engine {}

impl Engine {
    /// Create a new engine with the given grid size.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self::with_colors(cols, rows, Colors::default())
    }

    /// Create a new engine with the given grid size and startup dynamic color
    /// state (256-color palette + default fg/bg/cursor). Used to seed a
    /// theme's colors before the first frame; the running program can still
    /// override any of these at runtime via OSC 4/10/11/12, same as with the
    /// default palette. Mirrors the reference `crates/spike/src/engine.rs`'s
    /// `with_colors`.
    pub fn with_colors(cols: usize, rows: usize, colors: Colors) -> Self {
        let terminal = Terminal::new(Options {
            cols: clamp_dim(cols),
            rows: clamp_dim(rows),
            colors,
            ..Default::default()
        });
        Self {
            stream: Stream::new(TerminalHandler::new(terminal)),
        }
    }

    fn terminal(&self) -> &Terminal {
        &self.stream.handler.terminal
    }

    fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.stream.handler.terminal
    }

    /// Feed PTY output bytes into the parser/terminal.
    pub fn write(&mut self, bytes: &[u8]) {
        self.stream.feed(bytes);
    }

    /// Drain any reply bytes (DSR/DA/CPR/DECRQSS/…) queued in response to fed
    /// bytes, destined for the PTY.
    pub fn take_output(&mut self) -> Vec<u8> {
        self.stream.handler.take_output()
    }

    /// Drain the most recent OSC 52 clipboard write request, if any:
    /// `(kind, raw_base64_data)`. Handed up raw (still base64-encoded, per
    /// upstream's apprt-decodes-it policy).
    pub fn take_clipboard(&mut self) -> Option<(u8, String)> {
        self.stream.handler.take_clipboard()
    }

    /// Drain the pending-bell flag: `true` if a BEL was received since the
    /// last drain. The app polls this each pace tick to fire its configured
    /// `bell-features` (see `crate::bell`).
    pub fn take_bell(&mut self) -> bool {
        self.stream.handler.take_bell()
    }

    /// Drain the most recent pending desktop notification `(title, body)` if
    /// one arrived (OSC 9 / OSC 777) since the last drain. The app polls this
    /// each pace tick, gates it on `desktop-notifications`, rate-limits, and
    /// delivers (see `crate::notify`).
    pub fn take_notification(&mut self) -> Option<(String, String)> {
        self.stream.handler.take_notification()
    }

    /// Drain the OSC 133 command boundaries (`C`/`D`) observed since the last
    /// drain, in order. The app pairs each `OutputStart` with the following
    /// `End` to time a command for `notify-on-command-finish`.
    pub fn take_command_boundaries(&mut self) -> Vec<qwertty_term_vt::stream::CommandBoundary> {
        self.stream.handler.take_command_boundaries()
    }

    /// Resize the grid.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.terminal_mut().resize(clamp_dim(cols), clamp_dim(rows));
    }

    /// Replace the terminal's colors (256-palette + default fg/bg/cursor) live —
    /// used by `config-reload` to apply a new `theme` without recreating the
    /// engine — and mark the whole active screen dirty so the next render
    /// repaints every cell with the new palette (a palette swap changes no cell
    /// contents, so it wouldn't otherwise re-dirty anything). Any OSC-set dynamic
    /// colors are replaced by the config's, matching a config re-derive.
    pub fn set_colors(&mut self, colors: Colors) {
        let terminal = self.terminal_mut();
        terminal.colors = colors;
        terminal.screen_mut().pages.mark_all_dirty();
    }

    /// Whether synchronized output (mode 2026) is currently active. When set,
    /// a program has asked the terminal to buffer rendering until it clears the
    /// mode; the termio hub's 1s reset timer force-clears a stuck one.
    pub fn synchronized_output(&self) -> bool {
        self.mode(Mode::SynchronizedOutput)
    }

    /// Force-clear synchronized output (mode 2026). Called from the termio
    /// hub's 1s sync-reset timer so a wedged program can't freeze rendering
    /// (see `docs/analysis/termio-hub.md` §4).
    pub fn reset_synchronized_output(&mut self) {
        self.terminal_mut()
            .modes
            .set(Mode::SynchronizedOutput, false);
    }

    pub fn cols(&self) -> usize {
        self.terminal().cols as usize
    }

    pub fn rows(&self) -> usize {
        self.terminal().rows as usize
    }

    /// The current window title (OSC 0/2), if set and valid UTF-8.
    pub fn title(&self) -> Option<String> {
        let title = self.terminal().get_title()?;
        std::str::from_utf8(title).ok().map(str::to_owned)
    }

    /// A cheap, windowed snapshot containing only the rows needed to render the
    /// viewport `scrollback_offset` rows up from the bottom (0 = live active
    /// area). Read-only: reports every row dirty and leaves the terminal's
    /// dirty state untouched. Use [`Engine::snapshot_window_tracking`] on the
    /// per-frame render path so incremental redraw can skip clean rows.
    pub fn snapshot_window(&self, scrollback_offset: usize) -> SnapshotWindow {
        self.terminal().snapshot_window(scrollback_offset)
    }

    /// The per-frame render capture: like [`Engine::snapshot_window`] but reads
    /// and *clears* the terminal's per-row / global dirty state so the renderer
    /// rebuilds only the rows (or the whole frame, on a global change) that
    /// actually changed since the last frame. This is the incremental-redraw
    /// path; call it once per frame drawn.
    pub fn snapshot_window_tracking(&mut self, scrollback_offset: usize) -> SnapshotWindow {
        self.terminal_mut()
            .snapshot_window_tracking(scrollback_offset)
    }

    /// A plain-text dump of the visible screen (used by smoke modes).
    pub fn screen_dump(&self) -> String {
        self.terminal().plain_string()
    }

    // -- selection -------------------------------------------------------

    /// Resolve a `(col, row)` cell coordinate in the currently-rendered
    /// viewport (row 0 = top of the visible window this frame rendered) to a
    /// [`Pin`], or `None` if it's out of the grid. The app currently always
    /// renders `snapshot_window(0)` (no scrollback UI wired yet — see
    /// `docs/analysis/renderer-r5.md`'s deferrals), so "viewport" and
    /// "currently visible" coincide; this is the seam a future scrollback
    /// offset would thread through.
    pub fn pin_at(&self, col: usize, row: usize) -> Option<Pin> {
        if col >= self.cols() || row >= self.rows() {
            return None;
        }
        let point = Point::viewport(col as qwertty_term_vt::page::size::CellCountInt, row as u32);
        self.terminal().screen().pages.pin(point)
    }

    /// Set (replace) the engine's selection. `None` clears it. Builds an
    /// untracked selection from `start`/`end` pins and lets `Screen::select`
    /// track it (matches upstream's `select` contract).
    pub fn select(&mut self, start: Pin, end: Pin, rectangle: bool) {
        let sel = Selection::init(start, end, rectangle);
        self.terminal_mut().screen_mut().select(Some(sel));
    }

    /// Clear the current selection, if any.
    pub fn clear_selection(&mut self) {
        self.terminal_mut().screen_mut().clear_selection();
    }

    /// The current selection's pins (start, end, rectangle), if any is set.
    /// Used by the render path to compute which cells to tint.
    pub fn selection(&self) -> Option<(Pin, Pin, bool)> {
        let sel = self.terminal().screen().selection.as_ref()?;
        Some((sel.start(), sel.end(), sel.rectangle))
    }

    /// Resolve a pair of pins (as returned by [`Engine::selection`]) to an
    /// ordered [`ScreenRange`] in absolute screen coordinates, the pin-free
    /// geometry the per-frame tint pass consumes. `None` only if a pin
    /// somehow doesn't resolve to a screen point (shouldn't happen for a live
    /// selection's own pins).
    pub fn screen_range(
        &self,
        start: Pin,
        end: Pin,
        rectangle: bool,
    ) -> Option<crate::selection::ScreenRange> {
        let sel = Selection::init(start, end, rectangle);
        let pages = &self.terminal().screen().pages;
        let tl = pages
            .point_from_pin(qwertty_term_vt::point::Tag::Screen, sel.top_left(pages))?
            .coord();
        let br = pages
            .point_from_pin(qwertty_term_vt::point::Tag::Screen, sel.bottom_right(pages))?
            .coord();
        Some(crate::selection::ScreenRange {
            top_left: (tl.x as usize, tl.y as usize),
            bottom_right: (br.x as usize, br.y as usize),
            rectangle,
        })
    }

    /// The current selection's text (trimmed trailing whitespace per row), or
    /// `None` if there is no selection. This may reach above the currently
    /// rendered window into scrollback; `Screen::selection_string` walks the
    /// pagelist directly rather than needing a full `Snapshot`. Always trims
    /// (the default copy path); use [`Engine::selection_string_opt`] to control
    /// trimming per `clipboard-trim-trailing-spaces`.
    pub fn selection_string(&self) -> Option<String> {
        self.selection_string_opt(true)
    }

    /// Like [`Engine::selection_string`] but with explicit trailing-whitespace
    /// trimming (`trim`), so the copy path can honor
    /// `clipboard-trim-trailing-spaces`.
    pub fn selection_string_opt(&self, trim: bool) -> Option<String> {
        let sel = self.terminal().screen().selection.as_ref()?;
        Some(self.terminal().screen().selection_string(sel, trim))
    }

    // -- selection gestures (absolute-screen space) ------------------------
    //
    // The selection-gesture state machine (`crate::gesture`, port of upstream
    // `SelectionGesture.zig`) works in *absolute screen* coordinates — the
    // `Tag::Screen` space covering scrollback + active area, the same space
    // [`Engine::screen_range`] and the tint pass use. These accessors are the
    // engine boundary: they resolve screen points to pins, call the ported
    // `Screen` selection primitives, and hand back pin-free geometry.

    /// Map a cell coordinate in the *rendered window* (`row` 0 = the top row
    /// the frame at `scrollback_offset` shows) to an absolute screen
    /// coordinate. Uses the same windowing math as `snapshot_window`: the
    /// window is exactly `rows` rows ending `offset` rows up from the bottom
    /// (offset clamped to history), top-padded with blank rows when less than
    /// a full grid has been written. `None` for cells outside the grid or on
    /// a blank pad row (nothing is written there to select).
    ///
    /// This is the mapping [`Engine::pin_at`] lacks: `pin_at` resolves
    /// against the pagelist's own viewport (pinned to the active area — the
    /// app never scrolls it), so it is only correct at offset 0.
    pub fn window_to_screen_point(
        &self,
        col: usize,
        row: usize,
        scrollback_offset: usize,
    ) -> Option<(usize, usize)> {
        let (cols, rows) = (self.cols(), self.rows());
        if col >= cols || row >= rows {
            return None;
        }
        let total = self.terminal().screen().pages.total_rows();
        let scrollback_len = total.saturating_sub(rows);
        let offset = scrollback_offset.min(scrollback_len);
        // `offset <= scrollback_len <= total`, so this cannot underflow.
        let window_len = rows.min(total - offset);
        let pad = rows - window_len;
        if row < pad {
            return None;
        }
        let window_top = total.saturating_sub(offset + rows);
        Some((col, window_top + (row - pad)))
    }

    /// Resolve an absolute screen coordinate to a [`Pin`], or `None` if it
    /// lies beyond the written screen space.
    pub fn pin_at_screen(&self, x: usize, y: usize) -> Option<Pin> {
        let point = Point::screen(x as qwertty_term_vt::page::size::CellCountInt, y as u32);
        self.terminal().screen().pages.pin(point)
    }

    /// Whether an absolute screen coordinate resolves to a written cell
    /// location (used by the gesture's threshold math for pin-wrap bounds).
    pub fn screen_cell_exists(&self, x: usize, y: usize) -> bool {
        self.pin_at_screen(x, y).is_some()
    }

    /// Set the selection from a pair of absolute screen points in anchor →
    /// active order. Returns `false` (selection untouched) if either point
    /// doesn't resolve to a written cell.
    pub fn select_screen_points(
        &mut self,
        a: (usize, usize),
        b: (usize, usize),
        rectangle: bool,
    ) -> bool {
        let (Some(start), Some(end)) = (self.pin_at_screen(a.0, a.1), self.pin_at_screen(b.0, b.1))
        else {
            return false;
        };
        self.select(start, end, rectangle);
        true
    }

    /// A selection's `(start, end)` endpoints as absolute screen points, in
    /// the selection's own start→end order (callers that need top-left /
    /// bottom-right ordering use [`Engine::screen_range`] instead).
    fn selection_endpoints(&self, sel: &Selection) -> Option<((usize, usize), (usize, usize))> {
        let pages = &self.terminal().screen().pages;
        let s = pages
            .point_from_pin(qwertty_term_vt::point::Tag::Screen, sel.start())?
            .coord();
        let e = pages
            .point_from_pin(qwertty_term_vt::point::Tag::Screen, sel.end())?
            .coord();
        Some(((s.x as usize, s.y as usize), (e.x as usize, e.y as usize)))
    }

    /// The word under the screen point (upstream `selectWord` semantics: a
    /// run of exclusively-boundary or exclusively-non-boundary cells, across
    /// soft-wraps), as `(start, end)` screen points. `None` on an unwritten
    /// cell.
    pub fn select_word_bounds(
        &self,
        x: usize,
        y: usize,
        boundary_codepoints: &[u32],
    ) -> Option<((usize, usize), (usize, usize))> {
        let pin = self.pin_at_screen(x, y)?;
        let sel = self
            .terminal()
            .screen()
            .select_word(pin, boundary_codepoints)?;
        self.selection_endpoints(&sel)
    }

    /// The soft-wrapped line under the screen point (upstream `selectLine`
    /// semantics, semantic-prompt boundaries respected), as `(start, end)`
    /// screen points. `trim_whitespace` selects the default whitespace-trim
    /// behavior; `false` keeps blank ends (the triple-click-drag fallback for
    /// an all-blank line, upstream `dragSelectionLine`'s `.whitespace = null`
    /// retry).
    pub fn select_line_bounds(
        &self,
        x: usize,
        y: usize,
        trim_whitespace: bool,
    ) -> Option<((usize, usize), (usize, usize))> {
        use qwertty_term_vt::screen::SelectLine;
        let pin = self.pin_at_screen(x, y)?;
        let opts = if trim_whitespace {
            SelectLine::new(pin)
        } else {
            SelectLine {
                pin,
                whitespace: None,
                semantic_prompt_boundary: true,
            }
        };
        let sel = self.terminal().screen().select_line(opts)?;
        self.selection_endpoints(&sel)
    }

    /// The shell-integration command output under the screen point (upstream
    /// `selectOutput`), as `(start, end)` screen points. `None` if the point
    /// is not on output.
    pub fn select_output_bounds(
        &self,
        x: usize,
        y: usize,
    ) -> Option<((usize, usize), (usize, usize))> {
        let pin = self.pin_at_screen(x, y)?;
        let sel = self.terminal().screen().select_output(pin)?;
        self.selection_endpoints(&sel)
    }

    /// The nearest word to `from` walking toward `to` (inclusive), as
    /// `(start, end)` screen points — upstream `selectWordBetween`, the
    /// word-granular drag primitive.
    pub fn select_word_between_bounds(
        &self,
        from: (usize, usize),
        to: (usize, usize),
        boundary_codepoints: &[u32],
    ) -> Option<((usize, usize), (usize, usize))> {
        let start = self.pin_at_screen(from.0, from.1)?;
        let end = self.pin_at_screen(to.0, to.1)?;
        let sel = self
            .terminal()
            .screen()
            .select_word_between(start, end, boundary_codepoints)?;
        self.selection_endpoints(&sel)
    }

    // -- search ----------------------------------------------------------

    /// Run a literal case-insensitive-ASCII substring search over the *entire*
    /// scrollback (history + active area) for `needle`, returning every match as
    /// an ordered-top-to-bottom [`ScreenRange`] in absolute screen coordinates
    /// (the same space [`Engine::screen_range`] and the tint pass consume).
    ///
    /// This drives `qwertty-term-vt`'s [`PageListSearch`] under the exact lock
    /// discipline the rest of `Engine` uses (the caller holds the engine mutex):
    /// [`PageListSearch::from_end`] starts at the bottom node and searches in
    /// reverse toward the top of history, so `next()` yields matches most-recent
    /// first; we collect them and reverse so the returned vector is in reading
    /// order (top of scrollback → bottom of the active area). Empty needle → no
    /// matches.
    ///
    /// Upstream runs this incrementally on a dedicated thread; for slice 1 the
    /// app calls it synchronously on the main thread (measured — see the
    /// `search_timing` bench test). The `feed`/`next` structure here is the same
    /// one a future thread would drive.
    pub fn search_all(&mut self, needle: &[u8]) -> Vec<crate::selection::ScreenRange> {
        if needle.is_empty() {
            return Vec::new();
        }

        let mut ranges: Vec<crate::selection::ScreenRange> = Vec::new();
        {
            let pages = &mut self.terminal_mut().screen_mut().pages;
            let mut search = PageListSearch::from_end(needle, pages);
            loop {
                // Drain every match currently loaded in the window.
                while let Some(flat) = search.next() {
                    let untracked = flat.untracked();
                    if let Some(range) =
                        Self::flattened_to_range(pages, untracked.start, untracked.end)
                    {
                        ranges.push(range);
                    }
                }
                // Load more history; stop when the whole list has been searched.
                if !search.feed() {
                    break;
                }
            }
            search.deinit(pages);
        }

        // `PageListSearch` yields most-recent-first; reading order is top→bottom.
        ranges.reverse();
        ranges
    }

    /// Resolve a match's start/end [`Pin`] pair to an absolute-screen
    /// [`ScreenRange`]. A search match is never a rectangle. Shared shape with
    /// [`Engine::screen_range`], but the pins come straight from the searcher
    /// (already ordered start≤end), so no `Selection` reordering is needed.
    fn flattened_to_range(
        pages: &qwertty_term_vt::pagelist::PageList,
        start: Pin,
        end: Pin,
    ) -> Option<crate::selection::ScreenRange> {
        let tl = pages
            .point_from_pin(qwertty_term_vt::point::Tag::Screen, start)?
            .coord();
        let br = pages
            .point_from_pin(qwertty_term_vt::point::Tag::Screen, end)?
            .coord();
        Some(crate::selection::ScreenRange {
            top_left: (tl.x as usize, tl.y as usize),
            bottom_right: (br.x as usize, br.y as usize),
            rectangle: false,
        })
    }

    /// The OSC 7 working directory as a filesystem path, if the running shell
    /// has reported one. The stored value is a `file://host/path` URL (or a bare
    /// path); [`pwd_path_from_osc7`] extracts the local path. Used to spawn a new
    /// tab's shell in the current tab's directory.
    pub fn pwd(&self) -> Option<String> {
        let raw = self.terminal().get_pwd()?;
        let s = std::str::from_utf8(raw).ok()?;
        pwd_path_from_osc7(s)
    }

    // -- input-affecting modes ------------------------------------------------

    pub fn bracketed_paste(&self) -> bool {
        self.mode(Mode::BracketedPaste)
    }

    pub fn focus_reporting(&self) -> bool {
        self.mode(Mode::FocusEvent)
    }

    /// The kitty keyboard protocol flags currently active on the active screen.
    pub fn kitty_flags(&self) -> KittyFlags {
        let flags = self.terminal().screen().kitty_keyboard.current();
        KittyFlags::from_bits(flags.int())
    }

    /// Key-encoding options derived from current terminal mode state, for
    /// `qwertty_term_input::key_encode::encode`. `macos_option_as_alt` is left at its
    /// default here; the input path overlays the user's config value.
    pub fn key_encode_options(&self) -> EncodeOptions {
        EncodeOptions {
            cursor_key_application: self.mode(Mode::CursorKeys),
            keypad_key_application: self.mode(Mode::KeypadKeys),
            backarrow_key_mode: self.mode(Mode::BackarrowKeyMode),
            ignore_keypad_with_numlock: self.mode(Mode::IgnoreKeypadWithNumlock),
            alt_esc_prefix: self.mode(Mode::AltEscPrefix),
            modify_other_keys_state_2: self.terminal().flags.modify_other_keys_2,
            kitty_flags: self.kitty_flags(),
            ..Default::default()
        }
    }

    /// The terminal's requested mouse reporting mode (`None` if off).
    pub fn mouse_event(&self) -> MouseEvent {
        if self.mode(Mode::MouseEventAny) {
            MouseEvent::Any
        } else if self.mode(Mode::MouseEventButton) {
            MouseEvent::Button
        } else if self.mode(Mode::MouseEventNormal) {
            MouseEvent::Normal
        } else if self.mode(Mode::MouseEventX10) {
            MouseEvent::X10
        } else {
            MouseEvent::None
        }
    }

    /// The terminal's requested mouse report format. Precedence matches upstream:
    /// SGR-pixels, SGR, urxvt, UTF-8, else X10.
    pub fn mouse_format(&self) -> MouseFormat {
        if self.mode(Mode::MouseFormatSgrPixels) {
            MouseFormat::SgrPixels
        } else if self.mode(Mode::MouseFormatSgr) {
            MouseFormat::Sgr
        } else if self.mode(Mode::MouseFormatUrxvt) {
            MouseFormat::Urxvt
        } else if self.mode(Mode::MouseFormatUtf8) {
            MouseFormat::Utf8
        } else {
            MouseFormat::X10
        }
    }

    /// Whether the alternate screen is currently active (a full-screen program
    /// like vim/htop is running). Drives the wheel-scroll alternate-scroll
    /// path. Mirrors upstream `terminal.screens.active_key == .alternate`.
    pub fn alt_screen_active(&self) -> bool {
        self.terminal().screens.active_key() == qwertty_term_vt::terminal::ScreenKey::Alternate
    }

    /// Whether mode 1007 (`mouse_alternate_scroll`) is set. Combined with
    /// [`Engine::alt_screen_active`] and mouse reporting being off, this turns
    /// wheel events into cursor-key presses. Default is `true` (upstream's
    /// mode-table default).
    pub fn mouse_alternate_scroll(&self) -> bool {
        self.mode(Mode::MouseAlternateScroll)
    }

    /// The total number of scrollback rows above the active area (history the
    /// viewport can be scrolled up into). Used to clamp a wheel-driven
    /// scrollback offset at the top of history.
    pub fn scrollback_len(&self) -> usize {
        let total = self.terminal().screen().pages.total_rows();
        total.saturating_sub(self.rows())
    }

    fn mode(&self, mode: Mode) -> bool {
        self.terminal().modes.get(mode)
    }
}

/// Extract the local filesystem path from an OSC 7 value. OSC 7 carries a
/// `file://<host>/<path>` URL; we take the path component (everything from the
/// first `/` after the authority). A bare path (no scheme) is returned as-is.
/// Returns `None` for an empty result. Percent-decoding is handled minimally
/// (only `%20` → space, the common case) — full RFC 3986 decoding is deferred.
pub fn pwd_path_from_osc7(value: &str) -> Option<String> {
    let path = if let Some(rest) = value.strip_prefix("file://") {
        // rest = "<host>/<path>"; the path starts at the first '/'.
        match rest.find('/') {
            Some(i) => &rest[i..],
            None => rest,
        }
    } else {
        value
    };
    let decoded = path.replace("%20", " ");
    if decoded.is_empty() {
        None
    } else {
        Some(decoded)
    }
}

/// Clamp a requested dimension into the engine's supported `u16` range (at least
/// one cell; the engine panics on a zero dimension).
fn clamp_dim(value: usize) -> u16 {
    value.clamp(1, u16::MAX as usize) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_dumps_text() {
        let mut engine = Engine::new(20, 3);
        engine.write(b"hello");
        assert!(engine.screen_dump().contains("hello"));
    }

    #[test]
    fn drains_dsr_reply() {
        let mut engine = Engine::new(80, 24);
        engine.write(b"\x1b[6n");
        assert_eq!(engine.take_output(), b"\x1b[1;1R");
    }

    #[test]
    fn resize_changes_dims() {
        let mut engine = Engine::new(80, 24);
        engine.resize(100, 30);
        assert_eq!((engine.cols(), engine.rows()), (100, 30));
    }

    #[test]
    fn reports_title() {
        let mut engine = Engine::new(80, 24);
        engine.write(b"\x1b]0;hi\x07");
        assert_eq!(engine.title().as_deref(), Some("hi"));
    }

    #[test]
    fn tracks_pwd_via_osc7() {
        let mut engine = Engine::new(80, 24);
        engine.write(b"\x1b]7;file://localhost/Users/me/proj\x1b\\");
        assert_eq!(engine.pwd().as_deref(), Some("/Users/me/proj"));
    }

    #[test]
    fn osc7_path_extraction() {
        assert_eq!(
            pwd_path_from_osc7("file://host/Users/me").as_deref(),
            Some("/Users/me")
        );
        assert_eq!(
            pwd_path_from_osc7("file:///Users/me").as_deref(),
            Some("/Users/me")
        );
        assert_eq!(
            pwd_path_from_osc7("file://host/a/b%20c").as_deref(),
            Some("/a/b c")
        );
        assert_eq!(
            pwd_path_from_osc7("/bare/path").as_deref(),
            Some("/bare/path")
        );
        assert_eq!(pwd_path_from_osc7(""), None);
    }

    #[test]
    fn with_colors_seeds_startup_palette_and_default_fg_bg() {
        let mut colors = Colors::default();
        colors.palette.current[1] = qwertty_term_vt::color::Rgb::new(0x11, 0x22, 0x33);
        colors
            .foreground
            .set(qwertty_term_vt::color::Rgb::new(0xaa, 0xbb, 0xcc));
        colors
            .background
            .set(qwertty_term_vt::color::Rgb::new(0x00, 0x11, 0x22));

        let engine = Engine::with_colors(10, 3, colors);
        let window = engine.snapshot_window(0);
        assert_eq!(
            window.palette[1],
            qwertty_term_vt::color::Rgb::new(0x11, 0x22, 0x33)
        );
        assert_eq!(
            window.default_fg,
            Some(qwertty_term_vt::color::Rgb::new(0xaa, 0xbb, 0xcc))
        );
        assert_eq!(
            window.default_bg,
            Some(qwertty_term_vt::color::Rgb::new(0x00, 0x11, 0x22))
        );
    }

    #[test]
    fn set_colors_swaps_palette_live() {
        // The live palette / default bg swap on `set_colors` (the dirty-marking
        // half is covered by `pagelist::tests::mark_all_dirty_is_the_inverse_of_clear_dirty`).
        let mut engine = Engine::new(10, 3);
        let mut colors = Colors::default();
        colors.palette.current[1] = qwertty_term_vt::color::Rgb::new(0x44, 0x55, 0x66);
        colors
            .background
            .set(qwertty_term_vt::color::Rgb::new(0x77, 0x88, 0x99));
        engine.set_colors(colors);

        let window = engine.snapshot_window(0);
        assert_eq!(
            window.palette[1],
            qwertty_term_vt::color::Rgb::new(0x44, 0x55, 0x66)
        );
        assert_eq!(
            window.default_bg,
            Some(qwertty_term_vt::color::Rgb::new(0x77, 0x88, 0x99))
        );
    }

    #[test]
    fn pin_at_out_of_grid_returns_none() {
        let engine = Engine::new(10, 3);
        assert!(engine.pin_at(10, 0).is_none());
        assert!(engine.pin_at(0, 3).is_none());
        assert!(engine.pin_at(9, 2).is_some());
    }

    #[test]
    fn select_and_selection_string_round_trip_single_row() {
        let mut engine = Engine::new(20, 3);
        engine.write(b"hello world");
        let start = engine.pin_at(0, 0).unwrap();
        let end = engine.pin_at(4, 0).unwrap();
        engine.select(start, end, false);
        assert_eq!(engine.selection_string().as_deref(), Some("hello"));
    }

    #[test]
    fn selection_string_opt_controls_trailing_space_trim() {
        // A selection over "abc" plus trailing blanks: trim=true drops the
        // trailing spaces (clipboard-trim-trailing-spaces), trim=false keeps
        // them.
        let mut engine = Engine::new(10, 3);
        // Explicit trailing space characters (written cells), not blank
        // unwritten cells (which produce nothing either way).
        engine.write(b"abc   ");
        let start = engine.pin_at(0, 0).unwrap();
        let end = engine.pin_at(5, 0).unwrap();
        engine.select(start, end, false);
        assert_eq!(engine.selection_string_opt(true).as_deref(), Some("abc"));
        assert_eq!(
            engine.selection_string_opt(false).as_deref(),
            Some("abc   ")
        );
    }

    #[test]
    fn select_handles_backwards_drag() {
        // A drag from a later cell back to an earlier one (end before start in
        // press order) must still produce the same forward-ordered text —
        // `Screen::selection_string` orders the selection itself.
        let mut engine = Engine::new(20, 3);
        engine.write(b"hello world");
        let anchor = engine.pin_at(4, 0).unwrap();
        let active = engine.pin_at(0, 0).unwrap();
        engine.select(anchor, active, false);
        assert_eq!(engine.selection_string().as_deref(), Some("hello"));
    }

    #[test]
    fn select_spans_multiple_rows() {
        let mut engine = Engine::new(5, 3);
        engine.write(b"abcde\r\nfghij");
        let start = engine.pin_at(0, 0).unwrap();
        let end = engine.pin_at(4, 1).unwrap();
        engine.select(start, end, false);
        assert_eq!(engine.selection_string().as_deref(), Some("abcde\nfghij"));
    }

    #[test]
    fn clear_selection_removes_it() {
        let mut engine = Engine::new(10, 3);
        engine.write(b"hello");
        let start = engine.pin_at(0, 0).unwrap();
        let end = engine.pin_at(4, 0).unwrap();
        engine.select(start, end, false);
        assert!(engine.selection().is_some());
        engine.clear_selection();
        assert!(engine.selection().is_none());
        assert!(engine.selection_string().is_none());
    }

    #[test]
    fn search_all_finds_every_match_in_reading_order() {
        let mut engine = Engine::new(20, 5);
        // Three lines, two contain the needle "fox".
        engine.write(b"the quick fox\r\nlazy dog\r\nfox again\r\n");
        let matches = engine.search_all(b"fox");
        assert_eq!(matches.len(), 2, "two 'fox' occurrences");
        // Reading order: the first row's match comes before the later row's.
        assert!(matches[0].top_left.1 < matches[1].top_left.1);
        // First match is on row 0 at column 10 ("the quick fox").
        assert_eq!(matches[0].top_left, (10, 0));
    }

    #[test]
    fn search_all_is_case_insensitive_ascii() {
        let mut engine = Engine::new(20, 3);
        engine.write(b"Hello WORLD\r\n");
        assert_eq!(engine.search_all(b"world").len(), 1);
        assert_eq!(engine.search_all(b"HELLO").len(), 1);
    }

    #[test]
    fn search_all_empty_needle_is_no_matches() {
        let mut engine = Engine::new(20, 3);
        engine.write(b"anything\r\n");
        assert!(engine.search_all(b"").is_empty());
    }

    #[test]
    fn search_all_reaches_into_scrollback() {
        // A tiny viewport with many rows pushes early matches into history; the
        // whole-list search must still find them.
        let mut engine = Engine::new(20, 3);
        engine.write(b"MARKER-top\r\n");
        for _ in 0..50 {
            engine.write(b"filler\r\n");
        }
        engine.write(b"MARKER-bot\r\n");
        assert_eq!(engine.search_all(b"MARKER-top").len(), 1);
        assert_eq!(engine.search_all(b"MARKER-bot").len(), 1);
    }

    /// Timing probe for the synchronous-vs-thread decision (slice 1). Fills a
    /// full 10k-line scrollback with realistic text (a scattering of matches),
    /// with a scrollback budget large enough that all 10k lines are retained,
    /// then times one whole-list `search_all` on this thread. Not a hard bound
    /// (CI machines vary); it prints the measured time so the threading decision
    /// is evidence-backed. Run with `--nocapture` to see it.
    #[test]
    fn search_timing_10k_scrollback() {
        use std::time::Instant;
        // Build a Terminal directly with a large scrollback budget so the whole
        // 10k-line history is kept (the default 10_000-byte budget would prune
        // most of it, understating the search cost).
        let terminal = Terminal::new(Options {
            cols: 120,
            rows: 40,
            max_scrollback: 64 * 1024 * 1024,
            colors: Colors::default(),
        });
        let mut engine = Engine {
            stream: Stream::new(TerminalHandler::new(terminal)),
        };
        for i in 0..10_000u32 {
            if i % 37 == 0 {
                engine.write(b"the needle is here in this line of output\r\n");
            } else {
                engine.write(b"lorem ipsum dolor sit amet consectetur adipiscing elit sed do\r\n");
            }
        }
        let start = Instant::now();
        let matches = engine.search_all(b"needle");
        let elapsed = start.elapsed();
        eprintln!(
            "search_timing: {} matches over 10k lines in {:?} ({:.3} ms)",
            matches.len(),
            elapsed,
            elapsed.as_secs_f64() * 1000.0
        );
        assert!(matches.len() >= 270, "expected ~271 needle lines");
    }

    // ---- selection-gesture accessors (absolute-screen space) ------------

    /// Boundary set for the tests: the ported upstream default.
    const BOUNDARY: &[u32] = &qwertty_term_vt::screen::DEFAULT_WORD_BOUNDARIES;

    #[test]
    fn window_to_screen_point_maps_offsets() {
        let mut engine = Engine::new(5, 3);
        // 5 lines → total 5 rows, scrollback_len 2.
        engine.write(b"aaa\r\nbbb\r\nccc\r\nddd\r\neee");
        assert_eq!(engine.scrollback_len(), 2);
        // Offset 0: the visible window is rows ccc/ddd/eee (screen rows 2..4).
        assert_eq!(engine.window_to_screen_point(0, 0, 0), Some((0, 2)));
        assert_eq!(engine.window_to_screen_point(4, 2, 0), Some((4, 4)));
        // Offset 2 (top of history): visible aaa/bbb/ccc (screen rows 0..2).
        assert_eq!(engine.window_to_screen_point(0, 0, 2), Some((0, 0)));
        assert_eq!(engine.window_to_screen_point(0, 2, 2), Some((0, 2)));
        // Offset beyond history clamps (same as the snapshot).
        assert_eq!(engine.window_to_screen_point(0, 0, 99), Some((0, 0)));
        // Out of grid → None.
        assert_eq!(engine.window_to_screen_point(5, 0, 0), None);
        assert_eq!(engine.window_to_screen_point(0, 3, 0), None);
    }

    #[test]
    fn select_screen_points_round_trips_when_scrolled() {
        let mut engine = Engine::new(5, 3);
        engine.write(b"aaa\r\nbbb\r\nccc\r\nddd\r\neee");
        // Scrolled to the top (offset 2), select the visible top row ("aaa"):
        // window (0,0)..(2,0) maps to screen (0,0)..(2,0).
        let a = engine.window_to_screen_point(0, 0, 2).unwrap();
        let b = engine.window_to_screen_point(2, 0, 2).unwrap();
        assert!(engine.select_screen_points(a, b, false));
        assert_eq!(engine.selection_string().as_deref(), Some("aaa"));
    }

    #[test]
    fn select_word_bounds_finds_word_and_rejects_empty() {
        let mut engine = Engine::new(20, 3);
        engine.write(b"hello beta-gamma");
        // Word under (7,0): "beta-gamma" — '-' is not a boundary codepoint.
        assert_eq!(
            engine.select_word_bounds(7, 0, BOUNDARY),
            Some(((6, 0), (15, 0)))
        );
        // Unwritten cell → None.
        assert_eq!(engine.select_word_bounds(19, 2, BOUNDARY), None);
    }

    #[test]
    fn select_line_bounds_trims_and_falls_back() {
        let mut engine = Engine::new(20, 3);
        engine.write(b"  hi there  \r\n");
        // Trimmed: leading/trailing whitespace dropped.
        assert_eq!(
            engine.select_line_bounds(5, 0, true),
            Some(((2, 0), (9, 0)))
        );
        // A blank line has no trimmed selection…
        assert_eq!(engine.select_line_bounds(0, 1, true), None);
        // …but the untrimmed fallback selects it.
        assert!(engine.select_line_bounds(0, 1, false).is_some());
    }

    #[test]
    fn select_word_between_bounds_walks_toward_target() {
        let mut engine = Engine::new(10, 3);
        engine.write(b"ABC  DEF");
        // From the unwritten cell (9,0) toward (0,0): nearest word is "DEF".
        assert_eq!(
            engine.select_word_between_bounds((9, 0), (0, 0), BOUNDARY),
            Some(((5, 0), (7, 0)))
        );
    }

    #[test]
    fn screen_range_resolves_ordered_bounds() {
        let mut engine = Engine::new(10, 3);
        engine.write(b"hello");
        let start = engine.pin_at(4, 0).unwrap();
        let end = engine.pin_at(0, 0).unwrap();
        engine.select(start, end, false);
        let (s, e, rect) = engine.selection().unwrap();
        let range = engine.screen_range(s, e, rect).unwrap();
        // Backwards drag (4 -> 0): the range is still reported top-left to
        // bottom-right.
        assert_eq!(range.top_left, (0, 0));
        assert_eq!(range.bottom_right, (4, 0));
    }
}
