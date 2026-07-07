//! `ghostty-app` entry point.
//!
//! Modes:
//!
//! - (default) / `--window`: launch the native AppKit window host. On a machine
//!   with a Metal device this renders the terminal; native tabs (Cmd-T), a menu
//!   bar, and typing all work. See `docs/analysis/renderer-r5.md` for manual
//!   test steps.
//! - `--offscreen-smoke`: run the headless full-stack smoke (engine + PTY +
//!   renderer → IOSurface readback), print the result, and exit `0` on success
//!   (or on a graceful skip when no Metal device is present), non-zero on
//!   failure. No window.
//!
//! The `GHOSTTY_APP_SMOKE_MS` environment variable, if set to a positive integer
//! in window mode, schedules a clean auto-exit after that many milliseconds —
//! used to smoke-test app startup/teardown without a human closing the window.

fn main() {
    let mode = parse_mode(std::env::args().skip(1));

    match mode {
        Mode::OffscreenSmoke => run_offscreen_smoke(),
        Mode::Window => run_window(),
    }
}

/// What to run.
enum Mode {
    Window,
    OffscreenSmoke,
}

/// Parse the CLI args into a mode (only two flags; anything else → window).
fn parse_mode(args: impl Iterator<Item = String>) -> Mode {
    for arg in args {
        match arg.as_str() {
            "--offscreen-smoke" => return Mode::OffscreenSmoke,
            "--window" => return Mode::Window,
            _ => {}
        }
    }
    Mode::Window
}

#[cfg(target_os = "macos")]
fn run_offscreen_smoke() {
    match ghostty_app::smoke::run() {
        Ok(true) => {
            println!("OK: offscreen smoke rendered a verified frame");
            std::process::exit(0);
        }
        Ok(false) => {
            // Graceful skip (no Metal device): treat as success for CI.
            std::process::exit(0);
        }
        Err(e) => {
            eprintln!("FAIL: offscreen smoke: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(target_os = "macos")]
fn run_window() {
    let config = ghostty_app::config::load();
    let smoke_ms = std::env::var("GHOSTTY_APP_SMOKE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    ghostty_app::app::run(&config, smoke_ms);
}

#[cfg(not(target_os = "macos"))]
fn run_offscreen_smoke() {
    eprintln!("ghostty-app is macOS-only");
    std::process::exit(1);
}

#[cfg(not(target_os = "macos"))]
fn run_window() {
    eprintln!("ghostty-app is macOS-only");
    std::process::exit(1);
}
