# Termio hub (`termio/Termio.zig` + `termio/Thread.zig` promotion + app swap)

Surveyed and ported against ghostty commit `2da015cd6`
(`2da015cd6ac06cedc89e09756e895d2c1715205d`; the local checkout is at
`38e49a232`, and `docs/analysis/termio-foundations.md` records that the termio
foundation files are byte-identical between the two). The Rust port lives in
`crates/ghostty-termio/src/hub.rs` (the `Termio` hub + promoted `Thread` loop)
and rewires `crates/ghostty-app` onto it. This is M2 chunk E from
`docs/plans/m2-termio.md`; it builds on chunk D
(`docs/analysis/termio-exec.md`), the mailbox contract
(`docs/adr/002-termio-runtime.md`, binding), and the runtime spike
(`crates/spike-runtime`).

Zig references (all line numbers against `2da015cd6`):

| file                             | LoC | Rust module                            |
| -------------------------------- | --- | -------------------------------------- |
| `src/termio/Termio.zig`          | 761 | `ghostty-termio/src/hub.rs` (`Termio`) |
| `src/termio/Thread.zig`          | 531 | `ghostty-termio/src/hub.rs` (`Thread`) |
| `src/termio/Options.zig`         | 41  | folded into `Termio::spawn` args       |
| `src/Surface.zig` (io lifecycle) | ref | `ghostty-app/src/app.rs` `Tab` (ref)   |

## 1. What `Termio.zig` is (the hub's role)

`Termio` is not the pty and not the parser — it is the **state container and
wiring point** that both the surface thread and the IO thread reach through. Its
fields (`Termio.zig:30–64`):

- `terminal` — the `Terminal` state (grid/screen/modes). Guarded by
  `renderer_state.mutex`.
- `terminal_stream` (`StreamHandler.Stream`) — the VT parser feeding the
  terminal. `processOutput` (`Termio.zig:643`) is the parse sink: lock the
  renderer mutex, `stream.nextSlice(buf)`, unlock.
- `backend` (`termio.Backend`, one variant `exec`) — owns the pty, subprocess,
  read pipeline, and writer machinery (chunk D's `Exec`).
- `mailbox` (`termio.Mailbox`) — the bounded-64 writer mailbox (chunk B).
- `renderer_state` / `renderer_wakeup` / `renderer_mailbox` — the shared
  render state + its lock + the wakeup the writer pokes after each drain.
- `surface_mailbox` — where `child_exited` / `password_input` /
  `selection_scroll_tick` are posted (block-push for the balanced ones).
- `config` (`DerivedConfig`) — the config-derived knobs (cursor style, palette,
  linefeed default, …), re-derived on `change_config`.
- `size` — the current `renderer.Size` (screen+cell+padding), the source of
  truth resize coalescing collapses to.

Config derivation (`Termio.DerivedConfig.init`) pulls the ~20 fields Termio
actually reads out of the giant app `Config` so the IO thread never touches the
live config object. Chunk E ports only the subset the app exercises today
(command, env, cwd, term, linefeed default); the rest lands with the config
crate (out of M2-E scope).

## 2. `Thread.zig`'s loop — promoted from the spike runtime

`Thread.zig` is the writer event loop. Its shape (`Thread.threadMain_:230` +
`drainMailbox:288`) is exactly the `Driver`/`Handler` seam the spike ratified
(`crates/spike-runtime/src/{driver,threads}.rs`), so chunk E **promotes that
seam into `ghostty-termio`** rather than re-deriving it:

- **Mailbox wakeup + drain**: `polling::Poller::wait(timeout)` parks the loop;
  `Poller::notify()` (≙ `xev.Async.notify`) wakes it; `Receiver::drain` pulls
  the whole queue under one lock, then handlers run after the lock drops.
- **25ms resize coalesce** (`Coalesce.min_ms`): repeated resizes inside the
  window collapse to one `Exec::resize` with the last dims. The timer is armed
  only when idle — an in-flight window is **not** reset (Zig `Thread.zig`
  coalesce rule; `spike-runtime/src/threads.rs:125`).
- **1s sync-output reset** (`sync_reset_ms`): re-armed on every
  `StartSynchronizedOutput`; on expiry, force-clears mode 2026 so a wedged
  program can't freeze the display (see §4).
- **200ms termios poll** (chunk D's `termios_tick`): the password heuristic,
  folded into the same timer set.
- **stop**: a flag + `notify()` so `wait()` returns promptly; the loop then runs
  `Exec::thread_exit` teardown.

The promotion keeps the spike's `Waker`/`DriverHandle` names but retargets
`Handler` at the real `Termio` (the spike's `Handler` was a counting stub). The
`polling` dependency moves from `spike-runtime` into `ghostty-termio`.

The spike's `Message` (a toy enum) is dropped; the loop drains the real
`ghostty_termio::message::Message`. The chunk-D `WriterLoop` (a condvar-parked
minimal drainer written for Exec's tests) is subsumed by the promoted `Thread`
loop, which adds the real `polling` wakeup and the terminal-touching handlers.
`WriterLoop` is retained for the chunk-D integration tests (they drive `drain` +
`tick_timers` synchronously without a `polling` runtime); `Thread` is the
production loop.

## 3. App-integration design — thread topology in the R5 main-thread world

### 3.1 The problem

Chunk D's `Exec` parse sink is a `Box<dyn FnMut(&[u8]) + Send>` that runs on the
**io-reader (parse) thread**, and the Exec throughput number (106 MiB/s,
`docs/analysis/termio-exec.md`) is measured feeding *that sink directly*. The
app's engine, however, is **main-thread-owned**: `Tab { engine, .. }` lives in
`Rc<RefCell<ControllerState>>`, fed on the ~60Hz pace tick by
`pty.try_read() -> engine.write()`. There is **no render mutex** — R5 chose a
single-threaded main loop (`docs/analysis/renderer-r5.md`), so the engine, the
snapshot, and the Metal draw all happen on the run loop with no locking.

The task's decision 3 (backpressure-unlock ↔ renderer-mutex interaction) assumes
upstream's threading shape (a dedicated renderer thread holding a mutex the
writer loop's handlers contend for). The R5 app does not have that shape. So the
mapping has to be made explicit rather than copied.

### 3.2 The two candidate feed paths

- **(a) parse thread applies to the engine behind a mutex the pace-tick also
  takes** (upstream-like). The sink closure does
  `engine.lock().write(batch)` on the parse thread; the pace tick does
  `engine.lock()` to snapshot/render and to drain engine replies. This is
  exactly `Termio.processOutput` locking `renderer_state.mutex`
  (`Termio.zig:646`).
- **(b) parse thread sends byte-chunks to main via a channel; main applies**
  (extra copy, simpler, no shared mutex).

### 3.3 Decision: **(a)**, engine behind an `Arc<Mutex<Engine>>`

Rationale, on the same throughput axis the chunk mandates (≥80 MiB/s into a
**live** engine):

1. **(b) cannot meet the throughput bar and doesn't even test the right
   thing.** Under (b) the engine is only fed inside the ≤16ms pace tick at
   60Hz, so sustained throughput is gated by the frame cadence and the
   channel-copy, not by the VT parse. The Exec ring's backpressure would stall
   at the channel boundary (an unbounded channel hides runaway producers; a
   bounded one just moves the stall off the kernel pty queue where decision 4
   deliberately put it). "Throughput into a live engine" under (b) is really
   "throughput into a `VecDeque<Vec<u8>>`" — the engine never sees the flood at
   line rate. Rejected.
2. **(a) preserves the Exec pipeline as measured.** The sink locks the engine
   and calls `engine.write(batch)` on the parse thread at line rate; the ring's
   backpressure remains end-to-end (slow parse → full ring → gather parks →
   kernel queue fills → child's `write()` blocks), exactly decision 4's design.
   The 80 MiB/s target is met by the parse thread, uncontended for the ~microsecond
   the pace tick holds the lock 60×/sec.
3. **It is the upstream design**, so it maps 1:1 onto `Termio.processOutput` and
   is the reviewable port. Upstream's "renderer thread" becomes the app's "main
   pace-tick thread"; the mutex is the same mutex, just contended by a
   run-loop timer instead of a background renderer thread.

The lock is held for a single `stream.feed(batch)` on the parse side and a
single `snapshot_window` + `take_output` on the pace side — both short. At 60Hz
the pace tick's hold is a rounding error against the parse thread's duty cycle,
so contention does not cost throughput (verified by the throughput test, §5).

### 3.4 The writer path — non-blocking main-thread send policy

This is where decision 3's backpressure-unlock has to be re-expressed for a
main-thread producer. Upstream's `Mailbox.send(msg, mutex)` **blocks** the
producer on a full queue after unlocking the render mutex. The app's producer is
the **main thread** (input encode, paste, mouse, engine DSR replies). **Blocking
the main thread on a full write queue would freeze the UI** — the exact thing
R5's no-mutex main-loop design exists to avoid, and worse than upstream's case
(upstream's producer is a surface thread, not the UI run loop).

So the app's send policy is **`try_send` + a non-blocking full-queue fallback
that never blocks the run loop**:

- The main thread encodes input/paste/mouse into `Message::write_req(bytes)` and
  calls `Sender::send` (= `try_send` + `notify`).
- On `TrySendError::Full` (the writer loop is a full ring behind — pathological:
  64 queued writes undrained at 60Hz), the main thread does **not** block. It
  falls back to a direct, bounded, non-blocking best-effort: it re-`notify()`s
  the loop and drops the overflowing input chunk with a rate-limited warning.
  Dropping a keystroke under a 64-deep undrained backlog is strictly better than
  freezing the window, and is unreachable in practice (the writer loop drains
  the whole queue every wakeup; a human cannot outrun it).
- The engine's own reply bytes (DSR/DA/CPR — produced while the engine mutex is
  held on the pace tick) go through the same `send`; they are tiny and never
  fill the queue.

The `send_with_unlock` backpressure-unlock path stays in the mailbox (it is the
binding ADR contract and the chunk-B/D tests cover it) for any **future**
non-main-thread producer that legitimately wants to block (e.g. a background
paste-chunker). The app simply does not use it, because the app's only producer
is the run loop. This is documented at the `Tab` write call sites.

Net: no `renderer_state.mutex` unlock dance in the app, because the app's writer
does not hold the engine lock when it sends (it encodes from already-read engine
state, releases, then sends) — the deadlock decision 3 guards against
(producer holds render lock ← writer handler needs it) cannot arise. The
unlock contract is preserved *in the library* for producers that do hold it;
the app's topology sidesteps it by construction.

### 3.5 Resulting topology (per tab)

```text
main thread (NSTimer pace tick, ~60Hz)              io threads (per tab)
─────────────────────────────────────              ──────────────────────
 tick:                                              io-gather:  drain pty → ring
   engine.lock():                                   io-reader:  ring → sink:
     snapshot_window → render (Metal)                             engine.lock()
     take_output() → replies                                        .write(batch)
   unlock                                                          unlock
   for reply: mailbox.send(WriteAlloc)              io-writer (Thread loop):
                                                       poll.wait(next timer)
 input/paste/mouse (event, not tick):                  drain mailbox:
   encode → mailbox.send(write_req)                      Write* → Exec::queue_write → pty
   (Full → drop + warn, never block)                     Resize → coalesce 25ms → pty
                                                          Sync   → arm 1s reset
 resize (view bounds change):                          fire timers (resize/sync/termios)
   mailbox.send(Resize(size))
                                                    io-exit (waitpid):
 child_exited notifier → main (via                   on exit → Notifier::child_exited
   channel drained on pace tick) →                     → app: exit banner or close tab
   tab shows exit banner / closes
```

The `Notifier` (chunk D seam) is implemented by an app-side struct that forwards
`child_exited` / `password_input` into a `Sender`-free, lock-free channel the
pace tick drains (the notifier fires on the io-exit / io-writer threads, so it
cannot touch the `Rc<RefCell>` controller directly). The pace tick reads the
channel and: on `child_exited`, shows the exit banner then closes the tab (spike
behavior: the interim `PtySession` path closed the tab when `child_exited()`
returned true — see `app.rs::tick`); on `password_input`, sets a title suffix /
logs (surfacing-only for M2-E).

## 4. Sync-output 1s timeout mapping

Mode 2026 (synchronized output) tells the terminal to buffer rendering until the
program clears it. A wedged or malicious program that sets it and never clears it
would freeze the display. Upstream arms a 1s timer on `StartSynchronizedOutput`
and, on expiry, force-clears the mode (`Thread.zig` sync-reset). In the hub the
`Thread` loop's `on_sync_reset` reaches through the engine lock and clears mode
2026 on the terminal, then pokes the renderer wakeup so the next frame renders
the now-unblocked state. The app test drives a stuck `\x1b[?2026h` with no
clear and asserts a render is released within ~1s.

## 5. Test plan (chunk E)

- **Hub lifecycle** (`ghostty-termio/tests/hub.rs`, headless, no window): spawn
  `Termio` on `/bin/sh`, drive `echo` round-trip through gather→parse→engine,
  resize, clean exit + exit-code capture. Drives `Termio` directly.
- **Sync-output timeout**: feed `\x1b[?2026h` with no clear; assert the mode is
  force-cleared within ~1s (+slack).
- **Resize coalescing observable**: post N resizes inside 25ms; assert exactly
  one pty `TIOCSWINSZ` lands with the last dims.
- **Throughput into a live engine** (`#[ignore]`, run explicitly): feed a large
  byte flood through the real pipeline into a locked `Engine`; assert
  ≥80 MiB/s.
- **App smoke unchanged**: `--offscreen-smoke` and the ignored typing smoke
  still pass on the new stack.

## 6. Deferrals beyond M2-E

- Full `DerivedConfig` + `change_config` re-derivation (config crate scope).
- `renderer_mailbox` / `selection_scroll_tick` timer (no selection-scroll UI in
  the app yet).
- The inspector, `jump_to_prompt`, OSC-driven `clear_screen`/`scroll_viewport`
  mailbox handlers (no UI surface for them in M2-E; the loop ignores them as
  chunk D did).
- The error-screen "pty exhausted" banner rendering (Surface-level, chunk M).
- The spike (`crates/spike`) keeps `portable-pty` — it is scaffolding, per plan
  decision (only `ghostty-app` swaps).
