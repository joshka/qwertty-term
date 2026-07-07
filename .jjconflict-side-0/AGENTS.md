# Agent Guidance

**Start here: `docs/rewrite-prompt.md`** — the driving prompt for this project (a full Rust
rewrite of Ghostty). Read it, then `docs/roadmap.md` and `docs/handoff.md` for current state,
then continue from the next incomplete milestone. This file only covers repo mechanics.

## Repo layout and version control

- Version control is **jj** (colocated git). The repo root (`~/local/ghostty-rs`) holds only
  `.git`, `.jj`, and `work/`. All checkouts live under `work/`:
  - `work/default` — the integration workspace (trunk). Integrate and commit here.
  - `work/<chunk>` — one jj workspace per parallel work chunk (see the parallel execution
    model in the rewrite prompt). Created/retired by the orchestrating session.
  - `work/qwertty/` — shared drop-box with the qwertty project. NOT a jj workspace; never
    `jj workspace add` over it, never track its files.
- Advance `main` with `jj bookmark move main --to <rev>` after landing work on trunk.
- Cargo workspace: `crates/*` + `xtask`. `crates/ghostty-vt` is the terminal core (Phase 1);
  `crates/spike` is the pre-rewrite prototype kept as scaffolding and Phase-2 debug frontend.

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
cargo check --workspace
cargo fmt --check
cargo clippy --workspace
cargo test --workspace
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
