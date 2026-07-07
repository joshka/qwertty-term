//! The print path. Port of `Terminal.print` / `printCell` / `printWrap`
//! (`Terminal.zig:740-1301`).
//!
//! SCOPE: this ports the **non-grapheme-clustering** print path faithfully —
//! width computation (via `crate::unicode::codepoint_width`, the same tables
//! upstream uses), charset/single-shift mapping, soft-wrap, insert mode,
//! narrow/wide/spacer handling, and zero-width grapheme attach. The mode-2027
//! `grapheme_cluster` clustering block (`Terminal.zig:764-949`) is a marked
//! seam — see `TODO(chunk:terminal-print-grapheme)`.

use crate::charsets::{self, Charset};
use crate::page::size::CellCountInt;
use crate::page::style::DEFAULT_ID;
use crate::page::{Cell, ContentTag, SemanticContent, Wide};

use super::Terminal;
use crate::modes::Mode;

impl Terminal {
    /// Print a single codepoint. Port of `print`.
    pub fn print(&mut self, c: u32) {
        // If we're not on the main display, drop the char.
        if self.status_display != super::StatusDisplay::Main {
            return;
        }

        // Right margin depends on where the cursor is now.
        let right_limit = if self.screen().cursor.x > self.scrolling_region.right {
            self.cols
        } else {
            self.scrolling_region.right + 1
        };

        // Grapheme clustering (mode 2027). Ordered least-likely-first so we can
        // drop out fast. Port of `Terminal.zig:763-955`.
        if c > 255
            && self.modes.get(Mode::GraphemeCluster)
            && self.screen().cursor.x > 0
            && self.print_grapheme(c, right_limit)
        {
            return;
        }

        // Width: fast path for byte-sized chars.
        let width: usize = if c <= 0xFF {
            1
        } else {
            crate::unicode::codepoint_width(c) as usize
        };
        debug_assert!(width <= 2);

        // Zero-width: attach as grapheme to the previous cell.
        if width == 0 {
            self.print_zero_width(c);
            return;
        }

        // Printable char: save it for REP.
        self.previous_char = Some(c);

        // Soft-wrap first if pending.
        if self.screen().cursor.pending_wrap && self.modes.get(Mode::Wraparound) {
            self.print_wrap();
        }

        // Insert mode: shift cells right if not at EOL.
        if self.modes.get(Mode::Insert)
            && (self.screen().cursor.x as usize + width) < self.cols as usize
        {
            self.insert_blanks(width);
        }

        match width {
            1 => {
                self.screen_mut().cursor_mark_dirty();
                self.print_cell(c, Wide::Narrow);
            }
            2 => {
                if (right_limit - self.scrolling_region.left) > 1 {
                    if self.screen().cursor.x == right_limit - 1 {
                        // No room for the wide char at the edge.
                        if !self.modes.get(Mode::Wraparound) {
                            return;
                        }
                        if right_limit == self.cols {
                            // SAFETY: cursor row live.
                            unsafe {
                                (*self.screen().cursor.page_row).set_wrap(true);
                            }
                            self.print_cell(0, Wide::SpacerHead);
                        } else {
                            self.print_cell(0, Wide::Narrow);
                        }
                        self.print_wrap();
                    }
                    self.screen_mut().cursor_mark_dirty();
                    self.print_cell(c, Wide::Wide);
                    self.screen_mut().cursor_right(1);
                    self.print_cell(0, Wide::SpacerTail);
                } else {
                    // 1-wide terminal; degenerate.
                    self.screen_mut().cursor_mark_dirty();
                    self.print_cell(0, Wide::Narrow);
                }
            }
            _ => unreachable!(),
        }

        // Pending-wrap at the column limit.
        if self.screen().cursor.x == right_limit - 1 {
            self.screen_mut().cursor.pending_wrap = true;
            return;
        }
        self.screen_mut().cursor_right(1);
    }

    /// The mode-2027 grapheme-clustering branch of `print`. Returns `true` if
    /// `c` was consumed (attached to the previous grapheme cluster), `false` if
    /// the caller should fall through to the normal width path. Port of
    /// `Terminal.zig:766-955`.
    fn print_grapheme(&mut self, c: u32, right_limit: CellCountInt) -> bool {
        use crate::unicode::{
            BreakState, GraphemeWidthEffect, grapheme_break, grapheme_width_effect,
        };

        // Determine the previous cell (`prev`) and how far left of the cursor it
        // is (`left`). If we are NOT at a grapheme break, `c` combines with it.
        let wraparound = self.modes.get(Mode::Wraparound);
        // SAFETY: cursor pointers live throughout the block.
        unsafe {
            let left: CellCountInt = if wraparound {
                CellCountInt::from(!self.screen().cursor.pending_wrap)
            } else if self.screen().cursor.x != right_limit - 1 {
                1
            } else {
                CellCountInt::from((*self.screen().cursor.page_cell).codepoint() == 0)
            };

            let immediate = self.screen().cursor_cell_left(left);
            let (mut prev_cell, prev_left) = if (*immediate).wide() == Wide::SpacerTail {
                (self.screen().cursor_cell_left(left + 1), left + 1)
            } else {
                (immediate, left)
            };

            // Empty cell: a grapheme break; fall through.
            if (*prev_cell).codepoint() == 0 {
                return false;
            }

            // Run the grapheme break state machine over prev's cluster + c.
            let mut previous_codepoint = (*prev_cell).codepoint();
            let grapheme_break_result = {
                let mut state = BreakState::Default;
                if (*prev_cell).has_grapheme() {
                    let page = self.screen().cursor_page();
                    if let Some(cps) = (*page).lookup_grapheme(prev_cell) {
                        for &cp2 in &*cps {
                            debug_assert!(!grapheme_break(previous_codepoint, cp2, &mut state));
                            previous_codepoint = cp2;
                        }
                    }
                }
                grapheme_break(previous_codepoint, c, &mut state)
            };

            // A break means c starts a new cell: fall through.
            if grapheme_break_result {
                return false;
            }

            // c is part of the previous grapheme. Apply the width effect.
            match grapheme_width_effect(previous_codepoint, c) {
                GraphemeWidthEffect::Ignore => return true,
                GraphemeWidthEffect::NoChange => {}
                GraphemeWidthEffect::Wide => {
                    if (*prev_cell).wide() != Wide::Wide {
                        // Move the cursor back to the previous cell.
                        self.screen_mut().cursor_left(prev_left);

                        if self.screen().cursor.x == right_limit - 1 {
                            if !wraparound {
                                return true;
                            }
                            let row_wrap = right_limit == self.cols;
                            if row_wrap {
                                (*self.screen().cursor.page_row).set_wrap(true);
                            }

                            let prev_cp = (*prev_cell).codepoint();
                            if (*prev_cell).has_grapheme() {
                                // Like print_cell but keeps the grapheme data so
                                // we can move it after wrapping.
                                (*prev_cell).set_wide(if row_wrap {
                                    Wide::SpacerHead
                                } else {
                                    Wide::Narrow
                                });
                                (*prev_cell).set_codepoint(0);

                                self.print_wrap();
                                self.print_cell(prev_cp, Wide::Wide);

                                let new_pin = *self.screen().cursor.page_pin;
                                let (new_row, new_cell) = new_pin.row_and_cell();

                                // Transfer graphemes from the old cell to the new.
                                if let Some(mut old_pin) = new_pin.up(1) {
                                    old_pin.x = right_limit - 1;
                                    let (old_row, old_cell) = old_pin.row_and_cell();

                                    if new_pin.node == old_pin.node {
                                        let page = self.screen().cursor_page();
                                        (*page).move_grapheme(prev_cell, new_cell);
                                        (*prev_cell).set_content_tag(ContentTag::Codepoint);
                                        (*new_cell).set_content_tag(ContentTag::CodepointGrapheme);
                                        (*new_row).set_grapheme(true);
                                    } else {
                                        let old_page =
                                            self.screen().pages.node_data_mut(old_pin.node);
                                        if let Some(cps) = (*old_page).lookup_grapheme(old_cell) {
                                            let cps: Vec<u32> = (*cps).to_vec();
                                            for cp in cps {
                                                let _ =
                                                    self.screen_mut().append_grapheme(new_cell, cp);
                                            }
                                        }
                                        let old_page =
                                            self.screen().pages.node_data_mut(old_pin.node);
                                        (*old_page).clear_grapheme(old_cell);
                                    }

                                    let old_page = self.screen().pages.node_data_mut(old_pin.node);
                                    (*old_page).update_row_grapheme_flag(old_row);
                                }

                                prev_cell = new_cell;
                            } else {
                                self.print_cell(
                                    0,
                                    if row_wrap {
                                        Wide::SpacerHead
                                    } else {
                                        Wide::Narrow
                                    },
                                );
                                self.print_wrap();
                                self.print_cell(prev_cp, Wide::Wide);
                                prev_cell = self.screen().cursor.page_cell;
                            }
                        } else {
                            (*prev_cell).set_wide(Wide::Wide);
                        }

                        // Write the spacer tail after the (now wide) prev cell.
                        self.screen_mut().cursor_right(1);
                        self.print_cell(0, Wide::SpacerTail);

                        // Advance beyond the spacer.
                        if self.screen().cursor.x == right_limit - 1 {
                            self.screen_mut().cursor.pending_wrap = true;
                        } else {
                            self.screen_mut().cursor_right(1);
                        }
                    }
                }
                GraphemeWidthEffect::Narrow => {
                    if (*prev_cell).wide() == Wide::Wide {
                        (*prev_cell).set_wide(Wide::Narrow);

                        // Remove the wide spacer tail.
                        let tail = self.screen().cursor_cell_left(prev_left - 1);
                        (*tail).set_wide(Wide::Narrow);

                        if self.screen().cursor.x == right_limit - 1 {
                            self.screen_mut().cursor.pending_wrap = false;
                        } else {
                            self.screen_mut().cursor_left(1);
                        }
                    }
                }
            }

            self.screen_mut().cursor_mark_dirty();
            let _ = self.screen_mut().append_grapheme(prev_cell, c);
            true
        }
    }

    /// Attach a zero-width codepoint to the previous cell. Port of the width-0
    /// branch of `print` (`Terminal.zig:962-1012`).
    fn print_zero_width(&mut self, c: u32) {
        // With grapheme clustering we ignore lone zero-width chars.
        if self.modes.get(Mode::GraphemeCluster) {
            return;
        }

        let left: CellCountInt =
            if self.modes.get(Mode::Wraparound) && self.screen().cursor.pending_wrap {
                0
            } else {
                1
            };

        // Malformed: zero-width at col 0 with no prior char.
        if self.screen().cursor.x == 0 && left == 1 {
            return;
        }

        // SAFETY: cursor pointers live; left <= cursor.x by the guard above.
        unsafe {
            let immediate = self.screen().cursor_cell_left(left);
            let prev = if (*immediate).wide() != Wide::SpacerTail {
                immediate
            } else {
                self.screen().cursor_cell_left(left + 1)
            };

            if !(*prev).has_text() {
                return;
            }

            // VS15/VS16 only attach to an emoji base.
            if c == 0xFE0F || c == 0xFE0E {
                let base = (*prev).codepoint();
                if !crate::unicode::properties(base).emoji_vs_base {
                    return;
                }
            }

            let page = self.screen().cursor_page();
            let _ = (*page).append_grapheme(self.screen().cursor.page_row, prev, c);
        }
    }

    /// Write the cursor cell with `unmapped_c` mapped through the active
    /// charset. Port of `printCell`.
    fn print_cell(&mut self, unmapped_c: u32, wide: Wide) {
        // Charset mapping (single-shift wins for one char).
        let mapped: u32 = {
            let key = if let Some(k) = self.screen().charset.single_shift {
                self.screen_mut().charset.single_shift = None;
                k
            } else {
                self.screen().charset.gl
            };
            let set = self.screen().charset.charsets.get(key);
            if set == Charset::Utf8 || set == Charset::Ascii {
                unmapped_c
            } else if unmapped_c > u8::MAX as u32 {
                ' ' as u32
            } else {
                charsets::table(set)[unmapped_c as usize] as u32
            }
        };

        // SAFETY: cursor pointers live throughout.
        unsafe {
            let cell = self.screen().cursor.page_cell;

            // Clear wide-partner cells if the wide property changes.
            if (*cell).wide() != wide {
                self.print_cell_fix_wide(cell);
            }

            // Clear prior grapheme data.
            if (*cell).has_grapheme() {
                let page = self.screen().cursor_page();
                (*page).clear_grapheme(cell);
                (*page).update_row_grapheme_flag(self.screen().cursor.page_row);
            }

            // Release the old style ref if the id changes.
            let cursor_style_id = self.screen().cursor.style_id;
            let style_changed = (*cell).style_id() != cursor_style_id;
            if style_changed && (*cell).style_id() != DEFAULT_ID {
                let page = self.screen().cursor_page();
                let mem = (*page).memory_mut();
                (*page).styles().release(mem, (*cell).style_id());
            }

            let had_hyperlink = (*cell).hyperlink();

            // Write the cell.
            let mut new_cell = Cell::default();
            new_cell.set_content_tag(ContentTag::Codepoint);
            new_cell.set_codepoint(mapped);
            new_cell.set_style_id(cursor_style_id);
            new_cell.set_wide(wide);
            new_cell.set_protected(self.screen().cursor.protected);
            new_cell.set_semantic_content(self.screen().cursor.semantic_content);
            *cell = new_cell;

            // Use the new style ref.
            if style_changed && cursor_style_id != DEFAULT_ID {
                let page = self.screen().cursor_page();
                let mem = (*page).memory_mut();
                (*page).styles().use_id(mem, cursor_style_id);
                (*self.screen().cursor.page_row).set_styled(true);
            }

            // TODO(chunk:kitty-gfx): mark Kitty unicode-placeholder rows.

            // Re-apply the active hyperlink, or clear a stale one.
            if self.screen().cursor.hyperlink_id > 0 {
                let _ = self.screen_mut().cursor_set_hyperlink();
            } else if had_hyperlink {
                let page = self.screen().cursor_page();
                (*page).clear_hyperlink(cell);
                (*page).update_row_hyperlink_flag(self.screen().cursor.page_row);
            }
        }
    }

    /// Clear the wide-partner cells of the target when the wide property is
    /// changing. Port of the `cell.wide != wide` block of `printCell`.
    ///
    /// # Safety
    /// `cell` must be the live cursor cell.
    unsafe fn print_cell_fix_wide(&mut self, cell: *mut Cell) {
        unsafe {
            match (*cell).wide() {
                Wide::Narrow => {}
                Wide::Wide => {
                    if self.screen().cursor.x >= self.cols - 1 {
                        return;
                    }
                    let spacer = self.screen().cursor_cell_right(1);
                    let page = self.screen().cursor_page();
                    let row = self.screen().cursor.page_row;
                    // clear a single cell to the right (the spacer tail).
                    let x = self.screen().cursor.x as usize + 1;
                    self.screen_mut().clear_cells_page(page, row, x, x + 1);
                    let _ = spacer;
                    self.clear_stale_spacer_head();
                }
                Wide::SpacerTail => {
                    debug_assert!(self.screen().cursor.x > 0);
                    // So integrity checks pass while we clear the wide head to
                    // our left; the subsequent print overwrites this cell
                    // anyway. Mirrors Terminal.zig:1166's runtime_safety gate.
                    #[cfg(debug_assertions)]
                    unsafe {
                        (*self.screen_mut().cursor.page_cell).set_wide(Wide::Narrow);
                    }
                    let page = self.screen().cursor_page();
                    let row = self.screen().cursor.page_row;
                    let x = self.screen().cursor.x as usize - 1;
                    self.screen_mut().clear_cells_page(page, row, x, x + 1);
                    self.clear_stale_spacer_head();
                }
                Wide::SpacerHead => {}
            }
        }
    }

    /// Clear a stale spacer_head at the end of the previous row when a wide
    /// char near the left edge is overwritten. Port of the `cursorCellEndOfPrev`
    /// cleanup in `printCell`.
    fn clear_stale_spacer_head(&mut self) {
        if self.screen().cursor.y == 0 || self.screen().cursor.x > 1 {
            return;
        }
        // SAFETY: y > 0, so a previous row exists; end-of-prev is at cols-1.
        unsafe {
            let pin_up = (*self.screen().cursor.page_pin).up(1).unwrap();
            let page = pin_up.page();
            let (row, _) = pin_up.row_and_cell();
            let cells = (*page).get_cells(row) as *mut Cell;
            let end_cell = cells.add((self.cols - 1) as usize);
            if (*end_cell).wide() == Wide::SpacerHead {
                (*end_cell).set_wide(Wide::Narrow);
            }
        }
    }

    /// Soft-wrap to the next line. Port of `printWrap`.
    fn print_wrap(&mut self) {
        let mark_wrap = self.screen().cursor.x == self.cols - 1;
        if mark_wrap {
            // SAFETY: cursor row live.
            unsafe {
                (*self.screen().cursor.page_row).set_wrap(true);
            }
        }

        let old_semantic = self.screen().cursor.semantic_content;
        let old_semantic_clear = self.screen().cursor.semantic_content_clear_eol;

        self.index();
        let left = self.scrolling_region.left;
        self.screen_mut().cursor_horizontal_absolute(left);

        self.screen_mut().cursor.semantic_content = old_semantic;
        self.screen_mut().cursor.semantic_content_clear_eol = old_semantic_clear;
        if old_semantic == SemanticContent::Prompt {
            // SAFETY: cursor row live.
            unsafe {
                (*self.screen().cursor.page_row)
                    .set_semantic_prompt(crate::page::SemanticPrompt::PromptContinuation);
            }
        }

        if mark_wrap {
            // SAFETY: cursor row live.
            unsafe {
                (*self.screen().cursor.page_row).set_wrap_continuation(true);
            }
        }
        self.screen().assert_integrity();
    }
}
