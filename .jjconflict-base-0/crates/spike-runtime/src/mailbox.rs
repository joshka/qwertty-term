//! The termio writer mailbox — the API that is IDENTICAL under both runtimes.
//!
//! This is the Rust port of `src/termio/mailbox.zig` +
//! `src/datastruct/blocking_queue.zig` from Ghostty (`2da015cd6`).
//!
//! # Contract (survives the threads-vs-tokio decision)
//!
//! * **Bounded, fixed capacity 64.** Matches `BlockingQueue(Message, 64)`.
//!   Chosen empirically upstream; a small queue that drains fast is preferred
//!   over an unbounded one that can hide runaway producers.
//! * **SPSC.** Exactly one consumer (the writer loop). Any number of producer
//!   handles ([`Sender`]) may exist, but in the real system there is one
//!   logical producer (the surface/render thread). We keep the mutex+condvar
//!   so a `send_blocking` from any thread is correct.
//! * **Two send paths:**
//!   * [`Sender::try_send`] — non-blocking. Returns [`TrySendError::Full`]
//!     immediately if the queue is full. This is the fast path (Zig
//!     `push(.instant)`).
//!   * [`Sender::send_with_unlock`] — the backpressure-unlock path (Zig
//!     `Mailbox.send(msg, mutex)`). The caller is holding a lock the consumer
//!     needs (the renderer state mutex). If the queue is full we **unlock that
//!     lock**, wake the consumer, block until space frees, push, then re-lock.
//!     Without this, a full queue + held render lock deadlocks: the producer
//!     waits for space, the consumer waits for the lock to make space.
//! * **Wakeup is decoupled from enqueue.** Enqueue never implicitly notifies
//!   the consumer; the producer calls [`Sender::notify`] (or the send helpers
//!   do it for the full-queue case). This mirrors xev's async handle: the queue
//!   holds data, a separate one-shot waker kicks the event loop. This is what
//!   lets a runtime coalesce many enqueues into one loop wakeup.
//!
//! The [`Waker`] trait is the ONLY runtime-specific seam. `try_send`,
//! `send_with_unlock`, `drain`, capacity, and message ordering are runtime
//! agnostic.

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};

/// Fixed mailbox capacity. Ported verbatim from `BlockingQueue(Message, 64)`.
pub const CAPACITY: usize = 64;

/// The writer-thread message union. This is a stand-in for the real
/// `termio.Message` (chunk B) — enough variants to exercise the runtime:
/// small inline writes, a heap-allocated write, a resize (coalesced), and a
/// synchronized-output start (arms the 1s reset timer).
#[derive(Debug, Clone)]
pub enum Message {
    /// Inline small write (Zig `write_small`): bytes copied into the message.
    WriteSmall { data: [u8; 38], len: u8 },
    /// Heap write (Zig `write_alloc`): owned buffer.
    WriteAlloc(Vec<u8>),
    /// Resize — coalesced on a 25ms timer.
    Resize { cols: u16, rows: u16 },
    /// Start synchronized output — arms/resets the 1s sync-reset timer.
    StartSynchronizedOutput,
    /// Linefeed mode toggle (cheap flag flip; used as a tiny flood message).
    LinefeedMode(bool),
}

impl Message {
    /// Build a small write from a byte slice (truncated to inline capacity).
    pub fn small(bytes: &[u8]) -> Self {
        let mut data = [0u8; 38];
        let len = bytes.len().min(38);
        data[..len].copy_from_slice(&bytes[..len]);
        Message::WriteSmall {
            data,
            len: len as u8,
        }
    }
}

/// The one runtime-specific seam: something that can kick the consumer's event
/// loop from an arbitrary producer thread. Maps to `xev.Async` under threads
/// and to a `tokio` notify/pipe under tokio. Cheap and idempotent: repeated
/// notifies before the consumer drains collapse into one wakeup.
pub trait Waker: Send + Sync + 'static {
    /// Wake the consumer loop. Must be safe to call from any thread and must
    /// never block.
    fn wake(&self);
}

/// Shared queue state. Mutex + condvar exactly like the Zig `BlockingQueue`:
/// the condvar is signalled on `pop` (queue became not-full) so a blocked
/// `send_with_unlock` can proceed. There is deliberately NO not-empty condvar
/// — emptiness is communicated out of band via the [`Waker`].
struct Inner {
    queue: Mutex<VecDeque<Message>>,
    not_full: Condvar,
}

/// Producer handle. Cloneable; all clones share one queue and one waker.
#[derive(Clone)]
pub struct Sender {
    inner: Arc<Inner>,
    waker: Arc<dyn Waker>,
}

/// Consumer handle. Not cloneable — single consumer.
pub struct Receiver {
    inner: Arc<Inner>,
}

/// Error from the non-blocking [`Sender::try_send`].
#[derive(Debug)]
pub enum TrySendError {
    /// Queue was at capacity. The message was NOT enqueued.
    Full(Message),
}

/// Construct a mailbox pair around a runtime-provided [`Waker`].
pub fn channel(waker: Arc<dyn Waker>) -> (Sender, Receiver) {
    let inner = Arc::new(Inner {
        queue: Mutex::new(VecDeque::with_capacity(CAPACITY)),
        not_full: Condvar::new(),
    });
    (
        Sender {
            inner: inner.clone(),
            waker,
        },
        Receiver { inner },
    )
}

impl Sender {
    /// Non-blocking enqueue. Fast path. Does NOT notify — call [`notify`] after
    /// a batch, or use it standalone. Returns [`TrySendError::Full`] with the
    /// message handed back if there was no room.
    ///
    /// [`notify`]: Sender::notify
    pub fn try_send(&self, msg: Message) -> Result<(), TrySendError> {
        let mut q = self.inner.queue.lock().unwrap();
        if q.len() >= CAPACITY {
            return Err(TrySendError::Full(msg));
        }
        q.push_back(msg);
        Ok(())
    }

    /// Wake the consumer loop. Idempotent / coalescing.
    pub fn notify(&self) {
        self.waker.wake();
    }

    /// Convenience: [`try_send`] then [`notify`] on success. This is the common
    /// producer call for a single message.
    ///
    /// [`try_send`]: Sender::try_send
    /// [`notify`]: Sender::notify
    pub fn send(&self, msg: Message) -> Result<(), TrySendError> {
        self.try_send(msg)?;
        self.notify();
        Ok(())
    }

    /// The backpressure-unlock send (Zig `Mailbox.send(msg, mutex)`).
    ///
    /// `held` is a mutex guard the caller currently holds AND that the consumer
    /// may need to make progress (the renderer state lock). If the queue is
    /// full we:
    ///   1. notify the consumer (so it starts draining),
    ///   2. drop `held` so the consumer can acquire it,
    ///   3. block on the not-full condvar until there is room,
    ///   4. push,
    ///   5. re-acquire `held` and hand the guard back.
    ///
    /// The guard is moved in and a fresh guard for the same lock is returned,
    /// so the borrow checker enforces that the caller cannot touch the
    /// protected data while we've released it. This is the deadlock-avoidance
    /// contract from `docs/plans/m2-termio.md` decision 3, made explicit in the
    /// type signature.
    pub fn send_with_unlock<'a, T>(
        &self,
        msg: Message,
        held: std::sync::MutexGuard<'a, T>,
        lock: &'a Mutex<T>,
    ) -> std::sync::MutexGuard<'a, T> {
        // Fast path: room available, keep holding the lock.
        {
            let mut q = self.inner.queue.lock().unwrap();
            if q.len() < CAPACITY {
                q.push_back(msg);
                drop(q);
                self.waker.wake();
                return held;
            }
        }

        // Slow path: full. Wake the consumer and release the caller's lock so
        // the consumer can drain (a resize/focus handler may need it).
        self.waker.wake();
        drop(held);

        // Block until there is space, then push. `forever` semantics.
        {
            let mut q = self.inner.queue.lock().unwrap();
            while q.len() >= CAPACITY {
                q = self.inner.not_full.wait(q).unwrap();
            }
            q.push_back(msg);
        }
        self.waker.wake();

        // Re-acquire the caller's lock and hand back a fresh guard.
        lock.lock().unwrap()
    }
}

impl Receiver {
    /// Drain every currently-queued message into `out`, signalling any blocked
    /// producers that space is now available. Returns the number drained. This
    /// is the consumer's per-wakeup drain (Zig `drainMailbox`): grab the lock
    /// once, pull everything, release. Handlers run AFTER the lock is dropped
    /// so producers aren't starved (`out` is handed back to the caller).
    pub fn drain(&self, out: &mut Vec<Message>) -> usize {
        let mut q = self.inner.queue.lock().unwrap();
        let n = q.len();
        out.extend(q.drain(..));
        drop(q);
        if n > 0 {
            // Popping made room; wake every blocked `send_with_unlock`.
            self.inner.not_full.notify_all();
        }
        n
    }

    /// Current queue depth. Test/bench introspection only.
    pub fn len(&self) -> usize {
        self.inner.queue.lock().unwrap().len()
    }

    /// Whether the queue is currently empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
