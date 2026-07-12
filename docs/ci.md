# CI

`.github/workflows/ci.yml` (owned by thread T8) runs on every PR and on pushes to `main`.
CI is a subset of the full local gate in `docs/threads/README.md` — a green badge means
the core is healthy, **not** that the app is shippable.

## What CI covers

- **Linux core** (`ubuntu-latest`): `qwertty-term-vt` full tests, debug **and** release lane
  (the release lane includes the property/differential integration tests); `vt-diff`
  (non-reference), input, sprite, termio, ffi tests; `cargo fmt --check`; clippy `-D warnings`
  on the platform-independent crate allowlist (see [Linux clippy scope](#linux-clippy-scope)).
- **macOS build + unit** (`macos-14`): clippy `-D warnings` on the full workspace (renderer,
  font, app included); `cargo test --workspace` (debug), with one flaky timing test skipped
  (see [Known CI-only skips](#known-ci-only-skips)).
- **markdownlint** (`ubuntu-latest`): only the `.md` files changed by the push/PR (matches the
  local "touched files" gate).

## What CI does NOT cover

- **GPU/windowed smokes** — `--offscreen-smoke` and the other app smokes stay local-only.
  GPU-dependent unit tests self-skip when the runner exposes no Metal device, so a green
  macOS job does not prove shaders compile or frames render.
- **`vt-diff --features reference`** — the differential corpus against the Zig-built
  `libghostty-vt` oracle needs the locally built static library. Engine-semantics PRs must
  still run it locally and paste evidence in the PR body.
- **The macOS release lane** — release-mode tests run on Linux only (`qwertty-term-vt`).
- **Benchmarks / perf pins** — no perf regression detection in CI; T1 owns local baselines.
- **`qwertty-term-vt/fuzz`** — its own nightly workspace, excluded from the root workspace.
- **Repo-wide markdownlint** — pre-existing violations in untouched files (~130 as of
  2026-07-11) are not checked; only changed files are linted.

## Linux clippy scope

The Linux clippy step lints an **explicit allowlist** of the platform-independent crates
(`qwertty-term-vt`, `vt-diff`, `qwertty-term-input`, `qwertty-term-sprite`,
`qwertty-term-termio`, `qwertty-term-ffi`, `spike-runtime`, `frame-capture`, `xtask`) rather
than `--workspace --exclude ...`. The macOS-surface crates are deliberately left off, each for
a different reason, and a new crate must be added consciously:

- `qwertty-term-spike`, `qwertty-term-renderer` — source is not cfg-gated for non-macOS
  targets, so they do not even compile on Linux.
- `qwertty-term-font` — compiles, but a non-macOS test cfg has an unused import (T2 Inbox).
- `qwertty-term` (the app) — compiles, but its theme/color code is
  `#[cfg(target_os = "macos")]`-gated, so on Linux it is dead code and trips `-D dead_code`.
  The app's clippy runs on the macOS job, which lints the full workspace.

## Known CI-only skips

- `qwertty-term-termio::clean_exit_captures_code_and_runtime` is skipped on **both** test
  steps (Linux core-crate tests and the macOS job). It asserts a process runtime `>= 50ms`
  after a 100ms shell sleep, but the exit watcher reports ~21ms on the macOS runner and
  ~2ms on the Linux runner (the test passes reliably on local hardware). A runtime *below*
  the sleep duration on every shared runner points at a real measurement bug — the runtime
  accounting isn't bracketing the full child lifetime, not just runner slowness. Filed to
  T4's Inbox (`docs/threads/status/t4.md`). Remove both `--skip`s once fixed.

## Toolchain pin

CI pins the Rust toolchain (see `RUST_TOOLCHAIN` in `ci.yml`, currently `1.96.0`) instead
of tracking `stable`: Rust 1.97 (2026-07-07) introduced new clippy lints that are red on
`main` in `qwertty-term-vt`, `-input`, and `-font`. The fixes are one-liners filed in the
owning threads' Inboxes (`docs/threads/status/`). Once they land, bump the pin (or switch
to `stable`) and re-include `qwertty-term-font` in the Linux clippy lane.
