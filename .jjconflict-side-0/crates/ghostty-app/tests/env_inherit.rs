//! Regression: the spawned shell must inherit the app's environment.
//! Field bug 2026-07-08: `Config::default()` has an empty env, so the login
//! shell had no PATH -- oh-my-zsh init failed with "command not found" for
//! coreutils and the shell died (closing the tab, resembling an app crash).

use std::sync::{Arc, Mutex};

#[test]
fn spawned_shell_inherits_path() {
    // Drive the same TabIo config path the app uses, headless.
    let engine = Arc::new(Mutex::new(ghostty_app::engine::Engine::new(80, 24)));
    let io =
        ghostty_app::termio::TabIo::spawn(Arc::clone(&engine), 80, 24, 8, 16, None).expect("spawn");
    // `command -v ls` requires a working PATH lookup in the shell itself.
    io.write(b"command -v ls >/dev/null && echo ENV-$((40+2))-MARKER\n");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut ok = false;
    while std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(100));
        let text = engine.lock().unwrap().screen_dump();
        // "ENV-42-MARKER" only exists if the shell computed it (PATH lookup
        // succeeded); the typed line reads "$((40+2))" so it can't false-match.
        if text.contains("ENV-42-MARKER") {
            ok = true;
            break;
        }
    }
    drop(io);
    assert!(
        ok,
        "shell did not resolve `ls` via PATH: {}",
        engine.lock().unwrap().screen_dump()
    );
}
