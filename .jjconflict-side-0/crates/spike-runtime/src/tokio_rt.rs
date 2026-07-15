//! (b) tokio driver: current-thread runtime, `Notify` wakeup, `tokio::time`
//! timers, `select!` loop.
//!
//! Design mirrors `Thread.zig` one-for-one:
//!   * `xev.Async` (mailbox wakeup)  -> `tokio::sync::Notify`
//!   * `xev.Async` (stop)            -> a second `Notify`
//!   * `xev.Timer` (resize coalesce) -> `tokio::time::sleep_until` in `select!`
//!   * `xev.Timer` (sync reset)      -> another `sleep_until` arm
//!   * `loop.run(.until_done)`       -> `select!` in a loop on a
//!     `current_thread` runtime.
//!
//! The task prompt says "AsyncFd-style wakeup". For the spike there is no real
//! fd yet (the pty lands in chunk E), and `Notify` is the idiomatic, allocation
//! -free, thread-safe wakeup for exactly this "one task waits, many threads
//! signal" shape. When the pty fd arrives, `Notify` is swapped for
//! `AsyncFd<RawFd>` with no change to the mailbox API — that seam is isolated
//! here in [`NotifyWaker`].

use crate::driver::{Driver, DriverHandle, Handler, dispatch_batch, resize_window, sync_window};
use crate::mailbox::{Receiver, Waker};
use std::io;
use std::sync::Arc;
use tokio::runtime::Builder;
use tokio::sync::Notify;
use tokio::time::{Instant as TokioInstant, sleep_until};

/// Waker backed by `tokio::sync::Notify`. `notify_one()` is thread-safe,
/// non-blocking, and coalescing (a notify with no waiter is remembered, and
/// repeated notifies before a wait collapse) — matching xev.Async semantics.
pub struct NotifyWaker {
    notify: Arc<Notify>,
}

impl Waker for NotifyWaker {
    fn wake(&self) {
        self.notify.notify_one();
    }
}

/// Stop handle: a separate `Notify`. Signalling it drops the loop out of
/// `select!`.
#[derive(Clone)]
pub struct TokioHandle {
    stop: Arc<Notify>,
}

impl DriverHandle for TokioHandle {
    fn stop(&self) {
        self.stop.notify_one();
    }
}

/// The tokio current-thread driver.
pub struct TokioDriver {
    wakeup: Arc<Notify>,
    stop: Arc<Notify>,
    waker: Arc<NotifyWaker>,
}

impl Driver for TokioDriver {
    type Waker = NotifyWaker;
    type Handle = TokioHandle;

    fn new() -> io::Result<Self> {
        let wakeup = Arc::new(Notify::new());
        let stop = Arc::new(Notify::new());
        let waker = Arc::new(NotifyWaker {
            notify: wakeup.clone(),
        });
        Ok(TokioDriver {
            wakeup,
            stop,
            waker,
        })
    }

    fn waker(&self) -> Arc<NotifyWaker> {
        self.waker.clone()
    }

    fn handle(&self) -> TokioHandle {
        TokioHandle {
            stop: self.stop.clone(),
        }
    }

    fn run(self, recv: Receiver, mut handler: impl Handler) -> io::Result<()> {
        // current_thread: single logical thread, no work-stealing scheduler.
        // enable_time for the timers; no enable_io needed until the pty fd.
        let rt = Builder::new_current_thread().enable_time().build()?;

        rt.block_on(async move {
            let wakeup = self.wakeup;
            let stop = self.stop;
            let mut batch: Vec<crate::mailbox::Message> =
                Vec::with_capacity(crate::mailbox::CAPACITY);

            // Timer arms. We keep an Option<Instant> and rebuild the sleep each
            // loop iteration; a disarmed timer sleeps "forever" (far future) so
            // its select arm never wins.
            let mut resize_deadline: Option<TokioInstant> = None;
            let mut sync_deadline: Option<TokioInstant> = None;
            let mut pending_resize: Option<(u16, u16)> = None;

            loop {
                let far_future = TokioInstant::now() + std::time::Duration::from_secs(3600);
                let resize_at = resize_deadline.unwrap_or(far_future);
                let sync_at = sync_deadline.unwrap_or(far_future);

                tokio::select! {
                    biased;

                    // Stop wins so shutdown is prompt.
                    _ = stop.notified() => break,

                    // Mailbox wakeup: drain + dispatch.
                    _ = wakeup.notified() => {
                        batch.clear();
                        let n = recv.drain(&mut batch);
                        if n > 0 {
                            let (latest_resize, saw_sync) =
                                dispatch_batch(&batch, &mut handler);
                            if let Some(r) = latest_resize {
                                pending_resize = Some(r);
                                // Only arm if idle (Zig: don't reset in-flight).
                                if resize_deadline.is_none() {
                                    resize_deadline =
                                        Some(TokioInstant::now() + resize_window());
                                }
                            }
                            if saw_sync {
                                // Reset on every start (Zig semantics).
                                sync_deadline =
                                    Some(TokioInstant::now() + sync_window());
                            }
                        }
                    }

                    _ = sleep_until(resize_at), if resize_deadline.is_some() => {
                        resize_deadline = None;
                        if let Some((cols, rows)) = pending_resize.take() {
                            handler.on_resize(cols, rows);
                        }
                    }

                    _ = sleep_until(sync_at), if sync_deadline.is_some() => {
                        sync_deadline = None;
                        handler.on_sync_reset();
                    }
                }
            }
        });
        Ok(())
    }
}

/// Spawn the tokio driver on its own OS thread (it owns a current-thread
/// runtime inside). Convenience for benches/tests.
pub fn spawn(
    recv: Receiver,
    handler: impl Handler,
) -> io::Result<(
    std::thread::JoinHandle<io::Result<()>>,
    Arc<NotifyWaker>,
    TokioHandle,
)> {
    let driver = TokioDriver::new()?;
    let waker = driver.waker();
    let handle = driver.handle();
    let join = std::thread::Builder::new()
        .name("io-tokio".into())
        .spawn(move || driver.run(recv, handler))?;
    Ok((join, waker, handle))
}
