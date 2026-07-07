//! (a) OS-thread driver: `polling` crate + a manual timer wheel.
//!
//! Why `polling` over `mio`:
//!   * `Poller::notify()` is a first-class, thread-safe, coalescing wakeup —
//!     an exact analogue of `xev.Async.notify()`. `mio` requires constructing
//!     a `Waker` bound to a `Registry` and a reserved token; more moving parts
//!     for the same effect.
//!   * We don't register any real fds in this spike (the pty fd lands in
//!     chunk E). `polling` lets us block in `wait()` with a timeout and be
//!     woken purely by `notify()`, which is all the writer loop needs today.
//!   * Timers: neither crate has them. We implement a tiny two-slot timer wheel
//!     (resize + sync-reset) and compute the `wait()` timeout as the nearest
//!     deadline. This is what xev gives us for free; here it's ~30 lines.
//!
//! The loop is: compute next timeout -> `poll.wait(timeout)` -> on wake, drain
//! mailbox + fire any expired timers -> repeat. `stop()` sets a flag and
//! notifies so `wait()` returns promptly.

use crate::driver::{Driver, DriverHandle, Handler, dispatch_batch, resize_window, sync_window};
use crate::mailbox::{Receiver, Waker};
use polling::{Events, Poller};
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Waker backed by `Poller::notify`. Cloning shares the same poller, so any
/// producer thread can kick the loop.
pub struct PollWaker {
    poller: Arc<Poller>,
}

impl Waker for PollWaker {
    fn wake(&self) {
        // notify() is coalescing and non-blocking — exactly xev.Async.notify().
        let _ = self.poller.notify();
    }
}

/// Stop handle: flips a flag then notifies the poller so `wait()` unblocks.
#[derive(Clone)]
pub struct ThreadsHandle {
    stop: Arc<AtomicBool>,
    poller: Arc<Poller>,
}

impl DriverHandle for ThreadsHandle {
    fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.poller.notify();
    }
}

/// The OS-thread + `polling` driver.
pub struct ThreadsDriver {
    poller: Arc<Poller>,
    waker: Arc<PollWaker>,
    stop: Arc<AtomicBool>,
}

impl Driver for ThreadsDriver {
    type Waker = PollWaker;
    type Handle = ThreadsHandle;

    fn new() -> io::Result<Self> {
        let poller = Arc::new(Poller::new()?);
        let waker = Arc::new(PollWaker {
            poller: poller.clone(),
        });
        Ok(ThreadsDriver {
            poller,
            waker,
            stop: Arc::new(AtomicBool::new(false)),
        })
    }

    fn waker(&self) -> Arc<PollWaker> {
        self.waker.clone()
    }

    fn handle(&self) -> ThreadsHandle {
        ThreadsHandle {
            stop: self.stop.clone(),
            poller: self.poller.clone(),
        }
    }

    fn run(self, recv: Receiver, mut handler: impl Handler) -> io::Result<()> {
        let mut events = Events::new();
        // Two-slot timer wheel. `None` == disarmed.
        let mut resize_deadline: Option<Instant> = None;
        let mut sync_deadline: Option<Instant> = None;
        let mut pending_resize: Option<(u16, u16)> = None;
        let mut batch: Vec<crate::mailbox::Message> = Vec::with_capacity(crate::mailbox::CAPACITY);

        loop {
            if self.stop.load(Ordering::SeqCst) {
                break;
            }

            // Compute the nearest timer deadline -> wait timeout.
            let now = Instant::now();
            let next = [resize_deadline, sync_deadline].into_iter().flatten().min();
            let timeout = next.map(|d| d.saturating_duration_since(now));

            events.clear();
            // We register no sources; wakeups come purely from notify() and
            // the timeout. This is the "idle" path — parks the thread in the
            // kernel with zero CPU until notified or the timer fires.
            self.poller.wait(&mut events, timeout)?;

            if self.stop.load(Ordering::SeqCst) {
                break;
            }

            // Drain the mailbox (one lock acquisition, handlers after unlock).
            batch.clear();
            let n = recv.drain(&mut batch);
            if n > 0 {
                let (latest_resize, saw_sync) = dispatch_batch(&batch, &mut handler);
                if let Some(r) = latest_resize {
                    pending_resize = Some(r);
                    // Zig: if the coalesce timer is already active, DON'T reset
                    // it — let the in-flight window expire. Only arm if idle.
                    if resize_deadline.is_none() {
                        resize_deadline = Some(Instant::now() + resize_window());
                    }
                }
                if saw_sync {
                    // Zig: sync-reset timer is RESET on every start.
                    sync_deadline = Some(Instant::now() + sync_window());
                }
            }

            // Fire expired timers.
            let now = Instant::now();
            if let Some(d) = resize_deadline
                && now >= d
            {
                resize_deadline = None;
                if let Some((cols, rows)) = pending_resize.take() {
                    handler.on_resize(cols, rows);
                }
            }
            if let Some(d) = sync_deadline
                && now >= d
            {
                sync_deadline = None;
                handler.on_sync_reset();
            }
        }
        Ok(())
    }
}

/// Spawn the driver on its own OS thread. Returns the join handle plus the
/// waker and stop handle. Convenience for benches/tests.
pub fn spawn(
    recv: Receiver,
    handler: impl Handler,
) -> io::Result<(
    std::thread::JoinHandle<io::Result<()>>,
    Arc<PollWaker>,
    ThreadsHandle,
)> {
    let driver = ThreadsDriver::new()?;
    let waker = driver.waker();
    let handle = driver.handle();
    let join = std::thread::Builder::new()
        .name("io".into())
        .spawn(move || driver.run(recv, handler))?;
    Ok((join, waker, handle))
}

/// Duration re-export so callers don't need a separate `std::time` import.
pub type Ms = Duration;
