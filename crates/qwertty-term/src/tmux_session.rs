//! tmux control-mode session driver (ADR 006 slice 5c — the "make it live"
//! integration seam).
//!
//! A [`TmuxSession`] is the headless glue between the two landed pure modules:
//!
//! - [`Viewer`](crate::tmux_viewer::Viewer) (slice 5a) — the session/window/pane
//!   state machine that consumes decoded tmux [`Notification`]s, owns a
//!   `Terminal` per tmux pane, routes `%output`, and emits
//!   [`Action`](crate::tmux_viewer::Action)s (send a command to tmux, re-render
//!   surfaces, or exit);
//! - [`Reconciler`](crate::tmux_reconcile::Reconciler) (slice 5b) — the diff of
//!   the Viewer's window set into native tab/split intent (Option (a): a tmux
//!   window → a native tab, a tmux pane → a split), with a stable
//!   `pane_id → SurfaceId` map.
//!
//! It exposes one reducer, [`TmuxSession::ingest`], which folds a drained batch
//! of notifications through the Viewer and, whenever the window/pane tree
//! changed, runs the Reconciler once, returning a [`SessionUpdate`]:
//!
//! - `commands` — the exact control-mode command bytes to write **back to the
//!   same control pty** the `tmux -CC` process is on (control mode is in-band);
//! - `plan` — a [`ReconcilePlan`] the native layer applies to create/remove
//!   native tabs and set each window's split tree (slice 5b-native);
//! - `exit` — tmux left control mode (or the Viewer is defunct); tear the
//!   session's native surfaces down and drop the `TmuxSession`.
//!
//! ## Lifecycle
//!
//! The app owns an `Option<TmuxSession>` per surface. It is constructed on the
//! [`Notification::Enter`] that the DCS `\eP1000p` seam emits, fed every
//! subsequent drained notification via [`ingest`](TmuxSession::ingest), and
//! dropped on [`SessionUpdate::exit`]. This mirrors upstream `stream_handler.zig`
//! owning a `?*Viewer` keyed on the tmux enter/exit notifications; the
//! command-writing and surface-reconciling halves that upstream's apprt does are
//! surfaced here as data (`commands` / `plan`) so this driver stays headless and
//! unit-testable.
//!
//! ## Rendering a pane
//!
//! Each native pane surface renders a tmux pane's owned `Terminal` (fed by
//! `%output`), *not* a pty-backed engine. Given a split-tree leaf's
//! [`SurfaceId`], [`TmuxSession::pane_terminal`] resolves it back through the
//! Reconciler's stable map to the Viewer's pane `Terminal` to snapshot. See the
//! ADR 006 "5c surface-binding" section for the display-only-surface design.

use qwertty_term_vt::terminal::{Colors, Terminal};
use qwertty_term_vt::tmux::Notification;

use crate::splits::SurfaceId;
use crate::tmux_reconcile::{ReconcilePlan, Reconciler};
use crate::tmux_viewer::{Action, Viewer};

#[cfg(test)]
mod tests;

/// What one [`TmuxSession::ingest`] produced: the control-pty command bytes to
/// send, an optional native-surface reconcile plan, and whether tmux exited.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SessionUpdate {
    /// Control-mode command blocks to write to the control pty verbatim, in
    /// order. Each already includes its trailing newline (Viewer
    /// [`Action::Command`] bytes). Empty when no command was emitted this batch.
    pub commands: Vec<Vec<u8>>,
    /// The native-surface reconcile plan, present iff the window/pane tree
    /// changed this batch (a [`Action::WindowsChanged`] was emitted). `None`
    /// leaves the native tabs untouched.
    pub plan: Option<ReconcilePlan>,
    /// tmux closed control mode, or the Viewer became defunct. The caller tears
    /// down every native tab bound to this session and drops the `TmuxSession`.
    pub exit: bool,
    /// The native surface the app should move keyboard focus to, present iff
    /// tmux's **active pane** changed this batch to a pane bound to a surface
    /// (ADR 006 slice 5e — tmux→app focus sync). Set e.g. when a fresh
    /// `split-window` makes its new pane active. `None` leaves focus untouched.
    /// A change the app *itself* initiated via `select_pane` does not surface
    /// here (the Viewer sets its active pane optimistically, so the echo is a
    /// no-op).
    pub focus: Option<SurfaceId>,
}

impl SessionUpdate {
    /// Whether this update carries nothing to act on (no commands, no plan, no
    /// exit, no focus) — the common steady-state case for a plain `%output`
    /// batch.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.plan.is_none() && !self.exit && self.focus.is_none()
    }
}

/// A live tmux control-mode session: a [`Viewer`] plus its [`Reconciler`]. One
/// is owned per `tmux -CC` connection (per control surface). Headless — it holds
/// no AppKit state; the native layer reads [`SessionUpdate`]s out of
/// [`ingest`](Self::ingest) and this session's pane terminals out of
/// [`pane_terminal`](Self::pane_terminal).
#[derive(Default)]
pub struct TmuxSession {
    viewer: Viewer,
    reconciler: Reconciler,
}

impl TmuxSession {
    /// A fresh session. Construct this on the [`Notification::Enter`] the engine
    /// surfaces when a pane enters tmux control mode. `colors` is the app's
    /// configured palette, seeded into every pane `Terminal` so tmux panes match
    /// the user's theme (ADR 006 theme fix).
    pub fn new(colors: Colors) -> TmuxSession {
        TmuxSession {
            viewer: Viewer::new(colors),
            reconciler: Reconciler::default(),
        }
    }

    /// Feed a drained batch of notifications through the Viewer, collecting the
    /// resulting actions, and (if the window/pane tree changed) reconcile once.
    ///
    /// The order within `commands` matches the Viewer's emission order, which is
    /// the order tmux must receive them (the command-queue keeps only one
    /// command in flight, so at most one is emitted per notification anyway).
    /// A single `plan` per batch coalesces multiple `WindowsChanged` signals
    /// into one native reconciliation.
    pub fn ingest(
        &mut self,
        notifications: impl IntoIterator<Item = Notification>,
    ) -> SessionUpdate {
        let mut update = SessionUpdate::default();
        let mut windows_changed = false;
        // tmux's active pane before this batch: used to detect a tmux-initiated
        // active-pane change (focus sync). A change the app initiated via
        // `select_pane` set the active pane optimistically already, so it won't
        // register as a change here — only genuine tmux-side moves do.
        let active_before = self.viewer.active_pane();

        for n in notifications {
            for action in self.viewer.next(n) {
                match action {
                    Action::Command(bytes) => update.commands.push(bytes),
                    Action::WindowsChanged => windows_changed = true,
                    Action::Exit => update.exit = true,
                }
            }
        }

        // Reconcile at most once per batch, after the whole batch is folded in,
        // so a burst of layout changes produces a single native plan. On exit
        // the native tabs are torn down wholesale, so no plan is needed.
        if windows_changed && !update.exit {
            update.plan = Some(self.reconciler.reconcile(self.viewer.windows()));
        }

        // Surface a tmux-initiated active-pane change as a focus target, resolved
        // through the (now up-to-date) reconciler map so a pane created by this
        // same batch (e.g. a `split-window`'s new active pane) resolves. Skipped
        // on exit (everything tears down).
        if !update.exit {
            let active_after = self.viewer.active_pane();
            if active_after != active_before
                && let Some(pane_id) = active_after
            {
                update.focus = self.reconciler.surface_of(pane_id);
            }
        }

        update
    }

    /// Read access to the underlying Viewer (window/pane model, tmux version,
    /// session id). The native layer queries this for pane geometry and state.
    pub fn viewer(&self) -> &Viewer {
        &self.viewer
    }

    /// The stable [`SurfaceId`] a tmux pane id is bound to, if assigned.
    pub fn surface_of(&self, pane_id: usize) -> Option<SurfaceId> {
        self.reconciler.surface_of(pane_id)
    }

    /// The pane `Terminal` a native [`SurfaceId`] renders, resolved through the
    /// Reconciler's stable `pane_id → SurfaceId` map and the Viewer's pane set.
    /// `None` if the surface id isn't bound to a live pane. This is the seam the
    /// display-only pane surface snapshots each frame (ADR 006 slice 5c).
    pub fn pane_terminal(&self, surface: SurfaceId) -> Option<&Terminal> {
        let pane_id = self.reconciler.pane_of_surface(surface)?;
        Some(self.viewer.pane(pane_id)?.terminal())
    }

    /// Whether the session has become defunct (tmux exited). Once defunct it
    /// should be dropped along with its native tabs.
    pub fn is_defunct(&self) -> bool {
        self.viewer.is_defunct()
    }

    /// Route keyboard input for the tmux pane a native [`SurfaceId`] renders,
    /// returning the control-pty command bytes to write (ADR 006 slice 5d).
    ///
    /// A display-only tmux pane surface has no pty; its key encoder output
    /// (`bytes`, UTF-8) is delivered to tmux as a `send-keys` control command on
    /// the control pty — the same channel [`SessionUpdate::commands`] uses. The
    /// surface id is resolved to its pane id through the Reconciler's stable map;
    /// input for an unbound surface (or before steady state) yields no commands.
    pub fn send_keys(&mut self, surface: SurfaceId, bytes: &[u8]) -> Vec<Vec<u8>> {
        let Some(pane_id) = self.reconciler.pane_of_surface(surface) else {
            return Vec::new();
        };
        commands_of(self.viewer.send_keys(pane_id, bytes))
    }

    /// Redirect a native split action targeting a tmux pane surface into a tmux
    /// `split-window` control command (ADR 006 slice 5e). `horizontal` picks a
    /// left/right (`-h`) vs top/bottom (`-v`) split; `before` (`-b`) places the
    /// new pane before (left/above) the target. Returns the control-pty bytes to
    /// write (empty for an unbound surface or before steady state). The new pane
    /// is created+rendered by the resulting `%layout-change` reconcile — the
    /// caller must NOT mutate the native `SplitTree` for a tmux-managed tab.
    pub fn split_pane(
        &mut self,
        surface: SurfaceId,
        horizontal: bool,
        before: bool,
    ) -> Vec<Vec<u8>> {
        let Some(pane_id) = self.reconciler.pane_of_surface(surface) else {
            return Vec::new();
        };
        commands_of(self.viewer.split_window(pane_id, horizontal, before))
    }

    /// Redirect a native new-tab action (Cmd-T) while focus is in a tmux window
    /// into a tmux `new-window` control command (ADR 006 slice 5e — Josh's
    /// iTerm2-style choice). Returns the control-pty bytes to write (empty
    /// before steady state). The new window appears as a fresh native tab via
    /// the `%window-add` reconcile — the caller must NOT create a normal tab.
    pub fn new_window(&mut self) -> Vec<Vec<u8>> {
        commands_of(self.viewer.new_window())
    }

    /// Redirect a native close-surface action (Cmd-W) on a tmux pane into a tmux
    /// `kill-pane` control command (ADR 006 slice 5e). Returns the control-pty
    /// bytes to write (empty for an unbound surface or before steady state). The
    /// native surface/tab is removed by the resulting `%layout-change` /
    /// `%window-close` reconcile — the caller must NOT mutate the native tree.
    pub fn kill_pane(&mut self, surface: SurfaceId) -> Vec<Vec<u8>> {
        let Some(pane_id) = self.reconciler.pane_of_surface(surface) else {
            return Vec::new();
        };
        commands_of(self.viewer.kill_pane(pane_id))
    }

    /// Redirect a native tab-close action on a tmux-managed tab into a tmux
    /// `kill-window` control command (ADR 006 slice 5e — gap 1). Unlike the
    /// pane/surface-keyed helpers this takes the tmux **window id** directly:
    /// the caller (the window-close delegate) already knows which tmux window a
    /// native tab mirrors (its key in `tmux_tabs`), and a tab close targets the
    /// whole window, not one pane. Returns the control-pty bytes to write (empty
    /// for an unknown window or before steady state). The native tab is removed
    /// by the follow-up `list-windows` reconcile — the caller must NOT close the
    /// native tab directly (I3).
    pub fn kill_window(&mut self, window_id: usize) -> Vec<Vec<u8>> {
        commands_of(self.viewer.kill_window(window_id))
    }

    /// Make the tmux pane a native surface renders the active pane (ADR 006
    /// slice 5e — app→tmux focus sync). Called when the app's keyboard focus
    /// moves to a tmux pane so bare `split-window` and the active-pane indicator
    /// operate on the pane the user is in. Returns the control-pty bytes to write
    /// (empty for an unbound surface, before steady state, or when that pane is
    /// already active).
    pub fn select_pane(&mut self, surface: SurfaceId) -> Vec<Vec<u8>> {
        let Some(pane_id) = self.reconciler.pane_of_surface(surface) else {
            return Vec::new();
        };
        commands_of(self.viewer.select_pane(pane_id))
    }

    /// Detach this `tmux -CC` client so the control process exits and its
    /// surface returns to a plain shell (ADR 006 slice 5e — orphan teardown /
    /// I1). Issued when a reconcile leaves the session with zero windows.
    /// Returns the control-pty bytes to write (empty outside steady state).
    pub fn detach_client(&mut self) -> Vec<Vec<u8>> {
        commands_of(self.viewer.detach_client())
    }
}

/// Keep only the `Command` bytes from a Viewer action list (the write helpers
/// only ever emit `Action::Command`, but this filters defensively).
fn commands_of(actions: Vec<Action>) -> Vec<Vec<u8>> {
    actions
        .into_iter()
        .filter_map(|a| match a {
            Action::Command(b) => Some(b),
            _ => None,
        })
        .collect()
}
