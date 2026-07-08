//! Shell integration end-to-end: spawn a REAL zsh login shell (the app's
//! default-enabled target, `docs/analysis/shell-integration.md`) with
//! [`ghostty_termio::shell_integration::setup`] wired into its `Config`, feed
//! the pty output through a real `ghostty-vt` [`Stream`]/[`Terminal`] (the
//! same engine the app uses), and confirm:
//!
//! 1. OSC 133 prompt marks actually arrive -- `Terminal::cursor_is_at_prompt`
//!    (the engine's public semantic-prompt query) flips true once the shell
//!    draws its first prompt.
//! 2. The zsh integration's bar-cursor-at-prompt hook fires -- the raw
//!    DECSCUSR bar sequence (`CSI 5 SP q` / `CSI 6 SP q`, feature `cursor`,
//!    on by default) appears in the pty output stream.
//!
//! On (2): this crate's `ghostty-vt` dependency does not yet store cursor
//! *shape* anywhere queryable (only blink-mode, derived from DECSCUSR, is
//! tracked today -- see `TerminalHandler::cursor_style` and the analysis
//! doc's note on this gap, which is the deferred M2 chunk F "stream_handler
//! delta"). So this test asserts on the raw bytes the shell integration
//! script actually sent, which is the strongest claim this Rust tree can
//! currently make about "does the cursor become a bar at the prompt" -- the
//! DECSCUSR sequence unambiguously reaches the terminal; only the
//! terminal-side bookkeeping of cursor *shape* remains unported.

#![cfg(all(unix, target_os = "macos"))]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ghostty_termio::exec::{Command, Config, Exec, NullNotifier, Sink, ThreadData, WriterLoop};
use ghostty_termio::mailbox::{self};
use ghostty_termio::message::Message;
use ghostty_termio::shell_integration::{self, EnvMap, Shell, ShellIntegrationFeatures};
use ghostty_termio::size::{GridSize, ScreenSize};

use ghostty_vt::stream::{Stream, TerminalHandler};
use ghostty_vt::terminal::{Options, Terminal};

/// Locate a real zsh on this machine; skip (not fail) if none is installed --
/// this is an environment-dependent integration test, same policy as
/// upstream's own CI treats shell availability.
fn find_zsh() -> Option<String> {
    for candidate in ["/opt/homebrew/bin/zsh", "/usr/bin/zsh", "/bin/zsh"] {
        if std::path::Path::new(candidate).is_file() {
            return Some(candidate.to_string());
        }
    }
    None
}

/// Pump: drain the mailbox (a no-op here -- we only need the read pipeline)
/// and feed every captured batch into the real `Stream`/`Terminal`, until
/// `cond` is satisfied or the deadline passes. `stream` is only ever touched
/// from this (the test) thread -- the parse sink just appends to `capture` --
/// so plain ownership is enough; no `Arc<Mutex<_>>` needed (`Terminal` isn't
/// `Send`/`Sync` the way `ghostty-app`'s `Engine` wrapper is, since it isn't
/// wrapped with that crate's documented-sound `unsafe impl Send`).
fn pump_until(
    stream: &mut Stream<TerminalHandler>,
    capture: &Arc<Mutex<Vec<u8>>>,
    fed: &mut usize,
    deadline: Duration,
    mut cond: impl FnMut(&Stream<TerminalHandler>) -> bool,
) -> bool {
    let start = Instant::now();
    loop {
        {
            let buf = capture.lock().unwrap();
            if buf.len() > *fed {
                stream.feed(&buf[*fed..]);
                *fed = buf.len();
                if cond(stream) {
                    return true;
                }
            }
        }
        if start.elapsed() >= deadline {
            return cond(stream);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Spawn a real zsh with shell integration wired exactly the way
/// `TabIo::spawn` (`crates/ghostty-app/src/termio.rs`) wires it: build
/// `GHOSTTY_SHELL_FEATURES` via `setup_features`, then force `Shell::Zsh`
/// through `shell_integration::setup` to get the ZDOTDIR-redirected env.
#[test]
fn zsh_integration_prompt_marks_and_bar_cursor() {
    let Some(zsh) = find_zsh() else {
        eprintln!("skipping: no zsh found on this machine");
        return;
    };

    // Sanity: the vendored resources are actually where we expect (dev-time
    // CARGO_MANIFEST_DIR fallback).
    let resources = shell_integration::resources_dir();
    assert!(
        std::path::Path::new(&resources)
            .join("shell-integration/zsh/ghostty-integration")
            .is_file(),
        "vendored zsh integration script not found under resolved resources dir {resources}"
    );

    let mut si_env = EnvMap::from_pairs(std::env::vars().collect());
    // Force blinking off nothing -- keep upstream defaults (cursor: blink),
    // matching the app's wiring.
    shell_integration::setup_features(&mut si_env, ShellIntegrationFeatures::default(), true);

    let base_command = Command::Direct(vec![zsh.clone()]);
    let integration =
        shell_integration::setup(&resources, &base_command, &mut si_env, Some(Shell::Zsh))
            .expect("zsh integration setup must succeed against the vendored resources");
    assert_eq!(integration.shell, Shell::Zsh);
    assert!(
        si_env
            .get("ZDOTDIR")
            .is_some_and(|v| v.ends_with("shell-integration/zsh")),
        "ZDOTDIR was not redirected to the vendored zsh integration dir"
    );

    let capture: Arc<Mutex<Vec<u8>>> = Arc::default();
    let sink: Sink = {
        let capture = Arc::clone(&capture);
        Box::new(move |batch: &[u8]| capture.lock().unwrap().extend_from_slice(batch))
    };

    let mut exec = Exec::init(Config {
        command: Some(integration.command),
        env: si_env.into_pairs(),
        ..Config::default()
    });
    exec.set_notifier(Arc::new(NullNotifier));
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

    let td: ThreadData = exec.thread_enter(sink).expect("thread_enter");
    let mut writer = WriterLoop::new(exec, td);
    let (waker, _wait_handle) = ghostty_termio::exec::CondvarWaker::new();
    let (tx, rx) = mailbox::channel(waker);

    let mut stream = Stream::new(TerminalHandler::new(Terminal::new(Options {
        cols: 80,
        rows: 24,
        ..Default::default()
    })));
    let mut fed = 0usize;

    // Pump until the shell draws its first prompt (semantic-prompt marks
    // land -- OSC 133 A/B) AND we've observed it via the real engine's public
    // `cursor_is_at_prompt` query, the same accessor the app could use for
    // prompt-jump readiness.
    let saw_prompt = pump_until(
        &mut stream,
        &capture,
        &mut fed,
        Duration::from_secs(15),
        |s| s.handler.terminal.cursor_is_at_prompt(),
    );

    // Also drain the writer's mailbox/timers a couple of times so the pty
    // stays serviced (interactive shell; no input needed to reach a prompt,
    // but keep the loop's bookkeeping alive as `TabIo` would).
    writer.drain(&rx);
    writer.tick_timers();

    let transcript = capture.lock().unwrap().clone();
    assert!(
        saw_prompt,
        "engine never observed a semantic prompt mark (cursor_is_at_prompt stayed false); \
         raw transcript: {:?}",
        String::from_utf8_lossy(&transcript)
    );

    // Raw-byte evidence the prompt hooks actually ran: OSC 133 B (end of
    // prompt, start of input) must be present somewhere in the stream --
    // this is the mark `cursor_is_at_prompt` above is keying off of, made
    // concrete.
    assert!(
        contains(&transcript, b"\x1b]133;B"),
        "no OSC 133;B (prompt end) mark seen in transcript: {:?}",
        String::from_utf8_lossy(&transcript)
    );

    // Bar-cursor-at-prompt (the maintainer's question): the `cursor` feature
    // defaults on, so zsh's `_ghostty_zle_line_init`/`zle-keymap-select` hook
    // must have emitted a DECSCUSR bar sequence (CSI 5 SP q blinking, or
    // CSI 6 SP q steady -- `setup_features` above requested blink=true, so
    // expect the blinking form, but accept either since a plugin/theme could
    // still race the first render).
    let saw_bar_cursor = contains(&transcript, b"\x1b[5 q") || contains(&transcript, b"\x1b[6 q");
    assert!(
        saw_bar_cursor,
        "no DECSCUSR bar-cursor sequence (CSI 5/6 SP q) seen at the prompt; transcript: {:?}",
        String::from_utf8_lossy(&transcript)
    );

    tx.send(Message::write_req(b"exit\n")).unwrap();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(5) {
        writer.drain(&rx);
        writer.tick_timers();
        if writer.thread_data().exited() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    drop(tx);
    writer.shutdown();
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
