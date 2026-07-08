//! Headless end-to-end test: spawn a real PTY running scripted shell commands,
//! feed its output through the `qwertty-term-vt`-backed [`Engine`], and assert on the
//! final rendered screen text + styled snapshot. This exercises the same path
//! the interactive frontends use (pty bytes -> engine.write -> snapshot) without
//! a GUI.

use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use qwertty_term_spike::{Engine, SnapshotColor};

/// Spawn `/bin/sh -c <script>` on a PTY of the given size, pump its output into
/// an [`Engine`] until the child exits (or a timeout), and return the engine.
fn run_script(cols: u16, rows: u16, script: &str) -> Engine {
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open pty");

    let mut command = CommandBuilder::new("/bin/sh");
    command.arg("-c");
    command.arg(script);
    command.env("TERM", "xterm-256color");
    command.env("COLORTERM", "truecolor");
    // Keep output deterministic regardless of the developer's environment.
    command.env("PS1", "");
    command.env("LC_ALL", "C.UTF-8");

    let mut child = pair.slave.spawn_command(command).expect("spawn child");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("clone reader");
    let mut writer = pair.master.take_writer().expect("take writer");

    let mut engine = Engine::new(cols as usize, rows as usize);

    // Reader thread -> channel, so we can poll with a timeout.
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match std::io::Read::read(&mut reader, &mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while let Ok(bytes) = rx.try_recv() {
            engine.write(&bytes);
            let reply = engine.take_output();
            if !reply.is_empty() {
                use std::io::Write;
                let _ = writer.write_all(&reply);
                let _ = writer.flush();
            }
        }
        if child.try_wait().ok().flatten().is_some() {
            break;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // Drain anything buffered after exit.
    std::thread::sleep(Duration::from_millis(50));
    while let Ok(bytes) = rx.try_recv() {
        engine.write(&bytes);
    }

    engine
}

fn visible_lines(engine: &Engine) -> Vec<String> {
    let snap = engine.snapshot();
    snap.visible_window(0)
        .iter()
        .map(|row| {
            let mut s: String = row
                .cells
                .iter()
                .filter(|c| !c.is_spacer())
                .map(|c| c.ch)
                .collect();
            while s.ends_with(' ') {
                s.pop();
            }
            s
        })
        .collect()
}

#[test]
fn printf_text_reaches_screen() {
    let engine = run_script(40, 6, "printf 'hello from pty'");
    let lines = visible_lines(&engine);
    assert!(
        lines.iter().any(|l| l.contains("hello from pty")),
        "expected greeting in screen, got: {lines:?}"
    );
}

#[test]
fn multiline_output_lands_on_separate_rows() {
    let engine = run_script(40, 6, "printf 'line-one\\nline-two\\n'");
    let lines = visible_lines(&engine);
    assert!(
        lines.iter().any(|l| l == "line-one"),
        "missing line-one: {lines:?}"
    );
    assert!(
        lines.iter().any(|l| l == "line-two"),
        "missing line-two: {lines:?}"
    );
}

#[test]
fn sgr_color_output_is_reflected_in_snapshot() {
    // Emit a red 'X' via an SGR escape, then reset.
    let engine = run_script(20, 4, "printf '\\033[31mX\\033[0m'");
    let snap = engine.snapshot();
    let window = snap.visible_window(0);
    // Find the styled 'X'.
    let mut found = false;
    for row in window {
        for cell in &row.cells {
            if cell.ch == 'X' {
                assert_eq!(
                    cell.style.fg,
                    SnapshotColor::Palette(1),
                    "X should be red (palette 1)"
                );
                found = true;
            }
        }
    }
    assert!(found, "did not find styled 'X' on screen");
}

#[test]
fn osc52_clipboard_write_from_real_pty_is_drainable() {
    // printf a raw OSC 52 clipboard-set sequence (base64 of "hi") through a
    // real PTY/shell round trip, exercising the same
    // pty-bytes -> engine.write -> take_clipboard path the egui frontend's
    // `drain_clipboard` uses (see `window::WindowTerminal::drain_clipboard`).
    let mut engine = run_script(20, 4, "printf '\\033]52;c;aGk=\\033\\\\'");
    assert_eq!(engine.take_clipboard(), Some((b'c', "aGk=".to_string())));
}

#[test]
fn cursor_addressing_places_text() {
    // Move cursor to row 3, col 5 (1-based) and print a marker.
    let engine = run_script(20, 6, "printf '\\033[3;5HMARK'");
    let snap = engine.snapshot();
    let window = snap.visible_window(0);
    let row = &window[2];
    let text: String = row.cells.iter().map(|c| c.ch).collect();
    assert!(
        text.trim_start().starts_with("MARK"),
        "expected MARK at col 5 of row 3, got: {text:?}"
    );
    // The marker starts at column index 4 (0-based).
    assert_eq!(row.cells[4].ch, 'M');
}
