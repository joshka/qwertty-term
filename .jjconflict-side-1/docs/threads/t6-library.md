# T6 — Library & publish thread

**Model:** Sonnet (escalate API-design questions to Josh via status file before shipping
breaking choices) · **Wave:** 2 · **Workspace:** `work/t6` · **Status:** `status/t6.md`
**Territory:** public API surface of the published crates, `examples/`, release/versioning
mechanics, crate docs. Internal implementations belong to their owning threads —
API-shaping PRs should be thin wrappers/re-exports/docs, with file-claims for signature
touch-ups. Rules: `docs/threads/README.md`.

## Mission

The crate family is LIVE on crates.io at 0.1.0 (all 8: `qwertty-term` + `-vt/-font/
-renderer/-termio/-input/-sprite/-ffi`, docs.rs built). This thread makes the library
story worthy of that: polish the public API (semver-consciously — it's published now),
ship 0.1.x/0.2 releases, and land the betamax integration track (MB1/MB4) that
embeddability exists FOR (betamax at `~/local/betamax` is the named reference consumer).

## Backlog

- [ ] **MB5 API polish** (M, FIRST — from MB2's findings list, verify each against the
      published 0.1.0 surface before assuming unfixed):
      (1) `Display`/`std::error::Error` on font error types; (2) matched
      `(Engine, Grid)` construction (or Engine-reads-Grid-metrics) killing the cell-size
      desync footgun; (3) pixel-format-honest readback (`draw_frame_rgba` or typed
      buffer); (4) one-call `render(snapshot, grid, opts)`; (5) `Stream::terminal()`
      accessor; (6) `capture_live()` alias. Additive where possible → 0.1.1; breaking →
      queue for a single 0.2.0 batch (semver discipline: never dribble breaks).
- [ ] **docs.rs quality pass** (M): crate-level docs with the embeddability quickstart
      (frame-capture inlined as doctest-ish example), README per crate, `#[deny(missing_
      docs)]` on -vt public items (stretch), intra-doc links. docs.rs badge/flags.
- [ ] **Injectable clock completion** (S/M): thread the remaining time sources (blink,
      pacing seams) through the existing injectable-clock seams; deterministic-render
      test proves it.
- [ ] **MB1 liaison** (ext): the betamax-side swap to `qwertty-term-vt` runs as its own
      session in the betamax repo (`work/betamax-thread-prompt.md` — refresh it for the
      rename + published crates first!). This thread is the qwertty-term-side contact:
      answer its Inbox, land API accommodations it surfaces.
- [ ] **MB4** (M): betamax renders via `qwertty-term-renderer` offscreen on macOS —
      likely mostly example/doc work here + fixes routed from the betamax session.
- [ ] **Release mechanics** (S, recurring): changelogs (T8 drafts), version bumps,
      `cargo publish` dry-runs in dependency order, git tags. Publishing itself:
      Josh-approved per release (status file PENDING-APPROVAL), since it's public and
      irreversible.
- [ ] **qwertty (the library) coordination** (S): `joshka/qwertty` consumes/oracles
      against these crates per the collab docs — refresh `work/qwertty/ghostty-rs-collab.md`
      for the rename + published-crate reality; headless conformance-target ask goes to
      T5's Inbox when their side is ready.

## Method rules

Published-crate discipline: additive-only in 0.1.x; breaking changes batched, documented
in CHANGELOG with migration notes, and Josh-approved. Every API change lands with a
doctest or example update proving the ergonomic claim. `cargo semver-checks` if
installable; else manual review noted in PR. Never publish without Josh's explicit go.

## Definition of done

MB5 list cleared (shipped or explicitly rejected with reason); 0.1.1 (or 0.2.0) published
with docs.rs green; betamax builds on the published (or path-pinned) crates and renders
through our stack on macOS; feature-coverage.md embeddability section fully `[x]`.
