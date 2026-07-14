# Agent Guidance

**Start here: `docs/rewrite-prompt.md`** (the constitution for this full Rust rewrite of
Ghostty), then `docs/threads/README.md` (the parallel-thread model + PR/gate/status
protocol), then `docs/handoff.md` / `docs/port-status.md` / `docs/feature-coverage.md` for
current state. This file only covers repo mechanics.

## Repo layout and version control

- Version control is **jj** (colocated git). The repo root (`~/local/ghostty-rs`) is a bare
  store — it holds `.git`, `.jj`, `work/`, and an `AGENTS.md`, but **no checkout** and is NOT
  a jj workspace. Never run jj/git/cargo at the root (it re-creates a phantom workspace that
  snapshots everything — see the root `AGENTS.md`).
- All work happens in per-workspace checkouts under `work/<id>`. Create one from an existing
  checkout: `cd work/josh && jj workspace add ../<id> --name <id> --revision main`. One
  writer per checkout; stay in your own, never touch a sibling's.
- **jj discipline** (full text in `docs/threads/README.md`): `jj st` after every edit burst
  to snapshot; if the working copy goes stale just `jj workspace update-stale` (it snapshots
  first — nothing is lost) and recover via `jj op log`; never fall back to git plumbing or
  scratchpad copies.
- **Ship via the PR pipeline** (`docs/threads/README.md`): `jj describe` → push a bookmark →
  `gh pr create` → merge (which advances `main`). Small doc-only changes may land direct to
  main. `trunk()` (== `main`) is the integration point; keep it green.
- Cargo workspace: `crates/*` + `xtask` + `examples/*`. `crates/qwertty-term-vt` is the
  terminal core; `crates/spike` is the pre-rewrite prototype kept as scaffolding.

## Local Project Rules

- Rust, edition 2024. Follow existing style; keep changes small, atomic, and reviewable.
- Keep trunk compilable: `cargo check --workspace` must pass at every integration point.
- The Zig source at `~/local/ghostty` is the spec; port its inline tests with each module.
- Track port/test/analysis status in `docs/port-status.md`; deviations from ghostty's design
  get ADRs in `docs/adr/`.
- Preserve unowned human or agent work.
- Report validation evidence in handoffs instead of confidence language.

## Validation

```bash
cargo check --workspace --all-targets
cargo fmt --check
cargo clippy --workspace --all-targets
cargo test --workspace
cargo test -p qwertty-term-vt --release --all-targets   # release lane — never skip
```

For Markdown changes: `markdownlint-cli2 "**/*.md"`.

## Shared Development Preferences

This repo carries a local copy of shared development guidance in `docs/development/`.
Use this repo's local rules first. When local guidance is silent, use the shared guidance as a
fallback.

Entry points:

- `docs/development/snippets/agents/rules.md`: generated single-file reviewed rule pack.
- `docs/development/rules/README.md`: generated index for reviewed rule domains.
- `docs/development/bootstrap-downstream.md`: instructions for refreshing and merging this guidance
  into a downstream repo.
- `docs/development/README.md`: local map for development guidance.
- [Software Practices](https://www.joshka.net/practice/): canonical rendered reference for guides,
  rules, patterns, principles, mechanisms, and tags.

Refresh copied guidance with:

```bash
python3 docs/development/update.py
```

If a shared rule causes friction or seems wrong for most Rust or agent work, capture that feedback
for the `development-preferences` repo instead of only patching around it locally.
