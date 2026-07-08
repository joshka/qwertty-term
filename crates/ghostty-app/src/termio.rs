//! The app's binding to the real termio stack (`ghostty-termio`, M2 chunk E).
//!
//! Replaces the interim `portable-pty`-backed `pty.rs`. A [`TabIo`] owns the
//! [`Termio`] hub (the io-writer thread + the two-stage read pipeline + the
//! exit watcher) and the cloneable [`Writer`], plus a channel the io threads
//! push surface events (`child_exited` / `password_input`) onto for the main
//! pace tick to drain.
//!
//! # Threading (see `docs/analysis/termio-hub.md` §3)
//!
//! The tab's engine is shared as `Arc<Mutex<Engine>>`:
//!
//! * the **parse thread** (inside the hub) locks it and applies pty output —
//!   the upstream `processOutput`-under-`renderer_state.mutex` design;
//! * the **main pace tick** locks it to snapshot/render and to drain engine
//!   reply bytes back to the pty.
//!
//! The writer is the main thread (input/paste/mouse/replies), and its send
//! policy is **non-blocking**: [`Writer::send`] never blocks the run loop; a
//! full queue drops the chunk (unreachable in practice — see §3.4). The
//! blocking `send_with_unlock` backpressure path is deliberately unused because
//! the app never holds the engine lock while sending.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use ghostty_termio::exec::{Command, Config, Notifier};
use ghostty_termio::hub::{HubHandler, Termio, Writer};
use ghostty_termio::size::{CellSize, GridSize, ScreenSize, Size};

use crate::engine::Engine;

/// A surface event pushed from an io thread to the main pace tick.
#[derive(Debug, Clone, Copy)]
pub enum IoEvent {
    /// The child shell exited (`exit_code`, `runtime_ms`). The tab shows an
    /// exit banner / closes (matching the interim `PtySession::child_exited`
    /// behavior).
    ChildExited { exit_code: u32, runtime_ms: u64 },
    /// The foreground program entered (`true`) or left (`false`) password
    /// input (canonical && !echo). Surfaced as a title suffix / log for M2-E.
    PasswordInput(bool),
}

/// The app's per-tab IO: the hub handle, the writer, and the surface-event
/// receiver. Dropping this joins the io threads (via `Termio`'s `Drop`).
pub struct TabIo {
    termio: Termio,
    writer: Writer,
    events: Receiver<IoEvent>,
    /// Rate-limit counter for dropped-write warnings (a full queue is
    /// pathological; we warn at most occasionally, never per keystroke).
    dropped_writes: AtomicU64,
}

/// Forwards chunk-D `Notifier` callbacks (fired on the io-exit / io-writer
/// threads) into the main-thread event channel. The io threads cannot touch
/// the `Rc<RefCell>` controller directly, so everything crosses here.
struct ChannelNotifier {
    tx: Sender<IoEvent>,
}

impl Notifier for ChannelNotifier {
    fn child_exited(&self, exit_code: u32, runtime_ms: u64) {
        let _ = self.tx.send(IoEvent::ChildExited {
            exit_code,
            runtime_ms,
        });
    }
    fn password_input(&self, active: bool) {
        let _ = self.tx.send(IoEvent::PasswordInput(active));
    }
}

/// The writer-thread terminal-touching seam: on the hub's 1s sync-output
/// reset, force-clear mode 2026 on the shared engine so a wedged program can't
/// freeze rendering (`docs/analysis/termio-hub.md` §4).
struct EngineHandler {
    engine: Arc<Mutex<Engine>>,
}

impl HubHandler for EngineHandler {
    fn on_sync_reset(&mut self) {
        if let Ok(mut e) = self.engine.lock() {
            e.reset_synchronized_output();
        }
    }
}

impl TabIo {
    /// Spawn a shell for `engine` at the given grid + cell size, optionally in
    /// `cwd`. The parse sink locks `engine` and applies pty output; the
    /// notifier + handler are wired to the shared engine / event channel.
    pub fn spawn(
        engine: Arc<Mutex<Engine>>,
        cols: u16,
        rows: u16,
        cell_width: u32,
        cell_height: u32,
        cwd: Option<&std::path::Path>,
    ) -> std::io::Result<TabIo> {
        let (tx, events) = channel();

        // Config: default shell ($SHELL), TERM/COLORTERM set by Exec; inherit
        // cwd when it exists (matches the interim path's `spawn_in_dir`).
        let command = std::env::var("SHELL")
            .ok()
            .map(|shell| Command::Direct(vec![shell]));
        let working_directory = cwd
            .filter(|p| p.is_dir())
            .map(|p| p.to_string_lossy().into_owned());
        let config = Config {
            command,
            working_directory,
            ..Config::default()
        };

        // Seed the pty winsize from the grid + cell metrics.
        let grid = GridSize {
            columns: cols,
            rows,
        };
        let screen = ScreenSize {
            width: u32::from(cols) * cell_width,
            height: u32::from(rows) * cell_height,
        };

        let notifier: Arc<dyn Notifier> = Arc::new(ChannelNotifier { tx });
        let exec = Termio::build_exec(config, grid, screen, notifier);

        // The parse sink: lock the shared engine, apply the batch. This runs on
        // the io-reader thread at line rate (§3.3).
        let sink: ghostty_termio::exec::Sink = {
            let engine = Arc::clone(&engine);
            Box::new(move |batch: &[u8]| {
                if let Ok(mut e) = engine.lock() {
                    e.write(batch);
                }
            })
        };

        let handler = EngineHandler {
            engine: Arc::clone(&engine),
        };
        let termio = Termio::spawn(exec, sink, handler)?;
        let writer = termio.writer();

        Ok(TabIo {
            termio,
            writer,
            events,
            dropped_writes: AtomicU64::new(0),
        })
    }

    /// Queue bytes to the pty (input, paste, mouse reports, engine replies).
    /// Non-blocking; a full write queue drops the chunk with a rate-limited
    /// warning rather than blocking the run loop (§3.4).
    pub fn write(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if !self.writer.write(bytes) {
            // Full queue: the writer loop is a full ring behind (64 undrained
            // writes). Pathological and effectively unreachable — a human can't
            // outrun a 60Hz-draining loop. Warn occasionally, never per key.
            let n = self.dropped_writes.fetch_add(1, Ordering::Relaxed);
            if n.is_multiple_of(64) {
                eprintln!(
                    "ghostty-app: pty write queue full, dropped {} input chunk(s)",
                    n + 1
                );
            }
        }
    }

    /// Post a resize to the hub (coalesced 25ms). `cell_*` are the current cell
    /// pixel metrics so the hub can derive the pty winsize.
    pub fn resize(&self, cols: u16, rows: u16, cell_width: u32, cell_height: u32) {
        let size = Size {
            screen: ScreenSize {
                width: u32::from(cols) * cell_width,
                height: u32::from(rows) * cell_height,
            },
            cell: CellSize {
                width: cell_width,
                height: cell_height,
            },
            ..Default::default()
        };
        let _ = self.writer.resize(size);
    }

    /// Post a focus change (starts/stops the 200ms termios password poll).
    pub fn focus(&self, focused: bool) {
        let _ = self.writer.focus(focused);
    }

    /// Drain any pending surface events (child-exit / password) for the pace
    /// tick to act on. Returns them in arrival order.
    pub fn drain_events(&self) -> Vec<IoEvent> {
        self.events.try_iter().collect()
    }

    /// Explicitly tear down the io threads. Also happens on `Drop`.
    pub fn shutdown(&mut self) {
        self.termio.shutdown();
    }
}
