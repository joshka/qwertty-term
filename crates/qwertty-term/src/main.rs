//! `qwertty-term` entry point.
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
//! The `QWERTTY_TERM_SMOKE_MS` environment variable, if set to a positive integer
//! in window mode, schedules a clean auto-exit after that many milliseconds —
//! used to smoke-test app startup/teardown without a human closing the window.
//!
//! Debug env vars (window mode):
//! - `QWERTTY_TERM_DUMP_FRAME=<prefix>` — after every Nth present, read the
//!   presented IOSurface back and write it to `<prefix>-NNNN.png`. The decisive
//!   probe for "blank window" bugs: if the PNGs contain glyphs but the window
//!   doesn't, it's a presentation-geometry bug (contentsScale); if the PNGs are
//!   blank too, it's the pump/draw path.
//! - `QWERTTY_TERM_DUMP_EVERY=<n>` — dump cadence (default 30 presents).
//! - `QWERTTY_TERM_ASSERT_PRESENT=1` — with `QWERTTY_TERM_SMOKE_TYPE`, also assert
//!   the *presented* frame has glyph coverage (not just the engine buffer).
//! - `QWERTTY_TERM_SMOKE_TABKEYS=1` — run the tab-navigation keybind smoke: open
//!   3 tabs, drive the built-in tab chords (ctrl+tab, ctrl+shift+tab, cmd+1..9,
//!   cmd+shift+[/]) through the real `performKeyEquivalent:` path, assert the
//!   active-tab index after each, and check the pty-encoding regression, then
//!   exit 0/1.
//! - `QWERTTY_TERM_SMOKE_SPLITS=1` — run the splits smoke: split the pane right
//!   then down (3 panes), assert 3 isolated shells (each marker only in its own
//!   pane), directional focus navigation, divider-driven per-pane resize,
//!   poison-resilience (crash one pane's engine → it alone dies + banners, app +
//!   siblings survive), and close-collapse (middle pane close → 2 panes; close
//!   all → tab closes), then exit 0/1. Pair with `QWERTTY_TERM_ASSERT_PRESENT=1`
//!   to also assert each pane presented real ink in its own rect.
//! - `QWERTTY_TERM_SMOKE_KEYBIND=1` — run the keybind smoke: seed the maintainer's
//!   `shift+enter=text:...` binding, drive Shift+Return + plain Return through the
//!   real key path, and assert shift+enter's `text:` bytes reached the focused
//!   pane's pty while plain enter still submitted (CR), then exit 0/1.
//! - `QWERTTY_TERM_SMOKE_FOCUS=1` — run the per-pane focus-reporting smoke: two
//!   `cat -v` panes with mode 1004 enabled; focus-switch and assert the focus-in
//!   (`CSI I`) / focus-out (`CSI O`) bytes reach the correct per-surface ptys,
//!   then exit 0/1.
//! - `QWERTTY_TERM_SMOKE_SEARCH=1` — run the scrollback-search smoke: fill the
//!   focused pane's scrollback with 3 marker lines, drive Cmd+F (opening the
//!   overlay), set the needle, assert the match counter reads 3, navigate
//!   next/next/prev asserting the viewport offset lands on each match's row, and
//!   assert Escape closes the bar and restores PTY input, then exit 0/1.
//! - `QWERTTY_TERM_SMOKE_SELECTION=1` — run the selection-gestures smoke: feed
//!   deterministic screen content, then drive synthetic mouse NSEvents through
//!   the real window event path asserting double-click selects a word (with
//!   the upstream boundary set), triple-click selects the line, a fresh single
//!   click clears, press-drag-release selects by cell, shift-click extends,
//!   and a drag parked past the top edge autoscrolls the viewport into
//!   scrollback while extending the selection, then exit 0/1.
//! - `QWERTTY_TERM_SMOKE_TITLE=1` — run the tab-title smoke: feed OSC 2 titles
//!   into two tabs' engines and assert each tab's window title (= its native
//!   tab label) tracks its own title live, updates on change, and falls back
//!   to the ghost emoji after the 500ms grace when a title is cleared, then
//!   exit 0/1.
//! - `QWERTTY_TERM_SMOKE_QUICKTERM=1` — run the quick-terminal smoke: toggle the
//!   dropdown in (assert it becomes visible, its window frame lands at the
//!   configured screen edge, and its shell echoes typed input), toggle it out
//!   (assert hidden), and toggle back in (assert re-shown), then exit 0/1.
//! - `QWERTTY_TERM_SMOKE_BELL=1` — run the bell smoke: feed a BEL into the
//!   focused pane's engine, tick, and assert the tab shows the 🔔 title
//!   indicator (default `bell-features` = attention+title); then refocus the
//!   window and assert the indicator clears, then exit 0/1.
//! - `QWERTTY_TERM_SMOKE_MOUSE=1` — run the mouse-behaviors smoke: assert the
//!   right-click context menu's items for the focused pane (Paste/splits/close,
//!   Copy only with a selection), then invoke Split Right (assert 2 panes) and
//!   Close Pane (assert back to 1), then exit 0/1.
//! - `QWERTTY_TERM_SMOKE_CLIPBOARD=1` — run the clipboard-hardening smoke:
//!   assert paste-protection classifies multiline pastes as unsafe (gated by a
//!   confirmation) while single-line pastes pass, and that typing clears the
//!   selection (`selection-clear-on-typing`), then exit 0/1. Uses `cat` as the
//!   child so pastes echo deterministically.
//! - `QWERTTY_TERM_SMOKE_WINDOWSTATE=1` — run the window-state smoke: with
//!   `window-width`/`window-height` cells configured, assert the first window's
//!   content size matches the requested cell grid (later windows keep the
//!   default), then exit 0/1.
//! - `QWERTTY_TERM_SMOKE_NOTIFY=1` — run the desktop-notification smoke: feed
//!   OSC 9 and OSC 777 to the focused surface and assert each is parsed,
//!   drained, gated, throttled, and delivered to the notification seam, then
//!   exit 0/1.

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
    match qwertty_term::smoke::run() {
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
    let mut config = qwertty_term::config::load();
    let smoke_ms = std::env::var("QWERTTY_TERM_SMOKE_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    // Synthetic-input smoke: if set, type this string through the real window
    // keyDown path after launch and assert its round-trip (see app::run).
    // `\n` / `\t` escapes in the env value are unescaped for convenience.
    let smoke_type = std::env::var("QWERTTY_TERM_SMOKE_TYPE")
        .ok()
        .map(|v| v.replace("\\n", "\n").replace("\\t", "\t"))
        .unwrap_or_default();
    // Tab-strip geometry smoke: dump + assert window geometry across the
    // 1-tab→2-tab→1-tab transition, then exit (see app::run).
    let smoke_geometry = std::env::var_os("QWERTTY_TERM_SMOKE_GEOMETRY").is_some();
    // Tab-navigation keybind smoke: open 3 tabs, drive the built-in tab chords,
    // assert the active-tab index after each, then exit (see app::run).
    let smoke_tabkeys = std::env::var_os("QWERTTY_TERM_SMOKE_TABKEYS").is_some();
    // Splits smoke: split into 3 panes, assert isolated shells / directional
    // focus / divider resize / close-collapse, then exit (see app::run).
    let smoke_splits = std::env::var_os("QWERTTY_TERM_SMOKE_SPLITS").is_some();
    // Keybind smoke: seed the maintainer's `text:` binding on shift+enter, drive
    // it (and a plain enter) through the real key path, assert the pty round-trip
    // (see app::run). The binding is injected here so the smoke is self-contained
    // (no user config file needed); it uses a visible marker so the assertion can
    // read it off the screen.
    let smoke_keybind = std::env::var_os("QWERTTY_TERM_SMOKE_KEYBIND").is_some();
    if smoke_keybind {
        // A marker-carrying `text:` value so shift+enter's bytes are observable
        // in the pane; the real maintainer binding (`\x1b\r`) is unit-tested for
        // exact bytes in `keybind.rs`.
        config
            .keybind
            .push("shift+enter=text:zzKBMARKERzz".to_string());
    }
    // Focus-reporting smoke: two panes running `cat -v` with mode 1004 enabled;
    // focus-switch between them and assert the focus-in/out bytes reach the right
    // ptys (see app::run).
    let smoke_focus = std::env::var_os("QWERTTY_TERM_SMOKE_FOCUS").is_some();
    // Search smoke: fill scrollback with markers, Cmd+F, type the needle, assert
    // the counter, navigate, and assert Escape restores PTY input (see app::run).
    let smoke_search = std::env::var_os("QWERTTY_TERM_SMOKE_SEARCH").is_some();
    // Selection smoke: drive synthetic mouse gestures (double/triple click,
    // drag, shift-extend, edge autoscroll) and assert the selection text.
    let smoke_selection = std::env::var_os("QWERTTY_TERM_SMOKE_SELECTION").is_some();
    // Title smoke: feed OSC 2 titles into two tabs and assert per-tab window/
    // tab titles + the ghost-emoji fallback.
    let smoke_title = std::env::var_os("QWERTTY_TERM_SMOKE_TITLE").is_some();
    // Quick-terminal smoke: toggle the dropdown in/out and assert visibility,
    // geometry, and a live shell.
    let smoke_quickterm = std::env::var_os("QWERTTY_TERM_SMOKE_QUICKTERM").is_some();
    // Bell smoke: feed a BEL and assert the tab's 🔔 title indicator appears,
    // then clears on refocus.
    let smoke_bell = std::env::var_os("QWERTTY_TERM_SMOKE_BELL").is_some();
    // Mouse smoke: assert the right-click context menu + split/close actions.
    let smoke_mouse = std::env::var_os("QWERTTY_TERM_SMOKE_MOUSE").is_some();
    // Clipboard smoke: paste-protection + selection-clear-on-typing.
    let smoke_clipboard = std::env::var_os("QWERTTY_TERM_SMOKE_CLIPBOARD").is_some();
    // Window-state smoke: assert the first window honors configured initial
    // geometry (window-width/-height cells + window-position).
    let smoke_windowstate = std::env::var_os("QWERTTY_TERM_SMOKE_WINDOWSTATE").is_some();
    // Notify smoke: assert OSC 9/777 desktop notifications reach the delivery seam.
    let smoke_notify = std::env::var_os("QWERTTY_TERM_SMOKE_NOTIFY").is_some();
    qwertty_term::app::run(
        &config,
        smoke_ms,
        smoke_type,
        smoke_geometry,
        smoke_tabkeys,
        smoke_splits,
        smoke_keybind,
        smoke_focus,
        smoke_search,
        smoke_selection,
        smoke_title,
        smoke_quickterm,
        smoke_bell,
        smoke_mouse,
        smoke_clipboard,
        smoke_windowstate,
        smoke_notify,
    );
}

#[cfg(not(target_os = "macos"))]
fn run_offscreen_smoke() {
    eprintln!("qwertty-term is macOS-only");
    std::process::exit(1);
}

#[cfg(not(target_os = "macos"))]
fn run_window() {
    eprintln!("qwertty-term is macOS-only");
    std::process::exit(1);
}
