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

// Re-export so the stream/dispatch layer can name these without reaching
// into `crate::screen`. `ProtectedMode` originates in `screen` (it is a
// Screen-owned enum in the Zig source too).
pub use crate::screen::ProtectedMode;

use crate::charsets::{self, ActiveSlot, Charset, Slots};
use crate::color::{DynamicPalette, DynamicRgb};
use crate::csi::TabClear;
use crate::modes::{Mode, ModeState};
use crate::page::SemanticPrompt as RowSemanticPrompt;
use crate::page::size::CellCountInt;
use crate::page::style::{self, Style};
use crate::page::{Cell, Page};
use crate::pagelist::Pin;
use crate::screen::semantic::Redraw;
use crate::screen::{Resize, Screen};
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

/// Modal screen changes (DEC modes 47/1047/1049). Port of
/// `Terminal.SwitchScreenMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchScreenMode {
    /// Legacy (mode 47): switch + copy cursor, no clear.
    M47,
    /// Mode 1047: clear the alternate screen on exit; copy cursor.
    M1047,
    /// Mode 1049: save cursor + clear alt on entry, restore on exit.
    M1049,
}

/// Options for scrolling the viewport of the terminal grid. Port of
/// `Terminal.ScrollViewport`.
#[derive(Debug, Clone, Copy)]
pub enum ScrollViewport {
    /// Scroll to the top of the scrollback.
    Top,
    /// Scroll to the bottom (top of the active area).
    Bottom,
    /// Scroll by a delta; up is negative.
    Delta(isize),
    /// Scroll to an absolute row offset from the top of the scrollable area.
    Row(usize),
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
                // Port of the scrollback-creating scroll.
                self.screen_mut().cursor_scroll_above();
                apply_semantic(self);
                return;
            }

            // Slow path for left/right margins OR when we have an SGR bg to
            // preserve in the erased rows (erase_row_bounded doesn't fill bg,
            // scroll_up does — but scroll_up is much slower).
            if self.scrolling_region.left != 0
                || self.scrolling_region.right != self.cols - 1
                || !self.screen().blank_cell().is_zero()
            {
                self.scroll_up(1);
                apply_semantic(self);
                return;
            }

            // Otherwise use the fast PageList scroll of the region contents.
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
    pub fn scroll_down(&mut self, count: usize) {
        // Preserve our x/y to restore.
        let old_x = self.screen().cursor.x;
        let old_y = self.screen().cursor.y;
        let old_wrap = self.screen().cursor.pending_wrap;

        // Move to the top of the scroll region, then insert lines.
        let left = self.scrolling_region.left;
        let top = self.scrolling_region.top;
        self.screen_mut().cursor_absolute(left, top);
        self.insert_lines(count);

        self.screen_mut().cursor_absolute(old_x, old_y);
        self.screen_mut().cursor.pending_wrap = old_wrap;
    }

    /// SU: scroll the text up by `count` rows. Port of `scrollUp`.
    pub fn scroll_up(&mut self, count: usize) {
        // Preserve our x/y to restore.
        let old_x = self.screen().cursor.x;
        let old_y = self.screen().cursor.y;
        let old_wrap = self.screen().cursor.pending_wrap;

        // If the region is at the top with no l/r margins, move scrolled-out
        // text into scrollback via cursor_scroll_above.
        if self.scrolling_region.top == 0
            && self.scrolling_region.left == 0
            && self.scrolling_region.right == self.cols - 1
        {
            let region_height = self.scrolling_region.bottom + 1;
            let adjusted_count = count.min(region_height as usize);
            let bottom = self.scrolling_region.bottom;
            self.screen_mut().cursor_absolute(0, bottom);
            for _ in 0..adjusted_count {
                self.screen_mut().cursor_scroll_above();
            }
            self.screen_mut().cursor_absolute(old_x, old_y);
            self.screen_mut().cursor.pending_wrap = old_wrap;
            return;
        }

        // Move to the top of the scroll region, then delete lines.
        let left = self.scrolling_region.left;
        let top = self.scrolling_region.top;
        self.screen_mut().cursor_absolute(left, top);
        self.delete_lines(count);

        self.screen_mut().cursor_absolute(old_x, old_y);
        self.screen_mut().cursor.pending_wrap = old_wrap;
    }

    /// Scroll the viewport of the terminal grid. Port of `scrollViewport`.
    pub fn scroll_viewport(&mut self, behavior: ScrollViewport) {
        let s = match behavior {
            ScrollViewport::Top => crate::screen::Scroll::Top,
            ScrollViewport::Bottom => crate::screen::Scroll::Active,
            ScrollViewport::Delta(d) => crate::screen::Scroll::DeltaRow(d),
            ScrollViewport::Row(r) => crate::screen::Scroll::Row(r),
        };
        self.screen_mut().scroll(s);
    }

    // ---- insert / delete lines -----------------------------------------

    /// Handle boundary conditions before shifting a row (insertLines/deleteLines):
    /// split wide chars across scrolling-region boundaries and orphaned spacer
    /// heads at line ends. Port of `rowWillBeShifted`.
    ///
    /// # Safety
    /// `page`/`row` must be live for the active screen.
    unsafe fn row_will_be_shifted(&self, page: *mut Page, row: *mut crate::page::Row) {
        use crate::page::Wide;
        let region = self.scrolling_region;
        let cols = self.cols;
        // SAFETY: page/row live per caller.
        unsafe {
            let base = (*row).cells().ptr((*page).mem());

            // If the region includes the rightmost column, or either of the 2
            // leftmost columns, turn any spacer head into a normal empty cell.
            if region.right == cols - 1 || region.left < 2 {
                let end_cell = base.add((*page).size.cols as usize - 1);
                if (*end_cell).wide() == Wide::SpacerHead {
                    (*end_cell).set_wide(Wide::Narrow);
                }
            }

            let left_cell = base.add(region.left as usize);
            let right_cell = base.add(region.right as usize);

            if (*left_cell).wide() == Wide::SpacerTail {
                let wide_cell = base.add(region.left as usize - 1);
                if (*wide_cell).has_grapheme() {
                    (*page).clear_grapheme(wide_cell);
                    (*page).update_row_grapheme_flag(row);
                }
                (*wide_cell).set_codepoint(0);
                (*wide_cell).set_wide(Wide::Narrow);
                (*left_cell).set_wide(Wide::Narrow);
            }

            if (*right_cell).wide() == Wide::Wide {
                let tail_cell = base.add(region.right as usize + 1);
                if (*right_cell).has_grapheme() {
                    (*page).clear_grapheme(right_cell);
                    (*page).update_row_grapheme_flag(row);
                }
                (*right_cell).set_codepoint(0);
                (*right_cell).set_wide(Wide::Narrow);
                (*tail_cell).set_wide(Wide::Narrow);
            }
        }
    }

    /// IL: insert `count` blank lines at the cursor row, shifting the region
    /// below down. Port of `insertLines`.
    pub fn insert_lines(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let region = self.scrolling_region;
        let cur = &self.screen().cursor;
        if cur.y < region.top
            || cur.y > region.bottom
            || cur.x < region.left
            || cur.x > region.right
        {
            return;
        }

        let start_y = self.screen().cursor.y;
        let left_right = region.left > 0 || region.right < self.cols - 1;
        let rem = (region.bottom - self.screen().cursor.y + 1) as usize;
        let adjusted_count = count.min(rem);

        // Track a pin at the bottom of the region.
        // SAFETY: cursor pin live; down(rem-1) valid within the region.
        let start_pin = unsafe { (*self.screen().cursor.page_pin).down(rem - 1).unwrap() };
        let cur_p = self.screen_mut().pages.track_pin(start_pin);

        let mut y = rem;
        // Traverse from the bottom up.
        while y > 0 {
            // SAFETY: cur_p is a live tracked pin.
            let (cur_row, cur_node) = unsafe { ((*cur_p).row_and_cell().0, (*cur_p).node) };

            if y > adjusted_count {
                // SAFETY: cur_p live; up(adjusted_count) valid.
                let off_p = unsafe { (*cur_p).up(adjusted_count).unwrap() };
                self.shift_row(cur_p, cur_row, cur_node, off_p, left_right);
            } else {
                // Clear the shifted-out row.
                // SAFETY: cur_p live; page/row live.
                unsafe {
                    let page: *mut Page = self.screen().pages.node_page_ptr(cur_node);
                    self.row_will_be_shifted(page, cur_row);
                    self.screen().clear_cells_page(
                        page,
                        cur_row,
                        region.left as usize,
                        region.right as usize + 1,
                    );
                }
            }

            // SAFETY: cur_p live.
            unsafe {
                (*cur_p).mark_dirty();
            }
            y -= 1;
            // Move the pin up.
            // SAFETY: cur_p live.
            if let Some(p) = unsafe { (*cur_p).up(1) } {
                unsafe {
                    *cur_p = p;
                }
            }
        }

        self.screen_mut().pages.untrack_pin(cur_p);

        // Return the cursor to the left margin on the starting row.
        let left = region.left;
        self.screen_mut().cursor_absolute(left, start_y);
        self.screen_mut().cursor.pending_wrap = false;
    }

    /// DL: delete `count` lines from the cursor row, shifting the region below
    /// up. Port of `deleteLines`.
    pub fn delete_lines(&mut self, count: usize) {
        if count == 0 {
            return;
        }
        let region = self.scrolling_region;
        let cur = &self.screen().cursor;
        if cur.y < region.top
            || cur.y > region.bottom
            || cur.x < region.left
            || cur.x > region.right
        {
            return;
        }

        let start_y = self.screen().cursor.y;
        let left_right = region.left > 0 || region.right < self.cols - 1;
        let rem = (region.bottom - self.screen().cursor.y + 1) as usize;
        let adjusted_count = count.min(rem);

        // SAFETY: cursor pin live.
        let start_pin = unsafe { *self.screen().cursor.page_pin };
        let cur_p = self.screen_mut().pages.track_pin(start_pin);

        let mut y = 0usize;
        // Traverse from the top down.
        while y < rem {
            // SAFETY: cur_p live.
            let (cur_row, cur_node) = unsafe { ((*cur_p).row_and_cell().0, (*cur_p).node) };

            if y < rem - adjusted_count {
                // SAFETY: cur_p live; down(adjusted_count) valid.
                let off_p = unsafe { (*cur_p).down(adjusted_count).unwrap() };
                self.shift_row(cur_p, cur_row, cur_node, off_p, left_right);
            } else {
                // SAFETY: cur_p live; page/row live.
                unsafe {
                    let page: *mut Page = self.screen().pages.node_page_ptr(cur_node);
                    self.row_will_be_shifted(page, cur_row);
                    self.screen().clear_cells_page(
                        page,
                        cur_row,
                        region.left as usize,
                        region.right as usize + 1,
                    );
                }
            }

            // SAFETY: cur_p live.
            unsafe {
                (*cur_p).mark_dirty();
            }
            y += 1;
            // SAFETY: cur_p live.
            if let Some(p) = unsafe { (*cur_p).down(1) } {
                unsafe {
                    *cur_p = p;
                }
            }
        }

        self.screen_mut().pages.untrack_pin(cur_p);

        let left = region.left;
        self.screen_mut().cursor_absolute(left, start_y);
        self.screen_mut().cursor.pending_wrap = false;
    }

    /// Shift a row's cells from `off_p`/`off_row` into `cur_p`/`cur_row`,
    /// handling same-page (swap/move) and cross-page (clone) cases. Shared inner
    /// loop of insert/delete lines.
    fn shift_row(
        &mut self,
        cur_p: *mut Pin,
        cur_row: *mut crate::page::Row,
        cur_node: *mut crate::pagelist::Node,
        off_p: Pin,
        left_right: bool,
    ) {
        let region = self.scrolling_region;
        // SAFETY: pins/rows live throughout.
        unsafe {
            let (off_row, off_node) = (off_p.row_and_cell().0, off_p.node);

            let cur_page: *mut Page = self.screen().pages.node_page_ptr(cur_node);
            let off_page: *mut Page = self.screen().pages.node_page_ptr(off_node);
            self.row_will_be_shifted(cur_page, cur_row);
            self.row_will_be_shifted(off_page, off_row);

            // Full-width region: unset wrap flags on both rows.
            if !left_right {
                (*off_row).set_wrap(false);
                (*cur_row).set_wrap(false);
                (*off_row).set_wrap_continuation(false);
                (*cur_row).set_wrap_continuation(false);
            }

            // src = off, dst = cur.
            if off_node != cur_node {
                // Cross-page: clone the (partial) row.
                // NB: our clone_partial_row_from grows capacity internally on
                // error paths where needed; port keeps it simple here.
                let _ = (*cur_page).clone_partial_row_from(
                    off_page,
                    cur_row,
                    off_row,
                    region.left as usize,
                    region.right as usize + 1,
                );
            } else if !left_right {
                // Same page, full width: swap the whole Row structs.
                std::ptr::swap(cur_row, off_row);
                (*cur_page).assert_integrity();
            } else {
                // Same page, l/r margins: move the cell range.
                (*cur_page).move_cells(
                    off_row,
                    region.left as usize,
                    cur_row,
                    region.left as usize,
                    (region.right - region.left) as usize + 1,
                );
            }
            let _ = cur_p;
        }
    }

    // ---- insert / delete / erase chars ---------------------------------

    /// ICH: insert `count` blank cells at the cursor, shifting cells right. Port
    /// of `insertBlanks`.
    pub fn insert_blanks(&mut self, count: usize) {
        // Unset pending wrap BEFORE the region check (matches xterm/upstream).
        self.screen_mut().cursor.pending_wrap = false;
        if count == 0 {
            return;
        }
        let region = self.scrolling_region;
        if self.screen().cursor.x < region.left || self.screen().cursor.x > region.right {
            return;
        }

        use crate::page::Wide;
        let cursor_x = self.screen().cursor.x;
        // SAFETY: cursor page/row/cell live throughout.
        unsafe {
            let page: *mut Page = self.screen().cursor_page();
            let row = self.screen().cursor.page_row;
            let left: *mut Cell = self.screen().cursor.page_cell;

            // Wide spacer tail at cursor: erase the previous cell too.
            if (*self.screen().cursor.page_cell).wide() == Wide::SpacerTail {
                debug_assert!(cursor_x > 0);
                self.screen().clear_cells_page(
                    page,
                    row,
                    cursor_x as usize - 1,
                    cursor_x as usize + 1,
                );
            }

            let rem = (region.right - cursor_x + 1) as usize;

            // If the cell at the right margin is wide, its spacer tail is
            // outside the region; clear both halves up front.
            {
                let right_cell = left.add(rem - 1);
                if (*right_cell).wide() == Wide::Wide {
                    let rx = cursor_x as usize + rem - 1;
                    self.screen().clear_cells_page(page, row, rx, rx + 2);
                }
            }

            let adjusted_count = count.min(rem);
            let scroll_amount = rem - adjusted_count;
            if scroll_amount > 0 {
                (*page).pause_integrity_checks(true);

                // If the last cell we're shifting is wide, clear it.
                let end_idx = cursor_x as usize + scroll_amount - 1;
                let end = left.add(scroll_amount - 1);
                if (*end).wide() == Wide::Wide {
                    debug_assert!((*end.add(1)).wide() == Wide::SpacerTail);
                    self.screen()
                        .clear_cells_page(page, row, end_idx, end_idx + 2);
                }

                // Work backwards so we don't overwrite data.
                let mut xi = scroll_amount as isize - 1;
                while xi >= 0 {
                    let src = left.add(xi as usize);
                    let dst = left.add(xi as usize + adjusted_count);
                    (*page).swap_cells(src, dst);
                    xi -= 1;
                }
                (*page).pause_integrity_checks(false);
            }

            // Insert the blanks (preserving bg color).
            self.screen().clear_cells_page(
                page,
                row,
                cursor_x as usize,
                cursor_x as usize + adjusted_count,
            );
        }

        self.screen_mut().cursor_mark_dirty();
    }

    /// DCH: delete `count` cells at the cursor, shifting cells left. Port of
    /// `deleteChars`.
    pub fn delete_chars(&mut self, count_req: usize) {
        if count_req == 0 {
            return;
        }
        let region = self.scrolling_region;
        if self.screen().cursor.x < region.left || self.screen().cursor.x > region.right {
            return;
        }

        let cursor_x = self.screen().cursor.x;
        let rem = (region.right - cursor_x + 1) as usize;
        let count = count_req.min(rem);

        self.screen_mut().split_cell_boundary(cursor_x);
        self.screen_mut()
            .split_cell_boundary(cursor_x + count as CellCountInt);
        self.screen_mut().split_cell_boundary(region.right + 1);

        let scroll_amount = rem - count;
        // SAFETY: cursor page/row/cell live throughout.
        unsafe {
            let page: *mut Page = self.screen().cursor_page();
            let row = self.screen().cursor.page_row;
            let left: *mut Cell = self.screen().cursor.page_cell;

            let mut clear_start = cursor_x as usize;
            if scroll_amount > 0 {
                (*page).pause_integrity_checks(true);
                for xi in 0..scroll_amount {
                    let src = left.add(xi + count);
                    let dst = left.add(xi);
                    (*page).swap_cells(src, dst);
                }
                (*page).pause_integrity_checks(false);
                clear_start = cursor_x as usize + scroll_amount;
            }

            // Clear the vacated cells (preserving bg).
            self.screen()
                .clear_cells_page(page, row, clear_start, cursor_x as usize + rem);
        }

        self.screen_mut().cursor_reset_wrap();
        self.screen_mut().cursor_mark_dirty();
    }

    /// ECH: erase `count` cells at the cursor. Port of `eraseChars`.
    pub fn erase_chars(&mut self, count_req: usize) {
        use crate::page::Wide;
        let cursor_x = self.screen().cursor.x;
        let remaining = self.cols - cursor_x;
        let mut count = remaining.min((count_req.max(1)) as CellCountInt);

        // If our last cell is wide we must also clear the cell beyond it.
        if count != remaining {
            // SAFETY: cursor_cell_right in bounds since count < remaining.
            let last = unsafe { self.screen().cursor_cell_right(count - 1) };
            if unsafe { (*last).wide() } == Wide::Wide {
                count += 1;
            }
        }

        self.screen_mut().split_cell_boundary(cursor_x);
        self.screen_mut().split_cell_boundary(cursor_x + count);
        self.screen_mut().cursor_reset_wrap();
        self.screen_mut().cursor_mark_dirty();

        // SAFETY: cursor page/row live.
        unsafe {
            let page: *mut Page = self.screen().cursor_page();
            let row = self.screen().cursor.page_row;
            if self.screen().protected_mode != ProtectedMode::Iso {
                self.screen().clear_cells_page(
                    page,
                    row,
                    cursor_x as usize,
                    (cursor_x + count) as usize,
                );
            } else {
                self.screen().clear_unprotected_cells_page(
                    page,
                    row,
                    cursor_x as usize,
                    (cursor_x + count) as usize,
                );
            }
        }
    }

    /// EL: erase the line. Port of `eraseLine`.
    pub fn erase_line(&mut self, mode: crate::csi::EraseLine, protected_req: bool) {
        use crate::csi::EraseLine;
        use crate::page::Wide;

        let cursor_x = self.screen().cursor.x;
        let (start, end): (usize, usize) = match mode {
            EraseLine::Right => {
                let mut x = cursor_x;
                // Wide spacer tail at cursor: erase the previous cell too.
                // SAFETY: cursor cell live.
                if x > 0 && unsafe { (*self.screen().cursor.page_cell).wide() } == Wide::SpacerTail
                {
                    x -= 1;
                }
                self.screen_mut().cursor_reset_wrap();
                (x as usize, self.cols as usize)
            }
            EraseLine::Left => {
                let mut x = cursor_x;
                // SAFETY: cursor cell live.
                if unsafe { (*self.screen().cursor.page_cell).wide() } == Wide::Wide {
                    x += 1;
                }
                (0, x as usize + 1)
            }
            EraseLine::Complete => (0, self.cols as usize),
            EraseLine::RightUnlessPendingWrap | EraseLine::Other(_) => {
                // Not reachable from the CSI dispatch (a stream concern); match
                // upstream's "unimplemented erase line mode" no-op.
                return;
            }
        };

        self.screen_mut().cursor.pending_wrap = false;
        self.screen_mut().cursor_mark_dirty();

        let protected = self.screen().protected_mode == ProtectedMode::Iso || protected_req;

        // SAFETY: cursor page/row live.
        unsafe {
            let page: *mut Page = self.screen().cursor_page();
            let row = self.screen().cursor.page_row;
            if !protected {
                self.screen().clear_cells_page(page, row, start, end);
            } else {
                self.screen()
                    .clear_unprotected_cells_page(page, row, start, end);
            }
        }
    }

    /// ED: erase the display. Port of `eraseDisplay`.
    pub fn erase_display(&mut self, mode: crate::csi::EraseDisplay, protected_req: bool) {
        use crate::csi::{EraseDisplay, EraseLine};
        use crate::point::Point;

        let protected = self.screen().protected_mode == ProtectedMode::Iso || protected_req;

        match mode {
            EraseDisplay::ScrollComplete => {
                self.screen_mut().scroll_clear();
                self.screen_mut().cursor.pending_wrap = false;
            }
            EraseDisplay::Complete => {
                // On the primary screen, if the last non-empty row is a prompt,
                // do a scroll_clear first (^L-at-prompt heuristic, see #905).
                if self.screens.active_key() == ScreenKey::Primary {
                    // Walk the active area from the bottom up; the first row we
                    // hit decides: a prompt row means we're at a prompt (Zig's
                    // `while ... else` where both arms break on the first row).
                    // SAFETY: pins live.
                    let at_prompt = unsafe {
                        let mut it = self.screen().pages.row_iterator(
                            crate::pagelist::Direction::LeftUp,
                            Point::active(0, 0),
                            None,
                        );
                        match it.next() {
                            Some(p) => matches!(
                                (*p.row_and_cell().0).semantic_prompt(),
                                RowSemanticPrompt::Prompt | RowSemanticPrompt::PromptContinuation
                            ),
                            None => false,
                        }
                    };
                    if at_prompt {
                        self.screen_mut().scroll_clear();
                    }
                }

                self.screen_mut()
                    .clear_rows(Point::active(0, 0), None, protected);
                self.screen_mut().cursor.pending_wrap = false;
                self.flags.dirty.clear = true;
            }
            EraseDisplay::Below => {
                // All cells to the right (including the cursor), then all rows
                // below.
                self.erase_line(EraseLine::Right, protected_req);
                let y = self.screen().cursor.y;
                if y + 1 < self.rows {
                    self.screen_mut()
                        .clear_rows(Point::active(0, (y + 1) as u32), None, protected);
                }
                debug_assert!(!self.screen().cursor.pending_wrap);
            }
            EraseDisplay::Above => {
                // Erase to the left (including the cursor), then all rows above.
                self.erase_line(EraseLine::Left, protected_req);
                let y = self.screen().cursor.y;
                if y > 0 {
                    self.screen_mut().clear_rows(
                        Point::active(0, 0),
                        Some(Point::active(0, (y - 1) as u32)),
                        protected,
                    );
                }
                debug_assert!(!self.screen().cursor.pending_wrap);
            }
            EraseDisplay::Scrollback => {
                self.screen_mut().erase_history(None);
            }
        }
    }

    // ---- DECALN --------------------------------------------------------

    /// DECALN: reset margins and fill the whole screen with 'E'. Port of `decaln`.
    pub fn decaln(&mut self) {
        use crate::page::{Cell as PCell, ContentTag};

        // Clear stylistic attributes but keep fg/bg colors.
        let old_style = self.screen().cursor.style;
        let new_style = Style {
            bg_color: old_style.bg_color,
            fg_color: old_style.fg_color,
            ..Style::default()
        };
        self.screen_mut().cursor.style = new_style;
        if self.screen_mut().manual_style_update().is_err() {
            self.screen_mut().cursor.style = old_style;
            let _ = self.screen_mut().manual_style_update();
            return;
        }

        // Reset margins and origin, move to top-left.
        self.scrolling_region = ScrollingRegion {
            top: 0,
            bottom: self.rows - 1,
            left: 0,
            right: self.cols - 1,
        };
        self.modes.set(Mode::Origin, false);
        self.set_cursor_pos(1, 1);

        // clearRows (NOT eraseDisplay: do not respect protected attrs).
        self.screen_mut()
            .clear_rows(crate::point::Point::active(0, 0), None, false);

        // Fill with 'E' by moving the cursor down row by row.
        loop {
            let style_id = self.screen().cursor.style_id;
            // SAFETY: cursor page/row live.
            unsafe {
                let page: *mut Page = self.screen().cursor_page();
                let row = self.screen().cursor.page_row;
                let cols = (*page).size.cols as usize;
                let base = (*row).cells().ptr((*page).mem());
                let mut cell = PCell::default();
                cell.set_content_tag(ContentTag::Codepoint);
                cell.set_codepoint('E' as u32);
                cell.set_style_id(style_id);
                for x in 0..cols {
                    base.add(x).write(cell);
                }
                if style_id != style::DEFAULT_ID {
                    let mem = (*page).memory_mut();
                    (*page).styles().use_multiple(mem, style_id, cols as u16);
                    (*row).set_styled(true);
                }
                (*page).assert_integrity();
            }
            self.screen_mut().cursor_mark_dirty();
            if self.screen().cursor.y == self.rows - 1 {
                break;
            }
            self.screen_mut().cursor_down(1);
        }

        self.set_cursor_pos(1, 1);
    }

    // ---- SGR -----------------------------------------------------------

    /// Set a style attribute (SGR). Port of `setAttribute` (Terminal + Screen).
    pub fn set_attribute(&mut self, attr: crate::sgr::Attribute) {
        use crate::page::style::{Color, Underline as StyleUnderline};
        use crate::sgr::Attribute;

        let old_style = self.screen().cursor.style;

        // Map the underline (sgr) enum to the (style) enum by discriminant.
        let map_underline = |u: crate::sgr::Underline| -> StyleUnderline {
            match u {
                crate::sgr::Underline::None => StyleUnderline::None,
                crate::sgr::Underline::Single => StyleUnderline::Single,
                crate::sgr::Underline::Double => StyleUnderline::Double,
                crate::sgr::Underline::Curly => StyleUnderline::Curly,
                crate::sgr::Underline::Dotted => StyleUnderline::Dotted,
                crate::sgr::Underline::Dashed => StyleUnderline::Dashed,
            }
        };

        {
            let style = &mut self.screen_mut().cursor.style;
            match attr {
                Attribute::Unset => *style = Style::default(),
                Attribute::Bold => style.flags.bold = true,
                Attribute::ResetBold => {
                    // Bold and faint share the reset SGR code.
                    style.flags.bold = false;
                    style.flags.faint = false;
                }
                Attribute::Italic => style.flags.italic = true,
                Attribute::ResetItalic => style.flags.italic = false,
                Attribute::Faint => style.flags.faint = true,
                Attribute::Underline(v) => style.flags.underline = map_underline(v),
                Attribute::UnderlineColor(rgb) => {
                    style.underline_color = Color::Rgb(rgb);
                }
                Attribute::UnderlineColor256(idx) => {
                    style.underline_color = Color::Palette(idx);
                }
                Attribute::ResetUnderlineColor => style.underline_color = Color::None,
                Attribute::Overline => style.flags.overline = true,
                Attribute::ResetOverline => style.flags.overline = false,
                Attribute::Blink => style.flags.blink = true,
                Attribute::ResetBlink => style.flags.blink = false,
                Attribute::Inverse => style.flags.inverse = true,
                Attribute::ResetInverse => style.flags.inverse = false,
                Attribute::Invisible => style.flags.invisible = true,
                Attribute::ResetInvisible => style.flags.invisible = false,
                Attribute::Strikethrough => style.flags.strikethrough = true,
                Attribute::ResetStrikethrough => style.flags.strikethrough = false,
                Attribute::DirectColorFg(rgb) => style.fg_color = Color::Rgb(rgb),
                Attribute::DirectColorBg(rgb) => style.bg_color = Color::Rgb(rgb),
                Attribute::Fg8(n) => style.fg_color = Color::Palette(n as u8),
                Attribute::Bg8(n) => style.bg_color = Color::Palette(n as u8),
                Attribute::ResetFg => style.fg_color = Color::None,
                Attribute::ResetBg => style.bg_color = Color::None,
                Attribute::Fg8Bright(n) => style.fg_color = Color::Palette(n as u8),
                Attribute::Bg8Bright(n) => style.bg_color = Color::Palette(n as u8),
                Attribute::Fg256(idx) => style.fg_color = Color::Palette(idx),
                Attribute::Bg256(idx) => style.bg_color = Color::Palette(idx),
                Attribute::Unknown(_) => return,
            }
        }

        // If the style didn't change, our style id is already correct.
        if self.screen().cursor.style == old_style {
            return;
        }

        if self.screen_mut().manual_style_update().is_err() {
            // Revert to the old style; if that fails, revert to default.
            self.screen_mut().cursor.style = old_style;
            if self.screen_mut().manual_style_update().is_err() {
                self.screen_mut().cursor.style = Style::default();
                let _ = self.screen_mut().manual_style_update();
            }
        }
    }

    // ---- alt-screen / reset --------------------------------------------

    /// Switch the active screen (primary↔alternate). Copies charset, clears
    /// selection, ends hyperlink state on the old screen. Returns the previous
    /// active key if a switch happened. Port of `switchScreen`.
    pub fn switch_screen(&mut self, key: ScreenKey) -> Option<ScreenKey> {
        if self.screens.active_key() == key {
            return None;
        }
        let old_key = self.screens.active_key();

        // End hyperlink state on the current (old) screen.
        self.screen_mut().end_hyperlink();
        let old_charset = self.screen().charset;

        // Ensure the target screen exists.
        let new = self.screens.get_init(key);
        debug_assert_eq!(new.cursor.hyperlink_id, 0);
        new.charset = old_charset;
        new.clear_selection();
        new.kitty_images_dirty = true;

        self.flags.dirty.clear = true;
        self.screens.switch_to(key);
        Some(old_key)
    }

    /// Switch screens via a DEC mode (47/1047/1049) with its clear/save/restore
    /// semantics. Port of `switchScreenMode`.
    pub fn switch_screen_mode(&mut self, mode: SwitchScreenMode, enabled: bool) {
        use crate::csi::{EraseDisplay, EraseLine};
        let _ = EraseLine::Right; // silence unused import if path unused

        match mode {
            SwitchScreenMode::M47 => {}
            SwitchScreenMode::M1047 => {
                // Disabling 1047 while on alt: clear the alt screen.
                if !enabled && self.screens.active_key() == ScreenKey::Alternate {
                    self.erase_display(EraseDisplay::Complete, false);
                }
            }
            SwitchScreenMode::M1049 => {
                // Enabling 1049 always saves the cursor.
                if enabled {
                    self.save_cursor();
                }
            }
        }

        let to = if enabled {
            ScreenKey::Alternate
        } else {
            ScreenKey::Primary
        };
        let old = self.switch_screen(to);

        match mode {
            SwitchScreenMode::M47 | SwitchScreenMode::M1047 => {
                if let Some(old_key) = old {
                    let src = self.screens.get(old_key).unwrap().cursor.to_copy();
                    self.screen_mut().cursor_copy(&src);
                }
            }
            SwitchScreenMode::M1049 => {
                if enabled {
                    debug_assert_eq!(self.screens.active_key(), ScreenKey::Alternate);
                    self.erase_display(EraseDisplay::Complete, false);
                    if let Some(old_key) = old {
                        let src = self.screens.get(old_key).unwrap().cursor.to_copy();
                        self.screen_mut().cursor_copy(&src);
                    }
                } else {
                    debug_assert_eq!(self.screens.active_key(), ScreenKey::Primary);
                    self.restore_cursor();
                }
            }
        }
    }

    /// Full reset (RIS). Port of `fullReset`.
    pub fn full_reset(&mut self) {
        self.screens.switch_to(ScreenKey::Primary);
        self.screens.remove(ScreenKey::Alternate);
        self.screen_mut().reset();

        self.modes.reset();
        self.flags = Flags::default();
        self.tabstops.reset(TABSTOP_INTERVAL);
        self.previous_char = None;
        self.pwd.clear();
        self.title.clear();
        self.status_display = StatusDisplay::Main;
        self.scrolling_region = ScrollingRegion {
            top: 0,
            bottom: self.rows - 1,
            left: 0,
            right: self.cols - 1,
        };
        self.flags.dirty.clear = true;
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
    ///
    /// `\n` is special-cased to carriage-return + linefeed, matching Zig's
    /// `printString` (it is not simply passed through to `print`, which
    /// would treat it as a printable codepoint).
    pub fn print_string(&mut self, s: &str) {
        for c in s.chars() {
            if c == '\n' {
                self.carriage_return();
                self.linefeed();
            } else {
                self.print(c as u32);
            }
        }
    }

    /// Repeat the previously-printed character `count` times (REP, `CSI b`).
    /// A no-op if nothing has been printed yet. `count` is clamped to a minimum
    /// of 1. Port of `printRepeat`.
    pub fn print_repeat(&mut self, count_req: usize) {
        if let Some(c) = self.previous_char {
            let count = count_req.max(1);
            for _ in 0..count {
                self.print(c);
            }
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

    // ---- resize / deccolm ---------------------------------------------------

    /// Resize the underlying terminal. Port of `resize`.
    pub fn resize(&mut self, cols: CellCountInt, rows: CellCountInt) {
        // If our cols/rows didn't change then we're done.
        if self.cols == cols && self.rows == rows {
            return;
        }

        // Resize our tabstops.
        if self.cols != cols {
            self.tabstops = Tabstops::new(cols as usize, TABSTOP_INTERVAL);
        }

        let redraw = self.flags.shell_redraws_prompt;
        let reflow = self.modes.get(Mode::Wraparound);

        // Resize primary screen, which supports reflow.
        self.screens.get_init(ScreenKey::Primary).resize(Resize {
            cols,
            rows,
            reflow,
            prompt_redraw: redraw,
        });

        // Alternate screen, if it exists, doesn't reflow.
        if self.screens.get(ScreenKey::Alternate).is_some() {
            self.screens.get_init(ScreenKey::Alternate).resize(Resize {
                cols,
                rows,
                reflow: false,
                prompt_redraw: redraw,
            });
        }

        // Whenever we resize we just mark it as a screen clear.
        self.flags.dirty.clear = true;

        // Set our size.
        self.cols = cols;
        self.rows = rows;

        // Reset the scrolling region.
        self.scrolling_region = ScrollingRegion {
            top: 0,
            bottom: rows - 1,
            left: 0,
            right: cols - 1,
        };
    }

    /// DECCOLM: switch between 80 and 132 columns. Port of `deccolm`.
    ///
    /// `cols132` selects 132-column mode when true, 80-column when false.
    pub fn deccolm(&mut self, cols132: bool) {
        // If DEC mode 40 (enable_mode_3) isn't enabled, then this is
        // ignored. We also make sure we don't have deccolm set because we
        // want to fully ignore set mode.
        if !self.modes.get(Mode::EnableMode3) {
            self.modes.set(Mode::Column132, false);
            return;
        }

        // Enable it.
        self.modes.set(Mode::Column132, cols132);

        // Resize to the requested size.
        let new_cols: CellCountInt = if cols132 { 132 } else { 80 };
        self.resize(new_cols, self.rows);

        // Erase our display and move our cursor.
        self.erase_display(crate::csi::EraseDisplay::Complete, false);
        self.set_cursor_pos(1, 1);
    }

    // ---- OSC 133 semantic prompt --------------------------------------------

    /// OSC 133 dispatch. Port of `semanticPrompt`.
    pub fn semantic_prompt(&mut self, cmd: &crate::osc::SemanticPrompt) {
        use crate::osc::SemanticPromptAction as A;
        use crate::screen::SemanticContentSet;

        match cmd.action {
            A::FreshLine => self.semantic_prompt_fresh_line(),

            A::FreshLineNewPrompt => {
                // "First do a fresh-line."
                self.semantic_prompt_fresh_line();

                // "Subsequent text is a prompt string."
                let kind = cmd.prompt_kind().unwrap_or(crate::osc::PromptKind::Initial);
                self.screen_mut()
                    .cursor_set_semantic_content(SemanticContentSet::Prompt(kind));

                // Kitty extension: the shell may not be capable of redraw.
                if let Some(v) = cmd.redraw() {
                    self.flags.shell_redraws_prompt = v;
                }

                // Handle click_events as priority over cl.
                use crate::screen::semantic::SemanticClick;
                if let Some(v) = cmd.click_events() {
                    self.screen_mut().semantic_prompt.click = SemanticClick::ClickEvents(v);
                } else if let Some(v) = cmd.cl() {
                    self.screen_mut().semantic_prompt.click = SemanticClick::Cl(v);
                }
            }

            A::NewCommand => {
                // Same as `A` for our purposes (no explicit command tracking).
                self.semantic_prompt(&crate::osc::SemanticPrompt {
                    action: A::FreshLineNewPrompt,
                    options_unvalidated: cmd.options_unvalidated.clone(),
                });
            }

            A::PromptStart => {
                let kind = cmd.prompt_kind().unwrap_or(crate::osc::PromptKind::Initial);
                self.screen_mut()
                    .cursor_set_semantic_content(SemanticContentSet::Prompt(kind));
            }

            A::EndPromptStartInput => {
                self.screen_mut()
                    .cursor_set_semantic_content(SemanticContentSet::Input { clear_eol: false });
            }

            A::EndPromptStartInputTerminateEol => {
                self.screen_mut()
                    .cursor_set_semantic_content(SemanticContentSet::Input { clear_eol: true });
            }

            A::EndInputStartOutput => {
                self.screen_mut()
                    .cursor_set_semantic_content(SemanticContentSet::Output);

                // fish heuristic: if the current row is a prompt and we're at
                // col 0, assume we're overwriting the prompt.
                let screen = self.screen_mut();
                let at_col0 = screen.cursor.x == 0;
                let row_is_prompt = unsafe {
                    (*screen.cursor.page_row).semantic_prompt() != crate::page::SemanticPrompt::None
                };
                if row_is_prompt && at_col0 {
                    unsafe {
                        (*screen.cursor.page_row)
                            .set_semantic_prompt(crate::page::SemanticPrompt::None);
                    }
                }
            }

            A::EndCommand => {
                self.screen_mut()
                    .cursor_set_semantic_content(SemanticContentSet::Output);
            }
        }
    }

    /// OSC 133;L. Port of `semanticPromptFreshLine`.
    fn semantic_prompt_fresh_line(&mut self) {
        let left_margin = if self.screen().cursor.x < self.scrolling_region.left {
            0
        } else {
            self.scrolling_region.left
        };

        if self.screen().cursor.x == left_margin {
            return;
        }

        self.carriage_return();
        self.index();
    }

    /// Returns true if the cursor is currently at a prompt. Port of
    /// `cursorIsAtPrompt`.
    pub fn cursor_is_at_prompt(&self) -> bool {
        // On the alternate screen we're never at a prompt.
        if self.screens.active_key() == ScreenKey::Alternate {
            return false;
        }

        let cursor = &self.screen().cursor;
        if unsafe { (*cursor.page_row).semantic_prompt() != crate::page::SemanticPrompt::None } {
            return true;
        }

        matches!(
            cursor.semantic_content,
            crate::page::SemanticContent::Input | crate::page::SemanticContent::Prompt
        )
    }

    // ---- DECRQSS (printAttributes) ------------------------------------------

    /// DECRQSS SGR-request reply body. Port of `printAttributes`: returns the
    /// numeric SGR parameter string (the part between `\eP1$r` and `m\e\\`)
    /// describing the current cursor style. Always starts with `0`
    /// (see <https://vt100.net/docs/vt510-rm/DECRPSS>).
    pub fn print_attributes(&self) -> String {
        use crate::page::style::{Color, Underline};
        use std::fmt::Write;

        let pen = self.screen().cursor.style;
        // The SGR response always starts with a 0.
        let mut out = String::from("0");

        if pen.flags.bold {
            out.push_str(";1");
        }
        if pen.flags.faint {
            out.push_str(";2");
        }
        if pen.flags.italic {
            out.push_str(";3");
        }
        if pen.flags.underline != Underline::None {
            out.push_str(";4");
        }
        if pen.flags.blink {
            out.push_str(";5");
        }
        if pen.flags.inverse {
            out.push_str(";7");
        }
        if pen.flags.invisible {
            out.push_str(";8");
        }
        if pen.flags.strikethrough {
            out.push_str(";9");
        }

        match pen.fg_color {
            Color::None => {}
            Color::Palette(idx) if idx >= 16 => {
                let _ = write!(out, ";38:5:{idx}");
            }
            Color::Palette(idx) if idx >= 8 => {
                let _ = write!(out, ";9{}", idx - 8);
            }
            Color::Palette(idx) => {
                let _ = write!(out, ";3{idx}");
            }
            Color::Rgb(rgb) => {
                let _ = write!(out, ";38:2::{}:{}:{}", rgb.r, rgb.g, rgb.b);
            }
        }
        match pen.bg_color {
            Color::None => {}
            Color::Palette(idx) if idx >= 16 => {
                let _ = write!(out, ";48:5:{idx}");
            }
            Color::Palette(idx) if idx >= 8 => {
                let _ = write!(out, ";10{}", idx - 8);
            }
            Color::Palette(idx) => {
                let _ = write!(out, ";4{idx}");
            }
            Color::Rgb(rgb) => {
                let _ = write!(out, ";48:2::{}:{}:{}", rgb.r, rgb.g, rgb.b);
            }
        }

        out
    }
}

#[cfg(test)]
mod tests;
