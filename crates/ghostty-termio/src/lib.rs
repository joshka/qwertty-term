//! Terminal IO foundations ported from Ghostty (`2da015cd6`).
//!
//! M2 chunks A+B+D (`docs/plans/m2-termio.md`); surveys in
//! `docs/analysis/termio-foundations.md` (A+B) and
//! `docs/analysis/termio-exec.md` (D). Modules:
//!
//! * [`pty`] — the POSIX PTY primitive (`src/pty.zig`): openpty semantics via
//!   rustix, IUTF8 setup, TIOCSWINSZ resize, and the documented-unsafe
//!   fork-child helpers (signal reset / setsid / TIOCSCTTY / dup2) that Exec
//!   (chunk D) runs between `fork` and `exec`.
//! * [`message`] — the writer-thread message union (`src/termio/message.zig`),
//!   1:1 including the small/stable/alloc `MessageData` write requests and the
//!   40-byte size pin.
//! * [`mailbox`] — the bounded-64 SPSC mailbox with the backpressure-unlock
//!   send (`src/termio/mailbox.zig`). This is the **binding API contract** of
//!   ADR-002 (`docs/adr/002-termio-runtime.md`), promoted from the
//!   `spike-runtime` crate that ratified it.
//! * [`backend`] — the backend dispatch seam (`src/termio/backend.zig`) as a
//!   trait; the Exec implementation lands in [`exec`].
//! * [`exec`] — the Exec backend (`src/termio/Exec.zig` plus the minimal
//!   `Thread.zig` writer glue): subprocess spawn (env/command construction,
//!   fork and the A-chunk child helpers), the two-stage read pipeline
//!   (io-gather then io-reader over a rotating ring, ports as-is per plan
//!   decision 4), the mailbox-driven writer loop, a `waitpid` exit watcher,
//!   and the 200ms termios password-detection poll. The VT parse sink and the
//!   terminal-touching mailbox handlers are a `dyn FnMut`/`Notifier` seam
//!   filled by chunk E.
//!
//! Deliberately NOT here (deferred):
//!
//! * `termio/Options.zig` — a field bag of pointers into config / renderer /
//!   surface subsystems that don't exist yet; it ports as the argument struct
//!   of `Termio::init` in chunk E.
//! * `termio/Thread.zig` — the full writer event loop (threads + `polling` per
//!   ADR-002) and with it the `Driver`/`Handler` runtime seam; chunk E
//!   promotes those from `spike-runtime` next to the real loop. Chunk D ports
//!   only the minimal writer-loop glue its tests need ([`exec::WriterLoop`]).
//!   The mailbox half of the seam ([`mailbox::Waker`]) landed with chunk B.
//! * Windows (`WindowsPty`/ConPTY) and iOS (`NullPty`) — no such targets in
//!   scope; `PosixPty` only.

pub mod backend;
pub mod exec;
pub mod mailbox;
pub mod message;
pub mod pty;
pub mod size;

pub use exec::{Command, Config, Exec, Notifier, Subprocess, ThreadData, WriterLoop};
pub use mailbox::{CAPACITY, Receiver, Sender, TrySendError, Waker, channel};
pub use message::Message;
pub use pty::{Mode, Pty, Winsize};
