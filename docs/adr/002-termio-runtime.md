# ADR 002: termio writer-thread runtime — OS threads vs tokio

- Status: **PROPOSED** (pending maintainer review)
- Date: 2026-07-07
- Chunk: M2-C (`docs/plans/m2-termio.md`, decision 2)
- Spike code: `crates/spike-runtime/` (workspace member; benchmark bin `bench`)
- Supersedes: nothing
- Confidence: **high** on the recommendation; **medium** on the margins (see
  variance note)

## Context

Ghostty's terminal IO is split across threads. The **reader** side lives in the
`Termio` implementation (chunk D). The **writer** side is a dedicated event
loop, `src/termio/Thread.zig` (`2da015cd6`, 531 LoC), built on `libxev`. Its job
is to drain a bounded mailbox of control/write messages, coalesce resize events
on a 25 ms timer, run a 1 s synchronized-output reset timer, and forward bytes
to the pty — all while offloading work from the hot reader/parser path.

The plan (decision 2) requires we settle "threads vs tokio" **by measurement,
not taste**: build `Thread.zig`'s exact semantics twice, drive both with
identical synthetic load, and record the result here. Decision 2 also fixes the
non-negotiable: whichever runtime wins, **the mailbox API is identical** so that
the Exec/Termio code (chunks D/E) is independent of the choice.

Both implementations were built behind one `Driver` trait and one `mailbox`
module. A CI smoke test (`crates/spike-runtime/tests/smoke.rs`) runs the same
six test bodies against both runtimes.

## The load (identical for both runtimes)

Implemented once, generic over the `Driver` trait, in `src/bin/bench.rs`:

1. **Wakeup latency** — message posted → handler runs, p50/p99, measured both
   with the loop idle (cold wakeup) and under a background flood (warm loop).
2. **Mailbox flood throughput** — 1,000,000 small messages, single producer,
   bounded-64 queue, coalesced notifies.
3. **Resize coalescing accuracy** — 10 bursts × 20 resizes; ideal is exactly
   10 `on_resize` callbacks with the last dims of each burst winning.
4. **Idle CPU over 10 s** — process user+sys CPU delta (`getrusage`) while the
   loop is parked doing nothing.
5. **Backpressure-unlock** — producer fills the queue to capacity **while
   holding the renderer-state lock the consumer needs**, then calls
   `send_with_unlock`. Proves no deadlock and measures handoff latency.

## Results

Release build, Apple Silicon (aarch64-darwin), foreground, one scenario at a
time. Numbers are **representative of three runs**; the machine is a noisy dev
laptop, so tail latencies and throughput swing run-to-run (see variance note).

### Wakeup latency (posted → handler runs)

| runtime              | idle p50 | idle p99   | flood p50  | flood p99  |
| -------------------- | -------- | ---------- | ---------- | ---------- |
| threads + polling    | 4.7–7 µs | 49 µs–1 ms | 0.9–4 µs   | 38 µs–2 ms |
| tokio current-thread | 5–10 µs  | 21 µs–2 ms | 4.3–5.6 µs | 30–49 µs   |

### Mailbox flood throughput (1,000,000 msgs)

| runtime              | elapsed     | msgs/sec    |
| -------------------- | ----------- | ----------- |
| threads + polling    | 0.11–0.18 s | 5.4–8.8 M/s |
| tokio current-thread | 0.12–0.34 s | 2.9–8.3 M/s |

### Resize coalescing (10 bursts × 20 resizes; ideal fired = 10)

| runtime              | callbacks fired | final dims correct |
| -------------------- | --------------- | ------------------ |
| threads + polling    | 10              | true               |
| tokio current-thread | 10              | true               |

### Idle CPU over 10 s (process user+sys delta)

| runtime              | cpu time |
| -------------------- | -------- |
| threads + polling    | 10–15 µs |
| tokio current-thread | 12–18 µs |

### Backpressure-unlock (full queue, producer holds render lock)

| runtime              | no deadlock | handoff latency |
| -------------------- | ----------- | --------------- |
| threads + polling    | true        | 10 µs–3.4 ms    |
| tokio current-thread | true        | 21 µs–870 µs    |

### Variance note

Across three runs, p99 latency and flood throughput swing by an order of
magnitude on **both** runtimes — this is scheduler/OS jitter on a shared dev
machine, not a property of either runtime. What is **perfectly stable** across
all runs, and what actually matters:

- coalescing is exactly `10` fired with correct dims on both;
- idle CPU is `~10–18 µs / 10 s` (i.e. indistinguishable from zero) on both —
  both correctly park in the kernel, no busy-poll;
- backpressure-unlock never deadlocks on either;
- p50 wakeup latency is `~5 µs` on both — three orders of magnitude below the
  ~16 ms frame budget and far below human perception.

Real terminal traffic (`seq 1 100000`, `cat` of a large file) produces **far
fewer, larger** messages than the 1 M tiny-message flood — the reader batches
bytes into `write_alloc` chunks. Neither runtime is remotely a bottleneck.

## Decision (PROPOSED)

**Recommendation: OS thread + `polling` + a small hand-rolled timer wheel
(implementation (a)).** Adopt tokio only if a *later* need (async pty I/O
elsewhere, an async ecosystem dependency) forces it — the mailbox API makes that
swap cheap.

Confidence: **high**. The performance data does not favor either runtime
(both are microseconds where we have milliseconds to spare, both park at idle,
both coalesce correctly, neither deadlocks). The decision therefore rests on
**fit and cost**, where the OS-thread version wins:

- It maps 1:1 onto `Thread.zig`. `polling::Poller::notify()` **is**
  `xev.Async.notify()` — a thread-safe, coalescing, non-blocking wakeup. The
  port is mechanical and reviewable against the Zig source.
- No async runtime, no executor, no `async fn` coloring, no `Send + 'static`
  futures constraints threaded through the termio types. The writer loop is a
  plain `while` loop a reader can follow top to bottom.
- The timer wheel we need is **two slots** (resize coalesce, sync reset). That
  is ~30 lines, versus pulling `tokio::time` + a `select!` state machine and
  its disarm/re-arm bookkeeping for the same two timers.
- One fewer heavy dependency in a hot, security-relevant path. `polling` is
  small and does one thing; tokio is large and does many.

`polling` was chosen over `mio` for the OS-thread impl because `mio`'s wakeup
(`mio::Waker` bound to a `Registry` + a reserved token) is heavier for the same
effect, and `mio` has no timer facility either. `polling`'s `notify()` is the
cleaner analogue of `xev.Async`.

### What the loser (tokio) would have cost

- **Async coloring** through termio: handlers become `async`, the mailbox
  consumer becomes a task, and the `Handler` trait would need `async` methods
  (or block-on shims) — friction the OS-thread version simply does not have.
- **A `select!` timer state machine** for two timers that a two-slot wheel
  handles in a few lines. `tokio::time` sleeps must be rebuilt/guarded each loop
  iteration (`if resize_deadline.is_some()`), which is more code and more ways
  to get the "don't reset an in-flight coalesce timer" rule subtly wrong.
- **A large dependency** and an executor in the IO hot path, for zero measured
  throughput or latency benefit.
- The one thing tokio would buy — a mature async I/O reactor — is not needed
  here: the pty fd (chunk E) is a single fd on a dedicated thread, well served
  by adding it to the `polling` set (or a blocking read on the reader side per
  decision 4). If a future chunk genuinely needs the async ecosystem, the
  mailbox API below lets us switch the driver without touching termio.

## The mailbox API contract (survives either runtime)

This is the load-bearing artifact of the spike. It is identical under both
drivers (`crates/spike-runtime/src/mailbox.rs`).

- **Bounded, capacity `64`** — ported from `BlockingQueue(Message, 64)`.
- **SPSC** — one consumer (the writer loop); producer handles are cloneable.
- **Wakeup is decoupled from enqueue.** Enqueue never implicitly wakes the
  consumer; a separate `Waker` does. This is the only runtime-specific seam
  (a trait): `polling::Poller` under threads, `tokio::sync::Notify` (later
  `AsyncFd`) under tokio.
- **Two send paths:**
  - `try_send(msg) -> Result<(), Full(msg)>` — non-blocking fast path
    (Zig `push(.instant)`). On `Full`, the message is handed back.
  - `send_with_unlock(msg, held_guard, &lock) -> MutexGuard` — the
    **backpressure-unlock** path (Zig `Mailbox.send(msg, mutex)`, plan
    decision 3). If the queue is full it: notifies the consumer, **drops the
    caller's lock guard**, blocks on a not-full condvar, pushes, then
    re-acquires the lock and returns a fresh guard. The guard is *moved in*
    and a new one returned, so the borrow checker forbids touching the
    protected data while the lock is released — the deadlock-avoidance
    invariant is encoded in the type signature, not left to a comment.
- **Drain semantics:** the consumer drains the whole queue under one lock
  acquisition, signals `not_full` (waking any blocked `send_with_unlock`), then
  runs handlers **after** releasing the queue lock so producers aren't starved
  (Zig `drainMailbox`).

```rust
pub const CAPACITY: usize = 64;

pub trait Waker: Send + Sync + 'static {
    fn wake(&self); // coalescing, non-blocking, any thread
}

pub fn channel(waker: Arc<dyn Waker>) -> (Sender, Receiver);

impl Sender {
    fn try_send(&self, msg: Message) -> Result<(), TrySendError>; // instant
    fn notify(&self);                                             // kick loop
    fn send(&self, msg: Message) -> Result<(), TrySendError>;     // try+notify
    fn send_with_unlock<'a, T>(
        &self,
        msg: Message,
        held: MutexGuard<'a, T>,
        lock: &'a Mutex<T>,
    ) -> MutexGuard<'a, T>; // backpressure-unlock, deadlock-safe
}

impl Receiver {
    fn drain(&self, out: &mut Vec<Message>) -> usize; // one-shot, signals not_full
}
```

The `Driver` trait (`src/driver.rs`) is the second half of the seam: `run(recv,
handler)` owns the loop; `Handler` gets `on_messages` / `on_resize` /
`on_sync_reset` with identical ordering under both runtimes. Termio code targets
`Driver` + mailbox and never names a runtime.

## Consequences

- **The renderer thread stays dedicated regardless** (plan decision). This ADR
  is only about the *termio writer* loop. The renderer keeps its own thread and
  its own wakeup; the writer notifies the renderer after each drain (Zig
  `renderer_wakeup.notify()`). Nothing here merges those loops onto one runtime.
- **Search `Thread` wrapper:** Ghostty also runs a search worker on the same
  `Thread.zig` pattern. If/when it is ported, it reuses this exact driver +
  mailbox seam (a different `Handler`), so this decision covers it by
  construction — no second runtime evaluation needed.
- **Chunk E (pty swap):** the real pty fd registers into the `polling` set (or
  is read on the dedicated reader thread per decision 4). No API change to the
  mailbox.
- **Reversibility is cheap:** because termio depends only on the mailbox +
  `Driver` traits, switching to tokio later is a new `Driver` impl, not a termio
  rewrite. That is the whole point of keeping the API identical.

## How to reproduce

```sh
cargo test -p spike-runtime            # smoke test (CI), both runtimes
cargo run -p spike-runtime --release --bin bench -- all
# or a single scenario:
cargo run -p spike-runtime --release --bin bench -- backpressure
```
