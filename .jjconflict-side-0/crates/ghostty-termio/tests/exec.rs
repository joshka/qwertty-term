//! Exec integration tests: spawn a real `/bin/sh`, drive output through the
//! full two-stage read pipeline (gather → parse → sink), write via the
//! mailbox writer loop, resize, and exercise clean/abnormal exit, teardown
//! under an output flood, and password-mode detection.
//!
//! These exercise the ported runtime the inline `execCommand` tests can't:
//! the fork/exec path, the ring-buffer pipeline, the exit watcher, the quit
//! pipe, and the termios poll.

#![cfg(unix)]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghostty_termio::exec::{Command, Config, Exec, Notifier, Sink, ThreadData, WriterLoop};
use ghostty_termio::mailbox::{self, Sender};
use ghostty_termio::message::Message;
use ghostty_termio::size::{GridSize, ScreenSize};

/// A sink that appends every parse batch to a shared buffer.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    fn sink(&self) -> Sink {
        let buf = Arc::clone(&self.0);
        Box::new(move |batch: &[u8]| buf.lock().unwrap().extend_from_slice(batch))
    }

    fn snapshot(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }

    fn contains(&self, needle: &[u8]) -> bool {
        let buf = self.0.lock().unwrap();
        buf.windows(needle.len()).any(|w| w == needle)
    }
}

/// Records surface notifications the tests assert on.
#[derive(Default)]
struct TestNotifier {
    exited: AtomicBool,
    exit_code: AtomicU32,
    runtime_ms: AtomicU64,
    password_active: AtomicBool,
    password_seen_true: AtomicBool,
}

impl Notifier for TestNotifier {
    fn child_exited(&self, exit_code: u32, runtime_ms: u64) {
        self.exit_code.store(exit_code, Ordering::SeqCst);
        self.runtime_ms.store(runtime_ms, Ordering::SeqCst);
        self.exited.store(true, Ordering::SeqCst);
    }
    fn password_input(&self, active: bool) {
        self.password_active.store(active, Ordering::SeqCst);
        if active {
            self.password_seen_true.store(true, Ordering::SeqCst);
        }
    }
}

/// Build a `/bin/sh -c <script>` command. Uses `Command::Direct` so on macOS
/// it becomes `login -flp <user> /bin/sh -c <script>` (login *runs* the given
/// command), avoiding the `exec -l` login-shell replacement that the `Shell`
/// variant applies — which would try to exec the script text as a login shell.
fn sh_c(script: &str) -> Command {
    Command::Direct(vec![
        "/bin/sh".to_string(),
        "-c".to_string(),
        script.to_string(),
    ])
}

/// An interactive `/bin/sh` reading commands from the pty. `Direct` with an
/// absolute path so it launches under login's restricted PATH.
fn interactive_sh() -> Command {
    Command::Direct(vec!["/bin/sh".to_string()])
}

/// macOS wraps every command in `/usr/bin/login`, which reaps its own child
/// and always exits 0 — so the child's real exit code/signal is not visible
/// to the exit watcher on macOS (an inherent `login(1)` property, shared with
/// upstream Ghostty). On other platforms there is no wrapper and the exact
/// code propagates. The decode logic itself is unit-tested in the lib
/// (`exit_code_from_status`).
const LOGIN_SWALLOWS_EXIT_CODE: bool = cfg!(target_os = "macos");

/// A started shell under test: the writer loop, a mailbox sender + receiver to
/// drive it, the output capture, and the notifier. The tests pump the writer
/// loop manually (no separate IO writer thread), so the waker handle is not
/// surfaced.
struct Started {
    writer: WriterLoop,
    tx: Sender,
    rx: mailbox::Receiver,
    capture: Capture,
    notifier: Arc<TestNotifier>,
}

/// Build a shell Exec wired to a capture sink + notifier, started and ready.
fn start_shell(cmd: Command) -> Started {
    let capture = Capture::default();
    let notifier = Arc::new(TestNotifier::default());

    let mut exec = Exec::init(Config {
        command: Some(cmd),
        ..Config::default()
    });
    exec.set_notifier(notifier.clone());
    // A sane initial size so `stty size` is meaningful.
    exec.set_initial_size(
        GridSize {
            columns: 80,
            rows: 24,
        },
        ScreenSize {
            width: 800,
            height: 480,
        },
    );

    let td: ThreadData = exec.thread_enter(capture.sink()).expect("thread_enter");
    let writer = WriterLoop::new(exec, td);

    let (waker, _wait_handle) = ghostty_termio::exec::CondvarWaker::new();
    let (tx, rx) = mailbox::channel(waker);

    Started {
        writer,
        tx,
        rx,
        capture,
        notifier,
    }
}

/// Pump the writer loop (drain mailbox + tick timers) until `cond` is true or
/// the deadline passes. Runs on the test thread — no separate IO writer
/// thread is needed for these tests.
fn pump_until(
    writer: &mut WriterLoop,
    rx: &mailbox::Receiver,
    deadline: Duration,
    mut cond: impl FnMut(&WriterLoop) -> bool,
) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        writer.drain(rx);
        writer.tick_timers();
        if cond(writer) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // One last drain/check.
    writer.drain(rx);
    cond(writer)
}

fn wait_for(deadline: Duration, mut cond: impl FnMut() -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < deadline {
        if cond() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    cond()
}

/// echo through the full pipeline: run a shell that echoes a marker, then
/// verify the parse sink saw it. Proves gather → parse → sink end to end.
#[test]
fn echo_through_pipeline() {
    let Started {
        mut writer,
        tx,
        rx,
        capture,
        ..
    } = start_shell(sh_c("echo gho''stty; sleep 0.2"));

    let saw = pump_until(&mut writer, &rx, Duration::from_secs(10), |_| {
        capture.contains(b"ghostty")
    });
    assert!(
        saw,
        "sink never saw echoed marker; transcript: {:?}",
        String::from_utf8_lossy(&capture.snapshot())
    );

    drop(tx);
    writer.shutdown();
}

/// Write via the mailbox writer loop: send `echo hi` bytes through the
/// mailbox, let the writer loop forward them to the pty, and read the shell's
/// response back through the pipeline.
#[test]
fn write_via_mailbox() {
    // Interactive shell reading stdin (`cat`-like via `sh` reading commands).
    let Started {
        mut writer,
        tx,
        rx,
        capture,
        ..
    } = start_shell(interactive_sh());

    // Drive a command in through the mailbox writer.
    tx.send(Message::write_req(b"echo ma''rker\n")).unwrap();

    let saw = pump_until(&mut writer, &rx, Duration::from_secs(10), |_| {
        capture.contains(b"marker")
    });
    assert!(
        saw,
        "mailbox-written command produced no output; transcript: {:?}",
        String::from_utf8_lossy(&capture.snapshot())
    );

    // Exit cleanly.
    tx.send(Message::write_req(b"exit\n")).unwrap();
    pump_until(&mut writer, &rx, Duration::from_secs(5), |w| {
        w.thread_data().exited()
    });

    drop(tx);
    writer.shutdown();
}

/// Resize via the mailbox: the writer loop coalesces a Resize and applies it;
/// `stty size` in the shell then reports the new dimensions, read back
/// through the pipeline.
#[test]
fn resize_via_mailbox_visible_to_shell() {
    let Started {
        mut writer,
        tx,
        rx,
        capture,
        ..
    } = start_shell(interactive_sh());

    // Resize to 41 rows x 123 cols (cell = 10x20 → screen 1230x820).
    tx.send(Message::Resize(ghostty_termio::size::Size {
        screen: ScreenSize {
            width: 1230,
            height: 820,
        },
        cell: ghostty_termio::size::CellSize {
            width: 10,
            height: 20,
        },
        padding: Default::default(),
    }))
    .unwrap();

    // Let the 25ms coalesce timer fire and apply the resize.
    pump_until(&mut writer, &rx, Duration::from_millis(200), |_| false);

    // Ask the shell what size it sees (absolute path — login's PATH is
    // restricted and may not include /bin).
    tx.send(Message::write_req(b"/bin/stty size\n")).unwrap();
    let saw = pump_until(&mut writer, &rx, Duration::from_secs(10), |_| {
        capture.contains(b"41 123")
    });
    assert!(
        saw,
        "shell did not report resized dims; transcript: {:?}",
        String::from_utf8_lossy(&capture.snapshot())
    );

    tx.send(Message::write_req(b"exit\n")).unwrap();
    pump_until(&mut writer, &rx, Duration::from_secs(5), |w| {
        w.thread_data().exited()
    });
    drop(tx);
    writer.shutdown();
}

/// Clean exit + exit-code capture: a shell that exits 7 is reaped by the exit
/// watcher, which reports the code and a runtime.
#[test]
fn clean_exit_captures_code_and_runtime() {
    let Started {
        mut writer,
        tx,
        rx,
        notifier,
        ..
    } = start_shell(sh_c("sleep 0.1; exit 7"));

    let exited = pump_until(&mut writer, &rx, Duration::from_secs(10), |_| {
        notifier.exited.load(Ordering::SeqCst)
    });
    assert!(exited, "exit watcher never fired");
    if !LOGIN_SWALLOWS_EXIT_CODE {
        assert_eq!(
            notifier.exit_code.load(Ordering::SeqCst),
            7,
            "wrong exit code"
        );
    }
    // The shell slept 100ms, so runtime should be non-trivial.
    assert!(
        notifier.runtime_ms.load(Ordering::SeqCst) >= 50,
        "runtime looks wrong: {}",
        notifier.runtime_ms.load(Ordering::SeqCst)
    );

    drop(tx);
    writer.shutdown();
}

/// Abnormal exit (kill -9) detection: the child SIGKILLs itself; the watcher
/// reports the signalled death (encoded 128 + SIGKILL = 137).
#[test]
fn abnormal_exit_kill9_detected() {
    // The shell kills its own process group's shell via SIGKILL to itself.
    let Started {
        mut writer,
        tx,
        rx,
        notifier,
        ..
    } = start_shell(sh_c("kill -9 $$"));

    let exited = pump_until(&mut writer, &rx, Duration::from_secs(10), |_| {
        notifier.exited.load(Ordering::SeqCst)
    });
    assert!(exited, "kill -9 not detected");
    // 128 + SIGKILL(9) = 137. Not observable through the macOS login wrapper.
    if !LOGIN_SWALLOWS_EXIT_CODE {
        assert_eq!(
            notifier.exit_code.load(Ordering::SeqCst),
            137,
            "expected 137 for SIGKILL"
        );
    }

    drop(tx);
    writer.shutdown();
}

/// Teardown under an active output flood: run `yes` for ~100ms of gathering,
/// then tear down. The quit pipe + join ordering must not hang and must not
/// leave a lost thread.
#[test]
fn teardown_under_output_flood() {
    let Started {
        mut writer,
        tx,
        rx,
        capture,
        ..
    } = start_shell(sh_c("exec /usr/bin/yes"));

    // Wait until `yes` is actually flooding (login startup can delay it),
    // then let the pipeline saturate for a further ~100ms.
    let flooding = pump_until(&mut writer, &rx, Duration::from_secs(10), |_| {
        capture.snapshot().len() > 4096
    });
    assert!(flooding, "yes never started flooding");
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(100) {
        writer.drain(&rx);
        writer.tick_timers();
        std::thread::sleep(Duration::from_millis(5));
    }

    // Teardown must complete promptly (no hang) even though yes is still
    // producing. Do it on a watchdog thread so a hang fails the test instead
    // of blocking forever.
    let done = Arc::new(AtomicBool::new(false));
    let done2 = Arc::clone(&done);
    let handle = std::thread::spawn(move || {
        drop(tx);
        writer.shutdown();
        done2.store(true, Ordering::SeqCst);
    });

    let finished = wait_for(Duration::from_secs(10), || done.load(Ordering::SeqCst));
    assert!(finished, "teardown hung under output flood");
    handle.join().unwrap();
}

/// Password-mode detection: put the pty into canonical + no-echo (what a
/// password prompt like `ssh`/`sudo` does — `stty -echo icanon` in shell
/// terms), and verify the 200ms termios poll observes the change and reports
/// password input via the notifier.
///
/// The mode is applied to the pty master directly rather than by typing
/// `stty` into a shell: an interactive shell holds the tty in its own
/// non-canonical line-editing mode and resets it between commands, so the
/// canonical+noecho window a shell command creates is too brief for the poll
/// to catch — and on macOS the `login` wrapper further muddies termios
/// timing. Driving the master directly tests exactly the chunk-D contract:
/// `Subprocess::mode` → `Exec::termios_tick` heuristic → `Notifier`.
#[test]
fn password_mode_detected() {
    use rustix::termios::{InputModes, LocalModes, OptionalActions, tcgetattr, tcsetattr};

    // A long-lived child so the pty stays open while we poll.
    let Started {
        mut writer,
        tx,
        rx,
        notifier,
        ..
    } = start_shell(sh_c("sleep 5"));

    // Let it start.
    pump_until(&mut writer, &rx, Duration::from_secs(3), |_| false);

    // Flip the master into canonical + no-echo (a password prompt's state).
    // Dup it to an owned fd so we don't hold a borrow of `writer` across the
    // pump loop below.
    let master = writer
        .exec()
        .test_master_fd()
        .expect("master fd available after start")
        .try_clone_to_owned()
        .expect("dup master");
    let mut attrs = tcgetattr(&master).expect("tcgetattr");
    attrs.local_modes |= LocalModes::ICANON;
    attrs.local_modes -= LocalModes::ECHO;
    // Keep IUTF8 as the pty set it.
    attrs.input_modes |= InputModes::IUTF8;
    tcsetattr(&master, OptionalActions::Now, &attrs).expect("tcsetattr -echo icanon");

    // The 200ms poll must observe canonical && !echo within a few ticks.
    let seen = pump_until(&mut writer, &rx, Duration::from_secs(3), |_| {
        notifier.password_seen_true.load(Ordering::SeqCst)
    });
    assert!(
        seen,
        "password mode (canonical && !echo) never detected by the termios poll"
    );
    assert!(
        notifier.password_active.load(Ordering::SeqCst),
        "notifier should currently report active password input"
    );

    // Restore echo and verify the poll clears the flag (balanced true/false).
    let mut attrs = tcgetattr(&master).expect("tcgetattr");
    attrs.local_modes |= LocalModes::ECHO;
    tcsetattr(&master, OptionalActions::Now, &attrs).expect("tcsetattr echo");
    let cleared = pump_until(&mut writer, &rx, Duration::from_secs(3), |_| {
        !notifier.password_active.load(Ordering::SeqCst)
    });
    assert!(cleared, "password flag never cleared after echo restored");

    drop(tx);
    writer.shutdown();
}

/// Throughput sanity (not a full bench): `cat` a ~10 MiB file through the
/// pipeline into a counting sink and report MiB/s. Verifies gather never
/// starves parse (upstream's design goal) — if the pipeline stalled between
/// stages this would be an order of magnitude slower and the batches tiny.
///
/// `cat` of a file is the representative bulk case (matching upstream's
/// `cat`/`seq` benchmark): the child writes large chunks to the pty and the
/// gather stage's bridging keeps the kernel queue drained, so batches land at
/// tens of KiB. (A `yes | head` arrangement instead is child-throttled — the
/// intermediate pipe + `head`'s small writes bound it well below the pipeline
/// itself, so it does not measure the pipeline.) Run with `--nocapture` to
/// see the number.
#[test]
fn throughput_cat_10mib() {
    use std::io::Write;
    use std::sync::atomic::AtomicUsize;

    // Generate a ~10 MiB file of printable ASCII.
    let path = std::env::temp_dir().join(format!("ghostty-termio-thru-{}.txt", std::process::id()));
    {
        let mut f = std::fs::File::create(&path).expect("create temp file");
        let line = [b'A'; 80];
        let mut written = 0usize;
        while written < 10 * 1024 * 1024 {
            f.write_all(&line).unwrap();
            f.write_all(b"\n").unwrap();
            written += line.len() + 1;
        }
        f.flush().unwrap();
    }
    let total_file = std::fs::metadata(&path).unwrap().len() as usize;

    let bytes = Arc::new(AtomicUsize::new(0));
    let batches = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&bytes);
    let batch_ct = Arc::clone(&batches);
    let sink: Sink = Box::new(move |batch: &[u8]| {
        counter.fetch_add(batch.len(), Ordering::Relaxed);
        batch_ct.fetch_add(1, Ordering::Relaxed);
    });

    let mut exec = Exec::init(Config {
        command: Some(sh_c(&format!("/bin/cat {}", path.display()))),
        ..Config::default()
    });
    exec.set_initial_size(
        GridSize {
            columns: 80,
            rows: 24,
        },
        ScreenSize {
            width: 800,
            height: 480,
        },
    );
    let td = exec.thread_enter(sink).expect("thread_enter");
    let mut writer = WriterLoop::new(exec, td);

    let (waker, _wait) = ghostty_termio::exec::CondvarWaker::new();
    let (tx, rx) = mailbox::channel(waker);

    let start = Instant::now();
    // Pump until the child exits and most of the file has come through (a
    // little may be lost to login banner interleaving, so accept 90%+).
    let target = total_file * 9 / 10;
    let done = pump_until(&mut writer, &rx, Duration::from_secs(20), |w| {
        w.thread_data().exited() && bytes.load(Ordering::Relaxed) >= target
    });
    let elapsed = start.elapsed();
    let total = bytes.load(Ordering::Relaxed);
    let nbatches = batches.load(Ordering::Relaxed);

    drop(tx);
    writer.shutdown();
    let _ = std::fs::remove_file(&path);

    assert!(
        done && total >= target,
        "pipeline did not move the file: got {total}/{total_file} bytes in {elapsed:?}"
    );
    let mib = total as f64 / (1024.0 * 1024.0);
    let mib_s = mib / elapsed.as_secs_f64();
    let avg = total.checked_div(nbatches).unwrap_or(0);
    eprintln!(
        "throughput: {mib:.1} MiB in {elapsed:?} = {mib_s:.1} MiB/s \
         ({nbatches} batches, avg {avg} B/batch — large batches ⇒ gather did \
         not starve parse)"
    );
    // Sanity floor well below the ~86+ MiB/s engine target but far above a
    // stalled pipeline; guards against a regression that reintroduces a
    // per-batch stall without pinning an exact number on a noisy machine.
    assert!(
        mib_s > 40.0,
        "throughput {mib_s:.1} MiB/s is far below expectation — gather may be \
         starving parse"
    );
}
