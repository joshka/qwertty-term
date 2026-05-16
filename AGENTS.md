# Agent Guidance

Use this file as the repo-local map. Keep project-specific rules here and route to
`docs/development/` for shared guidance instead of expanding this file into a full manual.

## Local Project Rules

- This is a Rust binary crate using Cargo and edition 2024.
- Follow existing Rust style and keep changes small, atomic, and reviewable.
- Preserve unowned human or agent work.
- Report validation evidence in handoffs instead of confidence language.
- Use the validation commands below before handoff.

## Validation

Run the checks that match the files changed. For normal Rust changes, run:

```bash
cargo check
cargo fmt --check
cargo test
```

For Markdown changes, run:

```bash
markdownlint-cli2 "**/*.md"
```

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
