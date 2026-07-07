//! The `Driver` seam: the event-loop semantics of `Thread.zig`, expressed once
//! and implemented twice (threads, tokio).
//!
//! A driver owns the consumer side. It runs an event loop that:
//!   * wakes when the mailbox is notified and drains it (calling the handler),
//!   * runs a 25ms **resize coalescing** timer: repeated resizes within the
//!     window collapse to one `on_resize`,
//!   * runs a 1s **sync-output reset** timeout, re-armed each time synchronized
//!     output starts,
//!   * stops cleanly on request.
//!
//! Both implementations satisfy [`Handler`] callbacks with identical ordering
//! guarantees so the benchmark and the eventual termio code don't care which
//! runtime is underneath.

use crate::mailbox::Message;
use std::time::Duration;

/// Milliseconds to coalesce resize events. Ported from `Coalesce.min_ms`.
pub const RESIZE_COALESCE_MS: u64 = 25;

/// Milliseconds before synchronized output is force-reset. Ported from
/// `sync_reset_ms`.
pub const SYNC_RESET_MS: u64 = 1000;

/// Callbacks the driver invokes on the consumer thread. The real termio code
/// implements this; the benchmark implements a counting version.
///
/// All methods run on the driver's own thread, serialized — never concurrently.
pub trait Handler: Send + 'static {
    /// A batch of messages was drained from the mailbox. Resize messages are
    /// intercepted by the driver for coalescing and are NOT delivered here;
    /// everything else is. Called with the whole batch so the handler can do
    /// its own bookkeeping (e.g. "redraw once per drain").
    fn on_messages(&mut self, batch: &[Message]);

    /// The resize coalescing timer fired. `cols`/`rows` are the most recent
    /// resize seen during the window. Called at most once per 25ms burst.
    fn on_resize(&mut self, cols: u16, rows: u16);

    /// The 1s synchronized-output timer expired without the program clearing
    /// it. Reset the mode.
    fn on_sync_reset(&mut self);
}

/// Runtime-agnostic handle to control a running driver from another thread.
pub trait DriverHandle: Send {
    /// Ask the loop to stop and return. Idempotent.
    fn stop(&self);
}

/// The trait both runtimes implement. `run` blocks the calling thread and owns
/// the event loop until stopped.
pub trait Driver {
    /// Opaque per-runtime waker type handed to the mailbox producers.
    type Waker: crate::mailbox::Waker;
    /// Handle used to stop the loop from outside.
    type Handle: DriverHandle;

    /// Build the runtime primitives. Returns the waker (for mailbox producers)
    /// and a stop handle. Does not start the loop.
    fn new() -> std::io::Result<Self>
    where
        Self: Sized;

    /// The waker this driver's loop listens on. Clone into the mailbox.
    fn waker(&self) -> std::sync::Arc<Self::Waker>;

    /// A handle to stop the loop from another thread.
    fn handle(&self) -> Self::Handle;

    /// Run the event loop until stopped. `recv` is the mailbox consumer;
    /// `handler` receives callbacks. Blocks the current thread.
    fn run(self, recv: crate::mailbox::Receiver, handler: impl Handler) -> std::io::Result<()>;
}

/// Small helper shared by both impls: given a freshly drained batch, split off
/// resize coalescing and sync-reset arming, forward the rest to the handler.
/// Returns `(latest_resize, saw_sync_start)`.
pub(crate) fn dispatch_batch(
    batch: &[Message],
    handler: &mut impl Handler,
) -> (Option<(u16, u16)>, bool) {
    let mut latest_resize = None;
    let mut saw_sync = false;
    // Deliver everything except resizes to the handler; intercept resize/sync.
    let mut forward: Vec<Message> = Vec::with_capacity(batch.len());
    for m in batch {
        match m {
            Message::Resize { cols, rows } => latest_resize = Some((*cols, *rows)),
            Message::StartSynchronizedOutput => {
                saw_sync = true;
                forward.push(m.clone());
            }
            other => forward.push(other.clone()),
        }
    }
    if !forward.is_empty() {
        handler.on_messages(&forward);
    }
    (latest_resize, saw_sync)
}

/// The coalescing window as a `Duration`.
pub(crate) fn resize_window() -> Duration {
    Duration::from_millis(RESIZE_COALESCE_MS)
}

/// The sync-reset timeout as a `Duration`.
pub(crate) fn sync_window() -> Duration {
    Duration::from_millis(SYNC_RESET_MS)
}
