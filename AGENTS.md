# Agent Guidance

qwertty-term is a full Rust rewrite of [Ghostty](https://ghostty.org): a native macOS
terminal emulator plus a family of reusable, embeddable crates. It is a byte-faithful port
of Ghostty's Zig (pinned at commit `2da015cd6`, source at `~/local/ghostty`), verified by
differential testing against the original.

**Start here:** `docs/rewrite-prompt.md` (the constitution), then `docs/threads/README.md`
(the parallel-thread model + PR/gate/status protocol), then `docs/handoff.md` /
`docs/port-status.md` / `docs/feature-coverage.md` for current state. Per-subsystem analysis
lives in `docs/analysis/` (commit-stamped) and decisions in `docs/adr/`.

## Repo layout and version control

- Version control is **jj** (colocated git). The repo root (`~/local/ghostty-rs`) is a bare
  store — it holds `.git`, `.jj`, `work/`, and an `AGENTS.md`, but **no checkout** and is NOT
  a jj workspace. Never run jj/git/cargo at the root (it re-creates a phantom workspace that
  snapshots everything — see the root `AGENTS.md`).
- All work happens in per-workspace checkouts under `work/<id>`. Create one from an existing
  checkout: `cd work/josh && jj workspace add ../<id> --name <id> --revision main`. One
  writer per checkout; stay in your own, never touch a sibling's.
- **jj discipline** (full text in `docs/threads/README.md`): run `jj st` after every edit
  burst to snapshot (the unsafe window is between "files edited" and "next jj command";
  `cargo`/`npx`/`git` do NOT snapshot). If the working copy goes stale, just `jj workspace
  update-stale` — it snapshots first, nothing is lost; recover a "vanished" edit via `jj op
  log` → `jj restore`. Never fall back to git plumbing or scratchpad copies. Verify commits
  are non-empty after `jj describe`.
- **Ship via the PR pipeline** (`docs/threads/README.md`): `jj new 'trunk()'` to start on
  current main, `jj describe` → push a bookmark (`jj git push --bookmark <id>/<feature>`) →
  `gh pr create` → merge (which advances `main`). Small doc-only changes may land direct to
  main. `trunk()` (== `main`) is the integration point; keep it green.

## Commands

```bash
# Build / run the app — macOS 13+ (Metal); the crate/binary is `qwertty-term`
cargo run -p qwertty-term --release
cargo run -p frame-capture -- --help        # headless VT-bytes → PNG (the embeddability demo)

# Test
cargo test --workspace
cargo test -p qwertty-term-vt <name>         # a single test by name substring
cargo test -p qwertty-term-vt --test <file>  # a single integration-test file (crates/*/tests/)
cargo test -p qwertty-term-vt --release --all-targets   # RELEASE LANE — never skip (see below)
cargo test -p qwertty-term-vt --release --lib --features slow_runtime_safety  # paranoid lane (ADR 0001)

# Differential parity vs the Ghostty reference oracle
cargo test -p vt-diff                        # curated corpus (no Zig artifact needed)
cargo test -p vt-diff --features reference   # vs libghostty-vt (build the ref lib first — bottom)

# Lint / format (repo markdownlint config: 100 cols, aligned tables, code-fence languages)
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
npx markdownlint-cli2 "**/*.md"              # scripts/align_md_tables.py fixes aligned tables

# App smokes — macOS, drive real Metal/windows; NOT in CI (run locally)
cargo run -p qwertty-term --release -- --offscreen-smoke
QWERTTY_TERM_SMOKE_SPLITS=1 cargo run -p qwertty-term --release   # also GEOMETRY/SEARCH/KEYBIND/FOCUS/MOUSE/BELL/…

# Fuzz (nightly, standalone workspace at crates/qwertty-term-vt/fuzz)
cargo +nightly fuzz run parser -- -max_total_time=180
cargo +nightly fuzz run resize -- -max_total_time=180   # resize-interleaved — catches the class CI misses

# Codegen (checked-in generated tables)
cargo xtask gen-unicode            # UCD tables — exact parity with Ghostty's generated table
cargo xtask gen-nerd-constraints   # nerd-font per-icon sizing table

# Benchmarks
scripts/bench-vtebench.sh [--terminal ghostty --app-path <bundle>]   # + docs/benchmarks/
scripts/bench-doomfire.sh

# Build the Ghostty reference lib (for `vt-diff --features reference`)
cd ~/local/ghostty && mise exec zig@0.15.2 -- zig build -Demit-lib-vt=true -Doptimize=ReleaseFast
```

**The gate** (must pass before `jj describe`/PR): `cargo check --workspace --all-targets`
(zero warnings) · `cargo test --workspace` · the **release lane** · `fmt` · `clippy -D
warnings`. Additionally: `vt-diff --features reference` + a new corpus case when engine
**semantics** change; the offscreen + relevant `QWERTTY_TERM_SMOKE_*` smokes when the app
changes; `markdownlint` when docs change; before/after numbers when perf changes.

## Architecture (big picture)

Data flows PTY → engine → snapshot → renderer → window, split across reusable crates with
**no global state, no async, no window in the core** (so the engine/font/renderer can be
embedded headlessly — betamax is the reference consumer; `examples/frame-capture` packages
bytes → PNG):

| Crate                     | Role                                                                                                                                                                                                                                                |
| ------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `qwertty-term-vt`         | engine core: parser → stream/Handler → Terminal → Screen → PageList; page-based scrollback, ref-counted styles, kitty graphics/keyboard, search, unicode; `snapshot`/`formatter` emit an owned styled grid + a reply queue — the embeddability seam |
| `qwertty-term-termio`     | rustix PTY + upstream's two-stage read pipeline (gather + parse threads), exec, hub, vendored shell-integration scripts                                                                                                                             |
| `qwertty-term-font`       | CoreText (+ FreeType on Linux) faces, discovery, rustybuzz shaping, skyline atlas, metrics, nerd-font constraints                                                                                                                                   |
| `qwertty-term-sprite`     | procedural box/braille/powerline/legacy glyphs (tiny-skia), pixel-parity with upstream goldens                                                                                                                                                      |
| `qwertty-term-renderer`   | `GpuBackend` trait (Metal + a software backend); **frozen wire structs** (GPU layout bit-exact with the verbatim MSL shaders); cell engine; per-row dirty tracking; run-based shaping cache                                                         |
| `qwertty-term-input`      | key/mods/keymap, kitty + legacy encoders, 5 mouse formats, paste; the ported `Binding.zig` keybind system                                                                                                                                           |
| `qwertty-term`            | the macOS AppKit app: NSWindow tabs, splits, menu, `NSTextInputClient` IME, TOML config + live reload, keybind dispatch — wires all of the above                                                                                                    |
| `qwertty-term-ffi`        | C ABI over the stack                                                                                                                                                                                                                                |
| `vt-diff`                 | differential harness: `Oracle` trait, `ReferenceTerminal` (libghostty-vt FFI) vs `RustTerminal`, a corpus, and reply-byte diffing                                                                                                                   |
| `xtask`                   | codegen (`gen-unicode`, `gen-nerd-constraints`)                                                                                                                                                                                                     |
| `spike` / `spike-runtime` | pre-rewrite prototypes, kept as scaffolding                                                                                                                                                                                                         |

Load-bearing design decisions (each in `docs/adr/` — read before deviating): **threads +
polling, not tokio** for termio (ADR-002); **IOSurface-on-CALayer** presentation, not
CAMetalLayer; **frozen renderer wire structs**; **TOML config** as a deliberate deviation
(with a `+import-ghostty-config` converter); **`slow_runtime_safety` integrity checks are an
opt-in Cargo feature**, not `debug_assertions` (ADR 0001); **Linux via a software
`GpuBackend` + FreeType**, headless-first (ADR 003); `Engine<B: GpuBackend = Metal>`
genericizes the renderer with no app-crate ripple.

## Porting Ghostty (semantics that bite)

This is a byte-faithful port; the differential oracle (`vt-diff`) is the referee, and new
engine behavior does not merge without a corpus case. Verify semantics in the Zig source and
cite `file:line` in the PR. Hard-won gotchas (all have caused real bugs):

- **Zig `assert` evaluates in ReleaseSafe** (what Ghostty ships). Mapping it to Rust
  `debug_assert!` drops the expression in release builds — never put a side-effecting call
  inside `debug_assert!`; bind the result first. The **release lane** exists to catch this.
- **Numeric truncation is load-bearing.** Replicate Zig's exact `@intFromFloat`/cast
  truncation in ported control-flow math (an off-by-truncation caused an infinite loop).
- **Zig's valid zero-capacity/zero-dimension cases become Rust slice-bounds panics** — audit
  ported probe/growth/index paths for explicit guards.
- Ghostty's runtime safety checks are on in what users run; ours were not, so field-only
  bugs slip past an all-debug gate — hence the release + paranoid lanes above.

## Local Project Rules

- Rust, edition 2024. Follow existing style; keep changes small, atomic, reviewable.
- Keep trunk compilable and green; port Zig inline tests with each module.
- Track port/test/analysis status in `docs/port-status.md`; deviations from Ghostty's design
  get ADRs in `docs/adr/`. The product name is **qwertty-term** (trademark) — never put
  "ghostty" in user-facing strings, crate names, or binaries; upstream attribution stays.
- Preserve unowned human or agent work. Report validation evidence in handoffs, not
  confidence language.

## Shared Development Preferences

This repo carries a local copy of shared development guidance in `docs/development/`. Use
this repo's local rules first; when local guidance is silent, use the shared guidance as a
fallback.

Entry points:

- `docs/development/snippets/agents/rules.md`: generated single-file reviewed rule pack.
- `docs/development/rules/README.md`: generated index for reviewed rule domains.
- `docs/development/bootstrap-downstream.md`: instructions for refreshing/merging this guidance.
- `docs/development/README.md`: local map for development guidance.
- [Software Practices](https://www.joshka.net/practice/): canonical rendered reference.

Refresh copied guidance with `python3 docs/development/update.py`. If a shared rule causes
friction or seems wrong for most Rust or agent work, capture that feedback for the
`development-preferences` repo instead of only patching around it locally.
