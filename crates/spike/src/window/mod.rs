use std::time::Duration;

use base64::Engine as _;
use eframe::egui::{
    self, Event, Key, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Sense, Vec2,
    ViewportCommand,
};
use ghostty_input::mouse_encode::{self, Event as MouseEncodeEvent, Options as MouseEncodeOptions};
use ghostty_input::paste;
use ghostty_spike::{Engine, MouseTracking, Snapshot, SnapshotWindow};

use crate::pty::{PtyResult, PtySession};

mod app_shell;
mod font;
mod input;
mod renderer;
mod theme;
mod theme_file;

use crate::config::{self, Config};
use font::TerminalFont;
use input::{encode_key, mouse_button_code};
use renderer::{
    CellMetrics, is_nonblank, logical_cell, paint_exit_status, paint_terminal, selection_range,
    visible_logical_row_in_window,
};

pub(crate) fn run_window() -> PtyResult<()> {
    let preferences = app_shell::AppPreferences::load();
    let config = config::load();
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
            // config's `font-size` takes precedence over the saved
            // preferences value, but a user dragging the in-app slider
            // still updates `preferences` afterwards (see
            // `paint_preferences`), matching prior behavior when no config
            // override is set.
            let font_size = config.font_size.or(preferences.font_size);
            let terminal_font =
                font::configure_with_family(&cc.egui_ctx, font_size, config.font_family.as_deref());
            Ok(Box::new(WindowTerminal::new(
                terminal_font,
                preferences,
                config,
            )?))
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
    engine: Engine,
    pty: PtySession,
    scrollback_offset: usize,
    shown_title: String,
    selection: Option<Selection>,
    pressed_mouse_button: Option<u8>,
    /// Last cell reported for a motion event, for `mouse_encode`'s
    /// motion-dedup logic (skip identical consecutive same-cell motion
    /// reports, except in SGR-pixels format). See
    /// `ghostty_input::mouse_encode::encode`'s `last_cell` parameter.
    last_mouse_cell: Option<(i64, i64)>,
    close_requested: bool,
    exit_status: Option<String>,
    terminal_font: TerminalFont,
    show_preferences: bool,
    preferences: app_shell::AppPreferences,
    last_saved_window_size: Vec2,
    config: Config,
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
    fn new(
        terminal_font: TerminalFont,
        preferences: app_shell::AppPreferences,
        config: Config,
    ) -> PtyResult<Self> {
        let cols = 100;
        let rows = 30;
        let last_saved_window_size = preferences.window_size;
        let engine = match config.theme.as_deref() {
            Some(name) => match theme_file::load_theme(name) {
                Some(theme) => Engine::with_colors(cols, rows, theme.to_colors()),
                None => Engine::new(cols, rows),
            },
            None => Engine::new(cols, rows),
        };
        Ok(Self {
            engine,
            pty: PtySession::spawn(cols as u16, rows as u16)?,
            scrollback_offset: 0,
            shown_title: "ghostty-rs".to_string(),
            selection: None,
            pressed_mouse_button: None,
            last_mouse_cell: None,
            close_requested: false,
            exit_status: None,
            terminal_font,
            show_preferences: false,
            preferences,
            last_saved_window_size,
            config,
        })
    }

    fn drain_pty(&mut self) -> PtyResult<()> {
        while let Some(bytes) = self.pty.try_read() {
            self.engine.write(&bytes);
            let response = self.engine.take_output();
            if !response.is_empty() {
                self.pty.write_all(&response)?;
            }
        }
        self.clamp_scrollback_offset();
        Ok(())
    }

    /// Drain any OSC 52 clipboard write requests the engine queued and copy
    /// them to the system clipboard via egui. Per upstream's
    /// `clipboardContents` policy, `ghostty-vt` hands the request up raw
    /// (still base64-encoded) — decoding is this frontend's job. An invalid
    /// base64 payload is silently dropped (matches upstream logging-and-
    /// ignoring an OSC 52 decode failure rather than treating it as fatal).
    fn drain_clipboard(&mut self, ctx: &egui::Context) {
        while let Some((_kind, data)) = self.engine.take_clipboard() {
            if data.is_empty() {
                ctx.copy_text(String::new());
                continue;
            }
            if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&data)
                && let Ok(text) = String::from_utf8(bytes)
            {
                ctx.copy_text(text);
            }
        }
    }

    fn resize_to_rect(&mut self, rect: Rect, metrics: &CellMetrics) -> PtyResult<()> {
        let cols = (rect.width() / metrics.width).floor().max(1.0) as usize;
        let rows = (rect.height() / metrics.height).floor().max(1.0) as usize;
        if cols != self.engine.cols() || rows != self.engine.rows() {
            self.engine.resize(cols, rows);
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
                    let (prefix, body, suffix) = paste::encode(
                        &text,
                        paste::Options {
                            bracketed: self.engine.bracketed_paste(),
                        },
                    );
                    self.pty.write_all(prefix.as_bytes())?;
                    self.pty.write_all(body.as_bytes())?;
                    self.pty.write_all(suffix.as_bytes())?;
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    repeat,
                    ..
                } => {
                    if app_shell::handle_shortcut(ctx, key, modifiers, &mut self.show_preferences)?
                    {
                        continue;
                    }
                    if self.handle_scrollback_key(key, modifiers) {
                        continue;
                    }
                    let opts = self.engine.key_encode_options();
                    if let Some(bytes) = encode_key(key, modifiers, repeat, &opts) {
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
                    if self.engine.mouse_tracking() == Some(MouseTracking::Any) =>
                {
                    self.report_mouse_motion(rect, metrics, pos)?;
                }
                Event::PointerMoved(pos)
                    if self.engine.mouse_tracking() == Some(MouseTracking::Drag) =>
                {
                    if self.pressed_mouse_button.is_some() {
                        self.report_mouse_motion(rect, metrics, pos)?;
                    }
                }
                Event::Copy => {
                    // A full (not windowed) snapshot is needed here: the
                    // selection may reach above the currently visible
                    // window, into scrollback. This is a rare, user-
                    // initiated event (not the per-frame render path), so
                    // its O(history) cost doesn't matter the way it would if
                    // paid on every frame.
                    let snapshot = self.engine.snapshot();
                    if let Some(text) = self.selected_text(&snapshot) {
                        ctx.copy_text(text);
                    }
                }
                Event::WindowFocused(focused) if self.engine.focus_reporting() => {
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
            Key::PageUp => self.scroll_by_lines(self.engine.rows() as isize),
            Key::PageDown => self.scroll_by_lines(-(self.engine.rows() as isize)),
            Key::Home => self.scrollback_offset = self.engine.scrollback_len(),
            Key::End => self.scrollback_offset = 0,
            _ => return false,
        }
        true
    }

    fn scroll_by_delta(&mut self, unit: MouseWheelUnit, delta: Vec2, metrics: &CellMetrics) {
        let lines = match unit {
            MouseWheelUnit::Point => (delta.y / metrics.height).round() as isize,
            MouseWheelUnit::Line => delta.y.round() as isize,
            MouseWheelUnit::Page => (delta.y * self.engine.rows() as f32).round() as isize,
        };
        self.scroll_by_lines(lines);
    }

    fn scroll_by_lines(&mut self, lines: isize) {
        if lines > 0 {
            self.scrollback_offset = self
                .scrollback_offset
                .saturating_add(lines as usize)
                .min(self.engine.scrollback_len());
        } else {
            self.scrollback_offset = self.scrollback_offset.saturating_sub((-lines) as usize);
        }
    }

    fn clamp_scrollback_offset(&mut self) {
        self.scrollback_offset = self.scrollback_offset.min(self.engine.scrollback_len());
    }

    fn handle_pointer_selection(
        &mut self,
        window: &SnapshotWindow,
        response: &egui::Response,
        rect: Rect,
        metrics: &CellMetrics,
    ) {
        let shift_pressed = response.ctx.input(|input| input.modifiers.shift);
        if self.engine.mouse_tracking().is_some() && !shift_pressed {
            return;
        }

        if response.drag_started_by(PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
            && let Some(coord) = self.coord_at_pos(window, rect, metrics, pos)
        {
            self.selection = Some(Selection {
                anchor: coord,
                active: coord,
            });
        }

        if response.dragged_by(PointerButton::Primary)
            && let Some(pos) = response.interact_pointer_pos()
            && let (Some(coord), Some(selection)) = (
                self.coord_at_pos(window, rect, metrics, pos),
                self.selection.as_mut(),
            )
        {
            selection.active = coord;
        }

        // copy-on-select: a finished drag copies the selection immediately,
        // same text a subsequent explicit copy (`Event::Copy`) would produce
        // (see `selected_text`), without requiring the user to press the
        // platform copy shortcut.
        if self.config.copy_on_select
            && response.drag_stopped_by(PointerButton::Primary)
            && self.selection.is_some()
        {
            let snapshot = self.engine.snapshot();
            if let Some(text) = self.selected_text(&snapshot) {
                response.ctx.copy_text(text);
            }
        }
    }

    fn should_report_mouse(&self, modifiers: Modifiers) -> bool {
        self.engine.mouse_tracking().is_some() && !modifiers.shift
    }

    /// Build the `mouse_encode::Size` for the terminal's current on-screen
    /// rectangle, so pixel positions convert to the same grid cells
    /// `screen_coord_at_pos`'s manual math would produce.
    fn mouse_encode_size(&self, metrics: &CellMetrics) -> mouse_encode::Size {
        mouse_encode::Size {
            screen_width: self.engine.cols() as f64 * metrics.width as f64,
            screen_height: self.engine.rows() as f64 * metrics.height as f64,
            cell_width: metrics.width as f64,
            cell_height: metrics.height as f64,
        }
    }

    /// Encode and send a mouse event through `ghostty_input::mouse_encode`.
    /// `pos` is in window (egui) coordinates; `rect` is the terminal's
    /// on-screen rectangle, so `pos - rect.min` gives the surface-space
    /// position `mouse_encode` expects (origin at the terminal's top-left).
    fn write_mouse_event(
        &mut self,
        metrics: &CellMetrics,
        event: MouseEncodeEvent,
    ) -> PtyResult<()> {
        let opts = MouseEncodeOptions {
            event: self.engine.mouse_event(),
            format: self.engine.mouse_format(),
            size: self.mouse_encode_size(metrics),
            any_button_pressed: self.pressed_mouse_button.is_some()
                || event.action == ghostty_input::mouse::Action::Press,
        };
        let bytes = mouse_encode::encode(event, &opts, &mut self.last_mouse_cell);
        if bytes.is_empty() {
            return Ok(());
        }
        self.pty.write_all(&bytes)
    }

    fn report_mouse_button(
        &mut self,
        rect: Rect,
        metrics: &CellMetrics,
        pos: Pos2,
        button: PointerButton,
        pressed: bool,
    ) -> PtyResult<()> {
        let Some(mouse_button) = map_pointer_button(button) else {
            return Ok(());
        };

        self.scrollback_offset = 0;
        self.selection = None;
        self.pressed_mouse_button = if pressed {
            mouse_button_code(button)
        } else {
            None
        };

        let event = MouseEncodeEvent {
            action: if pressed {
                ghostty_input::mouse::Action::Press
            } else {
                ghostty_input::mouse::Action::Release
            },
            button: Some(mouse_button),
            mods: ghostty_input::key_mods::Mods::default(),
            pos: surface_pos(rect, pos),
        };
        self.write_mouse_event(metrics, event)
    }

    fn report_mouse_motion(
        &mut self,
        rect: Rect,
        metrics: &CellMetrics,
        pos: Pos2,
    ) -> PtyResult<()> {
        self.scrollback_offset = 0;
        self.selection = None;
        let button = self.pressed_mouse_button.and_then(mouse_button_from_code);
        let event = MouseEncodeEvent {
            action: ghostty_input::mouse::Action::Motion,
            button,
            mods: ghostty_input::key_mods::Mods::default(),
            pos: surface_pos(rect, pos),
        };
        self.write_mouse_event(metrics, event)
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
        let button = if delta.y > 0.0 {
            ghostty_input::mouse::Button::Four
        } else {
            ghostty_input::mouse::Button::Five
        };
        self.scrollback_offset = 0;
        self.selection = None;
        let event = MouseEncodeEvent {
            action: ghostty_input::mouse::Action::Press,
            button: Some(button),
            mods: ghostty_input::key_mods::Mods::default(),
            pos: surface_pos(rect, pos),
        };
        self.write_mouse_event(metrics, event)
    }

    fn sync_title(&mut self, ctx: &egui::Context) {
        let title = self
            .engine
            .title()
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| "ghostty-rs".to_string());
        if title != self.shown_title {
            self.shown_title = title;
            ctx.send_viewport_cmd(ViewportCommand::Title(self.shown_title.clone()));
        }
    }

    fn coord_at_pos(
        &self,
        window: &SnapshotWindow,
        rect: Rect,
        metrics: &CellMetrics,
        pos: Pos2,
    ) -> Option<CellCoord> {
        self.screen_coord_at_pos(rect, metrics, pos)
            .map(|(col, row)| CellCoord {
                col,
                logical_row: visible_logical_row_in_window(window, row),
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
        if col >= self.engine.cols() || row >= self.engine.rows() {
            return None;
        }

        Some((col, row))
    }

    fn selected_text(&self, snapshot: &Snapshot) -> Option<String> {
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
                self.engine.cols() - 1
            };
            push_selected_row(snapshot, &mut out, logical_row, start_col, end_col);
        }
        if out.is_empty() { None } else { Some(out) }
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
        self.drain_clipboard(ctx);
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
        // Windowed, not a full `Snapshot`: this runs once per rendered
        // frame, so its cost must stay proportional to the visible rows,
        // not to total scrollback length (see `Engine::snapshot_window`).
        // Anything that needs to reach into scrollback beyond the window
        // (e.g. copying a selection) fetches a full snapshot separately,
        // lazily, only when that rare event actually happens.
        let window = self.engine.snapshot_window(self.scrollback_offset);
        let focused = ui.ctx().input(|input| input.focused);
        self.handle_pointer_selection(&window, &response, rect, &metrics);
        let _ = self.handle_events(ui.ctx(), rect, &metrics);
        paint_terminal(
            ui,
            rect,
            &metrics,
            &window,
            self.scrollback_offset,
            self.selection,
            focused,
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

/// Append the selected span of a logical (all-rows) row to `out`, trimming
/// trailing blanks and skipping wide-glyph spacer cells.
fn push_selected_row(
    snapshot: &Snapshot,
    out: &mut String,
    logical_row: usize,
    start_col: usize,
    end_col: usize,
) {
    let last_non_blank = (start_col..=end_col)
        .rev()
        .find(|&col| logical_cell(snapshot, logical_row, col).is_some_and(is_nonblank));
    let Some(last_col) = last_non_blank else {
        return;
    };

    for col in start_col..=last_col {
        let Some(cell) = logical_cell(snapshot, logical_row, col) else {
            continue;
        };
        if !cell.is_spacer() {
            out.push(cell.ch);
        }
    }
}

fn exit_status_message(status: &portable_pty::ExitStatus) -> String {
    if let Some(signal) = status.signal() {
        format!("Process exited from signal: {signal}")
    } else {
        format!("Process exited with status {}", status.exit_code())
    }
}

/// Map an egui pointer button to `ghostty_input::mouse::Button`.
fn map_pointer_button(button: PointerButton) -> Option<ghostty_input::mouse::Button> {
    match button {
        PointerButton::Primary => Some(ghostty_input::mouse::Button::Left),
        PointerButton::Middle => Some(ghostty_input::mouse::Button::Middle),
        PointerButton::Secondary => Some(ghostty_input::mouse::Button::Right),
        _ => None,
    }
}

/// Inverse of [`mouse_button_code`]: recover the `mouse::Button` held during
/// a drag from the legacy button code cached in `pressed_mouse_button`.
fn mouse_button_from_code(code: u8) -> Option<ghostty_input::mouse::Button> {
    match code {
        0 => Some(ghostty_input::mouse::Button::Left),
        1 => Some(ghostty_input::mouse::Button::Middle),
        2 => Some(ghostty_input::mouse::Button::Right),
        _ => None,
    }
}

/// Convert a window (egui) pointer position into the surface-space position
/// `mouse_encode` expects: pixels from the terminal rect's top-left corner.
fn surface_pos(rect: Rect, pos: Pos2) -> mouse_encode::Pos {
    mouse_encode::Pos {
        x: pos.x - rect.left(),
        y: pos.y - rect.top(),
    }
}
