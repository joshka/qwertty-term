//! Differential replay of upstream ghostty's AFL++ fuzz corpus.
//!
//! ghostty ships minimized fuzz corpora under
//! `test/fuzz-libghostty/corpus/{stream,parser,osc}-{initial,cmin}`; each file
//! is one raw byte stream. We feed each to both the pure-Rust engine and the
//! Zig reference and assert the full observable dump agrees (text + styled VT +
//! cursor + scalar state). These are real-world minimized inputs our synthetic
//! [`generative_sweep`](super) vocabulary won't produce.
//!
//! This lane is local/manual only — CI builds vt-diff WITHOUT the `reference`
//! feature, so it never runs there. It also needs the corpus on disk:
//!
//! - Corpus location: `$GHOSTTY_FUZZ_CORPUS_DIR`, else
//!   `$HOME/local/ghostty/test/fuzz-libghostty/corpus`. Absent ⇒ the test logs
//!   and passes (nothing to check).
//! - The small `*-initial` seeds always run (and must agree exactly). The large
//!   `*-cmin` corpora (~3900 files, 25 MB) run only when `GHOSTTY_FUZZ_FULL=1`
//!   and currently carry a known-divergence budget (issue #169: OSC bodies with
//!   non-UTF-8 bytes are dropped wholesale).
//!
//! `GHOSTTY_VT_LIB_DIR=… GHOSTTY_FUZZ_FULL=1 cargo test -p vt-diff \
//!     --features reference --test afl_corpus -- --nocapture`

#![cfg(feature = "reference")]

use std::fs;
use std::path::{Path, PathBuf};

use vt_diff::{Oracle, ReferenceTerminal, RustTerminal};

/// Resolve the corpus root, or `None` if it isn't on disk.
fn corpus_root() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("GHOSTTY_FUZZ_CORPUS_DIR") {
        let p = PathBuf::from(dir);
        return p.is_dir().then_some(p);
    }
    let home = std::env::var("HOME").ok()?;
    let p = PathBuf::from(home).join("local/ghostty/test/fuzz-libghostty/corpus");
    p.is_dir().then_some(p)
}

/// The corpus subdirectories to replay, given whether the full (`cmin`) run is
/// enabled.
fn corpus_dirs(root: &Path, full: bool) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = ["stream-initial", "parser-initial", "osc-initial"]
        .iter()
        .map(|d| root.join(d))
        .filter(|p| p.is_dir())
        .collect();
    if full {
        for d in ["stream-cmin", "parser-cmin"] {
            let p = root.join(d);
            if p.is_dir() {
                dirs.push(p);
            }
        }
    }
    dirs
}

/// Feed one seed to both oracles at a fixed grid and return `Some(detail)` on
/// divergence. 80x24 is the conventional default; the exact size is immaterial
/// to differential agreement since both sides use it.
fn replay(input: &[u8]) -> Option<String> {
    let mut reference = ReferenceTerminal::new(80, 24);
    let mut rust = RustTerminal::new(80, 24);
    reference.feed(input);
    rust.feed(input);
    let rd = reference.dump();
    let ud = rust.dump();
    if rd == ud {
        return None;
    }
    let mut detail = format!("cursor ref={:?} rust={:?}", rd.cursor, ud.cursor);
    if rd.state != ud.state {
        detail.push_str(&format!("\nstate ref={:?} rust={:?}", rd.state, ud.state));
    }
    if rd.text != ud.text {
        detail.push_str("\n(text differs)");
    }
    if rd.styled != ud.styled {
        detail.push_str("\n(styled differs)");
    }
    Some(detail)
}

#[test]
fn afl_corpus_agrees() {
    let Some(root) = corpus_root() else {
        eprintln!(
            "SKIP afl_corpus: corpus not found (set GHOSTTY_FUZZ_CORPUS_DIR or clone ghostty to \
             ~/local/ghostty)"
        );
        return;
    };
    let full = std::env::var("GHOSTTY_FUZZ_FULL").as_deref() == Ok("1");
    let dirs = corpus_dirs(&root, full);
    assert!(
        !dirs.is_empty(),
        "no corpus subdirs under {}",
        root.display()
    );

    let mut files = 0usize;
    let mut divergences: Vec<(PathBuf, String)> = Vec::new();
    for dir in &dirs {
        for entry in fs::read_dir(dir).expect("read corpus subdir") {
            let path = entry.expect("read corpus entry").path();
            if !path.is_file() {
                continue;
            }
            let bytes = match fs::read(&path) {
                Ok(b) => b,
                Err(_) => continue,
            };
            files += 1;
            if let Some(detail) = replay(&bytes) {
                divergences.push((path, detail));
            }
        }
    }

    eprintln!(
        "afl_corpus: replayed {files} seeds from {} dir(s){}, {} divergence(s)",
        dirs.len(),
        if full {
            " (full/cmin)"
        } else {
            " (initial only; set GHOSTTY_FUZZ_FULL=1 for cmin)"
        },
        divergences.len(),
    );

    // The small `-initial` seeds must agree exactly. The large `-cmin` corpora
    // currently surface a known cluster of divergences, all one root cause —
    // OSC bodies with non-UTF-8 bytes are dropped wholesale (issue #169). Until
    // that's fixed, tolerate up to that baseline in full mode but still fail on
    // any *regression* beyond it. Drop `CMIN_KNOWN_DIVERGENCES` to 0 once #169
    // lands.
    const CMIN_KNOWN_DIVERGENCES: usize = 14;
    let budget = if full { CMIN_KNOWN_DIVERGENCES } else { 0 };

    if divergences.len() > budget {
        let show = divergences.len().min(10);
        let mut msg = format!(
            "afl_corpus found {} divergence(s) over {files} seeds (budget {budget}); showing \
             {show}:\n",
            divergences.len()
        );
        for (path, detail) in divergences.iter().take(show) {
            msg.push_str(&format!("\n=== {} ===\n{detail}\n", path.display()));
        }
        panic!("{msg}");
    }
}
