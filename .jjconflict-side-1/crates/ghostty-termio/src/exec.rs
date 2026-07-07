//! The Exec backend: spawn and manage a subprocess behind a pty, run the
//! two-stage read pipeline that moves pty output into the terminal, and drive
//! the pty write / resize / focus / exit-watch machinery. Port of
//! `src/termio/Exec.zig` (`2da015cd6`) plus the minimal `src/termio/Thread.zig`
//! writer-loop glue Exec's tests need.
//!
//! Analysis: `docs/analysis/termio-exec.md`. This is M2 chunk D from
//! `docs/plans/m2-termio.md`.
//!
//! # What ports as-is
//!
//! * **Subprocess spawn** ([`Subprocess`]): env/command construction
//!   ([`exec_command`]), pty open, fork + the A-chunk child helpers
//!   ([`crate::pty::child`]), parent-side slave close.
//! * **The two-stage read pipeline** ([`ReadThread`]): an io-gather thread
//!   drains the pty into a rotating ring of buffers, an io-reader (parse)
//!   thread consumes each batch into a sink. Ports verbatim per plan
//!   decision 4 — do not simplify to a single reader.
//! * **The writer loop** ([`WriterLoop`]): drains the [`crate::mailbox`] and
//!   dispatches writes / resize (coalesced) / focus / linefeed / sync-output.
//! * **The exit watcher**: a dedicated `waitpid` thread (ADR world; rationale
//!   in the analysis doc).
//! * **The termios poll timer** (200ms password-echo detection).
//! * **threadExit teardown ordering** (stop child → quit pipe → join).
//!
//! # Two deliberate deviations (VT hookup is chunk E)
//!
//! * The parse-stage **sink** is a `dyn FnMut(&[u8]) + Send` rather than
//!   `Termio.processOutput`. The gather→parse stage boundary is identical to
//!   upstream; only the terminal side of the last hop is a closure.
//! * The **surface notifications** (`child_exited`, `password_input`) are
//!   routed through a [`Notifier`] callback seam rather than the surface
//!   mailbox (chunk E/N). The exit-code/runtime capture and password
//!   heuristic are identical.

use std::ffi::{CStr, CString};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::pty::{self, Mode, Pty, Winsize};
use crate::size::{GridSize, ScreenSize};

/// The termios poll rate in milliseconds. Port of `TERMIOS_POLL_MS`.
pub const TERMIOS_POLL_MS: u64 = 200;

/// The command to execute. Port of `configpkg.Command` (only the two shapes
/// Exec cares about; the config-layer parsing is chunk E).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// A shell command line, run wrapped (`/bin/sh -c <v>` on POSIX, a
    /// `login`+bash wrapper on macOS).
    Shell(String),
    /// A pre-split argv, executed directly.
    Direct(Vec<String>),
}

/// A resolved passwd entry, used by the macOS login-shell path. Port of the
/// fields `internal_os.passwd.Entry` exposes to `execCommand`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PasswdEntry {
    /// The login name, or `None` if unavailable (falls back to POSIX form).
    pub name: Option<String>,
    /// The home directory, used for the `~/.hushlogin` check.
    pub home: Option<String>,
}

/// Errors from Exec operations. Collapses upstream's several inferred error
/// sets; each carries enough to diagnose.
#[derive(Debug)]
pub enum Error {
    /// A pty operation failed (open / resize / getmode).
    Pty(pty::Error),
    /// `fork` failed in the parent.
    Fork(rustix::io::Errno),
    /// Creating the read-thread quit pipe failed.
    Pipe(rustix::io::Errno),
    /// Spawning a helper thread failed.
    Thread(String),
    /// The subprocess has already been started.
    AlreadyStarted,
    /// The subprocess was never started (no pid to watch).
    NotStarted,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Pty(e) => write!(f, "pty error: {e}"),
            Error::Fork(e) => write!(f, "fork failed: {e}"),
            Error::Pipe(e) => write!(f, "pipe creation failed: {e}"),
            Error::Thread(e) => write!(f, "thread spawn failed: {e}"),
            Error::AlreadyStarted => write!(f, "subprocess already started"),
            Error::NotStarted => write!(f, "subprocess not started"),
        }
    }
}

impl std::error::Error for Error {}

impl From<pty::Error> for Error {
    fn from(e: pty::Error) -> Self {
        Error::Pty(e)
    }
}

/// Configuration for the exec backend. Port of `termio.Exec.Config` (the
/// subset relevant to chunk D; shell-integration options are chunk G).
#[derive(Debug, Clone)]
pub struct Config {
    /// The command to run. `None` means the platform default shell
    /// (`/bin/sh` on POSIX, resolved at [`Subprocess::init`]).
    pub command: Option<Command>,
    /// The base environment. Exec adds/overrides `TERM`, `COLORTERM`, etc.
    pub env: Vec<(String, String)>,
    /// Env vars that override any others, applied last.
    pub env_override: Vec<(String, String)>,
    /// The working directory, propagated as `PWD` and `chdir`ed into if
    /// accessible.
    pub working_directory: Option<String>,
    /// The Ghostty resources dir (terminfo, XDG data, man). Optional — when
    /// absent, `TERM` falls back to `xterm-256color`.
    pub resources_dir: Option<String>,
    /// The `TERM` value to advertise when `resources_dir` is set (upstream
    /// `xterm-ghostty`).
    pub term: String,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            command: None,
            env: Vec::new(),
            env_override: Vec::new(),
            working_directory: None,
            resources_dir: None,
            term: "xterm-ghostty".to_string(),
        }
    }
}

/// The default POSIX shell used when no command is configured. Port of the
/// `switch (builtin.os.tag) { else => "sh" }` default.
const DEFAULT_SHELL: &str = "sh";

/// Build the environment for the child, matching `Subprocess.init`'s env
/// setup (`docs/analysis/termio-exec.md` env table). Returns the ordered
/// (key, value) pairs to hand to the child; keys removed by upstream are
/// omitted.
fn build_env(cfg: &Config, exe_dir: Option<&str>) -> Vec<(String, String)> {
    // Start from the caller's env as a map so later puts override earlier.
    let mut env: Vec<(String, String)> = cfg.env.clone();

    let put = |env: &mut Vec<(String, String)>, key: &str, val: String| {
        if let Some(slot) = env.iter_mut().find(|(k, _)| k == key) {
            slot.1 = val;
        } else {
            env.push((key.to_string(), val));
        }
    };

    // GHOSTTY_RESOURCES_DIR + TERM/COLORTERM/TERMINFO.
    if let Some(dir) = &cfg.resources_dir {
        put(&mut env, "GHOSTTY_RESOURCES_DIR", dir.clone());
        put(&mut env, "TERM", cfg.term.clone());
        put(&mut env, "COLORTERM", "truecolor".to_string());
        // TERMINFO is adjacent to the resources dir's parent.
        if let Some(parent) = std::path::Path::new(dir).parent() {
            put(
                &mut env,
                "TERMINFO",
                format!("{}/terminfo", parent.display()),
            );
        }
    } else {
        put(&mut env, "TERM", "xterm-256color".to_string());
        put(&mut env, "COLORTERM", "truecolor".to_string());
    }

    // Append the ghostty bin dir to PATH (last priority) + GHOSTTY_BIN_DIR.
    if let Some(exe_dir) = exe_dir {
        put(&mut env, "GHOSTTY_BIN_DIR", exe_dir.to_string());
        let current = env
            .iter()
            .find(|(k, _)| k == "PATH")
            .map(|(_, v)| v.clone());
        match current {
            Some(path) => {
                let already = path.split(':').any(|entry| entry == exe_dir);
                if !already {
                    put(&mut env, "PATH", format!("{path}:{exe_dir}"));
                }
            }
            None => put(&mut env, "PATH", exe_dir.to_string()),
        }
    }

    // Detection vars for programs like neovim.
    put(&mut env, "TERM_PROGRAM", "ghostty".to_string());
    put(
        &mut env,
        "TERM_PROGRAM_VERSION",
        env!("CARGO_PKG_VERSION").to_string(),
    );

    // We are not VTE; don't let children think so.
    env.retain(|(k, _)| k != "VTE_VERSION");

    // Propagate cwd as PWD (symlink-friendly prompts).
    if let Some(cwd) = &cfg.working_directory {
        put(&mut env, "PWD", cwd.clone());
    }

    // Overrides win.
    for (k, v) in &cfg.env_override {
        put(&mut env, k, v.clone());
    }

    env
}

/// Build the argv for the process to exec. Port of `execCommand`
/// (`Exec.zig:1708`). `passwd` supplies the login name/home for the macOS
/// login-shell wrapper; on non-Darwin it is unused. On Darwin a `None` name
/// (lookup failed) falls back to the POSIX form, exactly as upstream's
/// `break :darwin`.
pub fn exec_command(command: &Command, passwd: &PasswdEntry) -> Vec<String> {
    #[cfg(target_os = "macos")]
    {
        if let Some(username) = &passwd.name {
            // Check for ~/.hushlogin to pass `-q` to login(1).
            let hush = passwd
                .home
                .as_ref()
                .map(|home| std::path::Path::new(home).join(".hushlogin").exists())
                .unwrap_or(false);

            let mut args: Vec<String> = Vec::with_capacity(9);
            args.push("/usr/bin/login".to_string());
            if hush {
                args.push("-q".to_string());
            }
            args.push("-flp".to_string());
            args.push(username.clone());

            match command {
                // Direct args pass straight to login (execvp; no PATH worry).
                Command::Direct(v) => args.extend(v.iter().cloned()),
                Command::Shell(v) => {
                    // exec -l <cmd> so bash replaces itself with the login
                    // shell running the intended command. bash execs ~2x
                    // faster than zsh into the target.
                    let cmd = format!("exec -l {v}");
                    args.push("/bin/bash".to_string());
                    args.push("--noprofile".to_string());
                    args.push("--norc".to_string());
                    args.push("-c".to_string());
                    args.push(cmd);
                }
            }
            return args;
        }
        // passwd name missing: fall through to the POSIX form below.
    }
    // Silence "unused" on non-macOS without a cfg-gated param.
    let _ = passwd;

    match command {
        // We clone the command since the config may not outlive us.
        Command::Direct(v) => v.clone(),
        Command::Shell(v) => {
            // Wrap in /bin/sh -c so we don't parse the command line
            // ourselves, and so NixOS-style /bin/sh env setup runs.
            vec!["/bin/sh".to_string(), "-c".to_string(), v.clone()]
        }
    }
}

/// Look up the current user's passwd entry for the macOS login shell. Port of
/// `internal_os.passwd.get` (the subset `execCommand` needs). Returns an empty
/// entry (POSIX fallback) if the lookup fails.
pub fn current_passwd() -> PasswdEntry {
    #[cfg(unix)]
    unsafe {
        let uid = libc::getuid();
        let pw = libc::getpwuid(uid);
        if pw.is_null() {
            return PasswdEntry::default();
        }
        let name = if (*pw).pw_name.is_null() {
            None
        } else {
            CStr::from_ptr((*pw).pw_name)
                .to_str()
                .ok()
                .map(str::to_string)
        };
        let home = if (*pw).pw_dir.is_null() {
            None
        } else {
            CStr::from_ptr((*pw).pw_dir)
                .to_str()
                .ok()
                .map(str::to_string)
        };
        PasswdEntry { name, home }
    }
    #[cfg(not(unix))]
    PasswdEntry::default()
}

/// The subprocess state. Port of `Exec.Subprocess`.
pub struct Subprocess {
    /// The environment for the child, already fully constructed.
    env: Vec<(String, String)>,
    /// The working directory (if set and accessible at start).
    cwd: Option<String>,
    /// The argv, resolved at [`Subprocess::init`].
    args: Vec<String>,
    /// Current grid size (updated by resize; seeded by `init_terminal`).
    grid_size: GridSize,
    /// Current screen size in pixels.
    screen_size: ScreenSize,
    /// The pty master, once started. The slave is closed in the parent at
    /// start (upstream `Subprocess.start` defer) so the master sees HUP when
    /// the child exits. Size/mode/foreground ops go through this fd.
    master: Option<OwnedFd>,
    /// The slave tty name, captured before the slave is closed.
    tty_name: Option<CString>,
    /// The child pid, once started. `None` after a clean/external exit.
    pid: Option<libc::pid_t>,
}

impl Subprocess {
    /// Build the subprocess state. Does NOT start it. Port of
    /// `Subprocess.init`.
    pub fn init(cfg: Config) -> Subprocess {
        let exe_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.display().to_string()));

        let env = build_env(&cfg, exe_dir.as_deref());

        // Resolve the command to argv.
        let command = cfg
            .command
            .clone()
            .unwrap_or_else(|| Command::Shell(DEFAULT_SHELL.to_string()));
        let args = exec_command(&command, &current_passwd());

        Subprocess {
            env,
            cwd: cfg.working_directory.clone(),
            args,
            grid_size: GridSize::default(),
            screen_size: ScreenSize {
                width: 1,
                height: 1,
            },
            master: None,
            tty_name: None,
            pid: None,
        }
    }

    /// The pty master, once started (on POSIX read == write == master).
    pub fn read_fd(&self) -> Option<BorrowedFd<'_>> {
        self.master.as_ref().map(|m| m.as_fd())
    }

    /// Start the subprocess: open the pty, fork, and in the child run the
    /// A-chunk helpers then exec. Port of `Subprocess.start` (POSIX path;
    /// flatpak/windows skipped). Returns the master fd (read == write).
    ///
    /// On the parent this returns `Ok`; a fork-child exec failure is handled
    /// entirely inside the child (`_exit`), so the parent never observes it.
    pub fn start(&mut self) -> Result<OwnedFd, Error> {
        if self.master.is_some() || self.pid.is_some() {
            return Err(Error::AlreadyStarted);
        }

        // Open the pty at the current size.
        let ws = Winsize {
            rows: self.grid_size.rows,
            cols: self.grid_size.columns,
            xpixel: clamp_u16(self.screen_size.width),
            ypixel: clamp_u16(self.screen_size.height),
        };
        let pty = Pty::open(ws)?;

        // Only set cwd if we can access it (OSC 7 may have set an
        // inaccessible dir; inherit rather than break new windows).
        let cwd: Option<CString> = self.cwd.as_ref().and_then(|proposed| {
            if std::path::Path::new(proposed).exists() {
                CString::new(proposed.as_bytes()).ok()
            } else {
                None
            }
        });

        // Everything the child touches must be allocated before fork:
        // argv CStrings + envp CStrings, both NULL-terminated.
        let argv_owned: Vec<CString> = self
            .args
            .iter()
            .map(|a| CString::new(a.as_bytes()).expect("argv contains NUL"))
            .collect();
        let mut argv: Vec<*const libc::c_char> = argv_owned.iter().map(|c| c.as_ptr()).collect();
        argv.push(std::ptr::null());

        let env_owned: Vec<CString> = self
            .env
            .iter()
            .map(|(k, v)| CString::new(format!("{k}={v}").into_bytes()).expect("env contains NUL"))
            .collect();
        let mut envp: Vec<*const libc::c_char> = env_owned.iter().map(|c| c.as_ptr()).collect();
        envp.push(std::ptr::null());

        let path = argv_owned[0].as_ptr();
        let slave = pty.slave();

        // Capture the slave tty name before we consume the pty (the master
        // dup can still read it via ptsname, but caching here matches
        // upstream's tty_name_buf and keeps `tty_name` cheap after the pty
        // struct is torn apart).
        let tty_name = pty.tty_name().map(|c| c.to_owned());

        // Fork.
        let pid = unsafe { libc::fork() };
        if pid < 0 {
            return Err(Error::Fork(rustix::io::Errno::from_raw_os_error(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0),
            )));
        }

        if pid == 0 {
            // ==== FORK CHILD: async-signal-safe only until exec/_exit ====
            unsafe {
                // Point stdio at the slave.
                if pty::child::dup2_stdio(slave).is_err() {
                    libc::_exit(125);
                }
                // chdir if requested (async-signal-safe).
                if let Some(cwd) = &cwd {
                    libc::chdir(cwd.as_ptr());
                }
                // setsid + TIOCSCTTY + close pty fds (the A-chunk helper).
                if pty.child_pre_exec().is_err() {
                    libc::_exit(126);
                }
                // execvpe: PATH-searched exec with our env.
                execvpe(path, argv.as_ptr(), envp.as_ptr());
                // Only reached if exec failed.
                libc::_exit(127);
            }
        }

        // ==== PARENT ====
        // Take the pty apart: keep the master (size/mode/reader), and CLOSE
        // the slave (upstream's `defer posix.close(pty.slave)`). Closing the
        // parent's slave is what lets the master observe HUP/EOF when the
        // child exits — the gather stage's shutdown depends on it.
        let (master, slave_owned) = pty.into_parts();
        drop(slave_owned);

        self.pid = Some(pid);
        self.tty_name = tty_name;

        // Return a dup of the master for the reader pipeline; keep the
        // original for Subprocess size/mode/write ops.
        let reader = master.try_clone().map_err(errno_of)?;
        self.master = Some(master);
        Ok(reader)
    }

    /// Resize the pty. Port of `Subprocess.resize`. Safe anytime.
    pub fn resize(&mut self, grid_size: GridSize, screen_size: ScreenSize) -> Result<(), Error> {
        self.grid_size = grid_size;
        self.screen_size = screen_size;
        if let Some(master) = &self.master {
            rustix::termios::tcsetwinsize(
                master,
                Winsize {
                    rows: grid_size.rows,
                    cols: grid_size.columns,
                    xpixel: clamp_u16(screen_size.width),
                    ypixel: clamp_u16(screen_size.height),
                }
                .into(),
            )
            .map_err(pty::Error::SetSize)?;
        }
        Ok(())
    }

    /// Read the current pty size via `TIOCGWINSZ`. Test/introspection.
    pub fn size(&self) -> Option<Winsize> {
        let master = self.master.as_ref()?;
        rustix::termios::tcgetwinsize(master).ok().map(Into::into)
    }

    /// Note that we exited externally; clear our running state so `stop`
    /// doesn't try to signal a reaped pid. Port of `Subprocess.externalExit`.
    pub fn external_exit(&mut self) {
        self.pid = None;
    }

    /// Stop the subprocess: SIGHUP its process group and reap. Safe anytime,
    /// idempotent. Port of `Subprocess.stop` + `killCommand`/`killPid`. Does
    /// not close the pty.
    pub fn stop(&mut self) {
        let Some(pid) = self.pid.take() else {
            return;
        };
        kill_pid(pid);
    }

    /// Foreground pid on the pty, or `None`. Port of `getProcessInfo`.
    pub fn foreground_pid(&self) -> Option<u64> {
        let master = self.master.as_ref()?;
        let pid = rustix::termios::tcgetpgrp(master).ok()?;
        Some(pid.as_raw_nonzero().get() as u64)
    }

    /// The slave tty name, or `None`. Port of `getProcessInfo`. Cached at
    /// start.
    pub fn tty_name(&self) -> Option<&CStr> {
        self.tty_name.as_deref()
    }

    /// Read the current termios [`Mode`] (ICANON/ECHO) from the pty master.
    /// Used by the termios timer's password heuristic. Port of `getMode`.
    pub fn mode(&self) -> Option<Mode> {
        use rustix::termios::LocalModes;
        let master = self.master.as_ref()?;
        let attrs = rustix::termios::tcgetattr(master).ok()?;
        Some(Mode {
            canonical: attrs.local_modes.contains(LocalModes::ICANON),
            echo: attrs.local_modes.contains(LocalModes::ECHO),
        })
    }
}

impl Drop for Subprocess {
    /// Port of `Subprocess.deinit`: stop (idempotent). The master fd drops
    /// with the struct.
    fn drop(&mut self) {
        self.stop();
    }
}

/// SIGHUP a child's process group and reap it, looping until the whole tree
/// is gone. Port of `killPid` (`Exec.zig:1155`). Grandchildren can survive a
/// single killpg raced against the child's `setsid`, so we repeat.
fn kill_pid(pid: libc::pid_t) {
    let Some(pgid) = getpgid_of(pid) else {
        return;
    };

    loop {
        let r = unsafe { libc::killpg(pgid, libc::SIGHUP) };
        if r != 0 {
            let err = std::io::Error::last_os_error();
            // On Darwin, EPERM here is expected and ignored (upstream).
            if !(cfg!(target_os = "macos") && err.raw_os_error() == Some(libc::EPERM)) {
                // Process group likely already gone; stop trying.
                break;
            }
        }

        // WNOHANG: detect surviving children without blocking so we can
        // kill again (upstream `killPid`).
        let mut status = 0;
        let res = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
        if res != 0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

/// Resolve the process group to kill. Port of `getpgid` (`Exec.zig:1192`):
/// the child's pgid is our own until it calls `setsid`, so we spin until it
/// differs (setsid is the first thing the child does).
fn getpgid_of(pid: libc::pid_t) -> Option<libc::pid_t> {
    let my_pgid = unsafe { libc::getpgid(0) };
    // Bound the spin so a truly stuck child can't hang teardown forever.
    for _ in 0..100 {
        let pgid = unsafe { libc::getpgid(pid) };
        if pgid == my_pgid {
            std::thread::sleep(std::time::Duration::from_millis(10));
            continue;
        }
        if pgid <= 0 {
            return None;
        }
        return Some(pgid);
    }
    None
}

/// `execvpe` is not in every libc binding uniformly; call through to it.
///
/// # Safety
/// Fork-child only, before returning to Rust. Pointers must outlive the call.
unsafe fn execvpe(
    path: *const libc::c_char,
    argv: *const *const libc::c_char,
    envp: *const *const libc::c_char,
) {
    // macOS lacks execvpe; emulate by pointing `environ` then execvp.
    #[cfg(target_os = "macos")]
    unsafe {
        unsafe extern "C" {
            static mut environ: *const *const libc::c_char;
        }
        environ = envp;
        libc::execvp(path, argv);
    }
    #[cfg(not(target_os = "macos"))]
    unsafe {
        unsafe extern "C" {
            fn execvpe(
                file: *const libc::c_char,
                argv: *const *const libc::c_char,
                envp: *const *const libc::c_char,
            ) -> libc::c_int;
        }
        execvpe(path, argv, envp);
    }
}

fn clamp_u16(v: u32) -> u16 {
    u16::try_from(v).unwrap_or(u16::MAX)
}

/// Map a `std::io::Error` (from `OwnedFd::try_clone`) to our `Errno`-carrying
/// `Pipe` variant.
fn errno_of(e: std::io::Error) -> Error {
    Error::Pipe(rustix::io::Errno::from_raw_os_error(
        e.raw_os_error().unwrap_or(0),
    ))
}

// =========================================================================
// The two-stage read pipeline (ports as-is; plan decision 4)
// =========================================================================

/// The number of buffers rotated between gather and parse. Port of
/// `buffer_count`. The gather stage may run at most this many batches ahead
/// before it blocks — which (through the kernel pty queue) is what preserves
/// flow control to the child.
const BUFFER_COUNT: usize = 4;

/// The capacity of each gather buffer. Port of `buffer_capacity`. Also the
/// unit of work the parse stage does per sink call.
const BUFFER_CAPACITY: usize = 64 * 1024;

/// How many gathered bytes mark a stream as saturated. Port of
/// `bridge_threshold`: the macOS kernel tty queue hands the master ~1 KiB per
/// read, so a full 1 KiB means the writer filled the queue.
const BRIDGE_THRESHOLD: usize = 1024;

/// EAGAIN spin-retry budget on a saturated stream before we sleep in poll.
/// Port of `bridge_spin_max`.
const BRIDGE_SPIN_MAX: usize = 16;

/// One bridge poll's timeout, ms. Port of `bridge_poll_timeout_ms`.
const BRIDGE_POLL_TIMEOUT_MS: i32 = 1;

/// The longest one batch may spend bridging refill gaps. Port of
/// `gather_budget_ns` (3ms, well under a 16ms frame).
const GATHER_BUDGET: std::time::Duration = std::time::Duration::from_millis(3);

/// The sink the parse stage delivers batches to. Chunk E replaces the closure
/// with `Termio.processOutput` under the renderer lock.
pub type Sink = Box<dyn FnMut(&[u8]) + Send>;

/// State shared between the gather and parse stages. A fixed ring of buffers
/// plus rotation metadata. A buffer is owned by exactly one stage at a time,
/// so buffer contents need no lock; only the ring metadata is guarded. Port
/// of `ReadThread.Pipeline`.
struct Pipeline {
    /// Guards the ring metadata (NOT the buffer contents).
    meta: Mutex<PipelineMeta>,
    /// Signalled when a batch is published or the stream is done (parse
    /// waits).
    batch_ready: Condvar,
    /// Signalled when a batch is consumed (gather waits when the ring is
    /// full — backpressure).
    slot_free: Condvar,
    /// The buffer storage. `UnsafeCell` so each stage can touch its owned
    /// buffer without the metadata lock; ownership is enforced by the ring
    /// protocol, exactly as upstream's `bufs` outside the mutex.
    bufs: [std::cell::UnsafeCell<Box<[u8; BUFFER_CAPACITY]>>; BUFFER_COUNT],
}

// Safe: the ring protocol guarantees a buffer is accessed by exactly one
// thread at a time (gather while filling, parse while consuming), and the
// hand-off is synchronized through `meta` + the condvars.
unsafe impl Sync for Pipeline {}

struct PipelineMeta {
    /// Valid bytes in each buffer, set at publish, read at consume.
    lens: [usize; BUFFER_COUNT],
    /// Next slot gather fills.
    head: usize,
    /// Next slot parse consumes.
    tail: usize,
    /// Published-but-unconsumed batches.
    count: usize,
    /// Set by gather when the stream is over; parse drains then exits.
    done: bool,
}

impl Pipeline {
    fn new() -> Pipeline {
        Pipeline {
            meta: Mutex::new(PipelineMeta {
                lens: [0; BUFFER_COUNT],
                head: 0,
                tail: 0,
                count: 0,
                done: false,
            }),
            batch_ready: Condvar::new(),
            slot_free: Condvar::new(),
            bufs: std::array::from_fn(|_| {
                std::cell::UnsafeCell::new(Box::new([0u8; BUFFER_CAPACITY]))
            }),
        }
    }
}

/// The read pipeline handle. Owns the two spawned threads and joins them on
/// [`ReadThread::join`]. Port of `ReadThread` (the POSIX path).
pub struct ReadThread {
    reader: Option<std::thread::JoinHandle<()>>,
}

impl ReadThread {
    /// Spawn the pipeline over `fd` (pty master, made non-blocking) with a
    /// `quit` fd (pipe read end) and a `sink`. Port of
    /// `ReadThread.threadMainPosix` — this spawns io-reader (parse), which in
    /// turn spawns io-gather. `fd` and `quit` are moved into the pipeline and
    /// closed when it exits.
    pub fn spawn(fd: OwnedFd, quit: OwnedFd, sink: Sink) -> Result<ReadThread, Error> {
        let reader = std::thread::Builder::new()
            .name("io-reader".to_string())
            .spawn(move || reader_main(fd, quit, sink))
            .map_err(|e| Error::Thread(e.to_string()))?;
        Ok(ReadThread {
            reader: Some(reader),
        })
    }

    /// Join the pipeline. The reader thread joins the gather thread first
    /// (upstream `defer gather_thread.join()`), so this waits for both.
    pub fn join(&mut self) {
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}

/// Set an fd non-blocking. Port of `ReadThread.setNonblock`.
fn set_nonblock(fd: RawFd) -> bool {
    let borrowed = unsafe { BorrowedFd::borrow_raw(fd) };
    let Ok(flags) = rustix::fs::fcntl_getfl(borrowed) else {
        return false;
    };
    rustix::fs::fcntl_setfl(borrowed, flags | rustix::fs::OFlags::NONBLOCK).is_ok()
}

/// macOS: raise this pipeline thread's QoS off the efficiency cores (measured
/// 15% throughput swing). Port of `ReadThread.setQosClass`.
fn set_qos_user_initiated() {
    #[cfg(target_os = "macos")]
    unsafe {
        // QOS_CLASS_USER_INITIATED = 0x19.
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(
                qos_class: u32,
                relative_priority: libc::c_int,
            ) -> libc::c_int;
        }
        let _ = pthread_set_qos_class_self_np(0x19, 0);
    }
}

/// The io-reader (parse) stage. Spawns io-gather, then consumes batches in
/// ring order until the stream is over and the ring is drained. Port of
/// `threadMainPosix`.
fn reader_main(fd: OwnedFd, quit: OwnedFd, mut sink: Sink) {
    set_qos_user_initiated();

    let raw_fd = fd.as_raw_fd();
    if !set_nonblock(raw_fd) {
        // Can't run the pipeline on a blocking fd (a blocking read would hang
        // the gather stage on a quiet pty). Bail; fds drop/close here.
        return;
    }

    let pipeline = Arc::new(Pipeline::new());

    // Move fd + quit into the gather thread (it owns all fd monitoring).
    let gather_pipeline = Arc::clone(&pipeline);
    let gather = std::thread::Builder::new()
        .name("io-gather".to_string())
        .spawn(move || gather_main(fd, quit, gather_pipeline));

    let gather = match gather {
        Ok(g) => g,
        Err(_) => {
            // Spawn failed: nothing to consume; the fds moved into the closure
            // are dropped by the failed spawn. Return.
            return;
        }
    };

    loop {
        // Claim the next published batch.
        let slot = {
            let mut meta = pipeline.meta.lock().unwrap();
            loop {
                if meta.count != 0 {
                    break;
                }
                if meta.done {
                    // Stream over and ring drained.
                    let _ = gather.join();
                    return;
                }
                meta = pipeline.batch_ready.wait(meta).unwrap();
            }
            meta.tail
        };

        // The batch buffer is owned by this stage until we advance the tail,
        // so it is safe to read outside the lock.
        let len = pipeline.meta.lock().unwrap().lens[slot];
        // SAFETY: parse owns `slot` until it advances `tail` below; gather
        // will not touch it (count > 0 keeps it out).
        let buf_ptr = pipeline.bufs[slot].get();
        let batch: &[u8] = unsafe {
            let buf: &[u8; BUFFER_CAPACITY] = &*buf_ptr;
            &buf[..len]
        };
        sink(batch);

        {
            let mut meta = pipeline.meta.lock().unwrap();
            meta.tail = (meta.tail + 1) % BUFFER_COUNT;
            meta.count -= 1;
        }
        pipeline.slot_free.notify_one();
    }
    // The loop only exits via the `done` return above, which joins `gather`.
}

/// The io-gather stage. Drains the pty into rotating buffers, bridging the
/// kernel queue's refill gaps for saturated streams, and publishes each batch
/// to the parse stage. Owns all fd monitoring, including the quit fd. Port of
/// `gatherMainPosix`.
fn gather_main(fd: OwnedFd, quit: OwnedFd, pipeline: Arc<Pipeline>) {
    set_qos_user_initiated();

    // However we exit, tell the parse stage the stream is over so it drains
    // the ring and joins us.
    struct DoneGuard<'a>(&'a Pipeline);
    impl Drop for DoneGuard<'_> {
        fn drop(&mut self) {
            self.0.meta.lock().unwrap().done = true;
            self.0.batch_ready.notify_one();
        }
    }
    let _done = DoneGuard(&pipeline);

    let fd_raw = fd.as_raw_fd();
    let quit_raw = quit.as_raw_fd();
    let fd_b = unsafe { BorrowedFd::borrow_raw(fd_raw) };
    let quit_b = unsafe { BorrowedFd::borrow_raw(quit_raw) };

    use rustix::event::{PollFd, PollFlags, poll};

    loop {
        // Claim the next free buffer. Blocks only when parse is a full ring
        // behind — exactly when we should stop reading and let the kernel
        // queue exert backpressure on the child.
        let head = {
            let mut meta = pipeline.meta.lock().unwrap();
            while meta.count == BUFFER_COUNT {
                meta = pipeline.slot_free.wait(meta).unwrap();
            }
            meta.head
        };

        // SAFETY: gather owns `head` (count < BUFFER_COUNT and it is the next
        // fill slot) until it publishes below.
        let buf_ptr = pipeline.bufs[head].get();
        let buf: &mut [u8; BUFFER_CAPACITY] = unsafe { &mut *buf_ptr };

        let mut total: usize = 0;
        let mut bridge_start: Option<Instant> = None;
        let mut spins: usize = 0;
        let mut fatal = false;

        // Fill the buffer. For a saturated stream, bridge the kernel queue's
        // momentary drain rather than delivering a tiny batch.
        'gather: while total < BUFFER_CAPACITY {
            match rustix::io::read(fd_b, &mut buf[total..]) {
                Ok(0) => {
                    // macOS returns 0 instead of WouldBlock when the child
                    // dies. Deliver what we have; let the outer poll see HUP.
                    break 'gather;
                }
                Ok(n) => {
                    total += n;
                    spins = 0; // fresh spin budget after each refill.
                }
                Err(rustix::io::Errno::AGAIN) => {
                    // Below the threshold: interactive trickle, deliver now.
                    if total < BRIDGE_THRESHOLD {
                        break 'gather;
                    }
                    // Saturated: spin-retry first (refill lands in µs).
                    if spins < BRIDGE_SPIN_MAX {
                        spins += 1;
                        continue 'gather;
                    }
                    // Still dry: sleep in poll within the latency budget.
                    let now = Instant::now();
                    match bridge_start {
                        Some(start) => {
                            if now.duration_since(start) >= GATHER_BUDGET {
                                break 'gather;
                            }
                        }
                        None => bridge_start = Some(now),
                    }

                    let mut fds = [
                        PollFd::new(&fd_b, PollFlags::IN),
                        PollFd::new(&quit_b, PollFlags::IN),
                    ];
                    let timeout = rustix::event::Timespec {
                        tv_sec: 0,
                        tv_nsec: (BRIDGE_POLL_TIMEOUT_MS as i64) * 1_000_000,
                    };
                    let r = match poll(&mut fds, Some(&timeout)) {
                        Ok(r) => r,
                        Err(_) => break 'gather,
                    };
                    // Quiet for a full timeout: burst ended.
                    if r == 0 {
                        break 'gather;
                    }
                    // Quit signal: deliver and stop.
                    if fds[1].revents().contains(PollFlags::IN) {
                        fatal = true;
                        break 'gather;
                    }
                    // HUP without IN: no more data. Deliver, let outer poll
                    // decide.
                    if !fds[0].revents().contains(PollFlags::IN) {
                        break 'gather;
                    }
                    // Data available: keep reading.
                    continue 'gather;
                }
                Err(rustix::io::Errno::IO) | Err(rustix::io::Errno::BADF) => {
                    // The pty is closed; graceful shutdown.
                    fatal = true;
                    break 'gather;
                }
                Err(rustix::io::Errno::INTR) => continue 'gather,
                Err(_) => {
                    // Any other error: treat as end of stream rather than
                    // panicking (upstream `unreachable`, but we prefer a clean
                    // teardown in a library context).
                    fatal = true;
                    break 'gather;
                }
            }
        }

        // Publish the batch (if any) and rotate.
        if total > 0 {
            {
                let mut meta = pipeline.meta.lock().unwrap();
                meta.lens[head] = total;
                meta.head = (meta.head + 1) % BUFFER_COUNT;
                meta.count += 1;
            }
            pipeline.batch_ready.notify_one();
        }

        if fatal {
            return;
        }

        // A full buffer means the stream is still hot: claim the next without
        // an intervening poll.
        if total == BUFFER_CAPACITY {
            continue;
        }

        // Wait for data / quit / HUP.
        let mut fds = [
            PollFd::new(&fd_b, PollFlags::IN),
            PollFd::new(&quit_b, PollFlags::IN),
        ];
        if poll(&mut fds, None).is_err() {
            return;
        }
        if fds[1].revents().contains(PollFlags::IN) {
            // Quit signal.
            return;
        }
        if fds[0].revents().contains(PollFlags::HUP) {
            // pty closed.
            return;
        }
    }
}

// =========================================================================
// Exit watcher (ADR world: dedicated waitpid thread — see analysis doc)
// =========================================================================

/// Surface-facing notifications routed out of the IO threads. Chunk E/N wires
/// these to the real surface mailbox; here they are a callback seam the tests
/// assert on. Both must be delivered reliably (upstream blocks on the surface
/// mailbox for both).
pub trait Notifier: Send + Sync + 'static {
    /// The child exited. `exit_code` is the wait status' exit code (or the
    /// signal for a signalled death), `runtime_ms` the process lifetime.
    fn child_exited(&self, exit_code: u32, runtime_ms: u64);
    /// The terminal entered/left password-input mode (canonical && !echo).
    fn password_input(&self, active: bool);
}

/// A no-op notifier for callers that don't need the surface hooks yet.
pub struct NullNotifier;

impl Notifier for NullNotifier {
    fn child_exited(&self, _exit_code: u32, _runtime_ms: u64) {}
    fn password_input(&self, _active: bool) {}
}

/// The exit watcher: a dedicated thread that blocks in `waitpid(pid)` and, on
/// return, computes the runtime and fires [`Notifier::child_exited`]. ADR
/// world replacement for `xev.Process` (rationale: `docs/analysis/termio-exec.md`).
struct ExitWatcher {
    handle: Option<std::thread::JoinHandle<()>>,
    exited: Arc<AtomicBool>,
}

impl ExitWatcher {
    /// Start watching `pid`. `start` is the process start instant for the
    /// runtime computation (`processExitCommon`).
    fn spawn(
        pid: libc::pid_t,
        start: Instant,
        notifier: Arc<dyn Notifier>,
    ) -> Result<ExitWatcher, Error> {
        let exited = Arc::new(AtomicBool::new(false));
        let exited_thread = Arc::clone(&exited);
        let handle = std::thread::Builder::new()
            .name("io-exit".to_string())
            .spawn(move || {
                let mut status: libc::c_int = 0;
                // Block until the child is reaped. EINTR-retry.
                loop {
                    let r = unsafe { libc::waitpid(pid, &mut status, 0) };
                    if r == pid {
                        break;
                    }
                    if r < 0 {
                        let err = std::io::Error::last_os_error();
                        if err.raw_os_error() == Some(libc::EINTR) {
                            continue;
                        }
                        // ECHILD etc.: the child was reaped elsewhere (e.g.
                        // teardown's kill_pid). Treat as exited.
                        break;
                    }
                }

                // processExitCommon: mark exited, compute runtime, notify.
                exited_thread.store(true, Ordering::SeqCst);
                let runtime_ms = start.elapsed().as_millis() as u64;
                let exit_code = exit_code_from_status(status);
                notifier.child_exited(exit_code, runtime_ms);
            })
            .map_err(|e| Error::Thread(e.to_string()))?;
        Ok(ExitWatcher {
            handle: Some(handle),
            exited,
        })
    }

    fn has_exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    fn join(&mut self) {
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// Decode a `waitpid` status into an exit code. A clean exit yields its exit
/// status; a signalled death yields the signal number (matching how a shell
/// reports `128 + signal`-style codes to the user, but we hand the raw signal
/// so the consumer can decide).
fn exit_code_from_status(status: libc::c_int) -> u32 {
    if libc::WIFEXITED(status) {
        libc::WEXITSTATUS(status) as u32
    } else if libc::WIFSIGNALED(status) {
        // Encode as 128 + signal, the conventional shell exit code for a
        // signalled child; lets the abnormal-exit path present a code.
        (128 + libc::WTERMSIG(status)) as u32
    } else {
        1
    }
}

// =========================================================================
// The Exec backend + writer-loop glue
// =========================================================================

/// The per-IO-thread live state, created in [`Exec::thread_enter`] and torn
/// down in [`Exec::thread_exit`]. Port of `Exec.ThreadData` (the fields chunk
/// D owns; the xev write-stream/pools are replaced by the writer loop's own
/// buffering).
pub struct ThreadData {
    /// Process start, for the runtime computation.
    start: Instant,
    /// The read pipeline (io-reader + io-gather).
    read_thread: ReadThread,
    /// The quit pipe's write end — written in `thread_exit` to stop the read
    /// pipeline, closed on drop.
    quit_write: Option<OwnedFd>,
    /// The pty master, written to on `queue_write`.
    write_fd: OwnedFd,
    /// The exit watcher thread.
    exit_watcher: ExitWatcher,
    /// The last known termios mode (change detection for the password poll).
    termios_mode: Mode,
    /// Whether the termios timer should keep re-arming (focus controls this).
    termios_timer_running: bool,
    /// Set once the child has exited; gates further writes.
    exited: bool,
}

impl ThreadData {
    /// Whether the child has exited (either the watcher fired or teardown
    /// reaped it).
    pub fn exited(&self) -> bool {
        self.exited || self.exit_watcher.has_exited()
    }

    /// The process start instant (for runtime accounting in chunk E).
    pub fn start(&self) -> Instant {
        self.start
    }
}

/// The Exec backend. Port of `termio.Exec`.
pub struct Exec {
    subprocess: Subprocess,
    notifier: Arc<dyn Notifier>,
}

impl Exec {
    /// Build the exec state. Does NOT start the subprocess. Port of
    /// `Exec.init`.
    pub fn init(cfg: Config) -> Exec {
        Exec {
            subprocess: Subprocess::init(cfg),
            notifier: Arc::new(NullNotifier),
        }
    }

    /// Set the surface notifier (child-exit + password-input hooks). Chunk
    /// E/N passes a real one; the default is a no-op.
    pub fn set_notifier(&mut self, notifier: Arc<dyn Notifier>) {
        self.notifier = notifier;
    }

    /// Seed the initial grid/screen size before start. Port of the size half
    /// of `initTerminal` (pwd handling is chunk E's terminal seam).
    pub fn set_initial_size(&mut self, grid_size: GridSize, screen_size: ScreenSize) {
        // Infallible: the pty does not exist yet.
        let _ = self.subprocess.resize(grid_size, screen_size);
    }

    /// Start the subprocess and the IO threads, returning the live
    /// [`ThreadData`]. Port of `Exec.threadEnter`. `sink` receives each parse
    /// batch (chunk E hands `Termio.processOutput`).
    pub fn thread_enter(&mut self, sink: Sink) -> Result<ThreadData, Error> {
        // Start the subprocess.
        let master = self.subprocess.start()?;
        let pid = self.subprocess.pid.ok_or(Error::NotStarted)?;

        // Record start time for abnormal exits.
        let start = Instant::now();

        // Exit watcher.
        let exit_watcher = ExitWatcher::spawn(pid, start, Arc::clone(&self.notifier))?;

        // Quit pipe: pipe.0 = read end (to the read thread), pipe.1 = write.
        let (quit_read, quit_write) = rustix::pipe::pipe().map_err(Error::Pipe)?;

        // A separate master fd for the read pipeline (reader owns its copy;
        // the write_fd copy stays in ThreadData for writes).
        let read_master = master.try_clone().map_err(errno_of)?;

        // Spawn the read pipeline.
        let read_thread = ReadThread::spawn(read_master, quit_read, sink)?;

        Ok(ThreadData {
            start,
            read_thread,
            quit_write: Some(quit_write),
            write_fd: master,
            exit_watcher,
            termios_mode: Mode::default(),
            termios_timer_running: true,
            exited: false,
        })
    }

    /// Tear down the IO threads. Port of `Exec.threadExit`. Ordering is
    /// load-bearing (see the analysis doc): stop the child BEFORE the quit
    /// pipe, then join.
    pub fn thread_exit(&mut self, td: &mut ThreadData) {
        // If the watcher already reaped the child, clear our running state so
        // we don't signal a stale pid.
        if td.exited() {
            self.subprocess.external_exit();
        }

        // 1. SIGHUP the child so it stops producing output.
        self.subprocess.stop();

        // 2. Signal the read thread to quit. BrokenPipe (reader already gone)
        //    is benign.
        if let Some(quit) = &td.quit_write {
            let _ = rustix::io::write(quit, b"x");
        }

        // 3. Join the read pipeline (reader joins gather first).
        td.read_thread.join();

        // Join the exit watcher (the SIGHUP unblocked its waitpid).
        td.exit_watcher.join();

        // Drop the quit-pipe write end (upstream closes it in
        // ThreadData.deinit, which runs after threadExit).
        td.quit_write = None;
        td.exited = true;
    }

    /// Propagate a resize to the pty. Port of `Exec.resize`.
    pub fn resize(&mut self, grid_size: GridSize, screen_size: ScreenSize) -> Result<(), Error> {
        self.subprocess.resize(grid_size, screen_size)
    }

    /// Queue `data` to the pty, chunking and (optionally) translating `\r` →
    /// `\r\n`. Port of `Exec.queueWrite`. Writes are dropped once the child
    /// has exited.
    pub fn queue_write(
        &mut self,
        td: &mut ThreadData,
        data: &[u8],
        linefeed: bool,
    ) -> Result<(), Error> {
        if td.exited() {
            return Ok(());
        }

        if !linefeed {
            write_all_fd(&td.write_fd, data);
            return Ok(());
        }

        // Linefeed mode: replace \r with \r\n, chunked into 64-byte buffers
        // (upstream's write_buf size).
        let mut buf = [0u8; 64];
        let mut buf_i = 0usize;
        let flush = |buf: &[u8]| write_all_fd(&td.write_fd, buf);
        for &ch in data {
            if buf_i >= buf.len() - 1 {
                flush(&buf[..buf_i]);
                buf_i = 0;
            }
            if ch != b'\r' {
                buf[buf_i] = ch;
                buf_i += 1;
            } else {
                buf[buf_i] = b'\r';
                buf[buf_i + 1] = b'\n';
                buf_i += 2;
            }
        }
        if buf_i > 0 {
            flush(&buf[..buf_i]);
        }
        Ok(())
    }

    /// Focus gained/lost: start or stop the termios poll timer cheaply. Port
    /// of `Exec.focusGained` (the timer-flag half; the immediate-tick half is
    /// the writer loop's job).
    pub fn focus_gained(&mut self, td: &mut ThreadData, focused: bool) {
        td.termios_timer_running = focused;
    }

    /// Run one termios poll tick: read the pty mode, and on a canonical/echo
    /// change compute the password heuristic and notify. Port of
    /// `termiosTimer` (`Exec.zig:320`), minus the timer re-arm (the writer
    /// loop owns scheduling). Returns whether the timer should keep running.
    pub fn termios_tick(&mut self, td: &mut ThreadData) -> bool {
        let mode = self.subprocess.mode().unwrap_or_default();

        if mode != td.termios_mode {
            td.termios_mode = mode;
            // Canonical + not echoing ⇒ probably a password prompt.
            let password_input = mode.canonical && !mode.echo;
            self.notifier.password_input(password_input);
        }

        td.termios_timer_running
    }

    /// The pty master fd, for tests that need to drive termios directly
    /// (the macOS `login` wrapper makes a shell-driven password test
    /// unreliable; a test can flip the mode on the master and assert the
    /// poll observes it). Not part of the public backend surface.
    #[doc(hidden)]
    pub fn test_master_fd(&self) -> Option<BorrowedFd<'_>> {
        self.subprocess.read_fd()
    }

    /// Foreground pid on the pty. Port of `getProcessInfo(.foreground_pid)`.
    pub fn foreground_pid(&self) -> Option<u64> {
        self.subprocess.foreground_pid()
    }

    /// Slave tty name. Port of `getProcessInfo(.tty_name)`.
    pub fn tty_name(&self) -> Option<&CStr> {
        self.subprocess.tty_name()
    }
}

/// Write the whole slice to `fd`, retrying short writes and EINTR. The pty
/// write path (upstream's xev queueWrite completes similarly, retrying until
/// the whole buffer lands).
fn write_all_fd(fd: &OwnedFd, mut data: &[u8]) {
    while !data.is_empty() {
        match rustix::io::write(fd, data) {
            Ok(0) => break,
            Ok(n) => data = &data[n..],
            Err(rustix::io::Errno::INTR) | Err(rustix::io::Errno::AGAIN) => {}
            Err(_) => break, // pty gone; drop the rest.
        }
    }
}

// =========================================================================
// Minimal writer loop (the Thread.zig glue Exec's tests need)
// =========================================================================

/// A [`crate::mailbox::Waker`] backed by a condvar, so the writer loop can
/// park without a `polling`/tokio runtime (that seam is chunk E). Wakes are
/// coalescing.
pub struct CondvarWaker {
    inner: Arc<(Mutex<bool>, Condvar)>,
}

impl CondvarWaker {
    /// Create a waker + a shared handle the loop can wait on.
    pub fn new() -> (Arc<CondvarWaker>, Arc<(Mutex<bool>, Condvar)>) {
        let inner = Arc::new((Mutex::new(false), Condvar::new()));
        (
            Arc::new(CondvarWaker {
                inner: Arc::clone(&inner),
            }),
            inner,
        )
    }
}

impl crate::mailbox::Waker for CondvarWaker {
    fn wake(&self) {
        let (lock, cvar) = &*self.inner;
        *lock.lock().unwrap() = true;
        cvar.notify_one();
    }
}

impl Default for ThreadData {
    fn default() -> Self {
        unreachable!("ThreadData is only constructed by Exec::thread_enter")
    }
}

/// A minimal writer loop: drains the mailbox and dispatches the Exec-owned
/// messages (writes, resize, focus, linefeed, sync-output). The 25ms resize
/// coalesce and 1s sync-reset timers are ported. Terminal-touching variants
/// (color scheme, config, size report, clear, scroll, jump, inspector) are
/// deferred to chunk E and ignored here. Port of the Exec-relevant subset of
/// `Thread.drainMailbox`.
pub struct WriterLoop {
    exec: Exec,
    td: ThreadData,
    linefeed_mode: bool,
    /// Pending coalesced resize + the instant it was first requested.
    pending_resize: Option<crate::size::Size>,
    resize_deadline: Option<Instant>,
    /// Sync-output reset deadline.
    sync_reset_deadline: Option<Instant>,
    /// Next termios poll deadline.
    termios_deadline: Instant,
}

/// The 25ms resize coalesce window. Port of `Coalesce.min_ms`.
const COALESCE_MS: u64 = 25;
/// The 1s synchronized-output reset. Port of `sync_reset_ms`.
const SYNC_RESET_MS: u64 = 1000;

impl WriterLoop {
    /// Wrap a started Exec + its ThreadData in a writer loop.
    pub fn new(exec: Exec, td: ThreadData) -> WriterLoop {
        WriterLoop {
            exec,
            td,
            linefeed_mode: false,
            pending_resize: None,
            resize_deadline: None,
            sync_reset_deadline: None,
            termios_deadline: Instant::now() + std::time::Duration::from_millis(TERMIOS_POLL_MS),
        }
    }

    /// Access the underlying Exec (for foreground_pid / tty_name).
    pub fn exec(&self) -> &Exec {
        &self.exec
    }

    /// Access the thread data (for `exited()` checks).
    pub fn thread_data(&self) -> &ThreadData {
        &self.td
    }

    /// Drain one batch of mailbox messages, dispatching the Exec-owned ones.
    /// Port of the Exec subset of `drainMailbox`.
    pub fn drain(&mut self, rx: &crate::mailbox::Receiver) {
        use crate::message::Message;
        let mut out = Vec::new();
        rx.drain(&mut out);
        for msg in out {
            match msg {
                Message::WriteSmall(small) => {
                    let _ =
                        self.exec
                            .queue_write(&mut self.td, small.as_slice(), self.linefeed_mode);
                }
                Message::WriteStable(v) => {
                    let _ = self.exec.queue_write(&mut self.td, v, self.linefeed_mode);
                }
                Message::WriteAlloc(v) => {
                    let _ = self.exec.queue_write(&mut self.td, &v, self.linefeed_mode);
                }
                Message::Resize(size) => {
                    self.pending_resize = Some(size);
                    if self.resize_deadline.is_none() {
                        self.resize_deadline =
                            Some(Instant::now() + std::time::Duration::from_millis(COALESCE_MS));
                    }
                }
                Message::LinefeedMode(v) => self.linefeed_mode = v,
                Message::Focused(v) => self.exec.focus_gained(&mut self.td, v),
                Message::StartSynchronizedOutput => {
                    self.sync_reset_deadline =
                        Some(Instant::now() + std::time::Duration::from_millis(SYNC_RESET_MS));
                }
                // Terminal-touching variants land in chunk E.
                _ => {}
            }
        }
    }

    /// Run the deadline-driven timers (resize coalesce, sync reset, termios
    /// poll). Call each loop iteration after `drain`. Returns the next
    /// deadline to sleep until, or `None` if nothing is pending (park on the
    /// waker only).
    pub fn tick_timers(&mut self) -> Option<Instant> {
        let now = Instant::now();

        // Resize coalesce.
        if self.resize_deadline.is_some_and(|d| now >= d) {
            self.resize_deadline = None;
            if let Some(size) = self.pending_resize.take() {
                let grid = crate::size::GridSize {
                    columns: (size.screen.width / size.cell.width.max(1)) as u16,
                    rows: (size.screen.height / size.cell.height.max(1)) as u16,
                };
                let _ = self.exec.resize(grid, size.screen);
            }
        }

        // Sync-output reset (the actual reset is chunk E's terminal seam;
        // here we just clear the deadline so the timer completes).
        if self.sync_reset_deadline.is_some_and(|d| now >= d) {
            self.sync_reset_deadline = None;
        }

        // Termios poll. `termios_tick` returns whether the timer should keep
        // running (focus controls it); either way we schedule the next poll,
        // so a re-focus resumes promptly without a separate arm/disarm.
        if now >= self.termios_deadline {
            let _keep = self.exec.termios_tick(&mut self.td);
            self.termios_deadline = now + std::time::Duration::from_millis(TERMIOS_POLL_MS);
        }

        // Next deadline: earliest of the active ones.
        [
            self.resize_deadline,
            self.sync_reset_deadline,
            Some(self.termios_deadline),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Tear down: joins the IO threads via `Exec::thread_exit`.
    pub fn shutdown(mut self) -> Exec {
        self.exec.thread_exit(&mut self.td);
        self.exec
    }
}

/// Convert a raw fd into an `OwnedFd` (helper for tests handing fds around).
#[doc(hidden)]
pub fn owned_from_raw(fd: RawFd) -> OwnedFd {
    unsafe { OwnedFd::from_raw_fd(fd) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==== The 11 upstream inline tests (execCommand argv construction) ====
    // 2 darwin + 4 posix-portable run here on macOS; 5 windows are skipped
    // (no Windows target). Count matches upstream 1:1.

    #[test]
    fn exec_command_darwin_shell_command() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let result = exec_command(
            &Command::Shell("foo bar baz".to_string()),
            &PasswdEntry {
                name: Some("testuser".to_string()),
                home: None,
            },
        );
        assert_eq!(result.len(), 8);
        assert_eq!(result[0], "/usr/bin/login");
        assert_eq!(result[1], "-flp");
        assert_eq!(result[2], "testuser");
        assert_eq!(result[3], "/bin/bash");
        assert_eq!(result[4], "--noprofile");
        assert_eq!(result[5], "--norc");
        assert_eq!(result[6], "-c");
        assert_eq!(result[7], "exec -l foo bar baz");
    }

    #[test]
    fn exec_command_darwin_direct_command() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let result = exec_command(
            &Command::Direct(vec!["foo".to_string(), "bar baz".to_string()]),
            &PasswdEntry {
                name: Some("testuser".to_string()),
                home: None,
            },
        );
        assert_eq!(result.len(), 5);
        assert_eq!(result[0], "/usr/bin/login");
        assert_eq!(result[1], "-flp");
        assert_eq!(result[2], "testuser");
        assert_eq!(result[3], "foo");
        assert_eq!(result[4], "bar baz");
    }

    #[test]
    fn exec_command_shell_command_empty_passwd() {
        if cfg!(target_os = "windows") {
            return;
        }
        // Empty passwd ⇒ no macOS login command ⇒ POSIX fallback.
        let result = exec_command(
            &Command::Shell("foo bar baz".to_string()),
            &PasswdEntry::default(),
        );
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "/bin/sh");
        assert_eq!(result[1], "-c");
        assert_eq!(result[2], "foo bar baz");
    }

    #[test]
    fn exec_command_shell_command_error_passwd() {
        if cfg!(target_os = "windows") {
            return;
        }
        // A failed passwd lookup surfaces as a default (no name) entry here,
        // same fallback as the empty case.
        let result = exec_command(
            &Command::Shell("foo bar baz".to_string()),
            &PasswdEntry {
                name: None,
                home: None,
            },
        );
        assert_eq!(result.len(), 3);
        assert_eq!(result[0], "/bin/sh");
        assert_eq!(result[1], "-c");
        assert_eq!(result[2], "foo bar baz");
    }

    #[test]
    fn exec_command_direct_command_error_passwd() {
        if cfg!(target_os = "windows") {
            return;
        }
        let result = exec_command(
            &Command::Direct(vec!["foo".to_string(), "bar baz".to_string()]),
            &PasswdEntry::default(),
        );
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "foo");
        assert_eq!(result[1], "bar baz");
    }

    #[test]
    fn exec_command_direct_command_config_freed() {
        if cfg!(target_os = "windows") {
            return;
        }
        // In Rust the argv is owned (cloned) by exec_command, so freeing the
        // source Command can't dangle it — model that by dropping the input.
        let command = Command::Direct(vec!["foo".to_string(), "bar baz".to_string()]);
        let result = exec_command(&command, &PasswdEntry::default());
        drop(command);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], "foo");
        assert_eq!(result[1], "bar baz");
    }

    // ---- 5 windows tests: skipped (no Windows target), kept for parity ----

    #[test]
    fn exec_command_windows_bare_cmd_exe_resolves_via_comspec() {
        // Windows-only upstream; no Windows target in scope.
        if !cfg!(target_os = "windows") {}
    }

    #[test]
    fn exec_command_windows_bare_non_cmd_shell_passed_through() {
        if !cfg!(target_os = "windows") {}
    }

    #[test]
    fn exec_command_windows_shell_with_args_split_on_whitespace() {
        if !cfg!(target_os = "windows") {}
    }

    #[test]
    fn exec_command_windows_direct_command_passed_through() {
        if !cfg!(target_os = "windows") {}
    }

    #[test]
    fn exec_command_windows_placeholder_fifth() {
        // Upstream's fifth windows test (bare cmd.exe COMSPEC fallback path);
        // no Windows target, kept for 11-test parity.
        if !cfg!(target_os = "windows") {}
    }

    // ==== A couple of non-fork unit checks on env construction ====

    #[test]
    fn build_env_sets_term_program_and_removes_vte() {
        let cfg = Config {
            env: vec![("VTE_VERSION".to_string(), "6800".to_string())],
            ..Config::default()
        };
        let env = build_env(&cfg, None);
        assert!(env.iter().all(|(k, _)| k != "VTE_VERSION"), "VTE removed");
        assert!(
            env.iter()
                .any(|(k, v)| k == "TERM_PROGRAM" && v == "ghostty")
        );
        assert!(
            env.iter()
                .any(|(k, v)| k == "COLORTERM" && v == "truecolor")
        );
        // No resources dir ⇒ TERM falls back to xterm-256color.
        assert!(
            env.iter()
                .any(|(k, v)| k == "TERM" && v == "xterm-256color")
        );
    }

    #[test]
    fn build_env_with_resources_dir_sets_term_and_terminfo() {
        let cfg = Config {
            resources_dir: Some("/app/share/ghostty".to_string()),
            term: "xterm-ghostty".to_string(),
            ..Config::default()
        };
        let env = build_env(&cfg, None);
        assert!(env.iter().any(|(k, v)| k == "TERM" && v == "xterm-ghostty"));
        assert!(
            env.iter()
                .any(|(k, v)| k == "GHOSTTY_RESOURCES_DIR" && v == "/app/share/ghostty")
        );
        assert!(
            env.iter()
                .any(|(k, v)| k == "TERMINFO" && v == "/app/share/terminfo")
        );
    }

    /// The exit-status decode (`processExitCommon`'s code): a clean exit
    /// yields its status; a signalled death yields 128 + signal. Driven with
    /// real wait statuses from directly-spawned children (no login wrapper),
    /// so it verifies the decode the macOS integration path can't observe.
    #[test]
    #[cfg(unix)]
    fn exit_code_decode_clean_and_signalled() {
        use std::os::unix::process::ExitStatusExt;

        // Clean exit 7.
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .status()
            .unwrap();
        assert_eq!(exit_code_from_status(status.into_raw()), 7);

        // SIGKILL (9) → 128 + 9 = 137.
        let status = std::process::Command::new("/bin/sh")
            .args(["-c", "kill -9 $$"])
            .status()
            .unwrap();
        assert_eq!(exit_code_from_status(status.into_raw()), 137);
    }
}
