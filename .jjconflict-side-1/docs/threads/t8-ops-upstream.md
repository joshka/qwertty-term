# T8 — Ops & upstream liaison thread

**Model:** Sonnet · **Wave:** 1 · **Workspace:** `work/t8` · **Status:** `status/t8.md`
**Territory:** `.github/`, `docs/upstream/`, `docs/threads/` upkeep, bench-lane upkeep
scripts. **No product code** — anything discovered goes to the owning thread's `## Inbox`.
Rules: `docs/threads/README.md`.

## Mission

Keep the machine around the code healthy: CI, upstream-bug liaison, upstream-drift watch,
and board hygiene — so the code threads never stall on infrastructure and Josh never
wonders "what's the state of everything".

## Backlog

- [ ] **CI bootstrap** (M, FIRST): GitHub Actions on `joshka/qwertty-term`. Linux runner
      (free) can run the platform-independent majority: `qwertty-term-vt` full tests +
      release lane + property tests, vt-diff corpus (non-reference), fmt, clippy,
      markdownlint. macOS-only crates: `cargo check` cross-surface where possible; a
      macos runner job for build+unit (skip GPU/windowed smokes; they stay local) — keep
      minutes modest, document what CI does NOT cover. Branch-protection suggestion for
      Josh once green (his call to enable).
- [ ] **Upstream issue filing** (S + Josh gate): the 4 drafts in `docs/upstream/` are
      ready (findings-status.md tracks). For each: re-verify against CURRENT ghostty main
      (`~/local/ghostty-main`, refresh it), update the draft if upstream moved, then
      present to Josh for approval — **filing is Josh-approved per-issue, never
      automatic**. Track filed-issue URLs + outcomes in findings-status.md.
- [ ] **Upstream drift watch** (S, recurring): periodically (each session) `git -C
      ~/local/ghostty fetch` and diff `origin/main` against pin `2da015cd6` for the
      subsystems we ported (terminal/, font/, renderer/, input/). Produce/refresh
      `docs/upstream/drift.md`: notable upstream changes, which thread's territory each
      touches, severity (bugfix-we-should-mirror / feature / irrelevant). File Inbox lines
      for must-mirror fixes. This is how the port avoids silently forking.
- [ ] **Re-pin proposal** (M, later): when drift gets heavy, draft the ADR + work plan for
      re-pinning to a newer upstream commit (what re-verifies: differential corpus vs new
      reference lib, goldens, metrics pins). Proposal only — execution is a dedicated
      effort Josh schedules.
- [ ] **Board hygiene** (S, recurring): stale status files, orphaned jj heads/workspaces
      (`jj workspace list` vs `ls work/`), dangling bookmarks, feature-coverage.md drift
      vs actually-merged PRs. Fix mechanically; report anomalies in status.
- [ ] **Release notes support** (S): when T6 cuts releases, draft CHANGELOG entries from
      merged PR titles.

## Method rules

You never modify product crates — CI yaml, docs, scripts only. Upstream re-verification
must build/test against the real checkout, not memory. Anything ambiguous about whether an
upstream change matters: write it up in drift.md with your read and let the owning thread
decide. All external-facing actions (filing issues, enabling branch protection, emailing
anyone) are Josh-approved first, listed in your status file as PENDING-APPROVAL.

## Definition of done (steady state, this thread never really "finishes")

CI green badge on main covering the vt core; drift.md fresh (< a week stale); all four
findings dispositioned (filed or closed-wontfile with reason); board clean. Cadence after
initial setup: short sessions, roughly weekly or when pinged.
