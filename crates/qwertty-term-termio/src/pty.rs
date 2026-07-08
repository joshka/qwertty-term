//! POSIX PTY creation and management. Port of `src/pty.zig` (`PosixPty`) +
//! `src/pty.c` from Ghostty `2da015cd6`.
//!
//! This is a thin layer over POSIX syscalls; the caller is responsible for
//! detail-oriented handling of the file handles. Upstream's `WindowsPty` and
//! `NullPty` (iOS) are not ported (no such targets in scope).
//!
//! Upstream calls libc `openpty()`; rustix has no `openpty` wrapper, so
//! [`Pty::open`] composes the identical sequence from primitives (`openpt` →
//! `grantpt` → `unlockpt` → `ptsname` → open slave → `TIOCSWINSZ`). The
//! termios surface managed here is deliberately tiny and must stay that way:
//!
//! * **IUTF8** — set on the master at open (upstream: on by default on Linux
//!   but NOT on macOS, so always set). The only flag we ever *write*.
//! * **ICANON / ECHO** — *read only*, surfaced as [`Mode`] (the 200ms
//!   password-detection poll upstream).
//! * Everything else is left at driver defaults: upstream passes
//!   `termp = NULL` to `openpty` and never configures the line discipline.

use std::ffi::CStr;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::sync::OnceLock;

use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
use rustix::termios::{
    InputModes, LocalModes, OptionalActions, tcgetattr, tcgetpgrp, tcgetwinsize, tcsetattr,
    tcsetwinsize,
};

/// Window size of a pty. Mirror of upstream's redeclared `winsize` extern
/// struct (row/col in cells, x/y in pixels).
///
/// The defaults are upstream's verbatim — "some reasonable screen size but
/// you should probably not use them".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Winsize {
    pub rows: u16,
    pub cols: u16,
    pub xpixel: u16,
    pub ypixel: u16,
}

impl Default for Winsize {
    fn default() -> Self {
        // Upstream: `ws_row = 100, ws_col = 80, ws_xpixel = 800, ws_ypixel = 600`.
        Winsize {
            rows: 100,
            cols: 80,
            xpixel: 800,
            ypixel: 600,
        }
    }
}

impl From<Winsize> for rustix::termios::Winsize {
    fn from(ws: Winsize) -> Self {
        rustix::termios::Winsize {
            ws_row: ws.rows,
            ws_col: ws.cols,
            ws_xpixel: ws.xpixel,
            ws_ypixel: ws.ypixel,
        }
    }
}

impl From<rustix::termios::Winsize> for Winsize {
    fn from(ws: rustix::termios::Winsize) -> Self {
        Winsize {
            rows: ws.ws_row,
            cols: ws.ws_col,
            xpixel: ws.ws_xpixel,
            ypixel: ws.ws_ypixel,
        }
    }
}

/// The modes of a pty. Port of `pty.zig` `Mode`.
///
/// Defaults are "the most typical values for a pty" so cross-platform code
/// that can't read them still behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    /// `ICANON` on POSIX.
    pub canonical: bool,
    /// `ECHO` on POSIX.
    pub echo: bool,
}

impl Default for Mode {
    fn default() -> Self {
        Mode {
            canonical: true,
            echo: true,
        }
    }
}

/// Errors from pty operations. Mirrors upstream's per-op error sets
/// (`OpenError`, `GetModeError`, `GetSizeError`, `SetSizeError`,
/// `ChildPreExecError`), each carrying the underlying errno.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Any failure while opening the pty pair (upstream `OpenptyFailed`).
    Openpty(rustix::io::Errno),
    /// `tcgetattr` failed while reading [`Mode`] (upstream `GetModeFailed`).
    GetMode(rustix::io::Errno),
    /// `TIOCGWINSZ` failed (upstream `IoctlFailed`).
    GetSize(rustix::io::Errno),
    /// `TIOCSWINSZ` failed (upstream `IoctlFailed`).
    SetSize(rustix::io::Errno),
    /// `setsid` failed in the fork child (upstream `ProcessGroupFailed`).
    ProcessGroup(rustix::io::Errno),
    /// `TIOCSCTTY` failed in the fork child (upstream
    /// `SetControllingTerminalFailed`).
    SetControllingTerminal(rustix::io::Errno),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Openpty(e) => write!(f, "openpty failed: {e}"),
            Error::GetMode(e) => write!(f, "getting pty mode failed: {e}"),
            Error::GetSize(e) => write!(f, "TIOCGWINSZ failed: {e}"),
            Error::SetSize(e) => write!(f, "TIOCSWINSZ failed: {e}"),
            Error::ProcessGroup(e) => write!(f, "setsid failed: {e}"),
            Error::SetControllingTerminal(e) => write!(f, "TIOCSCTTY failed: {e}"),
        }
    }
}

impl std::error::Error for Error {}

/// A POSIX pseudoterminal pair. Port of `pty.zig` `PosixPty`.
///
/// Ownership deviation from upstream (documented in the analysis): upstream's
/// `deinit` closes only the master and leaks the slave to the caller; here
/// both fds are `OwnedFd`, so dropping a `Pty` closes both. Exec (chunk D)
/// takes the fds out with [`Pty::into_parts`] before spawning so the slave
/// survives as the child's stdio source.
#[derive(Debug)]
pub struct Pty {
    master: OwnedFd,
    slave: OwnedFd,
    /// Cached slave tty name (upstream `tty_name_buf`/`tty_name`), computed
    /// lazily by [`Pty::tty_name`].
    tty_name: OnceLock<Option<std::ffi::CString>>,
}

impl Pty {
    /// Open a new PTY with the given initial size. Port of `PosixPty.open`.
    ///
    /// Reproduces `openpty(&master, &slave, NULL, NULL, &size)` +
    /// upstream's post-open setup:
    ///
    /// * `termp = NULL`: the line discipline is left at driver defaults;
    /// * the initial winsize is applied to the slave (as openpty does);
    /// * `FD_CLOEXEC` is set on the **master only** — best effort, failures
    ///   ignored like upstream (which logs and continues). The slave must be
    ///   inheritable by the child;
    /// * `IUTF8` is set on the master — fatal on failure like upstream.
    pub fn open(size: Winsize) -> Result<Pty, Error> {
        let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY).map_err(Error::Openpty)?;
        grantpt(&master).map_err(Error::Openpty)?;
        unlockpt(&master).map_err(Error::Openpty)?;
        let name = ptsname(&master, Vec::new()).map_err(Error::Openpty)?;
        let slave = rustix::fs::open(
            name.as_c_str(),
            rustix::fs::OFlags::RDWR | rustix::fs::OFlags::NOCTTY,
            rustix::fs::Mode::empty(),
        )
        .map_err(Error::Openpty)?;

        // openpty applies `winp` to the slave via TIOCSWINSZ.
        tcsetwinsize(&slave, size.into()).map_err(Error::Openpty)?;

        // Set CLOEXEC on the master fd; only the slave fd should be inherited
        // by the child process (shell/command). Best effort per upstream.
        if let Ok(flags) = fcntl_getfd(&master) {
            let _ = fcntl_setfd(&master, flags | FdFlags::CLOEXEC);
        }

        // Enable UTF-8 mode. Upstream: "I think this is on by default on
        // Linux but it is NOT on by default on macOS so we ensure that it is
        // always set." (Makes canonical-mode ERASE UTF-8 aware.)
        let mut attrs = tcgetattr(&master).map_err(Error::Openpty)?;
        attrs.input_modes |= InputModes::IUTF8;
        tcsetattr(&master, OptionalActions::Now, &attrs).map_err(Error::Openpty)?;

        Ok(Pty {
            master,
            slave,
            tty_name: OnceLock::new(),
        })
    }

    /// The master (controller) side. Read child output / write input here.
    pub fn master(&self) -> BorrowedFd<'_> {
        self.master.as_fd()
    }

    /// The slave (pty) side. Becomes the child's stdio and controlling
    /// terminal; not CLOEXEC by design.
    pub fn slave(&self) -> BorrowedFd<'_> {
        self.slave.as_fd()
    }

    /// Take ownership of `(master, slave)`, consuming the `Pty`. Exec uses
    /// this to keep the master and hand the slave to the spawned child.
    pub fn into_parts(self) -> (OwnedFd, OwnedFd) {
        (self.master, self.slave)
    }

    /// Read the current [`Mode`] flags (ICANON/ECHO) from the master.
    /// Port of `PosixPty.getMode`.
    pub fn mode(&self) -> Result<Mode, Error> {
        let attrs = tcgetattr(&self.master).map_err(Error::GetMode)?;
        Ok(Mode {
            canonical: attrs.local_modes.contains(LocalModes::ICANON),
            echo: attrs.local_modes.contains(LocalModes::ECHO),
        })
    }

    /// Return the size of the pty (`TIOCGWINSZ`). Port of `PosixPty.getSize`.
    pub fn size(&self) -> Result<Winsize, Error> {
        tcgetwinsize(&self.master)
            .map(Into::into)
            .map_err(Error::GetSize)
    }

    /// Set the size of the pty (`TIOCSWINSZ`). Port of `PosixPty.setSize`.
    /// The kernel delivers SIGWINCH to the foreground process group.
    pub fn set_size(&self, size: Winsize) -> Result<(), Error> {
        tcsetwinsize(&self.master, size.into()).map_err(Error::SetSize)
    }

    /// PID of the foreground process group on the pty, or `None` on error.
    /// Port of `PosixPty.getProcessInfo(.foreground_pid)` (upstream's
    /// comptime-keyed getter splits into two methods here).
    pub fn foreground_pid(&self) -> Option<u64> {
        let pid = tcgetpgrp(&self.master).ok()?;
        Some(pid.as_raw_nonzero().get() as u64)
    }

    /// Name of the slave pty (e.g. `/dev/ttys004`), or `None` on error.
    /// Cached after the first call, like upstream's `tty_name_buf`. Port of
    /// `PosixPty.getProcessInfo(.tty_name)`; rustix's `ptsname` internally
    /// uses the same macOS `TIOCPTYGNAME` ioctl / Linux `ptsname_r` upstream
    /// calls directly.
    pub fn tty_name(&self) -> Option<&CStr> {
        self.tty_name
            .get_or_init(|| ptsname(&self.master, Vec::new()).ok())
            .as_deref()
    }

    /// Fork-child setup: make the slave our controlling terminal. Port of
    /// `PosixPty.childPreExec` — call this in the forked child, after
    /// [`child::dup2_stdio`] (this closes the slave) and before `exec`:
    ///
    /// 1. reset the signal handlers Ghostty may have installed to `SIG_DFL`;
    /// 2. `setsid()` — new session, detach from the old controlling terminal;
    /// 3. `ioctl(slave, TIOCSCTTY)` — adopt the slave as controlling terminal
    ///    (requires being a session leader, hence after `setsid`);
    /// 4. close both pty fds (stdio already points at the slave).
    ///
    /// # Safety
    ///
    /// Must only be called in a forked child before `exec`. In a
    /// multithreaded process, POSIX allows only async-signal-safe functions
    /// between `fork` and `exec`; everything here qualifies (`sigaction`,
    /// `setsid`, `ioctl`, `close`) and nothing allocates, locks, or touches
    /// stdio. After this returns, `self`'s fds are closed *in the child's
    /// copy of the fd table* — the child must not use `self` again except to
    /// `exec` or `_exit`.
    pub unsafe fn child_pre_exec(&self) -> Result<(), Error> {
        // Reset our signals.
        unsafe { child::reset_signals() };

        // Create a new process group.
        rustix::process::setsid().map_err(Error::ProcessGroup)?;

        // Set controlling terminal.
        rustix::process::ioctl_tiocsctty(&self.slave).map_err(Error::SetControllingTerminal)?;

        // Can close master/slave pair now. Raw close: the child's fd table is
        // a copy; the parent's OwnedFds are unaffected.
        unsafe {
            libc::close(self.slave.as_raw_fd());
            libc::close(self.master.as_raw_fd());
        }

        Ok(())
    }
}

/// Fork-child helpers that belong to Exec's side of the child setup
/// (`Command.zig` `setupFd`), kept here so chunk D has the complete
/// between-fork-and-exec toolkit next to its safety rules.
///
/// Upstream child ordering (`Command.zig` start → `Exec.zig` os_pre_exec):
/// `fork` → [`dup2_stdio`] → chdir/rlimits (Exec) → [`Pty::child_pre_exec`]
/// → `execvpe`.
pub mod child {
    use std::os::fd::BorrowedFd;

    /// Reset the disposition of every signal Ghostty may have modified to
    /// `SIG_DFL` (upstream `childPreExec`'s sigaction block): ABRT ALRM BUS
    /// CHLD FPE HUP ILL INT PIPE SEGV TRAP TERM QUIT.
    ///
    /// # Safety
    ///
    /// Fork-child only, before `exec`. `sigaction` is async-signal-safe;
    /// results are ignored (best effort, as upstream).
    pub unsafe fn reset_signals() {
        const SIGNALS: [i32; 13] = [
            libc::SIGABRT,
            libc::SIGALRM,
            libc::SIGBUS,
            libc::SIGCHLD,
            libc::SIGFPE,
            libc::SIGHUP,
            libc::SIGILL,
            libc::SIGINT,
            libc::SIGPIPE,
            libc::SIGSEGV,
            libc::SIGTRAP,
            libc::SIGTERM,
            libc::SIGQUIT,
        ];
        unsafe {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut sa.sa_mask);
            sa.sa_flags = 0;
            for sig in SIGNALS {
                let _ = libc::sigaction(sig, &sa, std::ptr::null_mut());
            }
        }
    }

    /// Point the child's stdin/stdout/stderr at the pty slave. Port of
    /// `Command.zig` `setupFd` applied to the three stdio fds.
    ///
    /// On macOS/iOS/FreeBSD there is no `dup3`, so upstream first *clears*
    /// `FD_CLOEXEC` on the source fd (protects the `src == target` no-op
    /// case where `dup2` would preserve the flag through exec); on Linux
    /// upstream uses `dup3(…, flags = 0)`. Our slave is never CLOEXEC and
    /// never fd 0-2 in practice, but the flag clear is kept for exactness.
    ///
    /// # Safety
    ///
    /// Fork-child only, before `exec`. `fcntl` and `dup2` are
    /// async-signal-safe; nothing here allocates or locks.
    pub unsafe fn dup2_stdio(slave: BorrowedFd<'_>) -> rustix::io::Result<()> {
        #[cfg(not(target_os = "linux"))]
        {
            use rustix::io::{FdFlags, fcntl_getfd, fcntl_setfd};
            let flags = fcntl_getfd(slave)?;
            if flags.contains(FdFlags::CLOEXEC) {
                fcntl_setfd(slave, flags - FdFlags::CLOEXEC)?;
            }
        }
        rustix::stdio::dup2_stdin(slave)?;
        rustix::stdio::dup2_stdout(slave)?;
        rustix::stdio::dup2_stderr(slave)?;
        Ok(())
    }
}
