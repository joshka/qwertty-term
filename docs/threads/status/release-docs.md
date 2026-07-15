# release-docs status

- **Current item:** none — backlog drained, closed out
- **Last merged:** #242 (0.4.0 release, changelog) + #268 (ADR 004 ratification), both by
  Josh 2026-07-15
- **Blockers:** none
- **Claims:** released (both edits shipped)
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Log

- 2026-07-15: session start — bounded docs-only thread. Two Josh-authorized tasks:
  (1) hand-curate the 0.4.0 CHANGELOG section into release PR #242 (release-plz
  `changelog_update=false`, so it must be hand-written); (2) reword ADR 004 Status to the
  `ratified by Josh 2026-07-14` phrasing matching ADR 002/003.
- 2026-07-15: **Task 1 done.** Built the 0.4.0 CHANGELOG section from `git log
  qwertty-term-v0.3.0..origin/main` (32 commits / ~26 PRs, bookkeeping/status-doc PRs
  dropped) and PR body verification (title/body via `gh pr view`) for accurate phrasing.
  Grouped Added/Changed/Fixed, Keep-a-Changelog style, matching the existing compare-URL
  format. Landed directly onto release-plz's branch `release-plz-2026-07-14T21-21-22Z`
  (based a commit on the remote bookmark, edited `CHANGELOG.md`, moved the bookmark, pushed)
  so it's already part of open PR #242 — no separate fallback PR needed. release-plz
  regenerated that branch mid-session (old head `b16ecd8c` → new `87fd6366`, absorbing
  #266's scroll-region-fast-path commit that had just landed on main); rebased the
  changelog commit onto the new head and re-pushed — final branch head `36c160d5`, verified
  via `gh pr view 242 --json commits,files` that the changelog commit and `## [0.4.0]`
  heading are present. Re-checked `origin/main` after the rebase: still `0fb53969`, same 32
  commits — no further drift, changelog is current. **#242 is ready for Josh to
  merge/publish; did not merge or publish it.**
- 2026-07-15: **Task 2 done.** Reworded `docs/adr/004-tmux-control-mode.md` Status line to
  `ACCEPTED (ratified by Josh 2026-07-14 — ...)`, matching ADR 002/003 phrasing. Shipped as
  [PR #268](https://github.com/joshka/qwertty-term/pull/268) (docs-only, direct-to-main
  candidate — small enough for Josh to merge without a full gate per the thread README, but
  left open per this thread's non-self-merge default for a bounded/short-lived thread).
- 2026-07-15: `npx markdownlint-cli2` on all touched `.md` (ADR, status file, CHANGELOG) —
  0 errors. Closing out; backlog drained, both edits shipped, nothing else in scope.
- 2026-07-15: Josh merged both #242 and #268. #242's merge fast-forwarded `main` and fired
  the Release-plz workflow (crates.io Trusted Publishing) — `qwertty-term-sprite-v0.4.0`
  tag observed immediately; the remaining seven crate tags publish in workspace-dependency
  order as the workflow completes. Thread backlog fully drained; no further action pending.
