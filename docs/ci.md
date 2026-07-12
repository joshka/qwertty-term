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
  (see [Known CI-only skips](#known-ci-only-skips)); plus the `qwertty-term-font` `freetype`
  feature clippy + tests (see [Optional-feature coverage](#optional-feature-coverage)).
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
`qwertty-term-termio`, `qwertty-term-ffi`, `qwertty-term-renderer`, `qwertty-term-font`,
`spike-runtime`, `frame-capture`, `xtask`) rather than `--workspace --exclude ...`. The
macOS-surface crates are deliberately left off, each for a different reason, and a new crate
must be added consciously:

- `qwertty-term-spike` — source is not cfg-gated for non-macOS targets, so it does not even
  compile on Linux.
- `qwertty-term-renderer` — **now included** (ADR 003 P1): the lib is cfg-gated (the Metal
  backend/engine/present are `#[cfg(target_os = "macos")]`) and its macOS-only tests are
  gated, so it compiles and lints clean on Linux. It has no GPU backend on Linux yet, so
  `--all-targets` clippy compiles the test code but the macOS-gated acceptance tests are
  empty there; the platform-agnostic unit tests (geometry, cells, swap-chain semaphore) do
  run. The software backend (P1 next slice) will add real Linux render coverage.
- `qwertty-term-font` — **now included**: T2 gated the non-macOS test's unused import, so it
  compiles and lints clean on Linux.
- `qwertty-term` (the app) — compiles, but its theme/color code is
  `#[cfg(target_os = "macos")]`-gated, so on Linux it is dead code and trips `-D dead_code`.
  The app's clippy runs on the macOS job, which lints the full workspace.

## Optional-feature coverage

Default `cargo clippy`/`cargo test` only build default features, so any off-by-default feature
is uncovered unless a job enables it explicitly:

- **`qwertty-term-font` `freetype`** (ADR 003 P1/P2) — off by default (the CoreText face is the
  macOS default; FreeType is the Linux/software path). The macOS job runs
  `cargo clippy -p qwertty-term-font --features freetype --all-targets -- -D warnings` and
  `cargo test -p qwertty-term-font --features freetype`. FreeType is cross-platform and builds
  via `freetype-rs`'s bundled C build (`cc`), so no system FreeType is required. Without this
  step the FreeType face path (`--features freetype`) would compile nowhere in CI and could rot
  silently.

## Known CI-only skips

None — the two former load-sensitive termio skips were resolved (T4, mirroring how the
macOS exit-code check is gated):

- `qwertty-term-termio::clean_exit_captures_code_and_runtime` now runs on both steps. The
  short CI runtimes were a *test-harness* issue, not a runtime-accounting bug: the child
  used a bare `sleep 0.1`, which isn't in a login shell's restricted PATH on the runners, so
  the shell exited "not found" in ~2ms (Linux) instead of living 100ms. It now uses
  `/bin/sleep 0.1` (present on both platforms) so the child reliably lives ~100ms, and the
  runtime *magnitude* is asserted only where there's no `/usr/bin/login` wrapper (i.e. not
  macOS — login's lifetime need not bracket the shell's, the same reason the exit code is
  gated by `LOGIN_SWALLOWS_EXIT_CODE`). Linux keeps the real `>= 50ms` assertion.
- `qwertty-term-termio::throughput_cat_10mib` now runs on both steps. It still asserts
  delivery + no-hang (90%+ of the file moved), but the noisy absolute `> 40 MiB/s` *floor*
  is asserted only off-CI (`CI` env unset); under a loaded shared runner the number is
  printed but not asserted.

## Toolchain pin

CI pins the Rust toolchain to an explicit version (see `RUST_TOOLCHAIN` in `ci.yml`, currently
`1.97.0`) instead of tracking `stable`. The point is that a new rustc release can't turn the
shared gate red mid-work for every thread — which is exactly what 1.97's new clippy lints
(`question_mark`, `useless_borrows_in_formatting`) did on 2026-07-07. T8 owns bumping the pin:
after a rustc release, verify `cargo clippy`/`cargo fmt` are green at the new version across
both lanes, then bump `RUST_TOOLCHAIN` here and in `docs/ci.md`.

History: pinned to `1.96.0` as a stopgap when 1.97 landed; bumped to `1.97.0` once the lint
fixes landed (T1 #9, T2 font fix) and clippy/fmt were re-verified green at 1.97 on both lanes.
