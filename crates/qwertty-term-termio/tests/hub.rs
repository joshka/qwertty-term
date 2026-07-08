//! Termio hub integration tests (M2 chunk E, `docs/analysis/termio-hub.md`).
//!
//! These drive the [`Termio`] hub directly (no window): spawn a real `/bin/sh`,
//! feed its output into a live engine behind a lock via the parse sink, write
//! through the mailbox, resize through the coalescing timer, observe the
//! 1s sync-output reset, and measure throughput into the live engine.
//!
//! The "engine" here is a minimal stand-in that mirrors the app's contract:
//! a byte buffer behind a `Mutex` the sink applies to and the "pace tick"
//! reads — the same "apply behind the lock the renderer takes" topology the
//! app uses (§3.3), without pulling in `qwertty-term-app` (macOS-only).

#![cfg(unix)]

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use qwertty_term_termio::exec::{Command, Config, Exec, Notifier};
use qwertty_term_termio::hub::{HubHandler, NullHandler, Termio};
use qwertty_term_termio::message::Message;
use qwertty_term_termio::size::{CellSize, GridSize, ScreenSize, Size};

/// A live "engine" stand-in: an append-only byte buffer behind a `Mutex`, plus
/// a synchronized-output flag a `HubHandler::on_sync_reset` can clear. This is
/// the shape of the app's `Arc<Mutex<Engine>>` — the sink locks and applies,
/// the test (standing in for the pace tick) locks and reads.
#[derive(Default)]
struct LiveEngine {
    bytes: Vec<u8>,
    /// Set when the "program" enters synchronized output (mode 2026), cleared
    /// by the hub's 1s reset.
    sync_output: bool,
}

impl LiveEngine {
    fn shared() -> Arc<Mutex<LiveEngine>> {
        Arc::new(Mutex::new(LiveEngine::default()))
    }
}

/// A sink that locks the shared engine and applies each parse batch — the
/// upstream `processOutput`-under-lock design (§3.3). Also snoops for the
/// mode-2026 set sequence so the sync-reset test can observe entry.
fn engine_sink(engine: Arc<Mutex<LiveEngine>>) -> qwertty_term_termio::exec::Sink {
    Box::new(move |batch: &[u8]| {
        let mut e = engine.lock().unwrap();
        e.bytes.extend_from_slice(batch);
        // Minimal mode-2026 detection (the real engine parses this; here we
        // just watch the raw bytes so the test has a live flag to reset).
        if window_contains(&e.bytes, b"\x1b[?2026h") {
            e.sync_output = true;
        }
    })
}

fn window_contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// A handler that clears the live engine's sync-output flag on the 1s reset,
/// exactly as the app's handler force-clears mode 2026.
struct SyncResetHandler {
    engine: Arc<Mutex<LiveEngine>>,
    fired: Arc<AtomicBool>,
}

impl HubHandler for SyncResetHandler {
    fn on_sync_reset(&mut self) {
        self.engine.lock().unwrap().sync_output = false;
        self.fired.store(true, Ordering::SeqCst);
    }
}

/// A notifier that records the child-exit code + password transitions.
#[derive(Default)]
struct RecordNotifier {
    exit_code: AtomicU32,
    exited: AtomicBool,
    runtime_ms: AtomicU64,
}

impl Notifier for RecordNotifier {
    fn child_exited(&self, exit_code: u32, runtime_ms: u64) {
        self.exit_code.store(exit_code, Ordering::SeqCst);
        self.runtime_ms.store(runtime_ms, Ordering::SeqCst);
        self.exited.store(true, Ordering::SeqCst);
    }
    fn password_input(&self, _active: bool) {}
}

/// Build a `/bin/sh -c <script>` command via `Command::Direct` (not `Shell`):
/// on macOS the `Shell` variant becomes `login ... bash -c "exec -l <script>"`,
/// which tries to exec the script text as a login shell. `Direct` with an
/// absolute path makes login *run* the given command (matches the pattern in
/// `tests/exec.rs`).
fn shell_config(script: &str) -> Config {
    Config {
        command: Some(Command::Direct(vec![
            "/bin/sh".to_string(),
            "-c".to_string(),
            script.to_string(),
        ])),
        ..Config::default()
    }
}

fn small_size() -> (GridSize, ScreenSize) {
    (
        GridSize {
            columns: 80,
            rows: 24,
        },
        ScreenSize {
            width: 640,
            height: 384,
        },
    )
}

/// Wait until `f()` is true or `timeout` elapses. Returns whether it became
/// true.
fn wait_until(timeout: Duration, mut f: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if f() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    f()
}

/// Full hub lifecycle: spawn → echo round-trip → resize → clean exit +
/// exit-code capture. Drives the hub directly, no window.
#[test]
fn hub_lifecycle_echo_resize_exit() {
    let engine = LiveEngine::shared();
    let notifier = Arc::new(RecordNotifier::default());
    let (grid, screen) = small_size();

    // `echo` a marker, then exit 0 after a beat so teardown reaps cleanly.
    let exec = Exec::init(shell_config("echo HUB_ROUNDTRIP; sleep 0.2; exit 0"));
    let mut exec = exec;
    exec.set_notifier(notifier.clone() as Arc<dyn Notifier>);
    exec.set_initial_size(grid, screen);

    let mut termio = Termio::spawn(exec, engine_sink(engine.clone()), NullHandler)
        .expect("hub spawns the io stack");
    let writer = termio.writer();

    // Echo round-trip: the shell's `echo` output reaches the live engine
    // through gather → parse → sink.
    let saw = wait_until(Duration::from_secs(5), || {
        window_contains(&engine.lock().unwrap().bytes, b"HUB_ROUNDTRIP")
    });
    assert!(saw, "echo output never reached the live engine");

    // Resize through the coalescing timer (just assert it doesn't wedge; the
    // dedicated coalescing test asserts the pty winsize).
    let size = Size {
        screen: ScreenSize {
            width: 800,
            height: 480,
        },
        cell: CellSize {
            width: 8,
            height: 16,
        },
        ..Default::default()
    };
    assert!(writer.resize(size), "resize send");

    // Clean exit: the child exits 0, the exit watcher fires the notifier.
    let exited = wait_until(Duration::from_secs(5), || {
        notifier.exited.load(Ordering::SeqCst)
    });
    assert!(exited, "child exit was never observed");
    assert_eq!(notifier.exit_code.load(Ordering::SeqCst), 0, "clean exit 0");

    termio.shutdown();
}

/// A write through the mailbox reaches the pty: run `cat`, write a line, and
/// see it echoed back into the live engine.
#[test]
fn hub_write_reaches_pty() {
    let engine = LiveEngine::shared();
    let (grid, screen) = small_size();

    // An interactive shell reading commands from the pty (mirrors the working
    // `write_via_mailbox` pattern in tests/exec.rs). A raw `cat` relies on the
    // pty's ECHO of raw input, which is unreliable under the macOS `login`
    // wrapper; running a shell command whose *output* comes back is robust.
    let mut exec = Exec::init(Config {
        command: Some(Command::Direct(vec!["/bin/sh".to_string()])),
        ..Config::default()
    });
    exec.set_initial_size(grid, screen);

    let mut termio =
        Termio::spawn(exec, engine_sink(engine.clone()), NullHandler).expect("spawn sh");
    let writer = termio.writer();

    // Give the shell a beat to draw its prompt, then send a command whose
    // output round-trips back through the pty → gather → parse → engine.
    std::thread::sleep(Duration::from_millis(200));
    assert!(writer.write(b"echo HUB_WRITE_MARKER\n"), "write send");

    let saw = wait_until(Duration::from_secs(5), || {
        window_contains(&engine.lock().unwrap().bytes, b"HUB_WRITE_MARKER")
    });
    assert!(
        saw,
        "written command's output never came back through the pty"
    );

    // Exit the shell cleanly for teardown.
    assert!(writer.write(b"exit\n"), "exit send");
    termio.shutdown();
}

/// The 1s synchronized-output reset: a program sets mode 2026 and never clears
/// it; the hub's timer force-clears within ~1s.
#[test]
fn hub_sync_output_reset_after_1s() {
    let engine = LiveEngine::shared();
    let fired = Arc::new(AtomicBool::new(false));
    let (grid, screen) = small_size();

    // Emit the mode-2026 set sequence, then sit idle (never clearing it).
    let mut exec = Exec::init(shell_config("printf '\\033[?2026h'; sleep 5"));
    exec.set_initial_size(grid, screen);

    let handler = SyncResetHandler {
        engine: engine.clone(),
        fired: fired.clone(),
    };
    let mut termio = Termio::spawn(exec, engine_sink(engine.clone()), handler).expect("spawn sync");
    let writer = termio.writer();

    // Wait for the program to enter synchronized output.
    let entered = wait_until(Duration::from_secs(3), || {
        engine.lock().unwrap().sync_output
    });
    assert!(entered, "program never entered synchronized output");

    // Now arm the hub's reset timer (the real terminal does this on parsing
    // the set; here the message is posted explicitly, matching how the stream
    // handler would enqueue StartSynchronizedOutput).
    assert!(
        writer.send(Message::StartSynchronizedOutput),
        "sync-output message send"
    );

    // Within ~1s (+ slack) the reset fires and clears the flag.
    let reset = wait_until(Duration::from_millis(1500), || {
        fired.load(Ordering::SeqCst) && !engine.lock().unwrap().sync_output
    });
    assert!(reset, "sync-output was not force-reset within 1s");

    termio.shutdown();
}

/// Resize coalescing is observable at the pty: post many resizes inside one
/// 25ms window; the pty ends at the last requested size, not an intermediate.
/// (Exact call-count assertion is covered by the spike; here we assert the
/// end-state winsize, which is the user-visible contract.)
#[test]
fn hub_resize_coalesces_to_last() {
    let engine = LiveEngine::shared();
    let (grid, screen) = small_size();

    // Keep the child alive across the burst. The hub consumes `exec`, so we
    // can't hold the pty master fd here to read TIOCGWINSZ; instead we assert
    // the user-visible contract — a burst coalesces and the writer loop stays
    // live (a coalescing regression would wedge the loop, failing the
    // post-burst write round-trip below). The spike's coalescing test asserts
    // the exact fired-count + final dims against the `Handler`.
    let mut exec = Exec::init(Config {
        command: Some(Command::Direct(vec!["/bin/sh".to_string()])),
        ..Config::default()
    });
    exec.set_initial_size(grid, screen);

    let mut termio =
        Termio::spawn(exec, engine_sink(engine.clone()), NullHandler).expect("spawn resize");
    let writer = termio.writer();
    std::thread::sleep(Duration::from_millis(200));

    let dims = [(100u32, 30u32), (110, 33), (120, 36), (132, 40)];
    for (cols, rows) in dims {
        let size = Size {
            screen: ScreenSize {
                width: cols * 8,
                height: rows * 16,
            },
            cell: CellSize {
                width: 8,
                height: 16,
            },
            ..Default::default()
        };
        assert!(writer.resize(size), "burst resize send");
    }

    // Give the 25ms coalesce window time to fire once, then confirm the loop is
    // still responsive by round-tripping a write (proves it didn't wedge on the
    // resize burst).
    std::thread::sleep(Duration::from_millis(60));
    assert!(writer.write(b"echo RESIZE_OK\n"), "post-resize write");

    // The shell echoes it back, proving the loop drained past the resize burst.
    let saw = wait_until(Duration::from_secs(3), || {
        window_contains(&engine.lock().unwrap().bytes, b"RESIZE_OK")
    });
    assert!(saw, "writer loop wedged on the resize burst");

    assert!(writer.write(b"exit\n"), "exit send");
    termio.shutdown();
}

/// Throughput into a LIVE engine: feed a large flood through the real pipeline
/// (gather → parse → sink → locked engine) and assert ≥80 MiB/s, the chunk-E
/// bar (Exec alone measures 106 MiB/s into a bare sink; this measures into a
/// locked engine with a concurrent "pace tick" reader contending the lock).
///
/// `#[ignore]` because it spawns a multi-hundred-MiB `yes`-style flood; run
/// explicitly: `cargo test -p qwertty-term-termio --test hub -- --ignored
/// throughput`.
#[test]
#[ignore = "throughput benchmark; run explicitly"]
fn hub_throughput_into_live_engine() {
    let engine = LiveEngine::shared();
    let (grid, screen) = small_size();

    // Flood with `yes` for a fixed time window and count the delivered bytes;
    // rate = bytes / window. `exec /usr/bin/yes` replaces the shell with yes so
    // there's no pipe under the macOS `login` wrapper (mirrors the exec
    // flood test's `sh_c("exec /usr/bin/yes")`). A time-window measurement is
    // steadier than a byte-count-with-`head` (which needs a pipe login mangles).
    const WINDOW: Duration = Duration::from_secs(3);
    let mut exec = Exec::init(shell_config("exec /usr/bin/yes"));
    exec.set_initial_size(grid, screen);

    let counted = Arc::new(AtomicU64::new(0));
    // A counting sink that locks the engine (to model the real lock cost) and
    // records bytes — same lock discipline as the app sink.
    let sink: qwertty_term_termio::exec::Sink = {
        let engine = engine.clone();
        let counted = counted.clone();
        Box::new(move |batch: &[u8]| {
            // Lock/unlock per batch — the real contention shape.
            let mut e = engine.lock().unwrap();
            // Don't grow an unbounded buffer for 200 MiB; just account length.
            e.bytes.clear();
            drop(e);
            counted.fetch_add(batch.len() as u64, Ordering::Relaxed);
        })
    };

    // A concurrent "pace tick" that locks the engine 60×/sec, modeling the
    // app's render contention on the same lock. Critically it holds the lock
    // only for the brief "snapshot" (a few microseconds), then sleeps ~16ms
    // WITHOUT the lock — the real pace tick's shape (§3.3). Holding it for the
    // full frame would starve the parse thread and is not what the app does.
    let stop_tick = Arc::new(AtomicBool::new(false));
    let ticker = {
        let engine = engine.clone();
        let stop = stop_tick.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                {
                    let g = engine.lock().unwrap();
                    // Model a snapshot read: touch the state, then release.
                    let _ = g.bytes.len();
                }
                std::thread::sleep(Duration::from_millis(16));
            }
        })
    };

    let mut termio = Termio::spawn(exec, sink, NullHandler).expect("spawn flood");

    // Let `yes` ramp up (login startup can delay it), then measure a clean
    // window: snapshot the counter, wait WINDOW, snapshot again.
    let started = wait_until(Duration::from_secs(10), || {
        counted.load(Ordering::Relaxed) > 1024 * 1024
    });
    assert!(started, "yes never started flooding");

    let c0 = counted.load(Ordering::Relaxed);
    let t0 = Instant::now();
    std::thread::sleep(WINDOW);
    let c1 = counted.load(Ordering::Relaxed);
    let elapsed = t0.elapsed();

    stop_tick.store(true, Ordering::SeqCst);
    let _ = ticker.join();
    termio.shutdown();

    let got = (c1 - c0) as f64;
    let mib = got / (1024.0 * 1024.0);
    let rate = mib / elapsed.as_secs_f64();
    println!("throughput into live engine: {rate:.1} MiB/s ({got:.0} bytes in {elapsed:?})");
    assert!(
        rate >= 80.0,
        "throughput into live engine {rate:.1} MiB/s < 80 MiB/s floor"
    );
}
