//! The termio writer mailbox. Port of `src/termio/mailbox.zig` +
//! `src/datastruct/blocking_queue.zig` (Ghostty `2da015cd6`), promoted from
//! `crates/spike-runtime/src/mailbox.rs` — the implementation that ratified
//! the **binding API contract** in `docs/adr/002-termio-runtime.md`.
//!
//! # Contract (survives the threads-vs-tokio decision)
//!
//! * **Bounded, fixed capacity 64.** Matches `BlockingQueue(Message, 64)`.
//!   Chosen empirically upstream; a small queue that drains fast is preferred
//!   over an unbounded one that can hide runaway producers. At ~40 bytes per
//!   [`Message`] the whole queue is ~2.5 KB.
//! * **SPSC.** Exactly one consumer (the writer loop). Any number of producer
//!   handles ([`Sender`]) may exist, but in the real system there is one
//!   logical producer (the surface/render thread). We keep the mutex+condvar
//!   so a blocking send from any thread is correct.
//! * **Two send paths:**
//!   * [`Sender::try_send`] — non-blocking. Returns [`TrySendError::Full`]
//!     (message handed back) immediately if the queue is full. This is the
//!     fast path (Zig `push(.instant)`).
//!   * [`Sender::send_with_unlock`] — the backpressure-unlock path (Zig
//!     `Mailbox.send(msg, mutex)`, plan decision 3). The caller is holding a
//!     lock the consumer needs (the renderer state mutex — the writer's
//!     drain handlers for resize/focus acquire it; plain writes don't). If
//!     the queue is full we **unlock that lock**, wake the consumer, block
//!     until space frees, push, then re-lock. Without this, a full queue +
//!     held render lock deadlocks: the producer waits for space, the
//!     consumer waits for the lock to make space.
//! * **Wakeup is decoupled from enqueue.** Enqueue never implicitly notifies
//!   the consumer; the producer calls [`Sender::notify`] (or the send helpers
//!   do it). This mirrors `xev.Async`: the queue holds data, a separate
//!   one-shot waker kicks the event loop, letting a runtime coalesce many
//!   enqueues into one wakeup.
//!
//! The [`Waker`] trait is the ONLY runtime-specific seam
//! (`polling::Poller::notify()` under the ADR-accepted threads runtime).
//! `try_send`, `send_with_unlock`, `drain`, capacity, and message ordering
//! are runtime agnostic.
//!
//! Deviations from upstream (see `docs/analysis/termio-foundations.md`):
//! [`Waker::wake`] is infallible where upstream logs-and-drops on a failed
//! `xev.Async.notify`; the ring buffer is a `VecDeque` with capacity checks
//! rather than fixed storage (same observable semantics).

use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use crate::message::Message;

/// Fixed mailbox capacity. Ported verbatim from `BlockingQueue(Message, 64)`.
pub const CAPACITY: usize = 64;

/// The one runtime-specific seam: something that can kick the consumer's
/// event loop from an arbitrary producer thread. Maps to `xev.Async` —
/// `polling::Poller::notify()` under the ADR-002 threads runtime. Cheap and
/// idempotent: repeated notifies before the consumer drains collapse into
/// one wakeup.
pub trait Waker: Send + Sync + 'static {
    /// Wake the consumer loop. Must be safe to call from any thread and must
    /// never block.
    fn wake(&self);
}

/// Shared queue state. Mutex + condvar exactly like the Zig `BlockingQueue`:
/// the condvar is signalled when draining makes room so a blocked
/// [`Sender::send_with_unlock`] can proceed. There is deliberately NO
/// not-empty condvar — emptiness is communicated out of band via the
/// [`Waker`].
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
    /// Non-blocking enqueue. Fast path. Does NOT notify — call [`notify`]
    /// after a batch, or use [`send`] for a single message. Returns
    /// [`TrySendError::Full`] with the message handed back if there was no
    /// room.
    ///
    /// [`notify`]: Sender::notify
    /// [`send`]: Sender::send
    pub fn try_send(&self, msg: Message) -> Result<(), TrySendError> {
        let mut q = self.inner.queue.lock().unwrap();
        if q.len() >= CAPACITY {
            return Err(TrySendError::Full(msg));
        }
        q.push_back(msg);
        Ok(())
    }

    /// Wake the consumer loop. Idempotent / coalescing. Port of
    /// `Mailbox.notify`.
    pub fn notify(&self) {
        self.waker.wake();
    }

    /// Convenience: [`try_send`] then [`notify`] on success. This is the
    /// common producer call for a single message.
    ///
    /// [`try_send`]: Sender::try_send
    /// [`notify`]: Sender::notify
    pub fn send(&self, msg: Message) -> Result<(), TrySendError> {
        self.try_send(msg)?;
        self.notify();
        Ok(())
    }

    /// The backpressure-unlock send. Port of `Mailbox.send(msg, mutex)`.
    ///
    /// `held` is a mutex guard the caller currently holds AND that the
    /// consumer may need to make progress (the renderer state lock). If the
    /// queue is full we:
    ///
    /// 1. notify the consumer (so it starts draining),
    /// 2. drop `held` so the consumer can acquire it,
    /// 3. block on the not-full condvar until there is room,
    /// 4. push,
    /// 5. re-acquire `held` and hand the guard back.
    ///
    /// The guard is moved in and a fresh guard for the same lock is
    /// returned, so the borrow checker enforces that the caller cannot touch
    /// the protected data while we've released it — the deadlock-avoidance
    /// invariant of `docs/plans/m2-termio.md` decision 3, encoded in the
    /// type signature rather than left to a comment.
    pub fn send_with_unlock<'a, T>(
        &self,
        msg: Message,
        held: MutexGuard<'a, T>,
        lock: &'a Mutex<T>,
    ) -> MutexGuard<'a, T> {
        // Fast path: room available, keep holding the lock. (Zig
        // `push(.instant)` succeeding.)
        {
            let mut q = self.inner.queue.lock().unwrap();
            if q.len() < CAPACITY {
                q.push_back(msg);
                drop(q);
                self.waker.wake();
                return held;
            }
        }

        // Slow path: full. Wake the consumer and release the caller's lock
        // so the consumer can drain — its handlers (resize, focus) may need
        // it. "This only gets triggered in certain pathological cases."
        self.waker.wake();
        drop(held);

        // Block until there is space, then push. Zig `push(.forever)`.
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
    /// Drain every currently-queued message into `out`, signalling any
    /// blocked producers that space is now available. Returns the number
    /// drained. This is the consumer's per-wakeup drain (Zig
    /// `drainMailbox`): grab the lock once, pull everything, release.
    /// Handlers run AFTER the queue lock is dropped so producers aren't
    /// starved (`out` is handed back to the caller).
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

    /// Current queue depth. Test/introspection only.
    pub fn len(&self) -> usize {
        self.inner.queue.lock().unwrap().len()
    }

    /// Whether the queue is currently empty.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    /// A waker that just counts. Lets tests assert the enqueue/notify
    /// decoupling without a runtime.
    #[derive(Default)]
    struct CountingWaker {
        wakes: AtomicUsize,
    }

    impl Waker for CountingWaker {
        fn wake(&self) {
            self.wakes.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn mailbox() -> (Arc<CountingWaker>, Sender, Receiver) {
        let waker = Arc::new(CountingWaker::default());
        let (tx, rx) = channel(waker.clone());
        (waker, tx, rx)
    }

    fn probe() -> Message {
        Message::Focused(true)
    }

    #[test]
    fn try_send_full_hands_message_back() {
        let (_w, tx, rx) = mailbox();
        for _ in 0..CAPACITY {
            tx.try_send(Message::write_req(b"x")).unwrap();
        }
        // 65th message: rejected, payload returned intact.
        let err = tx.try_send(Message::write_req(b"overflow")).unwrap_err();
        let TrySendError::Full(msg) = err;
        match msg {
            Message::WriteSmall(small) => assert_eq!(small.as_slice(), b"overflow"),
            other => panic!("message not handed back intact: {other:?}"),
        }
        assert_eq!(rx.len(), CAPACITY);
    }

    #[test]
    fn enqueue_is_decoupled_from_wakeup() {
        let (waker, tx, _rx) = mailbox();
        tx.try_send(probe()).unwrap();
        assert_eq!(
            waker.wakes.load(Ordering::SeqCst),
            0,
            "try_send must not wake"
        );
        tx.notify();
        assert_eq!(waker.wakes.load(Ordering::SeqCst), 1);
        tx.send(probe()).unwrap();
        assert_eq!(
            waker.wakes.load(Ordering::SeqCst),
            2,
            "send = try_send + notify"
        );
    }

    #[test]
    fn drain_is_fifo_one_shot_and_empties() {
        let (_w, tx, rx) = mailbox();
        for i in 0..5isize {
            tx.try_send(Message::JumpToPrompt(i)).unwrap();
        }
        let mut out = Vec::new();
        assert_eq!(rx.drain(&mut out), 5);
        let order: Vec<isize> = out
            .iter()
            .map(|m| match m {
                Message::JumpToPrompt(i) => *i,
                other => panic!("unexpected message {other:?}"),
            })
            .collect();
        assert_eq!(order, [0, 1, 2, 3, 4]);
        assert!(rx.is_empty());
        assert_eq!(rx.drain(&mut out), 0, "second drain finds nothing");
    }

    #[test]
    fn send_with_unlock_fast_path_keeps_guard() {
        let (waker, tx, rx) = mailbox();
        let render = Mutex::new(7u32);
        let guard = render.lock().unwrap();
        // Queue has room: message lands, we are woken once, guard still valid.
        let guard = tx.send_with_unlock(probe(), guard, &render);
        assert_eq!(*guard, 7);
        assert_eq!(waker.wakes.load(Ordering::SeqCst), 1);
        assert_eq!(rx.len(), 1);
    }

    /// The load-bearing test (plan decision 3, ADR-002 scenario 5): the
    /// producer fills the queue to capacity while holding the renderer-state
    /// lock the consumer needs, then calls `send_with_unlock`. The consumer
    /// can only drain after acquiring that lock — deadlock unless the send
    /// path releases it. Promoted from the spike smoke/bench, made stricter:
    /// the consumer *requires* the render lock before draining (the spike's
    /// consumer didn't take it).
    #[test]
    fn backpressure_unlock_no_deadlock() {
        let (_w, tx, rx) = mailbox();
        let render = Arc::new(Mutex::new(0u32));

        // Hold the render lock, fill the queue.
        let guard = render.lock().unwrap();
        for _ in 0..CAPACITY {
            tx.try_send(Message::write_req(b"fill")).unwrap();
        }

        // Consumer thread: models the writer loop whose drain handler needs
        // the render lock (resize/focus). It can't make room until the
        // producer's send_with_unlock releases the guard.
        let consumer = std::thread::spawn({
            let render = Arc::clone(&render);
            move || {
                let mut out = Vec::new();
                {
                    let mut state = render.lock().unwrap();
                    *state += 1; // proof we ran while the producer was parked
                    rx.drain(&mut out);
                } // release render lock so the producer can re-acquire it
                // Collect the probe message that the unblocked producer pushes.
                let deadline = Instant::now() + Duration::from_secs(10);
                while out.len() < CAPACITY + 1 {
                    assert!(Instant::now() < deadline, "probe message never arrived");
                    rx.drain(&mut out);
                    std::thread::sleep(Duration::from_millis(1));
                }
                out.len()
            }
        });

        // Full queue + held lock: without the unlock this deadlocks.
        let guard = tx.send_with_unlock(Message::write_req(b"probe"), guard, &render);
        assert_eq!(
            *guard, 1,
            "consumer must have held the lock while we were parked"
        );
        drop(guard);

        assert_eq!(consumer.join().unwrap(), CAPACITY + 1);
    }

    /// Drain must signal not_full: a sender blocked in the slow path wakes
    /// as soon as the consumer drains, without any extra nudging.
    #[test]
    fn drain_unblocks_parked_sender() {
        let (_w, tx, rx) = mailbox();
        let dummy = Arc::new(Mutex::new(()));
        for _ in 0..CAPACITY {
            tx.try_send(probe()).unwrap();
        }

        let sender = std::thread::spawn({
            let tx = tx.clone();
            let dummy = Arc::clone(&dummy);
            move || {
                let guard = dummy.lock().unwrap();
                let _guard = tx.send_with_unlock(probe(), guard, &dummy);
            }
        });

        // Give the sender time to park on the not_full condvar.
        std::thread::sleep(Duration::from_millis(50));
        let mut out = Vec::new();
        assert_eq!(rx.drain(&mut out), CAPACITY);

        sender.join().unwrap();
        assert_eq!(rx.len(), 1, "parked message lands after drain");
    }
}
