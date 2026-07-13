//! Corpus sweep: every case under `crates/vt-diff/corpus/` is fed to both
//! the Zig `libghostty-vt` reference and the Rust `qwertty-term-vt` port, and the
//! screen dump (text + cursor) *and* formatter output must agree.
//!
//! Layout (mirrors `crates/spike/tests/fixtures/replay`): one directory per
//! case containing `input.esc` (escaped byte stream, see
//! [`vt_diff::decode_escaped_stream`]) and an optional `size.txt`
//! (`"COLS ROWS"`, default 80x24). A `SKIP` sentinel file marks a known
//! divergence: the sweep excludes it and a dedicated `#[ignore]` test at the
//! bottom of this file documents the disagreement.
//!
//! Only runs with the `reference` feature:
//! `cargo test -p vt-diff --features reference`.

#![cfg(feature = "reference")]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use vt_diff::{Oracle, ReferenceTerminal, RustTerminal, decode_escaped_stream};

const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("corpus")
        .canonicalize()
        .expect("corpus directory exists")
}

/// A case is any directory (at any depth) containing `input.esc`.
fn collect_cases(dir: &Path, skipped: bool, out: &mut Vec<(PathBuf, bool)>) {
    if dir.join("input.esc").is_file() {
        let skip = skipped || dir.join("SKIP").exists();
        out.push((dir.to_path_buf(), skip));
        return;
    }
    let mut children: Vec<_> = fs::read_dir(dir)
        .expect("read corpus directory")
        .map(|e| e.expect("read corpus entry").path())
        .filter(|p| p.is_dir())
        .collect();
    children.sort();
    for child in children {
        collect_cases(&child, skipped, out);
    }
}

fn read_size(case: &Path) -> (u16, u16) {
    let path = case.join("size.txt");
    if !path.is_file() {
        return (DEFAULT_COLS, DEFAULT_ROWS);
    }
    let text = fs::read_to_string(&path).expect("read case size");
    let mut parts = text.split_whitespace();
    let cols = parts.next().expect("size cols").parse().expect("cols");
    let rows = parts.next().expect("size rows").parse().expect("rows");
    (cols, rows)
}

fn read_input(case: &Path) -> Vec<u8> {
    let text = fs::read_to_string(case.join("input.esc")).expect("read case input");
    decode_escaped_stream(&text)
}

/// Run one case through both engines, catching Rust-engine panics (an engine
/// panic on valid input is itself a divergence worth reporting); returns a
/// failure description if anything diverges.
fn run_case_caught(label: &str, cols: u16, rows: u16, input: &[u8]) -> Option<String> {
    use std::panic;
    let result = panic::catch_unwind(panic::AssertUnwindSafe(|| {
        run_case(label, cols, rows, input)
    }));
    match result {
        Ok(outcome) => outcome,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic payload>".to_string());
            Some(format!("=== {label} ({cols}x{rows}) ===\nPANIC: {msg}\n"))
        }
    }
}

/// Run one case through both engines; returns a failure description if any
/// of the four comparisons (text, cursor, formatter, reply bytes) diverge.
///
/// Reply-byte diffing (`reference.output()` vs `rust.output()`) compares the
/// bytes each engine writes back to the pty in response to queries (DECRQM,
/// DSR/DA, kitty-keyboard query, DECRQSS, ...). This only works because
/// `ReferenceTerminal` registers `GHOSTTY_TERMINAL_OPT_WRITE_PTY` (see
/// `src/reference.rs`) — without that callback the reference silently drops
/// all reply bytes and this comparison would spuriously fail for every case
/// exercising a query sequence.
fn run_case(label: &str, cols: u16, rows: u16, input: &[u8]) -> Option<String> {
    let mut reference = ReferenceTerminal::new(cols, rows);
    let mut rust = RustTerminal::new(cols, rows);
    reference.feed(input);
    rust.feed(input);

    let rd = reference.dump();
    let ud = rust.dump();
    let rf = vt_diff::normalize_screen_text(&reference.raw_text());
    let uf = vt_diff::normalize_screen_text(&rust.formatter_raw_text());
    let ro = reference.output();
    let uo = rust.output();

    if rd == ud && rf == uf && ro == uo {
        return None;
    }
    let mut msg = format!("=== {label} ({cols}x{rows}) ===\n");
    if rd.text != ud.text {
        let _ = write!(
            msg,
            "TEXT diverged:\n--- reference ---\n{}\n--- rust ---\n{}\n",
            rd.text, ud.text
        );
    }
    if rd.cursor != ud.cursor {
        let _ = writeln!(
            msg,
            "CURSOR diverged: reference {:?} vs rust {:?}",
            rd.cursor, ud.cursor
        );
    }
    if rf != uf {
        let _ = write!(
            msg,
            "FORMATTER diverged:\n--- reference ---\n{rf}\n--- rust ---\n{uf}\n"
        );
    }
    if ro != uo {
        let _ = write!(
            msg,
            "REPLY diverged:\n--- reference ---\n{:?}\n--- rust ---\n{:?}\n",
            String::from_utf8_lossy(ro),
            String::from_utf8_lossy(uo)
        );
    }
    Some(msg)
}

/// Sweep the whole corpus, reporting every divergence (not just the first).
#[test]
fn corpus_agrees() {
    let root = corpus_root();
    let mut cases = Vec::new();
    collect_cases(&root, false, &mut cases);
    assert!(
        cases.len() >= 60,
        "expected a substantial corpus, found {}",
        cases.len()
    );

    let mut failures = Vec::new();
    let mut ran = 0;
    let mut skipped = 0;
    for (case, skip) in &cases {
        let label = case
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .to_string();
        if *skip {
            skipped += 1;
            continue;
        }
        let (cols, rows) = read_size(case);
        let input = read_input(case);
        if let Some(msg) = run_case_caught(&label, cols, rows, &input) {
            failures.push(msg);
        }
        ran += 1;
    }

    assert!(
        failures.is_empty(),
        "{} of {} corpus cases diverged ({} known-divergence cases skipped):\n\n{}",
        failures.len(),
        ran,
        skipped,
        failures.join("\n")
    );
}

/// Canary: the sweep compares real content, not accidentally-empty dumps.
#[test]
fn corpus_canary_nontrivial_content() {
    let case = corpus_root().join("wrap_semantics/wrap_basic");
    let (cols, rows) = read_size(&case);
    let mut reference = ReferenceTerminal::new(cols, rows);
    reference.feed(&read_input(&case));
    assert_eq!(reference.dump().text, "abcdefgh\nij");
}

// ---- known divergences ---------------------------------------------------
//
// Each case below carries a `SKIP` sentinel in its corpus directory (so the
// sweep above stays green) and an `#[ignore]`d test here that demonstrates
// the exact disagreement. Run them with:
// `cargo test -p vt-diff --features reference -- --ignored`
// Each test asserts *agreement*, so it fails while the divergence exists and
// starts passing (remove the SKIP + this test) once fixed.

fn assert_case_agrees(rel: &str) {
    let case = corpus_root().join(rel);
    let (cols, rows) = read_size(&case);
    let input = read_input(&case);
    if let Some(msg) = run_case_caught(rel, cols, rows, &input) {
        panic!("{msg}");
    }
}

// `regression_wide_spacer_overwrite` (spacer-tail overwrite panic) was
// removed here: fixed upstream in commit df799546902f ("Fix spacer-tail
// overwrite panic; un-skip corpus case"), which also removed the case's
// `SKIP` file — `wrap_semantics/wide_spacer_overwrite` is exercised by the
// main `corpus_agrees` sweep now like any other case.

/// KNOWN DIVERGENCE: DA2 (secondary device attributes, `CSI > c`) reports a
/// firmware/patch-version of 10 in the Rust port vs upstream's documented
/// default of 0.
///
/// Input: `CSI > c`. Reference replies `\x1b[>1;0;0c`; Rust replies
/// `\x1b[>1;10;0c`. Upstream's `Secondary` struct
/// (`ghostty/src/terminal/device_attributes.zig:80-94`) defaults
/// `firmware_version: u16 = 0` (pinned by its own inline test
/// `"secondary default"`, line 208, asserting exactly `"\x1b[>1;0;0c"`). The
/// Rust port's `device_attributes` handler
/// (`crates/qwertty-term-vt/src/stream.rs`, `DeviceAttributesReq::Secondary` arm)
/// hardcodes `\x1b[>1;10;0c` with the comment "VT220-ish, version 10" — no
/// upstream basis for `10` was found; it appears to be an invented value.
///
/// The reference (and upstream default) is right; the Rust port's literal
/// should be `0`. Remove this test + the case's `SKIP` file when fixed. Two
/// real-app captures (`real_apps/nvim_edit` is unrelated; `real_apps/tmux_session`,
/// which issues a DA2 query on startup) carry `SKIP` for this same root
/// cause.
#[test]
fn regression_da2_firmware_version_mismatch() {
    assert_case_agrees("reply_diffing/da2_secondary_version");
}

/// KNOWN DIVERGENCE: DECRQSS SGR (`DCS $ q m ST`) is answered by the Rust
/// core engine but ignored by the reference.
///
/// Input: `\x1bP$qm\x1b\\` (request current SGR attributes). Reference
/// writes nothing back; Rust replies `\x1bP1$r0m\x1b\\` (or a fuller SGR
/// param list when attributes are set).
///
/// This is not a parity bug in the classic sense — it's a **scope**
/// mismatch. Upstream's DECRQSS *response* logic
/// (`ghostty/src/termio/stream_handler.zig:475-540`, the `.decrqss` arm)
/// lives entirely in the **app-level termio handler**, not in
/// `terminal/Terminal.zig` (the core the Rust `qwertty-term-vt` crate — and the
/// `libghostty-vt` C API this harness links — actually ports). Confirmed by
/// `grep`: `Terminal.zig` defines no `dcsHook`/`decrqss` handling at all;
/// only `terminal/dcs.zig` parses the *request* into a `.decrqss` enum
/// variant, which the core's `Stream` dispatches to `handler.vt(...)` — a
/// hook only `stream_handler.zig` (app layer) implements. So the reference
/// terminal here (built from the core alone) correctly has no way to answer
/// DECRQSS, matching its scope; the Rust port's `decrqss` method in
/// `crates/qwertty-term-vt/src/stream.rs` ported the app-layer response logic
/// into the vt-core crate, which is out of scope for a `Terminal`/core port
/// and produces output no core-only C-API consumer (like this harness, or
/// any other libghostty-vt embedder) should expect.
///
/// Resolution is a scoping decision, not just a bug fix: either (a) move
/// `decrqss` response formatting out of `qwertty-term-vt`'s core `Stream`/
/// `TerminalHandler` into whatever layer will eventually mirror
/// `termio/stream_handler.zig` (leaving the core silent, matching the
/// reference exactly), or (b) if `qwertty-term-vt` intentionally broadens scope
/// to include this, document that divergence from libghostty-vt explicitly
/// rather than silently disagreeing. Remove this test + the case's `SKIP`
/// file once resolved either way. `real_apps/nvim_edit` (nvim probes DECRQSS
/// twice on startup) carries `SKIP` for this same root cause.
#[test]
#[ignore]
fn regression_decrqss_answered_by_core_engine() {
    assert_case_agrees("reply_diffing/decrqss_sgr_scope");
}

/// KNOWN DIVERGENCE (post-pin, INTENTIONAL): scroll-region optimization.
///
/// Unlike the divergences above (where the Rust port is arguably wrong), here
/// the Rust port is deliberately AHEAD of the pinned oracle. Upstream commit
/// `77190bd02` ("terminal: handful of scroll region optimizations") stopped
/// creating scrollback for top-anchored full-width regions on screens that
/// don't retain scrollback (the alt screen). The `qwertty-term-vt` port mirrors
/// that (see `Terminal::index`/`scroll_up`), but our differential oracle
/// (`libghostty-vt`) is pinned at `2da015cd6`, which PREDATES `77190bd02` and
/// still routes those scrolls through `cursorScrollAbove`, leaving the
/// scrolled-out rows in scrollback.
///
/// The **visible grid and cursor are byte-identical**; the only difference is
/// phantom rows in the oracle's scrollback that upstream itself calls "never
/// visible ... simply pruned later". Three probe cases exercise the top-anchored
/// path (`scroll_regions/alt_top_region_{ind,csi_s,bg_ind}`); each carries a
/// `SKIP` sentinel. Un-SKIP these (and delete this test) once the oracle is
/// re-pinned past `77190bd02`. The non-diverging alt-screen cases
/// (`alt_bottom_region_ind`, `alt_full_screen_ind`) run in the main sweep.
#[test]
#[ignore]
fn regression_scroll_region_post_pin_scrollback() {
    assert_case_agrees("scroll_regions/alt_top_region_ind");
}
