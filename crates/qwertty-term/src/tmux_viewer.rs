//! Headless tmux control-mode Viewer model (ADR 004 slice 5a / ADR 006).
//!
//! Port of the *pure, AppKit-free* core of Ghostty's
//! `src/terminal/tmux/viewer.zig` (`2da015cd6`, ~2,283 LoC). A [`Viewer`] is a
//! tmux control-mode client: it consumes the engine's decoded
//! [`Notification`](qwertty_term_vt::tmux::Notification) stream (drained from
//! `stream::TerminalHandler::take_tmux_notifications`), maintains the tmux
//! **session → windows → panes** tree, owns a per-tmux-pane
//! [`Terminal`](qwertty_term_vt::terminal::Terminal), routes `%output` bytes to
//! the right pane, and emits [`Action`]s telling the caller what to do next
//! (send a command to tmux, re-render surfaces, or tear down).
//!
//! This is the **testable core** of the native Viewer. It contains no AppKit,
//! no `NSWindow`, no focus/resize wiring — that native half (slice 5b+) reads
//! this model through its query accessors ([`Viewer::windows`],
//! [`Viewer::pane`], [`Viewer::pane_rects`]) and maps tmux windows/panes to
//! native tabs/splits. See ADR 006 for the tab/split UX mapping (PROPOSED) and
//! the full slice breakdown.
//!
//! ## Lifecycle
//!
//! The Viewer is created on [`Notification::Enter`] (the DCS `\ePtmux;` seam)
//! and destroyed on [`Notification::Exit`] / when it becomes defunct, exactly
//! as upstream's `stream_handler.zig` owns a `?*Viewer` keyed on the tmux
//! enter/exit notifications. Between those, every notification is fed to
//! [`Viewer::next`], which returns the actions to apply before the next input.
//!
//! ## Faithful vs deferred (slice 5a vs 5b)
//!
//! The notification **reducer**, the **command-queue** correlation state
//! machine (`%begin`/`%end` block ↔ the command that triggered it), window/pane
//! **tree** maintenance, **layout** application (pane geometry), and live
//! **`%output`** routing are ported faithfully and unit-tested here.
//!
//! Applying a captured pane's *content* and *terminal state* to its
//! `Terminal` is only partially done in 5a, to stay within the engine's public
//! API (no engine-internal edits):
//!
//! - **`%output`** (the live path) is fed straight into the pane's stream —
//!   faithful and the important case.
//! - **`capture-pane` visible** content is fed into the pane's (primary) stream
//!   so the initial screen is populated and queryable.
//! - **`capture-pane` history** (scrollback) and **alternate-screen** captures
//!   are correlated by the state machine but not yet written into the
//!   `Terminal` — that needs a screen-history / alternate-screen write path
//!   that is not part of the engine's public surface today (slice 5b).
//! - **`list-panes` state** (cursor position/shape, terminal modes, scroll
//!   region, tab stops) is parsed into a queryable [`PaneState`] but not yet
//!   applied to the `Terminal`'s cursor/modes (slice 5b).
//!
//! These deferrals are behavioural refinements of an *already-correct* control
//! flow; the divergences are documented so slice 5b can close them.

use std::collections::VecDeque;

use qwertty_term_vt::stream::{Stream, TerminalHandler};
use qwertty_term_vt::terminal::{Colors, Options, Terminal};
use qwertty_term_vt::tmux::Notification;
use qwertty_term_vt::tmux::layout::{Content, Layout};
use qwertty_term_vt::tmux::output::{self, Value, Variable};

#[cfg(test)]
mod tests;

/// The variables requested by `list-windows -F`, in order. Port of upstream
/// `Format.list_windows.vars`.
const LIST_WINDOWS_VARS: &[Variable] = &[
    Variable::SessionId,
    Variable::WindowId,
    Variable::WindowWidth,
    Variable::WindowHeight,
    Variable::WindowLayout,
];

/// The delimiter for the `list-windows` format (a space — none of the requested
/// values contain one).
const LIST_WINDOWS_DELIM: u8 = b' ';

/// The variables requested by `list-panes -F`, in order. Port of upstream
/// `Format.list_panes.vars`. The delimiter is `;`.
const LIST_PANES_VARS: &[Variable] = &[
    Variable::PaneId,
    Variable::CursorX,
    Variable::CursorY,
    Variable::CursorFlag,
    Variable::CursorShape,
    Variable::CursorColour,
    Variable::CursorBlinking,
    Variable::AlternateOn,
    Variable::AlternateSavedX,
    Variable::AlternateSavedY,
    Variable::InsertFlag,
    Variable::WrapFlag,
    Variable::KeypadFlag,
    Variable::KeypadCursorFlag,
    Variable::OriginFlag,
    Variable::MouseAllFlag,
    Variable::MouseAnyFlag,
    Variable::MouseButtonFlag,
    Variable::MouseStandardFlag,
    Variable::MouseUtf8Flag,
    Variable::MouseSgrFlag,
    Variable::FocusFlag,
    Variable::BracketedPaste,
    Variable::ScrollRegionUpper,
    Variable::ScrollRegionLower,
    Variable::PaneTabs,
];

const LIST_PANES_DELIM: u8 = b';';

/// The variables requested by `display-message -p` for the tmux version.
const TMUX_VERSION_VARS: &[Variable] = &[Variable::Version];
const TMUX_VERSION_DELIM: u8 = b' ';

/// An action the caller (native layer / termio) must perform as a result of
/// feeding a notification. Port of `viewer.zig`'s `Action` union.
///
/// Upstream's `windows` action carries a `[]const Window` slice; because this
/// model is queryable, [`Action::WindowsChanged`] is a *signal* and the caller
/// reads the current tree via [`Viewer::windows`] / [`Viewer::pane_rects`].
/// This avoids threading borrowed slices through the reducer's return value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// tmux closed the control-mode connection (or the Viewer became defunct).
    /// The caller should tear down the native surfaces for this Viewer.
    Exit,

    /// Send these bytes to tmux verbatim (they already include the trailing
    /// newline). Port of `Action.command`.
    Command(Vec<u8>),

    /// The window/pane tree changed (windows added/removed, panes
    /// added/removed, or a layout changed). The caller diffs the new tree
    /// (via [`Viewer::windows`]) against its native surfaces. Window IDs are
    /// stable for a Viewer's lifetime, matching tmux. Port of `Action.windows`.
    WindowsChanged,
}

/// A command the Viewer has sent to tmux and is waiting for a `%begin`/`%end`
/// block response for. Port of `viewer.zig`'s `Command` union. Only one command
/// is in flight at a time (the head of the queue), so responses correlate
/// unambiguously.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    /// `list-windows`: refresh the whole window list + layouts.
    ListWindows,
    /// `capture-pane` history (scrollback) for a pane, on the given screen.
    PaneHistory { id: usize, alternate: bool },
    /// `capture-pane` visible area for a pane, on the given screen.
    PaneVisible { id: usize, alternate: bool },
    /// `list-panes`: capture cursor/mode state for all panes.
    PaneState,
    /// `display-message`: fetch the tmux server version.
    TmuxVersion,
    /// `send-keys`: deliver keyboard input to a pane (ADR 006 slice 5d). The
    /// `keys` are Unicode codepoints (from the app's key encoder, decoded from
    /// its UTF-8 output) sent via `send-keys -H` so raw control bytes and
    /// escape sequences pass through unmodified. Unlike the other commands this
    /// is a *write*: its `%begin`/`%end` response carries no data we consume, it
    /// only advances the command queue.
    SendKeys { pane_id: usize, keys: Vec<u32> },
    /// `split-window`: split a tmux pane (ADR 006 slice 5e — tmux-aware native
    /// actions). A native Cmd-D / split keybind targeting a tmux-managed tab is
    /// redirected here instead of mutating the native `SplitTree`; the resulting
    /// `%layout-change` reconcile creates+renders the new pane. `horizontal`
    /// selects a left/right split (`-h`) vs a top/bottom split (`-v`); `before`
    /// (`-b`) places the new pane before (left/above) the target. A *write*
    /// command: its `%begin`/`%end` reply carries nothing, it only advances the
    /// queue.
    SplitWindow {
        pane_id: usize,
        horizontal: bool,
        before: bool,
    },
    /// `new-window`: create a new tmux window (ADR 006 slice 5e). A native
    /// Cmd-T / new-tab while focus is in a tmux window is redirected here
    /// (Josh's iTerm2-style choice): tmux emits `%window-add`, the reconcile
    /// spawns a fresh native tab in the same session. A *write* command.
    NewWindow,
    /// `kill-pane`: close a tmux pane (ADR 006 slice 5e). A native Cmd-W /
    /// close-surface on a tmux pane is redirected here instead of mutating the
    /// native tree; tmux's `%layout-change` (or `%window-close` for the last
    /// pane) reconcile removes the native surface/tab. A *write* command.
    KillPane { pane_id: usize },
    /// `kill-window`: close a whole tmux window (ADR 006 slice 5e — the
    /// close-tab redirect). A native tab close (the tab's red button / close
    /// button) on a tmux-managed tab is redirected here instead of closing the
    /// native tab directly (I3); tmux removes the window and the native tab is
    /// dropped by the follow-up `list-windows` refresh (see
    /// [`Viewer::received_command_output`]). A *write* command.
    KillWindow { window_id: usize },
    /// `select-pane`: make a tmux pane the window's active pane (ADR 006 slice
    /// 5e — focus sync). Sent when the app's keyboard focus moves to a tmux pane
    /// so bare `split-window`, the active-pane indicator, and any no-`-t` command
    /// operate on the pane the user is actually in. A *write* command.
    SelectPane { pane_id: usize },
}

impl Command {
    /// Format this command into the exact bytes to send to tmux, including the
    /// trailing newline. Port of `Command.formatCommand`.
    fn to_bytes(&self) -> Vec<u8> {
        match self {
            Command::ListWindows => {
                let mut cmd = b"list-windows -F '".to_vec();
                cmd.extend_from_slice(&output::format(LIST_WINDOWS_VARS, LIST_WINDOWS_DELIM));
                cmd.extend_from_slice(b"'\n");
                cmd
            }
            // -p stdout, -e SGR escapes, -q quiet, -a alternate, -S -/-E -1 = full history.
            Command::PaneHistory { id, alternate } => {
                let alt = if *alternate { "-a " } else { "" };
                format!("capture-pane -p -e -q {alt}-S - -E -1 -t %{id}\n").into_bytes()
            }
            // Same, without -S/-E = visible area only.
            Command::PaneVisible { id, alternate } => {
                let alt = if *alternate { "-a " } else { "" };
                format!("capture-pane -p -e -q {alt}-t %{id}\n").into_bytes()
            }
            Command::PaneState => {
                let mut cmd = b"list-panes -F '".to_vec();
                cmd.extend_from_slice(&output::format(LIST_PANES_VARS, LIST_PANES_DELIM));
                cmd.extend_from_slice(b"'\n");
                cmd
            }
            Command::TmuxVersion => {
                let mut cmd = b"display-message -p '".to_vec();
                cmd.extend_from_slice(&output::format(TMUX_VERSION_VARS, TMUX_VERSION_DELIM));
                cmd.extend_from_slice(b"'\n");
                cmd
            }
            // `send-keys -t %<id> -H <hex codepoints…>`: each key is one
            // hexadecimal Unicode codepoint. `-H` bypasses tmux key-name lookup
            // so bytes like CR (`d`) / ESC (`1b`) and full escape sequences are
            // delivered to the pane verbatim.
            Command::SendKeys { pane_id, keys } => {
                let mut cmd = format!("send-keys -t %{pane_id} -H");
                for k in keys {
                    cmd.push_str(&format!(" {k:x}"));
                }
                cmd.push('\n');
                cmd.into_bytes()
            }
            // `split-window -t %<id> -h|-v [-b]`: split the target pane. `-h` is
            // a left/right split, `-v` a top/bottom one; `-b` puts the new pane
            // before (left/above) the target.
            Command::SplitWindow {
                pane_id,
                horizontal,
                before,
            } => {
                let orient = if *horizontal { "-h" } else { "-v" };
                let b = if *before { " -b" } else { "" };
                format!("split-window -t %{pane_id} {orient}{b}\n").into_bytes()
            }
            // `new-window`: create a new window in the attached session (the one
            // control mode is attached to). tmux picks the target session, so no
            // `-t` is needed.
            Command::NewWindow => b"new-window\n".to_vec(),
            // `kill-pane -t %<id>`: close the target pane.
            Command::KillPane { pane_id } => format!("kill-pane -t %{pane_id}\n").into_bytes(),
            // `kill-window -t @<id>`: close the target window (and all its panes).
            Command::KillWindow { window_id } => {
                format!("kill-window -t @{window_id}\n").into_bytes()
            }
            // `select-pane -t %<id>`: make the target pane the active pane.
            Command::SelectPane { pane_id } => format!("select-pane -t %{pane_id}\n").into_bytes(),
        }
    }
}

/// A tmux window in the current session. Port of `viewer.zig`'s `Window`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Window {
    /// Stable tmux window id (`@<n>`).
    pub id: usize,
    /// Window width/height in cells (from `list-windows`).
    pub width: usize,
    pub height: usize,
    /// The parsed layout tree (pane geometry).
    pub layout: Layout,
}

/// The geometry of one pane within a window, in cells, derived from the layout
/// tree. This is what the native layer (slice 5b) turns into split ratios.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PaneRect {
    pub pane_id: usize,
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

/// Parsed `list-panes` state for a pane. A queryable snapshot; applying these
/// to the pane's `Terminal` cursor/modes is slice 5b (see the module docs).
/// Only the fields the native layer needs first are surfaced; the full
/// `list-panes` line is still parsed so the response is consumed positionally.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PaneState {
    pub cursor_x: usize,
    pub cursor_y: usize,
    /// Cursor visibility (`cursor_flag`).
    pub cursor_visible: bool,
    /// Cursor shape as reported by tmux (`block`/`underline`/`bar`/`default`).
    pub cursor_shape: String,
    /// Whether the pane is on its alternate screen.
    pub alternate_on: bool,
}

/// A tmux pane: an owned engine terminal fed by that pane's `%output`. Port of
/// `viewer.zig`'s `Pane`.
pub struct Pane {
    /// Stable tmux pane id (`%<n>`).
    id: usize,
    /// The engine stream + terminal this pane's bytes drive.
    stream: Stream<TerminalHandler>,
    /// The most recent parsed `list-panes` state, if any.
    state: Option<PaneState>,
}

impl Pane {
    fn new(width: usize, height: usize, colors: &Colors) -> Pane {
        Pane {
            // placeholder id; set by the caller.
            id: 0,
            stream: Stream::new(TerminalHandler::new(Terminal::new(Options {
                cols: clamp_cells(width),
                rows: clamp_cells(height),
                // Seed the pane terminal with the app's configured palette/theme
                // so tmux panes match ordinary shell panes instead of rendering
                // on the engine default background (ADR 006 theme fix).
                colors: colors.clone(),
                ..Default::default()
            }))),
            state: None,
        }
    }

    /// The engine terminal for this pane (for the renderer / snapshotting).
    pub fn terminal(&self) -> &Terminal {
        self.stream.terminal()
    }

    /// The most recent parsed `list-panes` state, if captured.
    pub fn state(&self) -> Option<&PaneState> {
        self.state.as_ref()
    }
}

/// The Viewer state machine's phase. Port of `viewer.zig`'s `State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Just entered control mode; waiting for the initial `%begin`/`%end` block.
    StartupBlock,
    /// Got the initial block; waiting for `%session-changed` for the session id.
    StartupSession,
    /// Steady state: process command responses + live notifications.
    CommandQueue,
    /// tmux closed the connection; ignore everything.
    Defunct,
}

/// A headless tmux control-mode Viewer. See the module docs. Port of
/// `viewer.zig`'s `Viewer` (pure core).
pub struct Viewer {
    state: State,
    session_id: usize,
    tmux_version: Vec<u8>,
    /// Commands sent and awaiting a block response, in FIFO order. Only the
    /// head is in flight. Port of `command_queue` (a `CircBuf`).
    command_queue: VecDeque<Command>,
    /// The windows in the current session. Order matches `list-windows`.
    windows: Vec<Window>,
    /// The panes in the current session, keyed by pane id. A `Vec` (not a map)
    /// to preserve insertion order deterministically — pane counts are tiny and
    /// this mirrors upstream's insertion-ordered `AutoArrayHashMap`.
    panes: Vec<Pane>,
    /// The app's configured palette/theme, applied to every pane `Terminal` on
    /// creation so tmux panes honour the user's colors (ADR 006 theme fix).
    /// Retained across a `%session-changed` reset.
    colors: Colors,
    /// The tmux pane id tmux currently considers **active** (ADR 006 slice 5e —
    /// focus sync). Updated from `%window-pane-changed` (and set optimistically
    /// when the app sends a `select-pane`). Upstream ignores this and drives its
    /// own focus (`viewer.zig:508-510`, TODO `viewer.zig:23`); we track it so the
    /// app's keyboard focus and tmux's active pane stay in sync both ways.
    active_pane: Option<usize>,
}

impl Default for Viewer {
    fn default() -> Self {
        Self::new(Colors::default())
    }
}

impl Viewer {
    /// Create a fresh Viewer in the `StartupBlock` state. Call this when the
    /// engine reports [`Notification::Enter`]. Port of `Viewer.init`. `colors`
    /// is the app's configured palette, seeded into every pane `Terminal`.
    pub fn new(colors: Colors) -> Viewer {
        Viewer {
            state: State::StartupBlock,
            session_id: 0,
            tmux_version: Vec::new(),
            command_queue: VecDeque::new(),
            windows: Vec::new(),
            panes: Vec::new(),
            colors,
            active_pane: None,
        }
    }

    /// The tmux pane id tmux currently considers active, if known (ADR 006 slice
    /// 5e). The native layer resolves this to a `SurfaceId` to sync keyboard
    /// focus after a tmux-initiated active-pane change (e.g. a fresh split makes
    /// its new pane active).
    pub fn active_pane(&self) -> Option<usize> {
        self.active_pane
    }

    // ---- query API (the native layer / slice 5b reads through these) -------

    /// The windows in the current session, in `list-windows` order.
    pub fn windows(&self) -> &[Window] {
        &self.windows
    }

    /// The current tmux session id (`$<n>`), or 0 before the first
    /// `%session-changed`.
    pub fn session_id(&self) -> usize {
        self.session_id
    }

    /// The tmux server version string (e.g. `3.5a`), empty until captured.
    pub fn tmux_version(&self) -> &[u8] {
        &self.tmux_version
    }

    /// The pane with the given id, if tracked.
    pub fn pane(&self, id: usize) -> Option<&Pane> {
        self.panes.iter().find(|p| p.id == id)
    }

    /// The number of tracked panes.
    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    /// Whether the Viewer has become defunct (tmux exited). Once defunct it
    /// ignores all further input and should be dropped.
    pub fn is_defunct(&self) -> bool {
        self.state == State::Defunct
    }

    /// The geometry of every pane in the given window, flattened from its
    /// layout tree (each leaf carries its own x/y/width/height in cells). The
    /// native layer turns these rects into split ratios. Returns an empty vec
    /// for an unknown window id.
    pub fn pane_rects(&self, window_id: usize) -> Vec<PaneRect> {
        let mut out = Vec::new();
        if let Some(w) = self.windows.iter().find(|w| w.id == window_id) {
            collect_pane_rects(&w.layout, &mut out);
        }
        out
    }

    // ---- reducer -----------------------------------------------------------

    /// Feed one decoded tmux notification and return the actions to apply
    /// before the next input. Never fails: unrecoverable states transition to
    /// defunct and emit [`Action::Exit`]. Port of `Viewer.next` / `nextTmux`.
    pub fn next(&mut self, n: Notification) -> Vec<Action> {
        match self.state {
            State::Defunct => Vec::new(),
            State::StartupBlock => self.next_startup_block(n),
            State::StartupSession => self.next_startup_session(n),
            State::CommandQueue => self.next_command(n),
        }
    }

    fn next_startup_block(&mut self, n: Notification) -> Vec<Action> {
        match n {
            // Enter is only emitted by the DCS seam before a Viewer exists.
            Notification::Enter => Vec::new(),
            Notification::Exit => self.defunct(),
            // Any initial block (even an error) advances us; now we wait for
            // %session-changed for the initial session id.
            Notification::BlockEnd(_) | Notification::BlockErr(_) => {
                self.state = State::StartupSession;
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    fn next_startup_session(&mut self, n: Notification) -> Vec<Action> {
        match n {
            Notification::Enter => Vec::new(),
            Notification::Exit => self.defunct(),
            Notification::SessionChanged { id, .. } => {
                self.session_id = id;
                // Queue the startup commands and send the first one.
                self.enter_command_queue(&[Command::TmuxVersion, Command::ListWindows])
            }
            _ => Vec::new(),
        }
    }

    fn next_command(&mut self, n: Notification) -> Vec<Action> {
        debug_assert_eq!(self.state, State::CommandQueue);

        let mut actions: Vec<Action> = Vec::new();
        // A command slot is available if nothing is in flight, or once the
        // in-flight command completes / the queue is reset below.
        let mut command_consumed = self.command_queue.is_empty();

        match n {
            Notification::Enter => {}
            Notification::Exit => return self.defunct(),

            Notification::BlockEnd(content) => {
                self.received_command_output(&mut actions, &content);
                command_consumed = true;
            }
            Notification::BlockErr(content) => {
                self.received_command_output(&mut actions, &content);
                command_consumed = true;
            }

            Notification::Output { pane_id, data } => {
                self.received_output(pane_id, &data);
            }

            Notification::SessionChanged { id, .. } => {
                self.session_changed(&mut actions, id);
                command_consumed = true;
            }

            Notification::LayoutChange {
                window_id, layout, ..
            } => {
                self.layout_changed(&mut actions, window_id, &layout);
            }

            Notification::WindowAdd { .. } => {
                // Refresh the whole window list to pick up the new window.
                self.command_queue.push_back(Command::ListWindows);
            }

            Notification::WindowClose { .. } => {
                // A window closed (incl. a non-active window's last pane being
                // Ctrl-D'd → `%unlinked-window-close`). Refresh the window list
                // so the reconcile drops the vanished window's tab (I3 / gap 3).
                self.command_queue.push_back(Command::ListWindows);
            }

            // The active pane changed (ADR 006 slice 5e — focus sync). Unlike
            // upstream (which ignores this and drives its own focus,
            // `viewer.zig:508-510`), we record tmux's active pane so the native
            // layer can move the app's keyboard focus to match — e.g. a fresh
            // `split-window` makes its new pane active and tmux reports it here.
            Notification::WindowPaneChanged { pane_id, .. } => {
                self.active_pane = Some(pane_id);
            }
            // A session was created/destroyed elsewhere: we'll get exit or
            // session_changed for our own; ignore otherwise.
            Notification::SessionsChanged => {}
            // We don't use window names yet.
            Notification::WindowRenamed { .. } => {}
            // Other clients; nothing to do.
            Notification::ClientDetached { .. } | Notification::ClientSessionChanged { .. } => {}
        }

        // Send the next queued command, but only if the in-flight slot is free.
        if self.state == State::CommandQueue
            && command_consumed
            && let Some(cmd) = self.command_queue.front()
        {
            actions.push(Action::Command(cmd.to_bytes()));
        }

        actions
    }

    /// Queue keyboard input for a pane and return the command(s) to send now.
    /// ADR 006 slice 5d: control mode has no pane pty, so input is delivered as
    /// a `send-keys` control command on the same control pty.
    ///
    /// `bytes` is the app key encoder's output (UTF-8); it is decoded into
    /// Unicode codepoints and sent via `send-keys -H`. The command is threaded
    /// through the same in-flight command queue as every other control command
    /// so its `%begin`/`%end` reply correlates correctly (a stray reply would
    /// otherwise be mis-attributed to whatever command was in flight). When the
    /// queue is idle the command is emitted immediately; otherwise it is emitted
    /// once the in-flight command completes.
    ///
    /// Returns an empty vec (no action) when not in steady state, the pane is
    /// unknown, or `bytes` decodes to nothing.
    pub fn send_keys(&mut self, pane_id: usize, bytes: &[u8]) -> Vec<Action> {
        if self.state != State::CommandQueue {
            return Vec::new();
        }
        if !self.panes.iter().any(|p| p.id == pane_id) {
            return Vec::new();
        }
        let keys: Vec<u32> = String::from_utf8_lossy(bytes)
            .chars()
            .map(|c| c as u32)
            .collect();
        if keys.is_empty() {
            return Vec::new();
        }
        self.enqueue_write(Command::SendKeys { pane_id, keys })
    }

    /// Split the tmux pane a native surface renders, in the given orientation
    /// (ADR 006 slice 5e). `horizontal` picks a left/right (`-h`) vs top/bottom
    /// (`-v`) split; `before` (`-b`) places the new pane before the target.
    /// Returns the control command(s) to send now (empty when not in steady
    /// state or the pane is unknown). The new pane materialises via the
    /// subsequent `%layout-change` reconcile, not by mutating any native tree.
    pub fn split_window(&mut self, pane_id: usize, horizontal: bool, before: bool) -> Vec<Action> {
        if self.state != State::CommandQueue || !self.panes.iter().any(|p| p.id == pane_id) {
            return Vec::new();
        }
        self.enqueue_write(Command::SplitWindow {
            pane_id,
            horizontal,
            before,
        })
    }

    /// Create a new tmux window in the attached session (ADR 006 slice 5e — the
    /// Cmd-T redirect). Returns the control command to send now (empty when not
    /// in steady state). The new window materialises via the `%window-add`
    /// reconcile as a fresh native tab.
    pub fn new_window(&mut self) -> Vec<Action> {
        if self.state != State::CommandQueue {
            return Vec::new();
        }
        self.enqueue_write(Command::NewWindow)
    }

    /// Kill the tmux pane a native surface renders (ADR 006 slice 5e — the
    /// Cmd-W redirect). Returns the control command to send now (empty when not
    /// in steady state or the pane is unknown). The native surface/tab is torn
    /// down by the subsequent `%layout-change` / `%window-close` reconcile.
    pub fn kill_pane(&mut self, pane_id: usize) -> Vec<Action> {
        if self.state != State::CommandQueue || !self.panes.iter().any(|p| p.id == pane_id) {
            return Vec::new();
        }
        self.enqueue_write(Command::KillPane { pane_id })
    }

    /// Kill the tmux window a native tab mirrors (ADR 006 slice 5e — the
    /// close-tab redirect / gap 1). Returns the control command(s) to send now
    /// (empty when not in steady state or the window is unknown). The window
    /// (and all its panes) is removed by tmux; the native tab is torn down by
    /// the follow-up `list-windows` refresh queued in
    /// [`received_command_output`](Self::received_command_output) — the caller
    /// must NOT close the native tab directly (I3).
    pub fn kill_window(&mut self, window_id: usize) -> Vec<Action> {
        if self.state != State::CommandQueue || !self.windows.iter().any(|w| w.id == window_id) {
            return Vec::new();
        }
        self.enqueue_write(Command::KillWindow { window_id })
    }

    /// Make a tmux pane the active pane (ADR 006 slice 5e — app→tmux focus sync).
    /// Called when the app's keyboard focus moves to a tmux pane so bare
    /// `split-window` and the active-pane indicator operate on the pane the user
    /// is in. Sets [`active_pane`](Self::active_pane) optimistically so tmux's
    /// echoing `%window-pane-changed` is a no-op (no focus bounce), and returns
    /// the control command to send now (empty when not in steady state, the pane
    /// is unknown, or it is already active — nothing to do).
    pub fn select_pane(&mut self, pane_id: usize) -> Vec<Action> {
        if self.state != State::CommandQueue
            || self.active_pane == Some(pane_id)
            || !self.panes.iter().any(|p| p.id == pane_id)
        {
            return Vec::new();
        }
        self.active_pane = Some(pane_id);
        self.enqueue_write(Command::SelectPane { pane_id })
    }

    /// Queue a *write* control command (send-keys / split-window / new-window /
    /// kill-pane) onto the in-flight command queue and return the action to send
    /// it now, if the queue was idle. Threading writes through the same queue as
    /// reads keeps each `%begin`/`%end` reply correlated with the command that
    /// triggered it (a stray reply would otherwise be mis-attributed). When a
    /// command is already in flight the write is emitted by `next_command` once
    /// that completes. Precondition: caller has verified `State::CommandQueue`.
    fn enqueue_write(&mut self, cmd: Command) -> Vec<Action> {
        debug_assert_eq!(self.state, State::CommandQueue);
        let was_idle = self.command_queue.is_empty();
        self.command_queue.push_back(cmd);
        if was_idle {
            // Nothing in flight: emit now; its %begin/%end will pop it.
            vec![Action::Command(
                self.command_queue.front().expect("just pushed").to_bytes(),
            )]
        } else {
            // A command is in flight; next_command emits this once it completes.
            Vec::new()
        }
    }

    // ---- command responses -------------------------------------------------

    fn received_command_output(&mut self, actions: &mut Vec<Action>, content: &[u8]) {
        let Some(command) = self.command_queue.pop_front() else {
            // Unexpected block output with no pending command; ignore.
            return;
        };

        match command {
            Command::ListWindows => self.received_list_windows(actions, content),
            Command::PaneState => self.received_pane_state(content),
            Command::PaneHistory { id, alternate } => {
                self.received_pane_history(id, alternate, content)
            }
            Command::PaneVisible { id, alternate } => {
                self.received_pane_visible(id, alternate, content)
            }
            Command::TmuxVersion => self.received_tmux_version(content),
            // A `send-keys` write: tmux's reply carries nothing to apply; the
            // pop above already advanced the queue. The pane's echo (if any)
            // arrives separately as `%output`.
            Command::SendKeys { .. } => {}
            // Structural writes (split/new-window/select): the reply carries
            // nothing to apply either. The pop advanced the queue; the layout
            // effect arrives later as a `%layout-change` / `%window-add` that
            // drives the reconcile (ADR 006 slice 5e).
            Command::SplitWindow { .. } | Command::NewWindow | Command::SelectPane { .. } => {}
            // Window-removing writes (kill-pane / kill-window): tmux signals the
            // removal of the *last* pane of a window (or a whole window) with
            // `%window-close` / `%unlinked-window-close`, which the control-mode
            // decoder does NOT surface as a notification (verified against tmux
            // 3.7b: a non-last window close emits only `%unlinked-window-close`
            // + `%session-window-changed`, both undecoded). So a native tab would
            // linger after its tmux window is gone. Because *we* initiated this
            // write, we can close the loop app-side: queue a `list-windows`
            // refresh so the reconcile drops the now-missing window's tab (I1/I3).
            // A kill-pane that only removed a non-last pane already got a
            // `%layout-change`; the extra refresh is an idempotent no-op there.
            Command::KillPane { .. } | Command::KillWindow { .. } => {
                self.command_queue.push_back(Command::ListWindows);
            }
        }
    }

    fn received_tmux_version(&mut self, content: &[u8]) {
        let line = trim_ascii_ws(content);
        if line.is_empty() {
            return;
        }
        match output::parse_format(TMUX_VERSION_VARS, line, TMUX_VERSION_DELIM) {
            Ok(values) => {
                if let Some(Value::Str(v)) = values.into_iter().next() {
                    self.tmux_version = v;
                }
            }
            Err(_) => { /* leave version unset; non-fatal */ }
        }
    }

    fn received_list_windows(&mut self, actions: &mut Vec<Action>, content: &[u8]) {
        let mut windows: Vec<Window> = Vec::new();
        for line_raw in content.split(|&b| b == b'\n') {
            let line = trim_ascii_ws(line_raw);
            if line.is_empty() {
                continue;
            }
            let values = match output::parse_format(LIST_WINDOWS_VARS, line, LIST_WINDOWS_DELIM) {
                Ok(v) => v,
                // A malformed line aborts the whole refresh (upstream returns
                // the error); keep the prior window state.
                Err(_) => return,
            };
            let (
                Some(&Value::Usize(window_id)),
                Some(&Value::Usize(width)),
                Some(&Value::Usize(height)),
            ) = (values.get(1), values.get(2), values.get(3))
            else {
                return;
            };
            let Some(Value::Str(layout_str)) = values.get(4) else {
                return;
            };
            let layout = match Layout::parse_with_checksum(layout_str) {
                Ok(l) => l,
                Err(_) => return,
            };
            windows.push(Window {
                id: window_id,
                width,
                height,
                layout,
            });
        }

        // Signal the caller, then sync panes against the new window set.
        actions.push(Action::WindowsChanged);
        self.sync_layouts(windows);
    }

    fn received_pane_state(&mut self, content: &[u8]) {
        for line_raw in content.split(|&b| b == b'\n') {
            let line = trim_ascii_ws(line_raw);
            if line.is_empty() {
                continue;
            }
            let Ok(values) = output::parse_format(LIST_PANES_VARS, line, LIST_PANES_DELIM) else {
                // Malformed line; skip it (non-fatal for the rest).
                continue;
            };
            let Some(&Value::Usize(pane_id)) = values.first() else {
                continue;
            };
            let state = PaneState {
                cursor_x: usize_at(&values, 1),
                cursor_y: usize_at(&values, 2),
                cursor_visible: bool_at(&values, 3),
                cursor_shape: str_at(&values, 4),
                alternate_on: bool_at(&values, 7),
            };
            if let Some(pane) = self.panes.iter_mut().find(|p| p.id == pane_id) {
                pane.state = Some(state);
            }
        }
    }

    fn received_pane_history(&mut self, id: usize, alternate: bool, _content: &[u8]) {
        // Scrollback / alternate-screen capture application is slice 5b (needs a
        // screen-history write path not in the engine's public API). We only
        // consume the response here so the command queue advances. `id`/
        // `alternate` are retained for that future application.
        let _ = (id, alternate);
    }

    fn received_pane_visible(&mut self, id: usize, alternate: bool, content: &[u8]) {
        // Only the primary-screen visible capture is applied in 5a. The
        // alternate screen needs an explicit screen switch (slice 5b).
        if alternate {
            return;
        }
        if let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) {
            pane.stream.feed(content);
        }
    }

    fn received_output(&mut self, id: usize, data: &[u8]) {
        // The live path: route the pane's bytes straight into its terminal.
        if let Some(pane) = self.panes.iter_mut().find(|p| p.id == id) {
            pane.stream.feed(data);
        }
        // Output for an untracked pane is dropped (matches upstream).
    }

    // ---- layout / pane syncing ---------------------------------------------

    fn layout_changed(&mut self, actions: &mut Vec<Action>, window_id: usize, layout_str: &[u8]) {
        if !self.windows.iter().any(|w| w.id == window_id) {
            // Layout change for a window we don't know about; ignore.
            return;
        }
        let layout = match Layout::parse_with_checksum(layout_str) {
            Ok(l) => l,
            // Upstream becomes defunct on a parse failure here.
            Err(_) => {
                self.state = State::Defunct;
                actions.push(Action::Exit);
                return;
            }
        };
        if let Some(window) = self.windows.iter_mut().find(|w| w.id == window_id) {
            window.layout = layout;
        }

        // The window set is unchanged (same ids), but panes may be added/removed.
        actions.push(Action::WindowsChanged);
        let windows = self.windows.clone();
        self.sync_layouts(windows);
    }

    /// Rebuild the pane set from the given windows' layouts: reuse existing
    /// panes (preserving their terminals), create panes for new ids, drop
    /// removed ones, and queue capture commands for the additions. Port of
    /// `syncLayouts` + `initLayout`.
    fn sync_layouts(&mut self, windows: Vec<Window>) {
        // Collect the (id, width, height) of every pane leaf across all windows,
        // in layout order, de-duplicated (first occurrence wins).
        let mut leaves: Vec<(usize, usize, usize)> = Vec::new();
        for w in &windows {
            collect_leaves(&w.layout, &mut leaves);
        }

        // Which ids are new (not currently tracked)?
        let added: Vec<usize> = leaves
            .iter()
            .filter(|(id, _, _)| !self.panes.iter().any(|p| p.id == *id))
            .map(|(id, _, _)| *id)
            .collect();

        // Rebuild the pane list in new-layout order: move existing panes over,
        // create fresh ones for new ids. Anything left in `old` is dropped
        // (removed panes), freeing their terminals.
        let mut old: Vec<Pane> = std::mem::take(&mut self.panes);
        let mut rebuilt: Vec<Pane> = Vec::with_capacity(leaves.len());
        for (id, width, height) in &leaves {
            if let Some(pos) = old.iter().position(|p| p.id == *id) {
                rebuilt.push(old.remove(pos));
            } else {
                let mut pane = Pane::new(*width, *height, &self.colors);
                pane.id = *id;
                rebuilt.push(pane);
            }
        }
        self.panes = rebuilt;

        // Queue capture commands for each added pane (primary + alternate,
        // history + visible), then a single pane_state if anything was added.
        for id in &added {
            self.command_queue.push_back(Command::PaneHistory {
                id: *id,
                alternate: false,
            });
            self.command_queue.push_back(Command::PaneVisible {
                id: *id,
                alternate: false,
            });
            self.command_queue.push_back(Command::PaneHistory {
                id: *id,
                alternate: true,
            });
            self.command_queue.push_back(Command::PaneVisible {
                id: *id,
                alternate: true,
            });
        }
        if !added.is_empty() {
            self.command_queue.push_back(Command::PaneState);
        }

        self.windows = windows;
    }

    /// Handle `%session-changed` in steady state: completely reset, emit an
    /// empty-windows signal, and restart the `list-windows` flow. Port of
    /// `sessionChanged`.
    fn session_changed(&mut self, actions: &mut Vec<Action>, session_id: usize) {
        let version = std::mem::take(&mut self.tmux_version);
        let colors = self.colors.clone();
        *self = Viewer::new(colors);
        self.tmux_version = version;
        self.session_id = session_id;
        self.state = State::CommandQueue;

        // Signal callers to clear all surfaces, then restart the window listing.
        actions.push(Action::WindowsChanged);
        self.command_queue.push_back(Command::ListWindows);
    }

    // ---- helpers -----------------------------------------------------------

    /// Queue `commands` and return the action to send the first. Precondition:
    /// not already in the command-queue state. Port of `enterCommandQueue`.
    fn enter_command_queue(&mut self, commands: &[Command]) -> Vec<Action> {
        debug_assert_ne!(self.state, State::CommandQueue);
        debug_assert!(!commands.is_empty());
        let first = commands[0].to_bytes();
        for cmd in commands {
            self.command_queue.push_back(cmd.clone());
        }
        self.state = State::CommandQueue;
        vec![Action::Command(first)]
    }

    fn defunct(&mut self) -> Vec<Action> {
        self.state = State::Defunct;
        vec![Action::Exit]
    }
}

/// Recursively collect every pane leaf's (id, width, height), first-occurrence
/// wins. Port of `initLayout`'s recursion (geometry-only).
fn collect_leaves(layout: &Layout, out: &mut Vec<(usize, usize, usize)>) {
    match &layout.content {
        Content::Pane(id) => {
            if !out.iter().any(|(existing, _, _)| existing == id) {
                out.push((*id, layout.width, layout.height));
            }
        }
        Content::Horizontal(children) | Content::Vertical(children) => {
            for child in children {
                collect_leaves(child, out);
            }
        }
    }
}

/// Recursively collect every pane leaf's absolute rect.
fn collect_pane_rects(layout: &Layout, out: &mut Vec<PaneRect>) {
    match &layout.content {
        Content::Pane(id) => out.push(PaneRect {
            pane_id: *id,
            x: layout.x,
            y: layout.y,
            width: layout.width,
            height: layout.height,
        }),
        Content::Horizontal(children) | Content::Vertical(children) => {
            for child in children {
                collect_pane_rects(child, out);
            }
        }
    }
}

/// Clamp a layout dimension (cells, `usize`) into the engine's `CellCountInt`
/// (`u16`), keeping at least 1 (a zero-dimension terminal is invalid). tmux
/// dimensions are small in practice; this guards adversarial input.
fn clamp_cells(n: usize) -> u16 {
    n.clamp(1, u16::MAX as usize) as u16
}

/// Trim leading/trailing ASCII whitespace (space, tab, CR, LF) — mirrors
/// upstream's `std.mem.trim(u8, …, " \t\r\n")`.
fn trim_ascii_ws(s: &[u8]) -> &[u8] {
    let is_ws = |b: u8| matches!(b, b' ' | b'\t' | b'\r' | b'\n');
    let start = s.iter().position(|&b| !is_ws(b)).unwrap_or(s.len());
    let end = s.iter().rposition(|&b| !is_ws(b)).map_or(start, |i| i + 1);
    &s[start..end]
}

fn usize_at(values: &[Value], i: usize) -> usize {
    match values.get(i) {
        Some(Value::Usize(v)) => *v,
        _ => 0,
    }
}

fn bool_at(values: &[Value], i: usize) -> bool {
    matches!(values.get(i), Some(Value::Bool(true)))
}

fn str_at(values: &[Value], i: usize) -> String {
    match values.get(i) {
        Some(Value::Str(v)) => String::from_utf8_lossy(v).into_owned(),
        _ => String::new(),
    }
}
