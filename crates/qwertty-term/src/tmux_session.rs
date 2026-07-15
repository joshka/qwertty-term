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
}

impl SessionUpdate {
    /// Whether this update carries nothing to act on (no commands, no plan, no
    /// exit) — the common steady-state case for a plain `%output` batch.
    pub fn is_empty(&self) -> bool {
        self.commands.is_empty() && self.plan.is_none() && !self.exit
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
        self.viewer
            .send_keys(pane_id, bytes)
            .into_iter()
            .filter_map(|a| match a {
                Action::Command(b) => Some(b),
                _ => None,
            })
            .collect()
    }
}
