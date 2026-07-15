//! The termio backend seam. Port of `src/termio/backend.zig` (Ghostty
//! `2da015cd6`).
//!
//! Upstream is not a vtable: `Backend`/`Config`/`ThreadData` are
//! `union(Kind)` with hand-written dispatch and exactly one variant, `exec`.
//! The Rust port expresses the method set as a trait so Exec (chunk D) and
//! the Termio hub (chunk E) can land independently; E is free to hold a
//! closed `enum` over implementations rather than a trait object, mirroring
//! the Zig union.
//!
//! Exec does not exist yet, so the types it threads through the trait —
//! upstream's `termio.Termio` and per-backend `ThreadData` — are opaque
//! placeholders owned by this crate and fleshed out by chunks D/E. The trait
//! *shape* (method set, argument order) is the ported artifact here;
//! signatures that mention placeholders are expected to sharpen when Exec
//! lands. Allocator parameters are dropped throughout (Rust's global
//! allocator is implicit).

use std::ffi::CStr;

use crate::size::{GridSize, ScreenSize};
use qwertty_term_vt::terminal::Terminal;

/// The preallocation size for the write request pool. This should be big
/// enough to satisfy most write requests. It must be a power of 2. Port of
/// `WRITE_REQ_PREALLOC = 2^5` (consumed by Exec in chunk D).
pub const WRITE_REQ_PREALLOC: usize = 32;

/// The kinds of backends. Port of `backend.Kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Exec uses posix exec to run a command with a pty.
    Exec,
}

/// Configuration for the various backend types. Port of `backend.Config`.
/// The `Exec` payload (`termio.Exec.Config`: command, env, cwd, shell
/// integration options, …) arrives with chunk D.
#[derive(Debug, Clone)]
pub enum Config {
    /// Exec uses posix exec to run a command with a pty. Payload
    /// (`termio.Exec.Config`) arrives with chunk D.
    Exec,
}

/// Placeholder for the termio hub (`termio.Termio`, chunk E): the state
/// shared between the surface and IO threads that `thread_enter` wires a
/// backend into.
#[derive(Debug, Default)]
pub struct Termio {}

/// Placeholder for the backend's per-IO-thread state (upstream
/// `backend.ThreadData` wrapping `Exec.ThreadData`: write pool, pty stream
/// handle, process-exit watcher, …). Owned by chunk D.
#[derive(Debug, Default)]
pub struct ThreadData {}

/// A backend is responsible for owning the pty behavior and providing
/// read/write capabilities. Port of the `backend.Backend` union's method
/// set; `deinit` is `Drop`, and `getProcessInfo(comptime)` splits into
/// [`Backend::foreground_pid`] / [`Backend::tty_name`] as in
/// [`crate::pty::Pty`].
pub trait Backend {
    /// Errors surfaced by the fallible operations (upstream `!void` with
    /// inferred error sets; Exec picks the concrete type in chunk D).
    type Error: std::error::Error;

    /// Which kind of backend this is (the Zig union tag).
    fn kind(&self) -> Kind;

    /// Hook up the backend to a freshly-created terminal (upstream
    /// `initTerminal`: sets initial pwd, default cursor, …).
    fn init_terminal(&mut self, terminal: &mut Terminal);

    /// Called from the IO thread as it starts: spawn the subprocess, register
    /// pty reads with the event loop, populate `thread_data`.
    fn thread_enter(
        &mut self,
        termio: &mut Termio,
        thread_data: &mut ThreadData,
    ) -> Result<(), Self::Error>;

    /// Called from the IO thread as it exits: tear down watchers, restore
    /// pty state, reap.
    fn thread_exit(&mut self, thread_data: &mut ThreadData);

    /// The surface gained (`true`) or lost (`false`) focus; forwards
    /// focus-event mode reports to the pty when enabled.
    fn focus_gained(
        &mut self,
        thread_data: &mut ThreadData,
        focused: bool,
    ) -> Result<(), Self::Error>;

    /// Propagate a resize to the pty (`TIOCSWINSZ`).
    fn resize(&mut self, grid_size: GridSize, screen_size: ScreenSize) -> Result<(), Self::Error>;

    /// Queue `data` to be written to the pty. `linefeed` requests
    /// linefeed-mode translation (`\r` → `\r\n`).
    fn queue_write(
        &mut self,
        thread_data: &mut ThreadData,
        data: &[u8],
        linefeed: bool,
    ) -> Result<(), Self::Error>;

    /// The child process exited abnormally; render the exit-code notice into
    /// the terminal (upstream prints the "process exited" banner).
    fn child_exited_abnormally(
        &mut self,
        terminal: &mut Terminal,
        exit_code: u32,
        runtime_ms: u64,
    ) -> Result<(), Self::Error>;

    /// PID of the foreground process group on the backend's pty, or `None`
    /// if unavailable. (`getProcessInfo(.foreground_pid)`.)
    fn foreground_pid(&mut self) -> Option<u64>;

    /// Name of the backend's slave pty, or `None` if unavailable.
    /// (`getProcessInfo(.tty_name)`.)
    fn tty_name(&mut self) -> Option<&CStr>;
}
