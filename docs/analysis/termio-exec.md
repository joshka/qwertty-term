# Termio Exec (`termio/Exec.zig` + `termio/Thread.zig` writer glue)

Surveyed and ported against ghostty commit `2da015cd6`
(`2da015cd6ac06cedc89e09756e895d2c1715205d`; verify with
`git -C ~/local/ghostty rev-parse 2da015cd6`). The Rust port lives in
`crates/ghostty-termio/src/exec.rs`. This covers M2 chunk D (Exec) from
`docs/plans/m2-termio.md`; it builds on chunks A+B (`pty`, `message`,
`mailbox`, `backend`) and plugs into the runtime decision
`docs/adr/002-termio-runtime.md` (ACCEPTED — OS threads + `polling`; the
mailbox API contract is binding).

Zig references (all line numbers against `2da015cd6`):

| file                    | LoC   | inline tests | Rust module                          |
| ----------------------- | ----- | ------------ | ------------------------------------ |
| `src/termio/Exec.zig`   | 2,143 | 11           | `ghostty-termio/src/exec.rs`         |
| `src/termio/Thread.zig` | 531   | 0            | writer-loop glue folded into `exec`  |

Only the parts of `Thread.zig` that Exec's tests need are ported here (the
mailbox drain loop + resize coalesce + sync-reset timers). The full
`Driver`/`Handler` promotion — search worker, selection-scroll timer,
renderer-wakeup plumbing, error-screen rendering — remains chunk E.

## Scope and the two deliberate deviations

Two upstream couplings do not exist yet in the Rust tree, so the port keeps
the *stage boundaries* identical while stubbing the far side:

1. **The parse-stage sink.** Upstream's parse stage calls
   `io.processOutput(batch)` — the VT parser + terminal-state update under
   the renderer lock. That hookup is chunk E. Here the sink is a
   `dyn FnMut(&[u8]) + Send` handed to `Exec::thread_enter`. The gather →
   parse boundary (rotating ring, publish/consume protocol, backpressure) is
   byte-for-byte the upstream design; only the terminal side of the last hop
   is a closure instead of `Termio.processOutput`.

2. **The writer-loop handlers.** Upstream's `drainMailbox` dispatches 16
   message variants into `Termio` methods (`colorSchemeReport`,
   `changeConfig`, `sizeReport`, …), most of which need the terminal/renderer
   state that lands in E. Chunk D implements the writer loop against the
   mailbox and the subset of handlers Exec *owns*: `WriteSmall`/`WriteStable`/
   `WriteAlloc` → `queue_write` (the pty write path), `Resize` → coalesce →
   `Exec::resize`, `Focused` → `focus_gained`, `LinefeedMode`,
   `StartSynchronizedOutput` (arm the reset timer). The terminal-touching
   variants are routed to a `dyn Handler` seam so E can fill them without
   changing the loop.

Everything else — subprocess spawn, env/command construction, the two-stage
read pipeline, the exit watcher, the termios poll timer, teardown ordering —
ports as-is.

## Lifecycle map

The full Exec lifecycle, upstream call sites in parens:

```text
init(cfg)                 Exec.zig:48  Subprocess.init:619
  └─ build env + args, open nothing yet (pty opens at start)

initTerminal(term)        Exec.zig:66
  └─ set initial pwd, seed grid/screen size via resize (infallible here)

threadEnter(io, td)       Exec.zig:85   (called by Thread.threadMain_:267)
  ├─ subprocess.start()   Subprocess.start:888
  │    ├─ Pty.open(size)
  │    ├─ fork + [child: dup2_stdio → childPreExec → execvpe]
  │    └─ parent: close slave, keep master → {read_fd, write_fd}
  ├─ exit watcher init    Exec.zig:106  (xev.Process; Rust: wait thread)
  ├─ record process_start Exec.zig:120
  ├─ open quit pipe       Exec.zig:124  (kills the read thread)
  ├─ init write stream    Exec.zig:129
  ├─ init termios timer   Exec.zig:135
  ├─ spawn io-reader      Exec.zig:139  (ReadThread.threadMainPosix)
  │    └─ io-reader spawns io-gather (gatherMainPosix)
  ├─ populate td.backend.exec = ThreadData{…}
  ├─ arm exit watcher     Exec.zig:158
  └─ arm termios timer    Exec.zig:184

  [writer loop runs: drainMailbox on each wakeup — Thread.zig:288]
    write_* → queueWrite:406 → chunk into 64-byte bufs → write to pty
    resize  → coalesce 25ms → Exec.resize:264 → Subprocess.resize:1114 → TIOCSWINSZ
    focused → focusGained:230 → start/stop termios timer
    start_synchronized_output → arm 1s sync-reset timer

  [async, off the writer loop:]
    io-gather:  read()/poll() pty → rotating ring buffers      ReadThread:1449
    io-reader:  ring → processOutput(batch) [sink]             ReadThread:1419
    termios timer (200ms): getMode → password heuristic        termiosTimer:320
    exit watcher: waitpid → processExitCommon → surface msg     processExit:299

threadExit(td)            Exec.zig:195   (defer in Thread.threadMain_:269)
  ├─ if exited: subprocess.externalExit()  (clear process handle)
  ├─ subprocess.stop()    Subprocess.stop:1093  (SIGHUP the child group)
  ├─ write "x" to quit pipe   Exec.zig:205    (AFTER stop — see ordering below)
  └─ read_thread.join()   Exec.zig:227    (io-reader joins io-gather first)

ThreadData.deinit        Exec.zig:543   (defer cb.data.deinit in Thread:268)
  ├─ close quit pipe write end
  ├─ drop write pools
  ├─ deinit exit watcher, write stream, termios timer

deinit(self)             Exec.zig:58    Subprocess.deinit:878
  └─ stop() (idempotent), close pty, free env, drop arena
```

## The two-stage read pipeline (decision 4, ports AS-IS)

Source: `ReadThread` (`Exec.zig:1279–1691`). The design and its constants
port verbatim; the doc comment at `Exec.zig:1243–1278` is the authoritative
rationale.

### Why two stages (not one reader)

A single serial loop (`read(); process();` — still used on Windows,
`Exec.zig:1647`) stalls the producer: on macOS the kernel tty output queue
caps every master read at ~1 KiB regardless of buffer size, so any time the
reader spends in `process()` (VT parse under the terminal lock) is time the
kernel pty fd is not being drained, and `cat`/`seq`-style producers block.

Splitting **gather** (drain the kernel queue into preallocated buffers) from
**parse** (consume a buffer → `processOutput`) means that while the parse
stage holds the terminal lock, the gather stage keeps the kernel queue empty.
The stall between the two shrinks to effectively zero; the remaining
bottleneck is the VT parse itself, which is the intended one.

### The rotating ring (`Pipeline`, `Exec.zig:1338–1367`)

- `buffer_count = 4` fixed buffers of `buffer_capacity = 64 KiB` each.
- One mutex + two condvars: `batch_ready` (gather → parse) and `slot_free`
  (parse → gather). A buffer is owned by exactly one stage at a time, so
  **buffer contents need no lock** — only the ring metadata (`head`, `tail`,
  `count`, `lens`, `done`) is guarded.
- `head` = next slot gather fills; `tail` = next slot parse consumes;
  `count` = published-but-unconsumed batches.

Publish (gather, `Exec.zig:1575`): lock, `lens[head] = total`,
`head = (head+1) % 4`, `count += 1`, unlock, signal `batch_ready`.

Consume (parse, `Exec.zig:1419`): lock, while `count == 0` { if `done`
return; wait `batch_ready` }, take `bufs[tail][0..lens[tail]]`, unlock,
`processOutput(batch)` **outside the lock**, then lock, `tail = (tail+1)%4`,
`count -= 1`, signal `slot_free`.

### Backpressure semantics — what blocks when, why

- **Gather blocks** only when `count == buffer_count` (all 4 buffers in
  flight, parse is a full ring behind — `Exec.zig:1478`). This is exactly
  when it *should* stop reading: not draining the kernel queue makes the
  kernel exert flow control on the child. So a slow VT parse → full ring →
  gather parks → kernel queue fills → child's `write()` blocks. Clean
  end-to-end backpressure with no unbounded buffering (max 256 KiB in
  flight).
- **Parse blocks** only when `count == 0` (nothing to do —
  `Exec.zig:1423`). It parks on `batch_ready` until gather publishes or sets
  `done`.

### Why <1ms latency under saturation (`Exec.zig:1494–1608`)

The gather stage must not trade interactivity for throughput. Per batch:

- Read in a tight loop into the current buffer. On `EAGAIN`:
  - if `total < bridge_threshold` (1024) the stream is an interactive
    trickle → **deliver immediately** (`break :gather`), zero added latency.
  - otherwise the stream is *saturated* (the writer filled a full kernel
    queue), so bridge the microsecond refill gap rather than shipping a tiny
    batch:
    - spin-retry the read up to `bridge_spin_max = 16` times (each ~0.5µs;
      catches >90% of gaps without sleeping);
    - then `poll` with `bridge_poll_timeout_ms = 1`, bounded by a total
      `gather_budget_ns = 3ms` per batch (well under one 16ms frame, so
      batching is invisible);
    - `poll` returning 0 (quiet for a full ms) ends the burst; quit-fd
      readable ends the stream; pty HUP-without-IN ends it.
- A full 64 KiB buffer means the stream is still hot → claim the next buffer
  with no intervening poll (`Exec.zig:1588`).
- Otherwise `poll(-1)` waits for data / quit / HUP (`Exec.zig:1591`).

Net: idle/interactive terminals never spin (spin path is gated on a full
kernel queue); saturated streams stay within a 3ms bridge budget; the child
sees backpressure through the ring, not an unbounded buffer.

### QoS (`Exec.zig:1618`, macOS)

Both pipeline threads are set to `user_initiated` QoS so the scheduler keeps
them off efficiency cores (measured 15% throughput swing on an M4 Max). The
Rust port applies `pthread_set_qos_class_self_np(USER_INITIATED)` on each
pipeline thread on macOS.

## Env / command construction (`Subprocess.init:619`, `execCommand:1708`)

Environment variables set (in `Subprocess.init` order):

| var                     | value / source                                    | line    |
| ----------------------- | ------------------------------------------------- | ------- |
| `GHOSTTY_RESOURCES_DIR` | `cfg.resources_dir` (if set)                      | 633     |
| `TERM`                  | `cfg.term` if resources_dir else `xterm-256color` | 643/660 |
| `COLORTERM`             | `truecolor`                                       | 644     |
| `TERMINFO`              | `{dirname(resources_dir)}/terminfo` (if res dir)  | 652     |
| `GHOSTTY_BIN_DIR`       | dir of the ghostty exe                            | 684     |
| `PATH`                  | append exe dir (last priority), or set to it      | 696     |
| `XDG_DATA_DIRS`         | append `{resources_dir}/..` (macOS)               | 714     |
| `MANPATH`               | append `{resources_dir}/../man` (macOS)           | 731     |
| `TERM_PROGRAM`          | `ghostty`                                         | 746     |
| `TERM_PROGRAM_VERSION`  | build version string                              | 747     |
| `VTE_VERSION`           | **removed** (don't look like VTE)                 | 752     |
| `PWD`                   | `cfg.working_directory` (if set & accessible)     | 860     |
| shell-integration vars  | `GHOSTTY_SHELL_FEATURES` etc. (chunk G)           | 764     |
| `cfg.env_override`      | applied last, overrides all                       | 813     |

Rust port note: the port takes the caller's env map as the base (upstream's
`cfg.env`), sets `TERM`/`COLORTERM`/`TERM_PROGRAM`/`TERM_PROGRAM_VERSION`,
`GHOSTTY_RESOURCES_DIR`/`TERMINFO`/`GHOSTTY_BIN_DIR` when the inputs are
present, removes `VTE_VERSION`, sets `PWD`, and applies overrides last. Shell
integration (`GHOSTTY_SHELL_FEATURES`, script injection) is chunk G and is
**not** wired here; `resources_dir` is optional throughout.

### `execCommand` — argv construction (`Exec.zig:1708`)

- **macOS** (`Exec.zig:1716`): wrap in `/usr/bin/login` for a proper login
  shell (loads `~/.bash_profile`, sets `SHELL`, correct `getlogin()`):
  `["/usr/bin/login", ("-q" if ~/.hushlogin), "-flp", username, ...]`. For a
  `.shell` command it then execs `/bin/bash --noprofile --norc -c
  "exec -l <cmd>"` (bash execs ~2x faster than zsh into the target; `-l` on
  the inner exec makes it the login shell). For `.direct` args they append
  after `username`. Falls back to POSIX form if passwd lookup fails
  (`break :darwin`).
- **POSIX** (`Exec.zig:1834`): `.shell` → `["/bin/sh", "-c", <cmd>]`
  (sh so we don't parse args ourselves; also picks up NixOS `/bin/sh` env
  setup). `.direct` → the argv as-is.
- Flatpak paths are a **roadmap non-goal** and skipped entirely.

## Password-detection flow (`termiosTimer:320`, 200ms)

The termios poll timer fires every `TERMIOS_POLL_MS = 200`:

1. Read the master's mode via `getMode` (ICANON/ECHO — `Exec.zig:352`;
   upstream fakes a `Pty` struct from the raw fd, the Rust port reads the fd
   directly through the same `tcgetattr`).
2. If unchanged from `termios_mode`, return (avoids locking the renderer
   mutex on every tick — `Exec.zig:364`).
3. On change, compute `password_input = canonical && !echo` (the heuristic:
   a program in canonical mode that disabled echo is reading a secret —
   `Exec.zig:370`).
4. Compare against the terminal's current `password_input` flag under the
   renderer lock; if unchanged, stop (`Exec.zig:378`).
5. If changed, **block-push** `password_input` to the surface mailbox — the
   balanced true/false state is critical to apprt behavior so it must not be
   dropped (`Exec.zig:386`).
6. Re-arm the timer if `termios_timer_running` (focus toggling controls this
   flag — `focusGained:230`: unfocus stops it cheaply, focus restarts it via
   an immediate `termiosTimer` call).

Rust port: the timer lives on the writer loop's timer wheel. The password
transition is delivered through the exit/surface callback seam (a
`dyn FnMut` the test asserts on, since the surface mailbox is chunk E/N).

## Exit-code / runtime paths (`processExitCommon:272`)

When the exit watcher fires:

1. Mark `exited = true` (gates further writes — `queueWrite:417`).
2. `runtime_ms = (now - process_start) / 1ms` (`Exec.zig:278`).
3. Push `.child_exited{ exit_code, runtime_ms }` to the surface mailbox with
   `.forever` (block until delivered — `Exec.zig:291`).

The **abnormal** path is `Termio.childExited` deciding whether to render the
"process exited" banner (upstream's `child_exited_abnormally`, the
`Backend::child_exited_abnormally` trait method); the exit-code capture here
is the same for clean and abnormal exits — the distinction is made by the
consumer, not the watcher.

## Exit-watcher choice (ADR world) — wait thread, justified

Upstream uses `xev.Process` (kqueue `EVFILT_PROC` on macOS) integrated into
the writer's event loop. ADR-002 chose OS threads + `polling`, and `polling`
has no process-wait facility. Two options:

- **(a) kqueue `EVFILT_PROC` via `polling`/raw kqueue** — register the pid,
  get a readiness event on exit, `waitpid(WNOHANG)` to reap. Matches upstream
  semantics but needs a raw kqueue fd threaded into the writer loop's poller
  and platform `#[cfg]` for the Linux equivalent (pidfd).
- **(b) a dedicated wait thread** — one thread blocks in `waitpid(pid)`,
  and on return runs `processExitCommon` and notifies. **Chosen.**

Rationale (fit + cost, same axis as ADR-002):

- Decision 4 already commits to a dedicated **reader** thread rather than
  registering the pty fd in an async reactor; a dedicated **wait** thread is
  the identical philosophy for the one other blocking fd-less wait we need.
- A blocking `waitpid` is portable across macOS and Linux with **zero**
  `#[cfg]` and no raw kqueue/pidfd code — the reviewable, mechanical port.
- The exit event is rare (once per surface) and off the hot path, so the cost
  of a parked thread (a kernel wait, ~0 CPU) is nothing. There is no latency
  budget here the way there is for the read pipeline.
- It composes cleanly with teardown: `subprocess.stop()` SIGHUPs the child,
  which unblocks the wait thread's `waitpid`, which then joins. No
  cross-thread cancellation of a kqueue registration.

Cost accepted: one extra parked thread per surface (same order as the two
pipeline threads already spawned). If a future need makes a unified reactor
attractive, the exit path is isolated behind the same `dyn FnMut` seam and
swaps without touching Exec.

## Teardown-ordering notes (`threadExit:195`)

The order is load-bearing and preserved exactly:

1. **`subprocess.stop()` BEFORE the quit pipe.** `stop` SIGHUPs the child
   process group, which stops it *producing* output. Only then do we signal
   the read thread to quit. Upstream's comment (`Exec.zig:202`): *"Quit our
   read thread after exiting the subprocess so that we don't get stuck
   waiting for data to stop flowing if it is a particularly noisy process."*
   If we signalled the reader first, a noisy child (`yes`) could keep the
   kernel queue full and the gather stage's `poll` would keep seeing `POLL.IN`
   — but the gather loop checks the quit fd on every bridge poll and every
   outer poll, so it still exits promptly; stopping the child first removes
   the race where the reader spins on a flood while we wait to signal it.
2. **Write "x" to the quit pipe** (`Exec.zig:205`). `BrokenPipe` is benign
   (reader already gone). The gather stage's `poll` set includes this fd; a
   readable quit fd ends the gather loop, which sets `done`, which lets the
   parse stage drain the ring and return.
3. **`read_thread.join()`** (`Exec.zig:227`). The io-reader (parse) thread's
   `defer gather_thread.join()` (`Exec.zig:1411`) means joining the reader
   transitively joins the gather thread first — gather sets `done` on the way
   out, parse drains and returns, then reader joins gather, then threadExit
   joins reader. No lost thread, no hang, even under an active output flood.

The quit pipe **write end is closed in `ThreadData.deinit`** (`Exec.zig:544`),
which runs *after* `threadExit` (both are `defer`s in `threadMain_`, LIFO:
`threadExit` first, then `data.deinit`). Closing it earlier would race the
`write("x")`.

## Test coverage (11 inline + integration)

The 11 upstream inline tests are all `execCommand` argv-construction tests:
2 darwin (`shell command`, `direct command`), 4 posix-portable (`shell
command empty passwd`, `shell command error passwd`, `direct command error
passwd`, `direct command config freed`), 5 windows (skipped — no Windows
target). On macOS the 2 darwin + 4 posix run (6 active); the port keeps all
11 as `#[test]` with `cfg`/skip parity so the count matches 1:1.

Integration tests (new, exercising the ported runtime the inline tests can't):
spawn `/bin/sh`, drive `echo` through gather→parse→sink, write via the
mailbox writer, resize, clean exit + exit-code capture, abnormal exit
(`kill -9`) detection, quit-pipe teardown under a `yes` flood, password-mode
detection via `stty -echo`.

## Deferrals to chunk E

- `Termio.processOutput` VT hookup (the parse sink is a closure here).
- The 16-variant `drainMailbox` handlers that touch terminal/renderer state
  (`colorSchemeReport`, `changeConfig`, `sizeReport`, `clearScreen`,
  `scrollViewport`, `jumpToPrompt`, `colorScheme`, inspector).
- Selection-scroll timer + `surface_mailbox.selection_scroll_tick`.
- `renderer_wakeup.notify()` after each drain.
- The error-screen rendering in `Thread.threadMain` (pty-exhaustion banner).
- Full `Driver`/`Handler` promotion from the spike + real pty fd registration
  into the `polling` set.
- Surface mailbox delivery of `child_exited` / `password_input` (routed
  through a `dyn FnMut` seam here; wired to the real surface mailbox in E/N).
