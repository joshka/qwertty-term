//! The Termio hub + the promoted writer `Thread` loop. Port of
//! `src/termio/Termio.zig` (the state-container / wiring role) and
//! `src/termio/Thread.zig` (the writer event loop), Ghostty `2da015cd6`.
//!
//! Analysis: `docs/analysis/termio-hub.md`. This is M2 chunk E from
//! `docs/plans/m2-termio.md`.
//!
//! # What this adds over chunk D
//!
//! Chunk D ([`crate::exec`]) built [`Exec`] (spawn, the two-stage read
//! pipeline, the exit watcher, the termios poll) and a minimal condvar-parked
//! [`crate::exec::WriterLoop`] that its integration tests drive synchronously.
//! Chunk E promotes the ADR-002 `Driver`/`Handler` seam from
//! `crates/spike-runtime` into the production [`Thread`] loop:
//!
//! * the real `polling::Poller` wakeup (≙ `xev.Async.notify`) + park,
//! * the two-slot timer wheel (25ms resize coalesce, 1s sync-output reset),
//!   folded together with chunk D's 200ms termios poll,
//! * the terminal-touching handlers (sync-reset → force-clear mode 2026)
//!   routed through a [`HubHandler`] seam the caller (the app) fills, so the
//!   hub never names an engine type,
//! * a [`Termio`] struct that owns the `Exec`, spins the loop on its own OS
//!   thread, and hands back a cloneable [`Writer`] + a stop/join handle.
//!
//! # Threading (see `docs/analysis/termio-hub.md` §3)
//!
//! Upstream drives the parse sink under `renderer_state.mutex`. The R5 app has
//! no render mutex (single-threaded main loop), so the app supplies a sink that
//! locks its own `Arc<Mutex<Engine>>` and applies bytes there — the same
//! "apply behind the lock the renderer also takes" design, with the app's
//! pace-tick standing in for upstream's renderer thread. That lock is the
//! app's business; the hub only carries the `Sink` closure to the io-reader
//! thread and the [`HubHandler`] to the writer thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use polling::{Events, Poller};

use crate::exec::{Config, Exec, Sink, ThreadData};
use crate::mailbox::{self, Receiver, Sender, Waker};
use crate::message::Message;
use crate::size::{GridSize, Size};

/// The 25ms resize coalesce window. Port of `Coalesce.min_ms` /
/// `RESIZE_COALESCE_MS`.
pub const RESIZE_COALESCE_MS: u64 = 25;

/// The 1s synchronized-output reset. Port of `sync_reset_ms` / `SYNC_RESET_MS`.
pub const SYNC_RESET_MS: u64 = 1000;

/// The 200ms termios password poll. Re-export of the chunk-D constant so the
/// loop's timer set reads in one place.
pub use crate::exec::TERMIOS_POLL_MS;

/// The terminal-touching side of the writer loop the hub cannot own itself
/// (it would have to name an engine type). The app implements it; the hub
/// invokes it from the writer thread. All methods run serialized on the writer
/// thread — never concurrently with each other, but concurrently with the
/// io-reader sink and the app's pace tick, so implementations must take
/// whatever lock guards the shared engine (the same lock the sink takes).
///
/// Port of the `Termio` handlers `Thread.drainMailbox` dispatches into
/// (`Termio.zig` `colorSchemeReport`, `sizeReport`, `resetSynchronizedOutput`,
/// …). M2-E fills only the ones the app exercises; the rest default to no-ops.
pub trait HubHandler: Send + 'static {
    /// The 1s synchronized-output timer expired without the program clearing
    /// mode 2026 — force-clear it so a wedged program can't freeze rendering.
    /// Port of the sync-reset path (`Thread.zig` `sync_reset` /
    /// `Termio.resetSynchronizedOutput`). Default: no-op.
    fn on_sync_reset(&mut self) {}

    /// A drain completed; poke the renderer so it repaints the drained state.
    /// Port of `renderer_wakeup.notify()` after each `drainMailbox`. Default:
    /// no-op (the app's pace tick renders on its own cadence).
    fn on_drained(&mut self) {}
}

/// A `HubHandler` that does nothing — for tests / callers without an engine.
pub struct NullHandler;
impl HubHandler for NullHandler {}

// =========================================================================
// The writer Thread loop (promoted Driver, targeting the real Message + Exec)
// =========================================================================

/// A [`Waker`] backed by `Poller::notify`. Cloning shares the poller, so any
/// producer thread can kick the loop. Promoted verbatim from
/// `spike-runtime::threads::PollWaker`.
pub struct PollWaker {
    poller: Arc<Poller>,
}

impl Waker for PollWaker {
    fn wake(&self) {
        // notify() is coalescing and non-blocking — exactly xev.Async.notify().
        let _ = self.poller.notify();
    }
}

/// Stop handle for the writer loop: flips a flag then notifies the poller so
/// `wait()` unblocks promptly. Promoted from `spike-runtime::threads`.
#[derive(Clone)]
struct StopHandle {
    stop: Arc<AtomicBool>,
    poller: Arc<Poller>,
}

impl StopHandle {
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.poller.notify();
    }
}

/// The writer event loop. Owns the `Exec` + its `ThreadData` and the running
/// timer wheel; drains the mailbox on each wakeup. Port of `Thread.zig`'s
/// `threadMain_` + `drainMailbox`, promoted from the spike `ThreadsDriver`.
struct Thread<H: HubHandler> {
    exec: Exec,
    td: ThreadData,
    handler: H,
    poller: Arc<Poller>,
    stop: Arc<AtomicBool>,
    /// Linefeed mode (mode 20) — `\r` → `\r\n` on writes.
    linefeed_mode: bool,
    /// The two-slot timer wheel + the coalesced resize.
    resize_deadline: Option<Instant>,
    sync_deadline: Option<Instant>,
    termios_deadline: Instant,
    pending_resize: Option<Size>,
}

impl<H: HubHandler> Thread<H> {
    /// Run the loop until stopped, then tear down. Port of `threadMain_`.
    fn run(mut self, recv: Receiver) {
        let mut events = Events::new();
        let mut batch: Vec<Message> = Vec::with_capacity(mailbox::CAPACITY);

        loop {
            if self.stop.load(Ordering::SeqCst) {
                break;
            }

            // Park until notified or the nearest timer fires.
            let now = Instant::now();
            let next = [
                self.resize_deadline,
                self.sync_deadline,
                Some(self.termios_deadline),
            ]
            .into_iter()
            .flatten()
            .min();
            let timeout = next.map(|d| d.saturating_duration_since(now));

            events.clear();
            // We register no fds: wakeups come from notify() (mailbox) and the
            // timeout. The pty read side is its own dedicated thread pair
            // (decision 4), so the writer loop parks at zero idle CPU.
            if self.poller.wait(&mut events, timeout).is_err() {
                break;
            }

            if self.stop.load(Ordering::SeqCst) {
                break;
            }

            // Drain the mailbox (one lock, handlers after unlock).
            batch.clear();
            let n = recv.drain(&mut batch);
            if n > 0 {
                self.dispatch(&batch);
                self.handler.on_drained();
            }

            self.fire_timers();
        }

        // Teardown (stop child → quit pipe → join). Port of threadExit ordering.
        self.exec.thread_exit(&mut self.td);
    }

    /// Dispatch one drained batch. Writes/focus/linefeed apply immediately;
    /// resize is coalesced; sync-output arms the reset timer. Terminal-touching
    /// variants beyond sync are deferred (see `docs/analysis/termio-hub.md`
    /// §6). Port of the Exec-relevant subset of `drainMailbox`.
    fn dispatch(&mut self, batch: &[Message]) {
        for msg in batch {
            match msg {
                Message::WriteSmall(small) => {
                    let _ =
                        self.exec
                            .queue_write(&mut self.td, small.as_slice(), self.linefeed_mode);
                }
                Message::WriteStable(v) => {
                    let _ = self.exec.queue_write(&mut self.td, v, self.linefeed_mode);
                }
                Message::WriteAlloc(v) => {
                    let _ = self.exec.queue_write(&mut self.td, v, self.linefeed_mode);
                }
                Message::Resize(size) => {
                    self.pending_resize = Some(*size);
                    // Zig coalesce rule: arm only when idle; an in-flight
                    // window is NOT reset.
                    if self.resize_deadline.is_none() {
                        self.resize_deadline =
                            Some(Instant::now() + Duration::from_millis(RESIZE_COALESCE_MS));
                    }
                }
                Message::LinefeedMode(v) => self.linefeed_mode = *v,
                Message::Focused(v) => self.exec.focus_gained(&mut self.td, *v),
                Message::StartSynchronizedOutput => {
                    // Reset on every start (Zig sync-reset arming).
                    self.sync_deadline =
                        Some(Instant::now() + Duration::from_millis(SYNC_RESET_MS));
                }
                // Terminal-touching variants land beyond M2-E (see analysis §6).
                _ => {}
            }
        }
    }

    /// Fire any expired timers (resize coalesce, sync reset, termios poll).
    /// Port of the timer-callback half of `Thread.zig`.
    fn fire_timers(&mut self) {
        let now = Instant::now();

        if self.resize_deadline.is_some_and(|d| now >= d) {
            self.resize_deadline = None;
            if let Some(size) = self.pending_resize.take() {
                let grid = grid_from_size(&size);
                let _ = self.exec.resize(grid, size.screen);
            }
        }

        if self.sync_deadline.is_some_and(|d| now >= d) {
            self.sync_deadline = None;
            self.handler.on_sync_reset();
        }

        if now >= self.termios_deadline {
            let _keep = self.exec.termios_tick(&mut self.td);
            // Always reschedule; focus toggling is handled inside termios_tick
            // (a re-focus resumes promptly without a separate arm/disarm).
            self.termios_deadline = now + Duration::from_millis(TERMIOS_POLL_MS);
        }
    }
}

/// Derive the grid dims a resize collapses to from the pixel `Size`.
fn grid_from_size(size: &Size) -> GridSize {
    GridSize {
        columns: (size.screen.width / size.cell.width.max(1)) as u16,
        rows: (size.screen.height / size.cell.height.max(1)) as u16,
    }
}

// =========================================================================
// The Termio hub
// =========================================================================

/// A cloneable writer handle: posts messages to the io-writer loop's mailbox.
/// This is the app's producer side (input encode, paste, mouse, engine
/// replies, resize, focus). Port of the `Termio.mailbox` producer face.
///
/// The send policy is **non-blocking** (see `docs/analysis/termio-hub.md`
/// §3.4): the app's producer is the main run loop, and blocking it on a full
/// write queue would freeze the UI. [`Writer::send`] is `try_send` + `notify`;
/// on a full queue it re-notifies and returns `false` (the caller drops the
/// chunk rather than blocking). The blocking `send_with_unlock` backpressure
/// path stays in [`crate::mailbox`] for future non-main-thread producers.
#[derive(Clone)]
pub struct Writer {
    tx: Sender,
}

impl Writer {
    /// Post `msg` to the writer loop. Non-blocking. Returns `false` if the
    /// queue was full (message dropped) — the caller decides what to do
    /// (the app drops the input chunk with a rate-limited warning; this is
    /// unreachable in practice, see §3.4). On a full queue we still re-notify
    /// so the loop drains promptly.
    #[must_use]
    pub fn send(&self, msg: Message) -> bool {
        match self.tx.send(msg) {
            Ok(()) => true,
            Err(mailbox::TrySendError::Full(_dropped)) => {
                self.tx.notify();
                false
            }
        }
    }

    /// Queue raw bytes to the pty (input, paste, replies). Convenience over
    /// [`Writer::send`] + [`Message::write_req`].
    #[must_use]
    pub fn write(&self, bytes: &[u8]) -> bool {
        self.send(Message::write_req(bytes))
    }

    /// Post a resize (coalesced 25ms in the loop).
    #[must_use]
    pub fn resize(&self, size: Size) -> bool {
        self.send(Message::Resize(size))
    }

    /// Post a focus change (starts/stops the termios poll).
    #[must_use]
    pub fn focus(&self, focused: bool) -> bool {
        self.send(Message::Focused(focused))
    }
}

/// The Termio hub: owns the io-writer thread and the writer handle, and joins
/// the whole IO stack on [`Termio::shutdown`]. Port of the lifecycle role of
/// `termio.Termio` + `termio.Thread` (the state container is split — the
/// terminal itself lives in the app's engine behind the app's lock, per §3.3).
pub struct Termio {
    writer: Writer,
    stop: StopHandle,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Termio {
    /// Spawn the subprocess, the read pipeline, the exit watcher, and the
    /// io-writer loop on its own OS thread. Port of `Termio.init` +
    /// `Thread.threadMain` start-up, folding `Options.zig` into the args.
    ///
    /// * `config` — the Exec config (command, env, cwd, term).
    /// * `initial` — the starting grid/screen size (seeds the pty winsize).
    /// * `sink` — the parse sink; runs on the io-reader thread. The app hands
    ///   a closure that locks its engine and applies bytes (§3.3).
    /// * `notifier` — child-exit / password-input surface hooks (chunk D
    ///   seam); the app forwards them to the main thread.
    /// * `handler` — the writer-thread terminal-touching seam (sync reset,
    ///   renderer wakeup).
    pub fn spawn<H: HubHandler>(mut exec: Exec, sink: Sink, handler: H) -> std::io::Result<Termio> {
        // Start the subprocess + IO threads (Exec::thread_enter).
        let td = exec
            .thread_enter(sink)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        // The writer loop's poller + waker + stop flag.
        let poller = Arc::new(Poller::new()?);
        let stop = Arc::new(AtomicBool::new(false));
        let waker: Arc<dyn Waker> = Arc::new(PollWaker {
            poller: Arc::clone(&poller),
        });
        let (tx, rx) = mailbox::channel(waker);

        let stop_handle = StopHandle {
            stop: Arc::clone(&stop),
            poller: Arc::clone(&poller),
        };

        let thread = Thread {
            exec,
            td,
            handler,
            poller,
            stop,
            linefeed_mode: false,
            resize_deadline: None,
            sync_deadline: None,
            termios_deadline: Instant::now() + Duration::from_millis(TERMIOS_POLL_MS),
            pending_resize: None,
        };

        let join = std::thread::Builder::new()
            .name("io-writer".to_string())
            .spawn(move || thread.run(rx))?;

        Ok(Termio {
            writer: Writer { tx },
            stop: stop_handle,
            join: Some(join),
        })
    }

    /// Build the `Exec` for [`Termio::spawn`] from a [`Config`] + initial size.
    /// Convenience so callers don't reach into the `exec` module. Ports the
    /// `initTerminal` size seeding.
    pub fn build_exec(
        config: Config,
        initial: GridSize,
        screen: crate::size::ScreenSize,
        notifier: Arc<dyn crate::exec::Notifier>,
    ) -> Exec {
        let mut exec = Exec::init(config);
        exec.set_notifier(notifier);
        exec.set_initial_size(initial, screen);
        exec
    }

    /// The cloneable writer handle. Clone into every producer (input, paste,
    /// mouse, resize, replies).
    pub fn writer(&self) -> Writer {
        self.writer.clone()
    }

    /// Stop the io-writer loop and join every IO thread. Port of the
    /// `Thread.stop` + `threadExit` join. Idempotent-ish: safe to call once;
    /// the `Drop` impl calls it if the caller didn't.
    pub fn shutdown(&mut self) {
        self.stop.stop();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for Termio {
    fn drop(&mut self) {
        self.shutdown();
    }
}
