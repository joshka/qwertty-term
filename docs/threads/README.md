# Parallel thread plan — completing the port

Authored 2026-07-10 (Fable planning pass). This directory turns the remaining port work
(`docs/roadmap.md` + `docs/feature-coverage.md`) into **parallel Claude Code threads**:
long-lived sessions, each with its own jj workspace, territory, model tier, and the full
authority to ship — write code, run gates, push branches, open and merge PRs, and keep its
status file current. Threads replace the single-orchestrator/sub-agent model for the long
tail; each thread may still spawn its own sub-agents internally.

## Thread roster

| ID  | Thread                | Model  | Wave | Territory (crates/dirs)                                  | Spec                    |
| --- | --------------------- | ------ | ---- | -------------------------------------------------------- | ----------------------- |
| T1  | Performance           | Opus   | 1    | `qwertty-term-vt` (perf), `scripts/`, `docs/benchmarks/` | `t1-perf.md`            |
| T2  | Renderer completeness | Opus   | 1    | `qwertty-term-renderer`, renderer-side of `-font`        | `t2-render.md`          |
| T4  | App polish            | Opus   | 1    | `crates/qwertty-term` (the app)                          | `t4-app-polish.md`      |
| T8  | Ops & upstream        | Sonnet | 1    | CI, `docs/upstream/`, bench upkeep, no product code      | `t8-ops-upstream.md`    |
| T3  | Config & keybinds     | Opus   | 2    | config + keybind system + option wiring (app-wide)       | `t3-config-keybinds.md` |
| T5  | VT completeness       | Opus   | 2    | `qwertty-term-vt` (features), `vt-diff`                  | `t5-vt-complete.md`     |
| T6  | Library & publish     | Sonnet | 2    | public API surface, `examples/`, crates.io releases      | `t6-library.md`         |
| T7  | Linux                 | Opus   | 3    | GTK/OpenGL/fontconfig (ADR first — parked)               | `t7-linux.md`           |

**Waves exist to keep crate ownership disjoint.** Wave 1: vt=T1, renderer=T2, app=T4.
Wave 2 (starts as Wave-1 threads drain): app=T3, vt=T5, font/library=T6. Never run two
threads that own the same crate concurrently; anything cross-crate goes through the
file-claim protocol below. 3–4 active threads max — repo contention and human attention
are the real limits, not compute.

Model rationale: Opus for anything requiring design judgment against upstream semantics
(perf forensics, Metal, Binding.zig, engine features, AppKit subtlety). Sonnet where the
spec already encodes the decisions (release mechanics, CI boilerplate, issue liaison).
Threads spawn Sonnet sub-agents for mechanical items regardless of their own tier, and may
spawn Opus sub-agents for a hard item if the thread itself is Sonnet — tier the WORK, not
the seat.

## Launching a thread

```sh
# from an EXISTING workspace checkout (never the repo root):
cd ~/local/ghostty-rs/work/josh
jj workspace add ../<id> --name <id> --revision main
cd ../<id> && claude --model <opus|sonnet>
# first message: Read docs/threads/<spec>.md and docs/threads/README.md, then begin.
```

Threads may also be spawned as Claude Code background-task sessions (chips); the session's
first action is then to create/enter its jj workspace exactly as above, regardless of the
directory it wakes up in.

The spec file is the thread's constitution. On session death/limit, relaunch the same way;
the status file + jj describe + pushed branch are the durable state — a fresh session must
be able to resume from those three alone.

## Resume protocol (the standard nudge)

When told to continue (or just "go"), run this loop — don't wait for item-by-item
direction:

1. Read your `status/<id>.md`, then skim every sibling's **Blockers** and **Inbox** lines.
2. Process your **Inbox** first — you may be someone's blocker; unblocking others is
   higher priority than your own next item.
3. Pick the top **unblocked** backlog item. If your top item is gated, drop to the next
   collision-free one — **never idle**.
4. Ship it via the PR pipeline; self-merge when the gate is green per the merge policy.
5. Update your status file (current item / last merged / blockers / inbox triaged).
6. If genuinely blocked on everything, write **who** you're waiting on in your Blockers
   line and stop — don't spin. Otherwise loop until your backlog drains.

**Who to nudge when the fleet stalls** (heuristic, for the orchestrator/human): grep the
**Blockers** line across `status/*.md`. `none` → that thread just needs the standard loop
above. A line naming another thread → nudge the **named** thread (the unblocker) with the
specific ask, not the blocked one. A red shared CI check blocks everyone — whoever owns the
failing crate fixes it first, P0. Cross-thread asks travel as **Inbox** lines in the
target's status file (append-only; the owner triages into their backlog).

## Shared invariants (every thread, non-negotiable)

### jj discipline — working-copy safety in parallel workspaces (hard-won)

You're one of several threads editing in parallel `work/<id>` workspaces. A sibling's
`jj git fetch`/`rebase` can make your working copy stale; the next jj command reconciles it
to a "fresh commit," overwriting your on-disk edits. **This looks like data loss but isn't** —
jj snapshots un-committed changes into a saved commit *first* (it just doesn't tell you where,
pre jj#9786). Adopt, in priority order:

1. **`jj st` after every edit burst, before any long non-jj command.** THE habit — it's the
   single rule that prevents loss. The unsafe window is between "files edited" and "next jj
   command"; `cargo`/`npx`/`git` do NOT snapshot. So right after an Edit/Write burst run
   `cd .../work/<id> && jj st`, *then* run your build/test gate. This puts your tree in the
   object store so a concurrent rebase carries it losslessly.
2. **Commit incrementally on multi-step work.** `jj describe`/`commit` in stages; never
   edit-then-run-a-multi-minute-gate in one un-snapshotted block. Prefer working on a
   *described* commit (`jj new main@origin -m "<msg> [WIP]"` before editing) and pushing to a
   bookmark early — pushed work survives any local reconcile.
3. **If edits "vanish" or you see `Updated working copy to fresh commit <X>`: RECOVER, don't
   redo.** The changes were preserved. Find the saved commit and restore:
   - `jj op log` — find the `reconcile divergent operations` / `snapshot working copy` op
     around the loss; `jj log --at-op <snapshot-op> -r @` shows the (non-empty) commit it saved.
   - `jj evolog -r <change_id>` — prior versions of the change you were on.
   - `jj log -r 'all()'` + grep a unique token from your edit (a symbol/const name).
   - Then `jj restore --from <saved-commit> <files>`, or `jj rebase -r <saved-commit> -d
     main@origin` to replay your delta onto current main (3-way merge). Only redo from scratch
     if a real search turns up nothing. `jj undo` won't help after a stale reconcile
     ("Cannot undo a merge operation") — use `jj restore` / `jj op restore`.
4. **One writer per checkout.** Never run jj at the repo root or touch a sibling workspace /
   `work/josh`; every command block starts with `cd .../work/<id> &&` (shell cwd resets between
   commands). Before `jj workspace update-stale`, back up un-snapshotted edits (`command cp` —
   cp/rm are interactive-aliased) as belt-and-suspenders.
5. Verify commits are NON-empty after describe (`jj log -r @ -T 'if(empty,...)'`) — empty commits
   have silently shipped. Verify lint/gate BEFORE describing, not after.

**Correction:** any note claiming `jj workspace update-stale` *discards* un-snapshotted edits
(e.g. current `docs/orchestration.md`) is wrong — update-stale snapshots first; the edits are
recoverable via rule 3.

### Ship pipeline (jj → GitHub PR)

```sh
jj describe -m "<area>: <summary>"                  # after local gate passes
jj bookmark create <id>/<feature> -r @-             # or move an existing one
jj git push --bookmark <id>/<feature>    # jj >= 0.43 auto-tracks new bookmarks
gh pr create --base main --title "..." --body "...gate evidence..."
# merge policy below; after merge:
jj git fetch && jj rebase -d main ...               # rebase your line onto new main
```

- PR body MUST include the gate evidence block (see below) and the territory statement.
- **Merge policy**: a thread self-merges (`gh pr merge --rebase`) when (a) full gate is
  green locally on a rebase against current main, (b) no open PR or file-claim overlaps
  its files, (c) the PR sat visible ≥ one status-update cycle OR is urgent (crash/regression
  fix — say so). Otherwise leave the PR open and flag Josh in the status file. Josh can
  always review anything; visibility is the point of PRs, not ceremony.
- Small doc-only changes may land direct-to-main (bookmark move + push) — same gate rules
  minus tests.

### The gate (before every describe/PR)

```sh
cargo check --workspace --all-targets   # zero warnings
cargo test --workspace
cargo test -p qwertty-term-vt --release --all-targets   # release lane — NEVER skip
cargo test -p qwertty-term-vt --release --lib --features slow_runtime_safety
                        # paranoid lane (ADR 0001 integrity scans in release) —
                        # this combo caught a real release-only bug on 2026-07-11
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings
cargo run -p qwertty-term --release -- --offscreen-smoke  # + ALL app smokes if app touched
cargo test -p vt-diff            # + --features reference when engine semantics touched
npx markdownlint-cli2 <touched .md>
```

Engine semantic changes additionally require: differential corpus green against the
reference oracle, and new behavior gets a corpus case. Field-bug fixes require the missing
test CLASS, not just the fix. Perf changes require before/after numbers in the PR body.

### Territory & file claims

Your spec's territory is exclusive. For a file OUTSIDE it (e.g. wiring a config option
into another crate): add a line to your status file under `## Claims` naming file + reason
BEFORE editing, check other threads' status files for conflicting claims first, keep the
edit minimal, drop the claim when merged. Two threads wanting the same file at once:
whoever claimed first proceeds; the other queues the item and moves on.

### Status protocol

Each thread owns `docs/threads/status/<id>.md` (create on first session): a 5-line header
(current item, last merged PR, blockers, claims) + an append-only log line per session/
merge ("2026-07-11: merged #12 kitty-image slice 1; next: placements"). Update at minimum
at session start, before ending, and at every merge. Commit it with your PRs (it's in-repo
so every thread and Josh can read the whole board with one glance at the directory). Check
the other status files at session start — that's the coordination read.

### Upstream reference & provenance

Zig source at `~/local/ghostty`, pinned `2da015cd6` (a main-build checkout for A/B lives at
`~/local/ghostty-main`). Port rule: verify semantics in source, cite `file:line` in the PR;
Zig `assert` evaluates in ReleaseSafe (never wrap side effects in `debug_assert!`); match
numeric truncation semantics exactly (see memory notes in `docs/orchestration.md`).
New analysis goes to `docs/analysis/<area>.md`, commit-stamped. The product name is
**qwertty-term** (trademark) — never introduce "ghostty" into user-facing strings, crate
names, or binary names; upstream attribution in docs/LICENSE stays.

### Escalation

Blocked > 1 session on a design call → write the decision as a mini-ADR draft in
`docs/adr/`, put PROPOSED in your status file, continue on other backlog. Never invent a
deviation from upstream semantics without an ADR. If a fix belongs to another thread's
territory: file it as a line in THEIR status file under `## Inbox`, don't fix it yourself.
