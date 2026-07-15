//! Header drift check: regenerate `include/qwertty_term.h` with cbindgen and diff
//! it against the checked-in copy. If this fails, the C ABI changed but the
//! committed header did not (or vice versa) -- regenerate with:
//!
//! ```sh
//! cbindgen --config crates/qwertty-term-ffi/cbindgen.toml \
//!   --output crates/qwertty-term-ffi/include/qwertty_term.h
//! ```
//!
//! This mirrors the "checked-in header + CI drift check" decision locked in
//! `docs/plans/m5-ffi-spike.md`: the generated header is the source of truth
//! for the Swift side, so it must never silently fall out of sync with the
//! Rust ABI.
//!
//! The regeneration goes through the cbindgen *library* API (not the CLI) so
//! the check needs no external binary on PATH -- cbindgen is a build-dependency
//! and thus a dev-dependency-equivalent for tests via the same version pin.

use std::path::PathBuf;

fn crate_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Generate the header into a string using the same config as the checked-in
/// copy, so the only variable is the Rust source.
fn generate_header() -> String {
    let dir = crate_dir();
    let config =
        cbindgen::Config::from_file(dir.join("cbindgen.toml")).expect("cbindgen.toml must parse");
    let bindings = cbindgen::Builder::new()
        .with_crate(&dir)
        .with_config(config)
        .generate()
        .expect("cbindgen must generate bindings");
    let mut out = Vec::new();
    bindings.write(&mut out);
    String::from_utf8(out).expect("generated header is valid UTF-8")
}

#[test]
fn header_matches_checked_in() {
    let committed_path = crate_dir().join("include/qwertty_term.h");
    let committed = std::fs::read_to_string(&committed_path)
        .expect("checked-in include/qwertty_term.h must exist");
    let generated = generate_header();

    if normalize(&committed) != normalize(&generated) {
        // Write the freshly generated header next to the committed one so a
        // developer can `diff` / `mv` it, then fail loudly.
        let actual_path = crate_dir().join("include/qwertty_term.generated.h");
        let _ = std::fs::write(&actual_path, &generated);
        panic!(
            "qwertty_term.h is out of date with the Rust C ABI. Regenerate it \
             with: cbindgen --config crates/qwertty-term-ffi/cbindgen.toml --output \
             crates/qwertty-term-ffi/include/qwertty_term.h -- a freshly generated \
             copy was written to {} for diffing.",
            actual_path.display()
        );
    }
}

/// Normalize line endings and trailing whitespace so the diff is about content,
/// not platform line-ending or editor-trim differences.
fn normalize(s: &str) -> String {
    s.replace("\r\n", "\n")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim_end()
        .to_string()
}
