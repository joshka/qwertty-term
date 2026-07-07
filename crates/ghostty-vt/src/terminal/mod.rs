//! The terminal state machine. Port of `src/terminal/Terminal.zig`
//! (commit `2da015cd6`).
//!
//! `Terminal` is the protocol/policy layer that sits on top of two [`Screen`]s
//! (primary + alternate, via [`ScreenSet`]) and ties together the landed VT
//! modules: [`modes`](crate::modes), [`charsets`](crate::charsets),
//! [`Tabstops`](crate::tabstops), [`sgr`](crate::sgr), [`csi`](crate::csi),
//! [`color`](crate::color), and the OSC semantic-prompt parser.
//!
//! See `docs/analysis/terminal.md` for the maintainer-grade map. This chunk
//! ports the `Terminal` struct plus its operations wired to the real Screen
//! API. The stream/dispatch layer (`stream.zig`/`stream_terminal.zig`) is the
//! NEXT chunk; method names/signatures here are shaped so it maps 1:1.
//!
//! Seams left for sibling chunks (all marked `TODO(chunk:*)` inline):
//! - kitty graphics exec + storage (`kitty-gfx`)
//! - Glyph Protocol / APC glyph glossary (`apc`)
//! - mouse event/format interpretation (`input`)
//! - the stream handler that drives these methods (`stream`)

mod print;
mod screen_set;

pub use screen_set::{ScreenKey, ScreenSet};

use crate::charsets::{self, ActiveSlot, Charset, Slots};
use crate::color::{DynamicPalette, DynamicRgb};
use crate::csi::TabClear;
use crate::modes::{Mode, ModeState};
use crate::page::SemanticPrompt as RowSemanticPrompt;
use crate::page::size::CellCountInt;
use crate::page::style::Style;
use crate::screen::semantic::Redraw;
use crate::screen::{ProtectedMode, Screen};
use crate::tabstops::Tabstops;

/// Default tab stop interval. Port of `TABSTOP_INTERVAL`.
const TABSTOP_INTERVAL: usize = 8;

/// Whether the terminal is writing to the main display or a status line.
/// Port of `ansi.StatusDisplay`. We don't support a status line, so
/// `StatusLine` prints are black-holed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusDisplay {
    #[default]
    Main,
    StatusLine,
}

/// The scrolling region (incl. left/right margins). Port of
/// `Terminal.ScrollingRegion`. Preconditions: `top < bottom`, `left < right`,
/// `right <= cols - 1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScrollingRegion {
    pub top: CellCountInt,
    pub bottom: CellCountInt,
    pub left: CellCountInt,
    pub right: CellCountInt,
}

/// The dynamic color configuration. Port of `Terminal.Colors`.
#[derive(Debug, Clone)]
pub struct Colors {
    pub background: DynamicRgb,
    pub foreground: DynamicRgb,
    pub cursor: DynamicRgb,
    pub palette: DynamicPalette,
}

impl Default for Colors {
    /// Port of `Colors.default`.
    fn default() -> Self {
        Colors {
            background: DynamicRgb::UNSET,
            foreground: DynamicRgb::UNSET,
            cursor: DynamicRgb::UNSET,
            palette: DynamicPalette::DEFAULT,
        }
    }
}

/// Terminal-level renderer dirty flags. Port of `Terminal.Dirty`. Distinct
/// from `Screen::Dirty`.
#[derive(Debug, Clone, Copy, Default)]
pub struct Dirty {
    pub palette: bool,
    pub reverse_colors: bool,
    pub clear: bool,
    pub preedit: bool,
    pub glyph_glossary: bool,
}

/// The XTSHIFTESCAPE tri-state. Port of the inline `mouse_shift_capture` enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseShiftCapture {
    #[default]
    Null,
    False,
    True,
}

/// Packed terminal flags. Port of `Terminal.flags`.
#[derive(Debug, Clone, Copy)]
pub struct Flags {
    /// Kitty extension: OSC 133 `redraw=0` disables prompt clear on resize.
    pub shell_redraws_prompt: Redraw,
    /// Set via ESC[4;2m (XTMODKEYS); any other modify-key mode clears it.
    pub modify_other_keys_2: bool,
    // TODO(chunk:input): mouse_event / mouse_format / mouse_shape are stored
    // but interpreted by the stream/input layer, not by Terminal.
    pub mouse_shift_capture: MouseShiftCapture,
    pub focused: bool,
    pub password_input: bool,
    pub selection_scroll: bool,
    pub search_viewport_dirty: bool,
    pub dirty: Dirty,
}

impl Default for Flags {
    fn default() -> Self {
        Flags {
            shell_redraws_prompt: Redraw::True,
            modify_other_keys_2: false,
            mouse_shift_capture: MouseShiftCapture::Null,
            focused: true,
            password_input: false,
            selection_scroll: false,
            search_viewport_dirty: false,
            dirty: Dirty::default(),
        }
    }
}

/// Options for constructing a [`Terminal`]. Port of `Terminal.Options`
/// (kitty-image / glyph options are seams for later chunks).
#[derive(Debug, Clone)]
pub struct Options {
    pub cols: CellCountInt,
    pub rows: CellCountInt,
    pub max_scrollback: usize,
    pub colors: Colors,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            cols: 80,
            rows: 24,
            max_scrollback: 10_000,
            colors: Colors::default(),
        }
    }
}

/// The terminal state machine. Port of `Terminal`.
pub struct Terminal {
    /// The set of screens (primary + lazy alternate).
    pub screens: ScreenSet,

    /// Whether we're writing to the main display or a status line.
    pub status_display: StatusDisplay,

    /// The tab stops.
    pub tabstops: Tabstops,

    /// The grid size.
    pub rows: CellCountInt,
    pub cols: CellCountInt,

    /// The screen size in pixels (pty/image events).
    pub width_px: u32,
    pub height_px: u32,

    /// The current scrolling region.
    pub scrolling_region: ScrollingRegion,

    /// The last reported OSC 7 pwd.
    pub pwd: Vec<u8>,

    /// The OSC 0/2 window title.
    pub title: Vec<u8>,

    /// The dynamic color state.
    pub colors: Colors,

    /// The previous printed char (for REP `ESC [ n b`).
    pub previous_char: Option<u32>,

    /// The active modes.
    pub modes: ModeState,

    /// Packed terminal flags.
    pub flags: Flags,
}

impl Terminal {
    /// Initialize a new terminal. Port of `init`.
    pub fn new(opts: Options) -> Terminal {
        let cols = opts.cols;
        let rows = opts.rows;

        let screens = ScreenSet::new(crate::screen::Options {
            cols,
            rows,
            max_scrollback: opts.max_scrollback,
        });

        Terminal {
            screens,
            status_display: StatusDisplay::Main,
            tabstops: Tabstops::new(cols as usize, TABSTOP_INTERVAL),
            rows,
            cols,
            width_px: 0,
            height_px: 0,
            scrolling_region: ScrollingRegion {
                top: 0,
                bottom: rows - 1,
                left: 0,
                right: cols - 1,
            },
            pwd: Vec::new(),
            title: Vec::new(),
            colors: opts.colors,
            previous_char: None,
            modes: ModeState::new(),
            flags: Flags::default(),
        }
    }

    /// The active screen. Port of `self.screens.active`.
    #[inline]
    pub fn screen(&self) -> &Screen {
        self.screens.active()
    }

    /// The active screen (mutable).
    #[inline]
    pub fn screen_mut(&mut self) -> &mut Screen {
        self.screens.active_mut()
    }

    // ---- charset -------------------------------------------------------

    /// Set the charset into the given slot. Port of `configureCharset`.
    pub fn configure_charset(&mut self, slot: Slots, set: Charset) {
        self.screen_mut().charset.charsets.set(slot, set);
    }

    /// Invoke the charset in `slot` into the active slot. If `single`, it is
    /// only invoked for a single char. Port of `invokeCharset`.
    pub fn invoke_charset(&mut self, active: ActiveSlot, slot: Slots, single: bool) {
        if single {
            debug_assert_eq!(active, ActiveSlot::Gl);
            self.screen_mut().charset.single_shift = Some(slot);
            return;
        }
        match active {
            ActiveSlot::Gl => self.screen_mut().charset.gl = slot,
            ActiveSlot::Gr => self.screen_mut().charset.gr = slot,
        }
    }

    // ---- simple motion -------------------------------------------------

    /// Carriage return: move the cursor to the first column. Port of
    /// `carriageReturn`.
    pub fn carriage_return(&mut self) {
        self.screen_mut().cursor.pending_wrap = false;

        let origin = self.modes.get(Mode::Origin);
        let region_left = self.scrolling_region.left;
        let cursor_x = self.screen().cursor.x;
        // Structure kept 1:1 with Zig `carriageReturn`: origin mode OR cursor at
        // or past the left margin both go to the left margin; otherwise col 0.
        #[allow(clippy::if_same_then_else)]
        let target = if origin {
            region_left
        } else if cursor_x >= region_left {
            region_left
        } else {
            0
        };
        self.screen_mut().cursor_horizontal_absolute(target);
    }

    /// Linefeed: move the cursor to the next line. Port of `linefeed`.
    pub fn linefeed(&mut self) {
        self.index();
        if self.modes.get(Mode::Linefeed) {
            self.carriage_return();
        }
    }

    /// Backspace: move the cursor back one column. Port of `backspace`.
    pub fn backspace(&mut self) {
        self.cursor_left(1);
    }

    // ---- cursor moves with clamping ------------------------------------

    /// Move the cursor up, clamped to the scroll region. Port of `cursorUp`.
    pub fn cursor_up(&mut self, count_req: usize) {
        self.screen_mut().cursor.pending_wrap = false;

        let y = self.screen().cursor.y;
        let top = self.scrolling_region.top;
        let max = if y >= top { y - top } else { y };
        let count = max.min(count_req.max(1) as CellCountInt);
        self.screen_mut().cursor_up(count);
    }

    /// Move the cursor down, clamped to the scroll region. Port of `cursorDown`.
    pub fn cursor_down(&mut self, count_req: usize) {
        self.screen_mut().cursor.pending_wrap = false;

        let y = self.screen().cursor.y;
        let bottom = self.scrolling_region.bottom;
        let max = if y <= bottom {
            bottom - y
        } else {
            self.rows - y - 1
        };
        let count = max.min(count_req.max(1) as CellCountInt);
        self.screen_mut().cursor_down(count);
    }

    /// Move the cursor right, clamped to the scroll region. Port of `cursorRight`.
    pub fn cursor_right(&mut self, count_req: usize) {
        self.screen_mut().cursor.pending_wrap = false;

        let x = self.screen().cursor.x;
        let right = self.scrolling_region.right;
        let max = if x <= right {
            right - x
        } else {
            self.cols - x - 1
        };
        let count = max.min(count_req.max(1) as CellCountInt);
        self.screen_mut().cursor_right(count);
    }

    /// Move the cursor left, honoring the reverse-wrap / XTREVWRAP(2) modes.
    /// Port of `cursorLeft` — the trickiest motion op.
    pub fn cursor_left(&mut self, count_req: usize) {
        #[derive(PartialEq, Eq)]
        enum WrapMode {
            None,
            Reverse,
            ReverseExtended,
        }

        let wrap_mode = if !self.modes.get(Mode::Wraparound) {
            WrapMode::None
        } else if self.modes.get(Mode::ReverseWrapExtended) {
            WrapMode::ReverseExtended
        } else if self.modes.get(Mode::ReverseWrap) {
            WrapMode::Reverse
        } else {
            WrapMode::None
        };

        let mut count = count_req.max(1) as CellCountInt;

        // Fast/typical path: no wrap.
        if wrap_mode == WrapMode::None {
            let x = self.screen().cursor.x;
            self.screen_mut().cursor_left(count.min(x));
            self.screen_mut().cursor.pending_wrap = false;
            return;
        }

        // Pending-wrap in reverse mode decrements the move by one (xterm).
        if self.screen().cursor.pending_wrap {
            count -= 1;
            self.screen_mut().cursor.pending_wrap = false;
        }

        let top = self.scrolling_region.top;
        let bottom = self.scrolling_region.bottom;
        let right_margin = self.scrolling_region.right;
        let left_margin = if self.screen().cursor.x < self.scrolling_region.left {
            0
        } else {
            self.scrolling_region.left
        };

        // Edge case: already at the left margin.
        if self.screen().cursor.x == left_margin {
            match wrap_mode {
                WrapMode::Reverse => {
                    if self.screen().cursor.y <= top {
                        self.screen_mut().cursor_absolute(left_margin, top);
                        return;
                    }
                }
                WrapMode::ReverseExtended => {}
                WrapMode::None => unreachable!(),
            }
        }

        loop {
            // We can move at most to the left margin.
            let max = self.screen().cursor.x - left_margin;
            let amount = max.min(count);
            count -= amount;
            self.screen_mut().cursor_left(amount);

            if count == 0 {
                break;
            }

            // At the top of the region.
            if self.screen().cursor.y == top {
                if wrap_mode != WrapMode::ReverseExtended {
                    break;
                }
                self.screen_mut().cursor_absolute(right_margin, bottom);
                count -= 1;
                continue;
            }

            // Undefined-in-xterm guard: wrap up to (0,0) and stop.
            if self.screen().cursor.y == 0 {
                debug_assert_eq!(self.screen().cursor.x, left_margin);
                break;
            }

            // If the previous line isn't wrapped, we're done.
            if wrap_mode != WrapMode::ReverseExtended {
                // SAFETY: cursor.y > 0 here, so a row exists 1 above.
                let prev_row = unsafe { self.screen().cursor_row_up(1) };
                let wrapped = unsafe { (*prev_row).wrap() };
                if !wrapped {
                    break;
                }
            }

            let new_y = self.screen().cursor.y - 1;
            self.screen_mut().cursor_absolute(right_margin, new_y);
            count -= 1;
        }
    }

    // ---- save / restore cursor -----------------------------------------

    /// Save cursor position and state (DECSC). Port of `saveCursor`.
    pub fn save_cursor(&mut self) {
        let origin = self.modes.get(Mode::Origin);
        let screen = self.screen_mut();
        screen.saved_cursor = Some(crate::screen::cursor::SavedCursor {
            x: screen.cursor.x,
            y: screen.cursor.y,
            style: screen.cursor.style,
            protected: screen.cursor.protected,
            pending_wrap: screen.cursor.pending_wrap,
            origin,
            charset: screen.charset,
        });
    }

    /// Restore cursor position and state (DECRC). Port of `restoreCursor`.
    pub fn restore_cursor(&mut self) {
        let cols = self.cols;
        let rows = self.rows;
        let saved =
            self.screen()
                .saved_cursor
                .clone()
                .unwrap_or(crate::screen::cursor::SavedCursor {
                    x: 0,
                    y: 0,
                    style: Style::default(),
                    protected: false,
                    pending_wrap: false,
                    origin: false,
                    charset: charsets::CharsetState::default(),
                });

        // Set the style first because it can fail.
        self.screen_mut().cursor.style = saved.style;
        if self.screen_mut().manual_style_update().is_err() {
            // Revert to an unstyled cursor; the restore must otherwise succeed.
            self.screen_mut().cursor.style = Style::default();
            self.screen_mut()
                .manual_style_update()
                .expect("default-style update cannot fail");
        }

        self.screen_mut().charset = saved.charset;
        self.modes.set(Mode::Origin, saved.origin);
        self.screen_mut().cursor.pending_wrap = saved.pending_wrap;
        self.screen_mut().cursor.protected = saved.protected;
        let x = saved.x.min(cols - 1);
        let y = saved.y.min(rows - 1);
        self.screen_mut().cursor_absolute(x, y);
        self.screen().assert_integrity();
    }

    // ---- protected mode ------------------------------------------------

    /// Set the character protection mode. Port of `setProtectedMode`.
    pub fn set_protected_mode(&mut self, mode: ProtectedMode) {
        match mode {
            ProtectedMode::Off => {
                // screen.protected_mode is NEVER reset to Off — eraseChars
                // depends on knowing the most recent mode.
                self.screen_mut().cursor.protected = false;
            }
            ProtectedMode::Iso => {
                self.screen_mut().cursor.protected = true;
                self.screen_mut().protected_mode = ProtectedMode::Iso;
            }
            ProtectedMode::Dec => {
                self.screen_mut().cursor.protected = true;
                self.screen_mut().protected_mode = ProtectedMode::Dec;
            }
        }
    }

    // ---- tabs ----------------------------------------------------------

    /// Horizontal tab: move to the next tabstop. Port of `horizontalTab`.
    pub fn horizontal_tab(&mut self) {
        while self.screen().cursor.x < self.scrolling_region.right {
            self.screen_mut().cursor_right(1);
            if self.tabstops.get(self.screen().cursor.x as usize) {
                return;
            }
        }
    }

    /// Move to the previous tabstop. Port of `horizontalTabBack`.
    pub fn horizontal_tab_back(&mut self) {
        let left_limit = if self.modes.get(Mode::Origin) {
            self.scrolling_region.left
        } else {
            0
        };
        loop {
            if self.screen().cursor.x <= left_limit {
                return;
            }
            self.screen_mut().cursor_left(1);
            if self.tabstops.get(self.screen().cursor.x as usize) {
                return;
            }
        }
    }

    /// Clear tab stops. Port of `tabClear`.
    pub fn tab_clear(&mut self, cmd: TabClear) {
        match cmd {
            TabClear::Current => {
                let x = self.screen().cursor.x as usize;
                self.tabstops.unset(x);
            }
            TabClear::All => self.tabstops.reset(0),
            _ => {}
        }
    }

    /// Set a tab stop at the cursor. Port of `tabSet`.
    pub fn tab_set(&mut self) {
        let x = self.screen().cursor.x as usize;
        self.tabstops.set(x);
    }

    /// Reset tab stops to the default interval. Port of `tabReset`.
    pub fn tab_reset(&mut self) {
        self.tabstops.reset(TABSTOP_INTERVAL);
    }

    // ---- index family --------------------------------------------------

    /// IND / LF: move to the next line in the scroll region, possibly scrolling.
    /// Port of `index`.
    ///
    /// PROGRESS: the scrollback fast path (`cursorScrollAbove`) and the l/r-margin
    /// slow path (`scrollUp` with bg fill) are NOT yet wired — see PROGRESS note
    /// in `docs/analysis/terminal.md`. This implements the no-scroll moves and
    /// the full-screen `erase_row_bounded` hot path.
    pub fn index(&mut self) {
        // Unset pending wrap.
        self.screen_mut().cursor.pending_wrap = false;

        // Semantic-content propagation after any scroll.
        let apply_semantic = |t: &mut Terminal| {
            let clear_eol = t.screen().cursor.semantic_content_clear_eol;
            if t.screen().cursor.semantic_content != crate::page::SemanticContent::Output {
                if clear_eol {
                    t.screen_mut().cursor.semantic_content = crate::page::SemanticContent::Output;
                    t.screen_mut().cursor.semantic_content_clear_eol = false;
                } else {
                    // SAFETY: cursor row live.
                    unsafe {
                        (*t.screen().cursor.page_row)
                            .set_semantic_prompt(RowSemanticPrompt::PromptContinuation);
                    }
                }
            }
        };

        let y = self.screen().cursor.y;
        let top = self.scrolling_region.top;
        let bottom = self.scrolling_region.bottom;

        // Outside the scroll region: move down one (if room).
        if y < top || y > bottom {
            if y < self.rows - 1 {
                self.screen_mut().cursor_down(1);
            }
            apply_semantic(self);
            return;
        }

        let x = self.screen().cursor.x;
        // Inside the region and on the bottom-most line: scroll up.
        if y == bottom && x >= self.scrolling_region.left && x <= self.scrolling_region.right {
            // Full-screen (no margins) scrollback path.
            if self.scrolling_region.top == 0
                && self.scrolling_region.left == 0
                && self.scrolling_region.right == self.cols - 1
            {
                // Port of the scrollback-creating scroll. Screen's
                // `cursor_down_scroll` is the analogous primitive.
                self.screen_mut().cursor_down_scroll();
                apply_semantic(self);
                return;
            }

            // TODO(chunk:terminal-scroll): the l/r-margin slow path
            // (`scrollUp` with SGR bg fill) and the bg-fill-needed check are
            // not yet ported. For the common no-bg full-width-margin case we
            // use the `erase_row_bounded` hot path below.
            let region_top = self.scrolling_region.top;
            self.screen_mut().pages.erase_row_bounded(
                crate::point::Point::active(0, region_top as u32),
                (bottom - region_top) as usize,
            );
            // erase_row_bounded moves the cursor pin up by 1; move it back.
            self.screen_mut().cursor.y -= 1;
            self.screen_mut().cursor_down(1);
            if self.screen_mut().manual_style_update().is_err() {
                self.screen_mut().cursor.style = Style::default();
                self.screen_mut()
                    .manual_style_update()
                    .expect("default-style update cannot fail");
            }
            apply_semantic(self);
            return;
        }

        // Inside the region, not bottom: move down one (max to region bottom).
        if y < bottom {
            self.screen_mut().cursor_down(1);
        }
        apply_semantic(self);
    }

    /// RI: move to the previous line, possibly scrolling down. Port of
    /// `reverseIndex`.
    pub fn reverse_index(&mut self) {
        let cursor = &self.screen().cursor;
        if cursor.y != self.scrolling_region.top
            || cursor.x < self.scrolling_region.left
            || cursor.x > self.scrolling_region.right
        {
            self.cursor_up(1);
            return;
        }
        self.scroll_down(1);
    }

    // ---- cursor position / margins -------------------------------------

    /// Set cursor position (CUP), 1-indexed, origin/margin-aware. Port of
    /// `setCursorPos`.
    pub fn set_cursor_pos(&mut self, row_req: usize, col_req: usize) {
        let (x_offset, y_offset, x_max, y_max) = if self.modes.get(Mode::Origin) {
            (
                self.scrolling_region.left,
                self.scrolling_region.top,
                self.scrolling_region.right + 1,
                self.scrolling_region.bottom + 1,
            )
        } else {
            (0, 0, self.cols, self.rows)
        };

        self.screen_mut().cursor.pending_wrap = false;

        let row = if row_req == 0 { 1 } else { row_req };
        let col = if col_req == 0 { 1 } else { col_req };
        let x = (x_max.min((col as CellCountInt).saturating_add(x_offset))).saturating_sub(1);
        let y = (y_max.min((row as CellCountInt).saturating_add(y_offset))).saturating_sub(1);

        let cur_x = self.screen().cursor.x;
        let cur_y = self.screen().cursor.y;
        if y == cur_y {
            if x > cur_x {
                self.screen_mut().cursor_right(x - cur_x);
            } else {
                self.screen_mut().cursor_left(cur_x - x);
            }
            return;
        }
        self.screen_mut().cursor_absolute(x, y);
    }

    /// DECSTBM: set the top/bottom margins (1-indexed). Port of
    /// `setTopAndBottomMargin`.
    pub fn set_top_and_bottom_margin(&mut self, top_req: usize, bottom_req: usize) {
        let top = top_req.max(1);
        let bottom = if bottom_req == 0 {
            self.rows as usize
        } else {
            bottom_req
        }
        .min(self.rows as usize);
        if top >= bottom {
            return;
        }
        self.scrolling_region.top = (top - 1) as CellCountInt;
        self.scrolling_region.bottom = (bottom - 1) as CellCountInt;
        self.set_cursor_pos(1, 1);
    }

    /// DECSLRM: set the left/right margins (1-indexed). Port of
    /// `setLeftAndRightMargin`.
    pub fn set_left_and_right_margin(&mut self, left_req: usize, right_req: usize) {
        if !self.modes.get(Mode::EnableLeftAndRightMargin) {
            return;
        }
        let left = left_req.max(1);
        let right = if right_req == 0 {
            self.cols as usize
        } else {
            right_req
        }
        .min(self.cols as usize);
        if left >= right {
            return;
        }
        self.scrolling_region.left = (left - 1) as CellCountInt;
        self.scrolling_region.right = (right - 1) as CellCountInt;
        self.set_cursor_pos(1, 1);
    }

    /// SD: scroll the text down by `count` rows. Port of `scrollDown`.
    ///
    /// PROGRESS: depends on `insert_lines`, which is not yet ported. See PROGRESS
    /// note. Currently a stub that preserves the cursor.
    pub fn scroll_down(&mut self, _count: usize) {
        // TODO(chunk:terminal-scroll): port via insert_lines once the l/r-margin
        // row-shift machinery is exposed. See docs/analysis/terminal.md PROGRESS.
    }

    /// Set the reported pwd (OSC 7). Port of `setPwd`.
    pub fn set_pwd(&mut self, pwd: &[u8]) {
        self.pwd.clear();
        self.pwd.extend_from_slice(pwd);
    }

    /// The reported pwd, if any. Port of `getPwd`.
    pub fn get_pwd(&self) -> Option<&[u8]> {
        if self.pwd.is_empty() {
            None
        } else {
            Some(&self.pwd)
        }
    }

    /// Set the window title (OSC 0/2). Port of `setTitle`.
    pub fn set_title(&mut self, t: &[u8]) {
        self.title.clear();
        self.title.extend_from_slice(t);
    }

    /// The window title, if any. Port of `getTitle`.
    pub fn get_title(&self) -> Option<&[u8]> {
        if self.title.is_empty() {
            None
        } else {
            Some(&self.title)
        }
    }

    /// Print a string (sequence of codepoints). Port of `printString`.
    pub fn print_string(&mut self, s: &str) {
        for c in s.chars() {
            self.print(c as u32);
        }
    }

    /// The active viewport as plain text (soft-wrap boundaries kept as newlines).
    /// Port of `plainString` (via `Screen::dump_string`, `unwrap=false`).
    pub fn plain_string(&self) -> String {
        self.screen()
            .dump_string(crate::point::Tag::Viewport, false)
    }

    /// The active viewport as plain text with soft-wrapped rows joined. Port of
    /// `plainStringUnwrapped` (`unwrap=true`).
    pub fn plain_string_unwrapped(&self) -> String {
        self.screen().dump_string(crate::point::Tag::Viewport, true)
    }
}

#[cfg(test)]
mod tests;
