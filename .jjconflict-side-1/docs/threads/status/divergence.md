# divergence status — CLOSED

- **Current item:** DONE — divergent-change pass + merged-branch cleanup complete. Thread closed.
- **Last merged:** closeout PR **#240** (merged by Josh, `3926923f`). Follow-up status
  finalization tracked in its own PR (this edit).
- **Blockers:** none. Two branches deliberately left for a Josh decision (see below) — both
  harmless to leave, not blocking anything.
- **Claims:** none (released)
- **Inbox:** (other threads append requests here)

## Session outcome (final)

- **Divergent changes:** 26 → collapsed to live-churn only (self-heals as threads' branches
  auto-delete). 3 unprotected off-trunk leaf duplicates abandoned by hand; the rest cleared
  as a side effect of the branch cleanup below.
- **Merged-branch cleanup (Josh-authorized):** deleted **35** stale remote branches — 24
  fully-merged PR branches (verified merged-PR + `git cherry` patch-id-contained in `main` +
  no open PR) and 11 stale `release-plz-*` retry artifacts. Remote branch count **43 → ~6**.
  GitHub "delete head branch on merge" is now ON, so this stays tidy going forward.
- **Left for Josh (not deleted):** `integrate/agents-full` (superseded AGENTS draft, no
  merged PR) and `linux/fontconfig-discovery` (PR merged but a commit was pushed post-merge;
  linux thread active). Both safe to leave; delete only if Josh/owners confirm.
- **Nothing lost, nothing protected moved by this thread.** `main == main@origin` throughout.

## Result

One clean pass, per the method. `trunk()` advanced mid-pass (a concurrent `integrate`
merge fetched in), so numbers are stamped against the final trunk `f16d6114`
(== `main` == `main@origin`).

- Divergent at start: **26** (13 change-ids × 2 sides).
- **Abandoned: 3** unprotected off-trunk leaf duplicates (each had a canonical twin on trunk;
  each verified: no bookmark, no descendants, not any workspace `@`).
- Divergent at end: **22** — every remaining one verified protected (see below).

### Abandoned (safe — canonical twin on trunk, unprotected leaf)

| change-id  | abandoned commit | canonical twin (on trunk) | description                                            |
| ---------- | ---------------- | ------------------------- | ------------------------------------------------------ |
| `ssrrvmrs` | `e3bcad48`       | `55d3530a`                | chore: stop tracking default.profraw                   |
| `momuqosx` | `3ceab173`       | `d668b74b`                | font(shaper): platform-agnostic shaper (ADR 003 P2 s2) |
| `lxopoprq` | `efc68189`       | `2c51fb25`                | font: fix clippy unused-import non-macOS               |

### Left behind — protected (bookmark target), self-heals when the bookmark is forgotten / pushed-deleted

Each is the off-trunk pre-merge twin of a commit already on trunk; the canonical side is kept.
Not abandoned because a bookmark still points at it (method: bookmark target = protected).

| change-id  | off-trunk commit | held by bookmark                            | on-trunk twin                  |
| ---------- | ---------------- | ------------------------------------------- | ------------------------------ |
| `vpqnpsmu` | `89609d12`       | `integrate/autonomy` (local)                | `f16d6114` (== trunk tip/main) |
| `ytkrkrlv` | `6d655e43`       | `integrate/agents-guide@origin`             | `a6afe1cf`                     |
| `qprkvzrt` | `3299881c`       | `integrate/agents-full@origin`              | `9d1632fa`                     |
| `owstvtrv` | `387ed25e`       | `cursor-park-status-close` (local+origin)   | `c98a9636`                     |
| `twkyttzw` | `ac1f5e5c`       | `changelog/status@origin`                   | `b5bcc145`                     |
| `kqqsksrm` | `c0148f79`       | `release-plz-2026-07-13T10-50-28Z@origin`   | `0123944b`                     |
| `mmlrrylw` | `20a22bbf`       | `integrate/jj-discipline@git+origin`        | `87b170b7`                     |
| `wlonymvo` | `5cb5eef2`       | `integrate/app-readme@git+origin`           | `ecc4a9c7`                     |
| `qktlynpy` | `4cf04baf`       | `integrate/port-status-snapshot@git+origin` | `52f294e5`                     |
| `vkukrtsv` | `37b213e9`       | `t1/doomfire-grid-match@origin`             | `f4ea7cdb`                     |

Cleanup for these is outward-facing / cross-territory (`jj bookmark forget` on the merged
`integrate/*` bookmarks, `jj git push --deleted`, or the origin ref being pruned) — **not**
this thread's job. Left for the bookmark owners / Josh.

### Left behind — both sides on trunk (unsafe to touch)

| change-id      | commits                 | why left                                                                                                                                                   |
| -------------- | ----------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `ltqlnolslzsk` | `fa589878` + `5fef6ae6` | Both are ancestors of trunk (two distinct t5 docs commits sharing a change-id, both legitimately merged). Abandoning either rewrites trunk history. Leave. |

## Final verification

- `main` == `main@origin` == `f16d6114` ✓ (trunk advanced legitimately via a concurrent
  `integrate` merge fetched in mid-pass; `main@git` lags by 7 — normal colocated-HEAD lag,
  not touched).
- Divergence reduced 26 → 22; the 3 targeted duplicates collapsed (change-ids
  `ssrrvmrs`/`momuqosx`/`lxopoprq` gone from `divergent()`, commits `hidden=YES`), canonical
  twins intact on trunk.
- **No protected commit moved by me.** My only mutations: 3 abandons (verified leaf +
  unbookmarked + not-a-workspace-`@` before each) + snapshotting this status file. Other
  workspaces' `@` shifted (app-tails, integrate, issues, vt-tails) purely from their own
  concurrent commits (visible as interleaved ops in the shared op log) — expected, anticipated
  by the brief, and unrelated to my ops.
- Residual 22 self-heals as the `integrate/*` and other merged bookmarks are forgotten /
  their deleted-remote refs pruned, and as running threads rebase post-merge. Did **not** loop
  to chase it (per constraint: one clean pass).

## Merged-branch cleanup (2026-07-14, Josh-authorized)

GitHub "delete head branch on merge" was off, so merged PR branches piled up. Josh turned
it on (future merges) and authorized a one-time backfill. Deleted **24** remote branches,
each triple-verified: **merged PR** (`gh pr list --state merged`) + **fully contained in
`main`** (`git cherry origin/main` → 0 unmerged, reliable because the repo rebase-merges so
patch-ids are preserved) + **no open PR** + not `main`/mine. Pruning them cascaded jj to
abandon 18 now-unreachable commits (the divergent twins), so **divergence collapsed 26 → 7**.

Deleted: `app-tails/window-chrome`, `changelog/status`, `cursor-park-status-close`,
`integrate/{agents-guide,agents-refresh,app-readme,autonomy,handoff-orch,jj-discipline,port-status-snapshot}`,
`issues/triage-closeout`, `release-plz-2026-07-13T10-30-11Z`, `release-plz-2026-07-13T10-50-28Z`,
`t1/{doomfire-grid-match,pagelist-bounds-fixes,vtebench-refresh,wide-print-slice-fill,wip-scroll-region}`,
`t2/{r7-url-hover,t7-pr3-signoff}`, `t4/mouse-behaviors`, `t7/adr`, `vt-tails/{xtgettcap,xtwinops}`.

### Deliberately NOT deleted (flag for Josh)

- `linux/fontconfig-discovery` — PR merged but `git cherry` shows **1 commit not in main**
  (a commit pushed to the branch *after* merge; linux thread still active on
  `linux/fontconfig-wire` #248). Left — linux thread should confirm before deleting.
- `integrate/agents-full` — **no merged PR** and 1 unmerged commit. It's the superseded
  "comprehensive" AGENTS draft; the "refresh" version (`9d1632fa`) is what landed. Josh's
  call whether the draft has salvage value or should be dropped.
- **11 stale `release-plz-*` retry branches** (`…T10-07-55Z` … `…T10-29-14Z`) — release-plz
  force-push retry artifacts, no merged PR, each carries only a superseded auto-generated
  version-bump commit. Pure automation cruft, safe to delete, but not "merged branches" —
  left for a one-line Josh yes (or a `release-plz`-side cleanup).

### Residual divergence (7 — all correctly left)

- `lotqummlmwqo` — vt-tails/config-toggles #249 just merged (twin == main tip); self-heals on
  next prune (branch already auto-deleted).
- `znmtvkrvtnxn` — **live** linux fontconfig work, unmerged; leave.
- `qprkvzrtlvqo` — the `integrate/agents-full` superseded draft above; clears if that branch is dropped.
- `ltqlnolslzsk` — two distinct t5 docs commits sharing a change-id, both on trunk; permanent/cosmetic.

## Log

- 2026-07-14: session start; created workspace `divergence` off main; `jj git fetch`
  (nothing changed).
- 2026-07-14: built protected set (7 workspace `@`, `::trunk()`, all bookmark targets);
  enumerated 26 divergent (13 change-ids).
- 2026-07-14: abandoned 3 unprotected off-trunk leaf duplicates (`e3bcad48`, `3ceab173`,
  `efc68189`), one at a time, re-checking `divergent()` after each.
- 2026-07-14: concurrent `integrate` merge advanced trunk `a6afe1cf` → `f16d6114` mid-pass
  (new `vpqnpsmu` divergence appeared — protected, left).
- 2026-07-14: verified main==main@origin, twins intact, no protected `@` moved by me. One
  clean pass done. CLOSED — respawn only if a fresh divergence sweep is requested.
