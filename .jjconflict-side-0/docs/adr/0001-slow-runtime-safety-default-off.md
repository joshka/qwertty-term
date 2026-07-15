# ADR 0001: `slow_runtime_safety` integrity checks are opt-in (default off)

Status: accepted (2026-07-07)

## Context

Upstream Zig gates the expensive terminal-state integrity checks —
`Page.assertIntegrity`, `PageList.assertIntegrity`, `Screen.assertIntegrity`,
and `pauseIntegrityChecks` — behind a dedicated `slow_runtime_safety` build
option (`src/terminal/build_options.zig`), which `src/build/Config.zig`
defaults to **true for Debug** optimize mode and false for all release modes.

The Rust port originally used `#[cfg(debug_assertions)]`, the closest direct
analogue. That made every `cargo test` run the full O(rows×cols) page scan
(including a grapheme-map lookup per cell) after **every** mutating page
operation. Measured on 20,000 prints into an 80×24 terminal (debug build):

| Workload            | checks on | checks off |
| ------------------- | --------- | ---------- |
| ASCII `a`           | 69 ms     | 1.6 ms     |
| CJK wide `中`       | 310 ms    | 3.2 ms     |
| Grapheme `a`+U+0301 | 6.7 s     | 7.5 ms     |

Upstream tolerates Debug-default-on because its unit tests are small. This
port's differential/stress tests (e.g. `print_slice_differential_fuzz_vs_print`)
push orders of magnitude more mutations, and had already been scaled down
below upstream's op counts just to stay runnable.

## Decision

Expose the checks as a Cargo feature `slow_runtime_safety` (named after the
upstream build option for discoverability), **off by default in all
profiles** — including debug/test. Opt in with:

```bash
cargo test -p qwertty-term-vt --features slow_runtime_safety
```

`Page::verify_integrity` remains compiled and callable unconditionally, so
tests that want an explicit integrity check at a specific point call it
directly (`.verify_integrity().expect(...)`) instead of relying on the
feature-gated `assert_integrity`.

## Consequences

- Default `cargo test -p qwertty-term-vt` no longer exercises the per-mutation
  integrity scan; a paranoid CI job (or local run) must pass
  `--features slow_runtime_safety` to get upstream-Debug-equivalent coverage.
- This is a deliberate deviation from upstream's Debug default, trading
  blanket per-mutation checking for test throughput; the full check suite is
  one flag away rather than deleted.
- Grapheme/stress tests can be restored toward upstream op counts.
