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

## Shared invariants (every thread, non-negotiable)

### jj discipline (hard-won; violations have eaten work)

- ONE writer per checkout. Never run jj at the repo root. Every command block that runs
  jj or reads repo files starts with an explicit `cd .../work/<id> &&` — the shell cwd
  resets silently between commands.
- Before `jj workspace update-stale`: back up un-snapshotted edits (`command cp` — cp/rm
  are interactive-aliased); verify edits survived after. Run a trivial `jj st` right after
  editing files so they snapshot early.
- Verify commits are NON-empty after describe (`jj log -r @ -T 'if(empty,...)'`) — empty
  commits have silently shipped twice. Verify lint/gate BEFORE describing, not after.
- Do not touch sibling workspaces' files or `work/josh`. See `docs/orchestration.md`
  (failure-recovery section) for the full playbook; `~/local/jj/work/` has a deeper
  root-cause thread pending.

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
