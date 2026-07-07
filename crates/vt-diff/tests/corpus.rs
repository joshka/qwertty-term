//! Corpus sweep: every case under `crates/vt-diff/corpus/` is fed to both
//! the Zig `libghostty-vt` reference and the Rust `ghostty-vt` port, and the
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
/// of the three comparisons (text, cursor, formatter) diverge.
fn run_case(label: &str, cols: u16, rows: u16, input: &[u8]) -> Option<String> {
    let mut reference = ReferenceTerminal::new(cols, rows);
    let mut rust = RustTerminal::new(cols, rows);
    reference.feed(input);
    rust.feed(input);

    let rd = reference.dump();
    let ud = rust.dump();
    let rf = vt_diff::normalize_screen_text(&reference.raw_text());
    let uf = vt_diff::normalize_screen_text(&rust.formatter_raw_text());

    if rd == ud && rf == uf {
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

/// KNOWN DIVERGENCE: overwriting the spacer-tail cell of a wide character
/// panics the Rust engine in debug builds.
///
/// Input: print `中` at 1;1 (head col 1, spacer tail col 2), `CUP 1;2`, print
/// `X`. The Zig reference clears the wide head and prints `X` at column 2
/// (row becomes ` X`). Its `printCell` `.spacer_tail` branch pre-sets
/// `cell.wide = .narrow` under `runtime_safety` — with the comment "So
/// integrity checks pass. We fix this up later" — *before* calling
/// `clearCells` on the head (ghostty `src/terminal/Terminal.zig`, ~line
/// 1160). The Rust port (`print_cell_fix_wide`, `Wide::SpacerTail` branch in
/// `crates/ghostty-vt/src/terminal/print.rs`) omitted that pre-set, so the
/// debug integrity check that runs inside `clear_cells_page` still sees a
/// `SpacerTail` whose left neighbor is no longer `Wide` and panics with
/// `InvalidSpacerTailLocation` (`crates/ghostty-vt/src/page/page_impl.rs:940`).
///
/// The reference is right; the Rust port needs the same
/// transient-state fix. Remove this test + the case's `SKIP` file when fixed.
#[test]
#[ignore = "known divergence: Rust engine debug-panics on spacer-tail overwrite"]
fn known_divergence_wide_spacer_overwrite() {
    assert_case_agrees("wrap_semantics/wide_spacer_overwrite");
}
