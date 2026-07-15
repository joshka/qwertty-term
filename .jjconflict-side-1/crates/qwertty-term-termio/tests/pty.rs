//! PTY integration tests: the upstream `pty.zig` test ported, plus a real
//! fork/exec round trip through `/bin/sh` using the crate's own fork-child
//! helpers.
//!
//! On fork-in-test: `cargo test` runs a multithreaded process, and forking
//! one is only sound if the child restricts itself to async-signal-safe
//! calls before `exec` — which is precisely the documented contract of
//! `pty::child::dup2_stdio` / `Pty::child_pre_exec` (sigaction, setsid,
//! ioctl, dup2, fcntl, close, then execvp/_exit; no allocation, no locks).
//! `posix_spawn` was rejected because it cannot run those helpers, and
//! exercising the real fork path Exec (chunk D) will use is the point of
//! this test. All allocation (argv CStrings, buffers) happens before fork.

#![cfg(unix)]

use std::time::{Duration, Instant};

use qwertty_term_termio::pty::{Mode, Pty, Winsize, child};

const WS: Winsize = Winsize {
    rows: 50,
    cols: 80,
    xpixel: 1,
    ypixel: 1,
};

/// Port of the single upstream `pty.zig` test: open with a known size,
/// round-trip `getSize`, double the rows, round-trip again, and check the
/// slave tty name.
#[test]
fn upstream_open_resize_ttyname() {
    let pty = Pty::open(WS).expect("open pty");

    // Initialize size should match what we gave it.
    assert_eq!(pty.size().unwrap(), WS);

    // Can set and read new sizes.
    let doubled = Winsize {
        rows: WS.rows * 2,
        ..WS
    };
    pty.set_size(doubled).unwrap();
    assert_eq!(pty.size().unwrap(), doubled);

    // tty name: /dev/pts/N on Linux, /dev/ttysNNN on macOS.
    let name = pty.tty_name().expect("tty name").to_str().unwrap();
    if cfg!(target_os = "linux") {
        assert!(name.starts_with("/dev/pts/"), "{name}");
    } else {
        assert!(name.starts_with("/dev/"), "{name}");
    }
}

/// The termios contract: IUTF8 is set at open (the only flag the port ever
/// writes), and the line discipline is otherwise at driver defaults —
/// `Mode` reads back canonical+echo, which are on for a fresh pty.
#[test]
fn termios_iutf8_and_default_modes() {
    let pty = Pty::open(WS).expect("open pty");

    let attrs = rustix::termios::tcgetattr(pty.master()).unwrap();
    assert!(
        attrs
            .input_modes
            .contains(rustix::termios::InputModes::IUTF8),
        "IUTF8 must be set on the master at open"
    );

    assert_eq!(
        pty.mode().unwrap(),
        Mode {
            canonical: true,
            echo: true
        },
        "fresh pty line discipline defaults to canonical+echo"
    );
    assert_eq!(pty.mode().unwrap(), Mode::default());
}

/// Kills and reaps the child if the test panics between fork and the final
/// waitpid, so failed assertions don't leak shells.
struct ChildGuard(libc::pid_t);

impl ChildGuard {
    /// Reap the child, draining the master while we wait: a pty's output
    /// queue only empties when the master side reads it, and the shell's
    /// exit path drains its final output (tcsetattr `TCSADRAIN`) before
    /// terminating — a real terminal is always reading, so this must too
    /// (verified empirically: without the reads, `/bin/sh` on macOS never
    /// finishes exiting).
    fn wait(
        mut self,
        master: std::os::fd::BorrowedFd<'_>,
        transcript: &mut Vec<u8>,
        deadline: Duration,
    ) -> i32 {
        let pid = self.0;
        self.0 = -1; // disarm the Drop kill
        let start = Instant::now();
        let mut chunk = [0u8; 4096];
        loop {
            let mut status: libc::c_int = 0;
            let r = unsafe { libc::waitpid(pid, &mut status, libc::WNOHANG) };
            if r == pid {
                return status;
            }
            assert!(r >= 0, "waitpid failed");
            assert!(start.elapsed() < deadline, "child did not exit in time");
            match rustix::io::read(master, &mut chunk) {
                Ok(n) => transcript.extend_from_slice(&chunk[..n]),
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.0 > 0 {
            unsafe {
                libc::kill(self.0, libc::SIGKILL);
                let mut status = 0;
                libc::waitpid(self.0, &mut status, 0);
            }
        }
    }
}

fn write_all(fd: std::os::fd::BorrowedFd<'_>, mut data: &[u8]) {
    while !data.is_empty() {
        match rustix::io::write(fd, data) {
            Ok(n) => data = &data[n..],
            Err(rustix::io::Errno::INTR) => {}
            Err(e) => panic!("write to master failed: {e}"),
        }
    }
}

/// Read (non-blocking master) until `needle` appears in the accumulated
/// output or the deadline passes. Returns the transcript for diagnostics.
fn read_until(fd: std::os::fd::BorrowedFd<'_>, buf: &mut Vec<u8>, needle: &[u8]) {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut chunk = [0u8; 4096];
    while !buf.windows(needle.len()).any(|w| w == needle) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {:?}; transcript: {:?}",
            String::from_utf8_lossy(needle),
            String::from_utf8_lossy(buf)
        );
        match rustix::io::read(fd, &mut chunk) {
            Ok(0) => panic!(
                "master EOF before {:?}; transcript: {:?}",
                String::from_utf8_lossy(needle),
                String::from_utf8_lossy(buf)
            ),
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(rustix::io::Errno::AGAIN) | Err(rustix::io::Errno::INTR) => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => panic!("read from master failed: {e}"),
        }
    }
}

/// The real-shell round trip: fork, run the child through `dup2_stdio` +
/// `child_pre_exec` (upstream's exact child ordering), exec `/bin/sh`, then
/// from the parent:
///
/// * write `echo gho""stty` and read back `ghostty` (the quote split keeps
///   the marker out of the canonical-mode echo of the typed line, so a match
///   proves the shell *executed* it);
/// * `set_size` and read `TIOCGWINSZ` back through the master ioctl;
/// * run `stty size` in the shell and read back `41 123` — proving the
///   TIOCSWINSZ result is visible from the slave side too;
/// * `exit` cleanly and check the exit status.
#[test]
fn shell_roundtrip_and_resize() {
    let pty = Pty::open(WS).expect("open pty");

    // Everything the child needs, allocated BEFORE fork.
    let sh = c"/bin/sh";
    let argv = [sh.as_ptr(), std::ptr::null()];

    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");

    if pid == 0 {
        // Fork child: async-signal-safe calls only from here to exec.
        unsafe {
            if child::dup2_stdio(pty.slave()).is_err() {
                libc::_exit(125);
            }
            if pty.child_pre_exec().is_err() {
                libc::_exit(126);
            }
            libc::execvp(sh.as_ptr(), argv.as_ptr());
            libc::_exit(127); // exec failed
        }
    }

    let guard = ChildGuard(pid);

    // Parent: non-blocking reads from the master with a deadline.
    let master = pty.master();
    let flags = rustix::fs::fcntl_getfl(master).unwrap();
    rustix::fs::fcntl_setfl(master, flags | rustix::fs::OFlags::NONBLOCK).unwrap();

    let mut transcript = Vec::new();

    // Write "echo hi"-style probe, read the executed output back.
    write_all(master, b"echo gho\"\"stty\n");
    read_until(master, &mut transcript, b"ghostty");

    // Resize; verify via TIOCGWINSZ read-back on the master...
    let resized = Winsize {
        rows: 41,
        cols: 123,
        xpixel: 820,
        ypixel: 410,
    };
    pty.set_size(resized).unwrap();
    assert_eq!(pty.size().unwrap(), resized);

    // ...and via the slave side: `stty size` prints "rows cols" as the
    // child's tty reports them.
    write_all(master, b"stty size\n");
    read_until(master, &mut transcript, b"41 123");

    // The foreground process group is the shell's session.
    assert!(pty.foreground_pid().is_some());

    // Clean exit.
    write_all(master, b"exit\n");
    let status = guard.wait(master, &mut transcript, Duration::from_secs(10));
    assert!(
        libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
        "shell exited abnormally: status={status:#x}; transcript: {:?}",
        String::from_utf8_lossy(&transcript)
    );
}
