# Termio foundations (`pty.zig` + `termio/{message,mailbox,backend,Options}.zig`)

Surveyed and ported against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse HEAD`; the six files below are byte-identical
between that commit and current upstream HEAD `38e49a232`). The Rust port lives
in `crates/ghostty-termio`. This covers M2 chunks A (PTY primitive) and B
(termio plumbing) from `docs/plans/m2-termio.md`; the runtime decision it plugs
into is `docs/adr/002-termio-runtime.md` (ACCEPTED — threads + `polling`; its
mailbox API contract is binding).

Zig references:

| file                    | LoC | tests | Rust module                       |
| ----------------------- | --- | ----- | --------------------------------- |
| `src/pty.zig`           | 506 | 1     | `ghostty-termio/src/pty.rs`       |
| `src/pty.c`             | 40  | —     | (folded into rustix calls)        |
| `src/termio/message.zig`| 108 | 2     | `ghostty-termio/src/message.rs`   |
| `src/termio/mailbox.zig`| 106 | 0     | `ghostty-termio/src/mailbox.rs`   |
| `src/termio/backend.zig`| 129 | 0     | `ghostty-termio/src/backend.rs`   |
| `src/termio/Options.zig`| 41  | 0     | deferred to chunk E (see below)   |

## `pty.zig` — the PTY primitive

`Pty` is a comptime platform switch: `PosixPty` everywhere we care about,
`WindowsPty` (ConPTY named pipes) on Windows, `NullPty` stopgap on iOS. The
Rust port covers `PosixPty` only; Windows/iOS are out of scope for M2 (noted
as a deviation, revisit if a Windows target ever lands).

### open / openpty semantics

`PosixPty.open(size)` calls libc `openpty(&master, &slave, null, null, &size)`
(via `pty.c`, which exists only to pull in the right per-platform headers:
`<util.h>` on macOS, `<pty.h>` on Linux, `<libutil.h>` on FreeBSD, plus
fallback ioctl constants). Semantics to preserve:

1. **`termp = NULL`** — the slave termios is left at driver defaults. Upstream
   does NOT configure a line discipline; the only explicit termios change is
   IUTF8 (below). Everything else (ICANON, ECHO, ISIG, ICRNL, OPOST, …) is
   whatever the OS default pty line discipline provides.
2. **`winp = &size`** — the initial winsize is applied at open time (openpty
   does TIOCSWINSZ on the slave).
3. **CLOEXEC on the master only**, set post-open via
   `fcntl(F_GETFD/F_SETFD)`. Failures are logged and *ignored* (non-fatal).
   The slave must stay inheritable — the child's stdio comes from it.
4. **IUTF8 on the master**: `tcgetattr(master)`, `c_iflag |= IUTF8`,
   `tcsetattr(master, TCSANOW)`. Comment upstream: on by default on Linux,
   NOT on macOS, so it is set unconditionally. Failure here IS fatal
   (`error.OpenptyFailed`). IUTF8 makes canonical-mode ERASE processing
   UTF-8-aware (backspace deletes a whole codepoint, not one byte).
5. On any open failure both fds are closed (`errdefer`).

Rust mapping: rustix has no `openpty` wrapper, so the port composes the same
sequence from primitives — `openpt(RDWR|NOCTTY)` → `grantpt` → `unlockpt` →
`ptsname` → open slave `O_RDWR|O_NOCTTY` → `tcsetwinsize(slave)` — then the
CLOEXEC and IUTF8 steps exactly as upstream. rustix's `ptsname` handles the
macOS quirk internally (weak-linked `ptsname_r`, falling back to the
`TIOCPTYGNAME` ioctl with its 128-byte minimum buffer — the same ioctl
`pty.zig` uses directly for `getProcessInfo(.tty_name)`).

**Lifetime deviation**: `PosixPty.deinit` closes only the master; the slave is
deliberately leaked to the caller ("the slave side is never closed
automatically by this struct"). In Rust both fds are `OwnedFd`, so `Drop`
closes both; Exec (chunk D) takes the slave out (`Pty::take_slave` /
`into_parts`) before it can be dropped. RAII is strictly safer than upstream's
manual discipline; the observable contract (master closed on teardown, slave
owned by whoever spawns the child) is preserved.

### The mode flags Pty manages

Exactly three termios touchpoints — enumerate, because "matching upstream
exactly" means *not* configuring anything else:

- **IUTF8** (input flag) — set once on the master at open. The only flag
  upstream ever *sets*.
- **ICANON** (local flag) — *read only*, surfaced as `Mode.canonical`.
- **ECHO** (local flag) — *read only*, surfaced as `Mode.echo`.

`Mode { canonical = true, echo = true }` defaults match "the most typical
values for a pty" so cross-platform code works. `getMode` is what the 200 ms
password-detection poll (Surface, later) uses: canonical && !echo ⇒ the
foreground process is reading a password.

### Resize

`setSize` = `ioctl(master, TIOCSWINSZ, &winsize)`; `getSize` =
`ioctl(master, TIOCGWINSZ, &ws)`. Rust: `tcsetwinsize`/`tcgetwinsize` (same
ioctls under the hood). The winsize struct is row/col/xpixel/ypixel `u16`s;
upstream redeclares it with defaults `{100, 80, 800, 600}` ("reasonable screen
size but you should probably not use them") — the Rust `Winsize::default()`
mirrors those values verbatim. The kernel delivers SIGWINCH to the foreground
process group on TIOCSWINSZ; no extra signalling needed.

### The fork/child split: what is Pty's vs Exec's

Upstream child-side ordering (from `Command.zig:expandPath→start` +
`Exec.zig:1018-1034`), all between `fork()` and `execvpe`:

1. **Command.zig** (`setupFd`): dup2 slave → stdin/stdout/stderr. On
   macOS/FreeBSD it first *clears* FD_CLOEXEC on the source fd (no dup3), on
   Linux it uses dup3 with flags=0. Belongs to **Exec/Command** (chunk D).
2. **Command.zig**: chdir(cwd), restore rlimits — **Exec** (chunk D).
3. **`pty.childPreExec`** (the `os_pre_exec` hook) — **Pty** (this chunk):
   - reset 13 signals to SIG_DFL: ABRT ALRM BUS CHLD FPE HUP ILL INT PIPE
     SEGV TRAP TERM QUIT (empty mask, flags 0);
   - `setsid()` — new session, drop the old controlling terminal;
   - `ioctl(slave, TIOCSCTTY, 0)` — make the slave the controlling terminal
     (must be after setsid; the fd must be a session leader's);
   - close both slave and master fds (stdio already points at the slave via
     the dup2s from step 1).
4. **Command.zig**: `execvpe`.

So Pty owns *session/controlling-terminal/signal* setup; Exec owns *stdio
wiring, cwd, env, exec*. The Rust port keeps that split: `Pty::child_pre_exec`
(step 3) plus a `pty::child` helper module providing the dup2-stdio piece
(step 1) that chunk D's Command port will call — both `unsafe fn`s documented
as **fork-child only, async-signal-safe only** (no allocation, no locks, no
stdio, libc sigaction/dup2/close + raw syscalls via rustix only). This is also
why the helpers can't be "safe": between fork and exec in a multithreaded
process, POSIX permits only async-signal-safe calls, a property the type
system can't see.

`getProcessInfo` (comptime enum → type mapping) splits into two methods in
Rust: `foreground_pid()` (`tcgetpgrp(master)`, used by Surface for
"is vim running" checks) and `tty_name()` (cached; macOS `TIOCPTYGNAME`
ioctl / Linux `ptsname_r` — both inside rustix's `ptsname`). Both return
`Option` (upstream returns `?T`, logging errors).

### Upstream test (ported)

One test: open with `{50, 80, 1, 1}`, `getSize` round-trips, double the rows,
`setSize`/`getSize` round-trips, `tty_name` starts with `/dev/` (macOS) or
`/dev/pts/` (Linux).

## `termio/message.zig` — the writer-thread message union

`Message = union(enum)` — every message a producer (surface/renderer side) can
post to the IO thread. All 16 variants:

| variant                     | payload                               | Rust                                           |
| --------------------------- | ------------------------------------- | ---------------------------------------------- |
| `color_scheme_report`       | `{ force: bool }`                     | same                                           |
| `crash`                     | void (debug/testing)                  | same                                           |
| `change_config`             | `{ alloc, *DerivedConfig }`           | `Box<DerivedConfig>` (stub type until chunk D) |
| `inspector`                 | bool                                  | same                                           |
| `resize`                    | `renderer.Size` (screen+cell+padding) | local `Size` mirror (see below)                |
| `size_report`               | `size_report.Style` enum              | local `SizeReport` enum                        |
| `clear_screen`              | `{ history: bool }`                   | same                                           |
| `scroll_viewport`           | `Terminal.ScrollViewport`             | `ghostty_vt::terminal::ScrollViewport`         |
| `selection_scroll`          | bool (start/stop tick timer)          | same                                           |
| `jump_to_prompt`            | isize                                 | same                                           |
| `start_synchronized_output` | void (arms the 1 s reset timer)       | same                                           |
| `linefeed_mode`             | bool (mode 20)                        | same                                           |
| `focused`                   | bool                                  | same                                           |
| `write_small`               | `WriteReq.Small` (`[38]u8` + len)     | same                                           |
| `write_stable`              | `WriteReq.Stable` (`[]const u8`)      | `&'static [u8]` (see deviation)                |
| `write_alloc`               | `WriteReq.Alloc` (`{ alloc, []u8 }`)  | `Vec<u8>`                                      |

### The ~40-byte packing rationale

Upstream pins `@sizeOf(Message) == 40` with a test, "so we don't grow our IO
message size without explicitly wanting to". The layout math: the largest
payload is `write_small` = 38 data bytes + 1 len byte = 39, +1 tag byte = 40
(the 38 magic number is chosen *backwards* from the largest other member —
`change_config` at 24 bytes and `resize` at 32 bytes must fit — so small
writes use every byte the union already pays for). 40 bytes ⇒ ~26,000 queued
messages per MB; with the bounded-64 queue the mailbox is ~2.5 KB. The Rust
enum reproduces the exact same figure: `WriteSmall` = 39 bytes + tag, rounded
to 40 by the `Vec` variant's 8-byte alignment, and the port keeps the
`size_of::<Message>() == 40` test (meaningful in Rust for the same reason:
accidental payload growth is a perf smell on the hot write path).

`WriteReq = MessageData(u8, 38)` (from `datastruct/message_data.zig`) is a
three-way ownership union used for thread messaging:

- `small` — inline `[38]u8` + `IntFittingRange(0,38)` len (u8 in Rust);
- `stable` — a borrowed slice the sender guarantees outlives processing
  (used for static/const data). **Rust deviation**: `&'static [u8]`, because
  an unconstrained borrowed slice crossing threads is exactly what lifetimes
  exist to forbid; upstream's uses are static data. If chunk D finds a
  genuinely non-static stable use, that call site gets an `Arc<[u8]>` variant
  instead — do not weaken this to a raw pointer.
- `alloc` — owned heap data (Zig carries the allocator in the message; Rust's
  `Vec` owns implicitly).

`Message.writeReq(alloc, data)` picks small vs alloc by length (never
produces stable — stable is opt-in at the call site). Ported as
`Message::write_req(&[u8])`. `MessageData`'s own two inline tests (init
small / init alloc) are ported alongside the message-size test.

## `termio/mailbox.zig` — backpressure and the renderer mutex

Upstream `Mailbox` is a union with a single active variant `spsc`:
`BlockingQueue(termio.Message, 64)` + `xev.Async` wakeup handle. (An
`unbounded` variant for libghostty is commented out upstream — noted, not
ported.) Semantics that must survive:

- **`send(msg, mutex: ?*Mutex)`** — the deadlock-avoidance path, plan
  decision 3. Fast path: `push(.instant)`; if that fails the queue is full:
  1. `wakeup.notify()` — kick the writer thread so it drains (if notify
     fails upstream logs "data will be dropped" and returns);
  2. **unlock the caller's mutex** — this is the renderer state lock. The
     writer thread's drain handlers (resize, focus) may need that lock; if
     the producer kept holding it: producer waits for space ← writer waits
     for lock ← deadlock;
  3. `push(.forever)` — block until space;
  4. re-lock the caller's mutex (via `defer`).

  Writes themselves don't need the render lock — it's the *other* messages
  in the same queue (resize/focus) that do; that's why the unlock is
  unconditional on the full-queue path.
- **`notify()`** — wakeup decoupled from enqueue. Producers batch `try_send`s
  and notify once; the queue carries data, the async handle carries "look at
  the queue". This is what lets wakeups coalesce.

The Rust port is the **spike-runtime mailbox promoted verbatim** (it was
written against this file for ADR-002 and its API is the ADR's binding
contract): bounded-64 `Mutex<VecDeque>` + `not_full` condvar, `Waker` trait as
the one runtime seam (`polling::Poller::notify` ≙ `xev.Async.notify`),
`try_send` / `notify` / `send` (= try+notify), and `send_with_unlock(msg,
guard, &lock) -> guard` which *consumes* the caller's `MutexGuard` and
re-issues a fresh one — the unlock-while-blocked invariant is enforced by the
borrow checker instead of a comment. `Receiver::drain` pulls everything under
one lock acquisition, signals `not_full` (unblocking any parked
`send_with_unlock`), and returns the batch so handlers run *after* the queue
lock drops (Zig `drainMailbox` shape; upstream's consumer lives in
`Thread.zig`, ported in chunk E).

Differences from upstream, both deliberate: (1) upstream drops the message if
`notify()` fails — our `Waker::wake` is infallible (`Poller::notify` failure
has no recovery; the spike treats it as unreachable); (2) upstream's
`BlockingQueue` is a fixed ring buffer, ours is a `VecDeque` with capacity
checks — same observable semantics, no unsafe ring index math until profiling
says otherwise.

## `termio/backend.zig` — dispatch shape

Not a vtable: `Kind = enum { exec }`, and `Backend` / `Config` / `ThreadData`
are `union(Kind)` with hand-written comptime dispatch to `termio.Exec` — i.e.
closed-world static dispatch with exactly one variant today. The method set is
the actual backend contract:

`deinit`, `initTerminal(*Terminal)`, `threadEnter(alloc, *Termio,
*ThreadData)`, `threadExit(*ThreadData)`, `focusGained(td, bool)`,
`resize(GridSize, ScreenSize)`, `queueWrite(alloc, td, []u8, linefeed:
bool)`, `childExitedAbnormally(gpa, *Terminal, exit_code, runtime_ms)`,
`getProcessInfo(comptime)`.

Rust port: a `Backend` trait with the same nine methods (getProcessInfo split
into `foreground_pid`/`tty_name` as in Pty) and a `Kind` enum. Trait objects
are unnecessary — chunk E can hold an `enum BackendImpl { Exec(Exec) }`
mirroring the Zig union — but the *trait* is the seam Exec implements in
chunk D, so D and E can land independently. The `Termio` and `ThreadData`
parameter types don't exist yet; they appear as opaque placeholder structs
owned by this crate and filled in by chunks D/E. `WRITE_REQ_PREALLOC` (32,
power of two — Exec's write-request pool size) is carried as a constant.

## `termio/Options.zig` — deferred to chunk E

Options is a plain field bag handed to `Termio.init`: size, `*const Config` +
`DerivedConfig`, the backend, the mailbox, `*renderer.State`, renderer wakeup
`xev.Async`, renderer mailbox, surface mailbox. Six of its eight fields are
pointers into subsystems that don't exist yet (full config, renderer state /
wakeup / mailbox, surface mailbox) — porting it now would mean inventing stub
types for every neighbor and churning them in D/E. It ports naturally as the
argument struct of `Termio::init` in chunk E. Nothing in chunks A–D consumes
it.

## Port status / deviations summary

- `PosixPty` only; `WindowsPty`/`NullPty` not ported (no Windows/iOS target).
- `Pty` owns both fds RAII-style; upstream leaks the slave from `deinit`.
- `write_stable` is `&'static [u8]`, not an arbitrary borrowed slice.
- `Waker::wake` infallible vs upstream's logged-and-dropped notify failure.
- `getProcessInfo(comptime)` → two methods returning `Option`.
- `Options.zig` deferred to chunk E; `Termio`/`ThreadData` are placeholders.
- `Size`/`GridSize`/`ScreenSize` are local mirrors of `renderer.Size` types;
  chunk E reconciles them with `crates/ghostty-renderer/src/size.rs` (which
  already ports the same Zig structs) once the crates are allowed to couple.
