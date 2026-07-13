//! End-to-end: CLI `--key=value` overrides apply at load *and* replay on reload.
//!
//! This exercises the `OnceLock`-backed capture in `config`: `load_with_cli`
//! stores the overrides once at startup, and a later plain `load()` (what a live
//! config reload calls) must re-apply them on top of the freshly-read file — so a
//! flag like `--font-size=22` doesn't silently revert to the file value on reload.
//!
//! It runs as its own test binary (fresh process → fresh `OnceLock`) and isolates
//! the config to a single temp file via `QWERTTY_TERM_CONFIG_DIR`.

use std::fs;

#[test]
fn cli_overrides_apply_and_replay_on_reload() {
    let dir = std::env::temp_dir().join("qwertty_cli_override_e2e");
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("config.toml"),
        "theme = \"FromFile\"\nfont-size = 10\n",
    )
    .unwrap();

    // Isolate to this single file (also skips the macOS App Support location).
    // SAFETY: this is the sole test in this binary and sets the env before any
    // config load; nothing else reads it concurrently.
    unsafe {
        std::env::set_var("QWERTTY_TERM_CONFIG_DIR", &dir);
    }

    // Startup: overrides beat the file.
    let cfg = qwertty_term::config::load_with_cli(vec![
        "--theme=FromCli".into(),
        "--font-size=22".into(),
    ]);
    assert_eq!(cfg.theme.as_deref(), Some("FromCli"));
    assert_eq!(cfg.font_size, Some(22.0));

    // Reload: re-reads the file, but the captured overrides replay on top.
    let reloaded = qwertty_term::config::load();
    assert_eq!(reloaded.theme.as_deref(), Some("FromCli"));
    assert_eq!(reloaded.font_size, Some(22.0));
}
