use std::time::Duration;

use eframe::egui::{
    self, Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, Vec2,
    ViewportCommand,
};
use ghostty_rs::{Cell, MouseTracking, Terminal};

use crate::pty::{PtyResult, PtySession};

mod app_shell;
mod font;
mod input;
mod renderer;
mod theme;

use font::TerminalFont;
use input::{encode_key, mouse_button_code};
use renderer::{CellMetrics, logical_cell, paint_exit_status, paint_terminal, selection_range};

pub(crate) fn run_window() -> PtyResult<()> {
    let preferences = app_shell::AppPreferences::load();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("ghostty-rs")
            .with_inner_size([preferences.window_size.x, preferences.window_size.y])
            .with_app_id("net.joshka.ghostty-rs"),
        ..Default::default()
    };

    eframe::run_native(
        "ghostty-rs",
        options,
        Box::new(|cc| {
            let terminal_font = font::configure(&cc.egui_ctx, preferences.font_size);
            Ok(Box::new(WindowTerminal::new(terminal_font, preferences)?))
        }),
    )?;

    Ok(())
}

pub(crate) fn font_report_lines() -> Vec<String> {
    font::font_report_lines()
}

pub(crate) fn render_probe_lines() -> Vec<String> {
    renderer::render_probe_lines()
}

struct WindowTerminal {
    terminal: Terminal,
    pty: PtySession,
    scrollback_offset: usize,
    shown_title: String,
    selection: Option<Selection>,
    pressed_mouse_button: Option<u8>,
    close_requested: bool,
    exit_status: Option<String>,
    terminal_font: TerminalFont,
    show_preferences: bool,
    preferences: app_shell::AppPreferences,
    last_saved_window_size: Vec2,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct CellCoord {
    col: usize,
    logical_row: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Selection {
    anchor: CellCoord,
    active: CellCoord,
}

impl WindowTerminal {
    fn new(terminal_font: TerminalFont, preferences: app_shell::AppPreferences) -> PtyResult<Self> {
        let cols = 100;
        let rows = 30;
        let last_saved_window_size = preferences.window_size;
        Ok(Self {
            terminal: Terminal::new(cols, rows),
            pty: PtySession::spawn(cols as u16, rows as u16)?,
            scrollback_offset: 0,
            shown_title: "ghostty-rs".to_string(),
            selection: None,
            pressed_mouse_button: None,
            close_requested: false,
            exit_status: None,
            terminal_font,
            show_preferences: false,
            preferences,
            last_saved_window_size,
        })
    }

    fn drain_pty(&mut self) -> PtyResult<()> {
        while let Some(bytes) = self.pty.try_read() {
            self.terminal.write(&bytes);
            let response = self.terminal.take_output();
            if !response.is_empty() {
                self.pty.write_all(&response)?;
            }
        }
        self.clamp_scrollback_offset();
        Ok(())
    }

    fn resize_to_rect(&mut self, rect: Rect, metrics: &CellMetrics) -> PtyResult<()> {
        let cols = (rect.width() / metrics.width).floor().max(1.0) as usize;
        let rows = (rect.height() / metrics.height).floor().max(1.0) as usize;
        if cols != self.terminal.cols() || rows != self.terminal.rows() {
            self.terminal.resize(cols, rows);
            self.pty.resize(cols as u16, rows as u16)?;
            self.clamp_scrollback_offset();
        }
        Ok(())
    }

    fn handle_events(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        metrics: &CellMetrics,
    ) -> PtyResult<()> {
        let events = ctx.input(|input| input.events.clone());
        for event in events {
            match event {
                Event::Text(text) => {
                    self.scrollback_offset = 0;
                    self.selection = None;
                    self.pty.write_all(text.as_bytes())?;
                }
                Event::Paste(text) => {
                    self.scrollback_offset = 0;
                    self.selection = None;
                    if self.terminal.bracketed_paste() {
                        self.pty.write_all(b"\x1b[200~")?;
                        self.pty.write_all(text.as_bytes())?;
                        self.pty.write_all(b"\x1b[201~")?;
                    } else {
                        self.pty.write_all(text.as_bytes())?;
                    }
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if app_shell::handle_shortcut(ctx, key, modifiers, &mut self.show_preferences)?
                    {
                        continue;
                    }
                    if self.handle_scrollback_key(key, modifiers) {
                        continue;
                    }
                    if let Some(bytes) =
                        encode_key(key, modifiers, self.terminal.application_cursor_keys())
                    {
                        self.scrollback_offset = 0;
                        self.selection = None;
                        self.pty.write_all(&bytes)?;
                    }
                }
                Event::MouseWheel {
                    delta, modifiers, ..
                } if self.should_report_mouse(modifiers) => {
                    self.report_mouse_wheel(ctx, rect, metrics, delta)?;
                }
                Event::MouseWheel { unit, delta, .. } => self.scroll_by_delta(unit, delta, metrics),
                Event::PointerButton {
                    pos,
                    button,
                    pressed,
                    modifiers,
                    ..
                } if self.should_report_mouse(modifiers) => {
                    self.report_mouse_button(rect, metrics, pos, button, pressed)?;
                }
                Event::PointerMoved(pos)
                    if self.terminal.mouse_tracking() == Some(MouseTracking::Any) =>
                {
                    self.report_mouse_motion(rect, metrics, pos, 35)?;
                }
                Event::PointerMoved(pos)
                    if self.terminal.mouse_tracking() == Some(MouseTracking::Drag) =>
                {
                    if let Some(button_code) = self.pressed_mouse_button {
                        self.report_mouse_motion(rect, metrics, pos, button_code + 32)?;
                    }
                }
                Event::Copy => {
                    if let Some(text) = self.selected_text() {
                        ctx.copy_text(text);
                    }
                }
                Event::WindowFocused(focused) if self.terminal.focus_reporting() => {
                    if focused {
                        self.pty.write_all(b"\x1b[I")?;
                    } else {
                        self.pty.write_all(b"\x1b[O")?;
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_scrollback_key(&mut self, key: Key, modifiers: Modifiers) -> bool {
        if !modifiers.shift {
            return false;
        }
        match key {
            Key::PageUp => self.scroll_by_lines(self.terminal.rows() as isize),
            Key::PageDown => self.scroll_by_lines(-(self.terminal.rows() as isize)),
            Key::Home => self.scrollback_offset = self.terminal.scrollback_len(),
            Key::End => self.scrollback_offset = 0,
            _ => return false,
        }
        true
    }

    fn scroll_by_delta(&mut self, unit: MouseWheelUnit, delta: Vec2, metrics: &CellMetrics) {
        let lines = match unit {
            MouseWheelUnit::Point => (delta.y / metrics.height).round() as isize,
            MouseWheelUnit::Line => delta.y.round() as isize,
            MouseWheelUnit::Page => (delta.y * self.terminal.rows() as f32).round() as isize,
        };
        self.scroll_by_lines(lines);
    }

    fn scroll_by_lines(&mut self, lines: isize) {
        if lines > 0 {
            self.scrollback_offset = self
                .scrollback_offset
                .saturating_add(lines as usize)
                .min(self.terminal.scrollback_len());
        } else {
            self.scrollback_offset = self.scrollback_offset.saturating_sub((-lines) as usize);
        }
    }

    fn clamp_scrollback_offset(&mut self) {
        self.scrollback_offset = self.scrollback_offset.min(self.terminal.scrollback_len());
    }

    fn handle_pointer_selection(
        &mut self,
        response: &egui::Response,
        rect: Rect,
        metrics: &CellMetrics,
    ) {
        let shift_pressed = response.ctx.input(|input| input.modifiers.shift);
        if self.terminal.mouse_tracking().is_some() && !shift_pressed {
            return;
        }

        if response.drag_started_by(PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                if let Some(coord) = self.coord_at_pos(rect, metrics, pos) {
                    self.selection = Some(Selection {
                        anchor: coord,
                        active: coord,
                    });
                }
            }
        }

        if response.dragged_by(PointerButton::Primary) {
            if let Some(pos) = response.interact_pointer_pos() {
                if let (Some(coord), Some(selection)) = (
                    self.coord_at_pos(rect, metrics, pos),
                    self.selection.as_mut(),
                ) {
                    selection.active = coord;
                }
            }
        }
    }

    fn should_report_mouse(&self, modifiers: Modifiers) -> bool {
        self.terminal.mouse_tracking().is_some() && !modifiers.shift
    }

    fn report_mouse_button(
        &mut self,
        rect: Rect,
        metrics: &CellMetrics,
        pos: Pos2,
        button: PointerButton,
        pressed: bool,
    ) -> PtyResult<()> {
        let Some(button_code) = mouse_button_code(button) else {
            return Ok(());
        };
        let Some((col, row)) = self.screen_coord_at_pos(rect, metrics, pos) else {
            return Ok(());
        };

        self.scrollback_offset = 0;
        self.selection = None;
        if pressed {
            self.pressed_mouse_button = Some(button_code);
            self.write_mouse_report(button_code, col, row, true)
        } else {
            self.pressed_mouse_button = None;
            self.write_mouse_report(button_code, col, row, false)
        }
    }

    fn report_mouse_motion(
        &mut self,
        rect: Rect,
        metrics: &CellMetrics,
        pos: Pos2,
        code: u8,
    ) -> PtyResult<()> {
        let Some((col, row)) = self.screen_coord_at_pos(rect, metrics, pos) else {
            return Ok(());
        };
        self.scrollback_offset = 0;
        self.selection = None;
        self.write_mouse_report(code, col, row, true)
    }

    fn report_mouse_wheel(
        &mut self,
        ctx: &egui::Context,
        rect: Rect,
        metrics: &CellMetrics,
        delta: Vec2,
    ) -> PtyResult<()> {
        let Some(pos) = ctx.input(|input| input.pointer.hover_pos()) else {
            return Ok(());
        };
        let Some((col, row)) = self.screen_coord_at_pos(rect, metrics, pos) else {
            return Ok(());
        };
        let code = if delta.y > 0.0 { 64 } else { 65 };
        self.scrollback_offset = 0;
        self.selection = None;
        self.write_mouse_report(code, col, row, true)
    }

    fn write_mouse_report(
        &mut self,
        code: u8,
        col: usize,
        row: usize,
        pressed: bool,
    ) -> PtyResult<()> {
        if self.terminal.sgr_mouse() {
            let suffix = if pressed { 'M' } else { 'm' };
            let report = format!("\x1b[<{};{};{}{}", code, col + 1, row + 1, suffix);
            self.pty.write_all(report.as_bytes())
        } else {
            let release_code = if pressed { code } else { 3 };
            let report = [
                0x1b,
                b'[',
                b'M',
                release_code.saturating_add(32),
                (col as u8).saturating_add(33),
                (row as u8).saturating_add(33),
            ];
            self.pty.write_all(&report)
        }
    }

    fn sync_title(&mut self, ctx: &egui::Context) {
        let title = self
            .terminal
            .title()
            .filter(|title| !title.is_empty())
            .unwrap_or("ghostty-rs");
        if title != self.shown_title {
            self.shown_title = title.to_string();
            ctx.send_viewport_cmd(ViewportCommand::Title(self.shown_title.clone()));
        }
    }

    fn logical_cell(&self, logical_row: usize, col: usize) -> Option<&Cell> {
        logical_cell(&self.terminal, logical_row, col)
    }

    fn visible_logical_row(&self, row: usize) -> usize {
        renderer::visible_logical_row(&self.terminal, self.scrollback_offset, row)
    }

    fn coord_at_pos(&self, rect: Rect, metrics: &CellMetrics, pos: Pos2) -> Option<CellCoord> {
        self.screen_coord_at_pos(rect, metrics, pos)
            .map(|(col, row)| CellCoord {
                col,
                logical_row: self.visible_logical_row(row),
            })
    }

    fn screen_coord_at_pos(
        &self,
        rect: Rect,
        metrics: &CellMetrics,
        pos: Pos2,
    ) -> Option<(usize, usize)> {
        if !rect.contains(pos) {
            return None;
        }

        let col = ((pos.x - rect.left()) / metrics.width).floor() as usize;
        let row = ((pos.y - rect.top()) / metrics.height).floor() as usize;
        if col >= self.terminal.cols() || row >= self.terminal.rows() {
            return None;
        }

        Some((col, row))
    }

    fn selected_text(&self) -> Option<String> {
        let (start, end) = selection_range(self.selection)?;
        let mut out = String::new();
        for logical_row in start.logical_row..=end.logical_row {
            if logical_row > start.logical_row {
                out.push('\n');
            }

            let start_col = if logical_row == start.logical_row {
                start.col
            } else {
                0
            };
            let end_col = if logical_row == end.logical_row {
                end.col
            } else {
                self.terminal.cols() - 1
            };
            self.push_selected_row(&mut out, logical_row, start_col, end_col);
        }
        if out.is_empty() { None } else { Some(out) }
    }

    fn push_selected_row(
        &self,
        out: &mut String,
        logical_row: usize,
        start_col: usize,
        end_col: usize,
    ) {
        let last_non_blank = (start_col..=end_col)
            .rev()
            .find(|&col| self.logical_cell(logical_row, col).is_some_and(is_nonblank));
        let Some(last_col) = last_non_blank else {
            return;
        };

        for col in start_col..=last_col {
            let Some(cell) = self.logical_cell(logical_row, col) else {
                continue;
            };
            if !cell.is_wide_continuation() {
                out.push(cell.ch());
            }
        }
    }

    fn save_preferences(&self) {
        if let Err(err) = self.preferences.save() {
            eprintln!("failed to save preferences: {err}");
        }
    }

    fn remember_window_size(&mut self, size: Vec2) {
        if size.x < 320.0 || size.y < 200.0 {
            return;
        }
        let changed = (size.x - self.last_saved_window_size.x).abs() >= 1.0
            || (size.y - self.last_saved_window_size.y).abs() >= 1.0;
        if changed {
            self.preferences.window_size = size;
            self.last_saved_window_size = size;
            self.save_preferences();
        }
    }
}

impl eframe::App for WindowTerminal {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let _ = self.drain_pty();
        for text in self.terminal.take_clipboard() {
            ctx.copy_text(text);
        }
        self.sync_title(ctx);
        if !self.close_requested
            && self.exit_status.is_none()
            && let Ok(Some(status)) = self.pty.child_status()
        {
            if status.success() {
                self.close_requested = true;
                ctx.send_viewport_cmd(ViewportCommand::Close);
            } else {
                self.exit_status = Some(exit_status_message(&status));
            }
        }
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let rect = ui.available_rect_before_wrap();
        self.remember_window_size(rect.size());
        let metrics = CellMetrics::for_ui(ui, &self.terminal_font);
        let response = ui.allocate_rect(rect, Sense::click_and_drag());
        if response.clicked() {
            response.request_focus();
        }
        let _ = self.resize_to_rect(rect, &metrics);
        self.handle_pointer_selection(&response, rect, &metrics);
        let _ = self.handle_events(ui.ctx(), rect, &metrics);
        paint_terminal(
            ui,
            rect,
            &metrics,
            &self.terminal,
            self.scrollback_offset,
            self.selection,
        );
        self.paint_preferences(ui.ctx());
        if let Some(message) = &self.exit_status {
            paint_exit_status(ui, rect, message);
        }
    }
}

impl WindowTerminal {
    fn paint_preferences(&mut self, ctx: &egui::Context) {
        if !self.show_preferences {
            return;
        }

        egui::Window::new("Preferences")
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                let mut size = self.terminal_font.size();
                if ui
                    .add(egui::Slider::new(&mut size, 6.0..=48.0).text("Font size"))
                    .changed()
                {
                    self.terminal_font.set_size(size);
                    self.preferences.font_size = Some(self.terminal_font.size());
                    self.save_preferences();
                }
                if !self.terminal_font.diagnostics().is_empty() {
                    ui.separator();
                    ui.label("Font coverage");
                    for diagnostic in self.terminal_font.diagnostics() {
                        let name = diagnostic
                            .path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("unknown font");
                        ui.label(format!("{name}: {}", diagnostic.coverage.summary()));
                    }
                }
                if ui.button("Close").clicked() {
                    self.show_preferences = false;
                }
            });
    }
}

fn is_nonblank(cell: &Cell) -> bool {
    cell.ch() != ' ' || cell.is_wide_continuation()
}

fn exit_status_message(status: &portable_pty::ExitStatus) -> String {
    if let Some(signal) = status.signal() {
        format!("Process exited from signal: {signal}")
    } else {
        format!("Process exited with status {}", status.exit_code())
    }
}
