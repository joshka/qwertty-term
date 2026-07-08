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
//! * [`hub`] — the Termio hub + the promoted writer `Thread` loop
//!   (`src/termio/Termio.zig` + `src/termio/Thread.zig`): the [`hub::Termio`]
//!   state-container/wiring point that spawns the `Exec`, spins the io-writer
//!   loop on its own OS thread (threads + `polling` per ADR-002, promoted from
//!   `spike-runtime`), runs the 25ms resize-coalesce / 1s sync-output-reset /
//!   200ms termios timers, and hands back a cloneable [`hub::Writer`]. The
//!   terminal-touching side (sync reset, renderer wakeup) is a
//!   [`hub::HubHandler`] seam the caller fills (M2 chunk E).
//!
//! * [`shell_integration`] — the RC-injection machinery
//!   (`src/termio/shell_integration.zig`, M2 chunk G): shell detection, the
//!   per-shell env/command mutation (zsh ZDOTDIR indirection, bash
//!   `--posix`+`ENV` trickery, fish/elvish/nushell XDG_DATA_DIRS), and the
//!   `QWERTTY_TERM_SHELL_FEATURES` flag string. The vendored scripts themselves
//!   (copied verbatim from upstream, plan decision 5) live in
//!   `resources/shell-integration/`; `docs/analysis/shell-integration.md` has
//!   the full write-up including what the scripts emit (OSC 133, OSC 7,
//!   DECSCUSR bar-cursor-at-prompt).
//!
//! Deliberately NOT here (deferred):
//!
//! * `termio/Options.zig` — a field bag of pointers into config / renderer /
//!   surface subsystems; it is folded into [`hub::Termio::spawn`]'s arguments
//!   rather than ported as a standalone struct.
//! * Windows (`WindowsPty`/ConPTY) and iOS (`NullPty`) — no such targets in
//!   scope; `PosixPty` only.

pub mod backend;
pub mod exec;
pub mod hub;
pub mod mailbox;
pub mod message;
pub mod pty;
pub mod shell_integration;
pub mod size;

pub use exec::{Command, Config, Exec, Notifier, Subprocess, ThreadData, WriterLoop};
pub use hub::{HubHandler, NullHandler, Termio, Writer};
pub use mailbox::{CAPACITY, Receiver, Sender, TrySendError, Waker, channel};
pub use message::Message;
pub use pty::{Mode, Pty, Winsize};
pub use shell_integration::{
    EnvMap, Shell, ShellIntegration, ShellIntegrationFeatures, resources_dir, setup, setup_features,
};
