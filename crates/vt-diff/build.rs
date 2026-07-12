//! Locates the Zig-built `libghostty-vt.a` when the `reference` feature is
//! enabled. Without the feature this script emits nothing, so the crate
//! builds on machines that have never built the Zig artifact.
//!
//! Library search order:
//! 1. `$GHOSTTY_VT_LIB_DIR` (a directory containing `libghostty-vt.a`)
//! 2. `$HOME/local/ghostty/zig-out/lib` (the default install prefix of
//!    `zig build -Demit-lib-vt=true` run in the ghostty checkout)
//!
//! Build the library with (Zig 0.15.2, e.g. via `mise exec zig@0.15.2`):
//! ```sh
//! cd ~/local/ghostty && zig build -Demit-lib-vt=true
//! ```

use std::env;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=GHOSTTY_VT_LIB_DIR");

    // Set by cargo only when the `reference` feature is enabled.
    if env::var_os("CARGO_FEATURE_REFERENCE").is_none() {
        return;
    }

    let lib_dir = env::var_os("GHOSTTY_VT_LIB_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            env::var_os("HOME").map(|home| {
                PathBuf::from(home)
                    .join("local")
                    .join("ghostty")
                    .join("zig-out")
                    .join("lib")
            })
        })
        .expect("neither GHOSTTY_VT_LIB_DIR nor HOME is set");

    let lib = lib_dir.join("libghostty-vt.a");
    assert!(
        lib.is_file(),
        "libghostty-vt.a not found at {}.\n\
         Build it with `zig build -Demit-lib-vt=true` (Zig 0.15.2) in the \
         ghostty checkout, or point GHOSTTY_VT_LIB_DIR at a directory \
         containing libghostty-vt.a.",
        lib.display()
    );

    // zig-out/lib also contains libghostty-vt.dylib, and macOS ld prefers a
    // dylib over a static archive in the same directory (which would then
    // fail at runtime with an unresolvable @rpath). Copy the archive into
    // OUT_DIR so the search path contains only the static library.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR is set by cargo"));
    let staged = out_dir.join("libghostty-vt.a");
    std::fs::copy(&lib, &staged)
        .unwrap_or_else(|err| panic!("copy {} to {}: {err}", lib.display(), staged.display()));

    println!("cargo:rerun-if-changed={}", lib.display());
    println!("cargo:rustc-link-search=native={}", out_dir.display());
    // The staged archive is `libghostty-vt.a` (the Zig reference lib), so link
    // it by that basename. (The `qwertty-term-vt` Rust crate is an ordinary
    // rlib dependency, not a C archive — an earlier rename left this pointing
    // at a nonexistent `libqwertty-term-vt.a`, which broke the reference lane.)
    println!("cargo:rustc-link-lib=static=ghostty-vt");
}
