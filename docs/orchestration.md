# Orchestration playbook

How this project is run, distilled from the first two days (~25 chunks landed). Written for
the orchestrating session (any model tier) — follow it mechanically; deviate only with
reason. The rewrite prompt (`docs/rewrite-prompt.md`) is the constitution; this is the
operations manual.

## The loop

1. Pick the next chunk(s) from `docs/roadmap.md` (respect dependency spines; keep 3–6 in
   flight; prefer chunks whose files are disjoint from running siblings).
2. `jj workspace add ../<chunk-name>` from `work/default` (it bases on current main).
3. Launch a background agent with the standard prompt shape (below). Model: Sonnet for
   mechanical/well-specified, Opus for ordinary porting/design, top tier only by exception.
4. On the completion notification: integrate (below), update ledgers, launch the next chunk.
5. On a failure notification: recover (below). Never re-run work that's on disk.

## Standard chunk prompt shape

Every chunk prompt MUST contain, in this order:

- Mission sentence (what + why it matters now).
- MANDATORY block: work ONLY inside `/Users/joshka/local/ghostty-rs/work/<name>/`; never
  touch `work/default` or siblings; NAME the running siblings and their file territories;
  no `jj workspace`/`jj bookmark` commands; **NO background tasks or waits — foreground
  only**; Miri (if applicable) foreground, bounded, LAST step, skip tests >~3 min and name
  each skip.
- Read-first list (analysis docs, then code, then Zig refs with commit `2da015cd6`).
- Numbered task list with the analysis doc FIRST (`docs/analysis/NAME.md`, commit-stamped,
  line-referenced). Port ALL inline tests 1:1 (state exceptions explicitly); count Zig vs
  Rust per file.
- Gates: `cargo check --workspace` after every edit burst; full test suite; fmt; clippy
  clean on touched crates; markdownlint if docs touched.
- Finish: `jj describe -m "<milestone>: <summary>"` from the workspace dir.
- Return: an explicit report checklist (counts, decisions, bugs found, deferrals).
- If the chunk is large: a priority ladder ("if budget runs short, do X > Y > Z and leave a
  PROGRESS note in the analysis doc").

## Integration recipe (run from work/default, NEVER from the repo root)

```sh
CH=$(jj workspace list | grep <name> | awk '{print $2}' | tr -d ':')
jj workspace forget <name> && rm -rf /Users/joshka/local/ghostty-rs/work/<name>
jj rebase -r "$CH" -d main
jj log -r "$CH" --no-graph -T 'if(conflict, "CONFLICT", "clean")'
# if CONFLICT: jj new "$CH"; fix files (see conflict notes); jj squash
jj bookmark move main --to "$CH" && jj new main
# GATE (all must pass BEFORE the bookmark stays):
cargo check --workspace && cargo test --workspace && cargo fmt --check
cargo test -p vt-diff --features reference    # when engine code changed
markdownlint-cli2 "**/*.md" "!target"          # when docs changed
# then: ledger row update + roadmap checkbox, one commit, move main again
```

Conflict notes: `crates/ghostty-vt/src/lib.rs` module lists conflict often — resolve as the
UNION of `pub mod` lines, sorted (a python one-liner extracting `^pub mod \w+;` and
deduping is reliable). Ledger/table edits: aligned-table padding defeats naive string
matching — edit by REGEX row-key match (`re.match(r'^\| H +\|', line)` — padded columns defeat exact
`startswith`), ASSERT the replacement count (an unconditional "ok" print has masked silent
no-op edits twice), then rerun
`python3 scripts/align_md_tables.py <file>` and lint. **Verify lint shows 0 errors BEFORE
committing** — the recurring orchestrator mistake is commit-then-check.

## Failure recovery (all proven, in order of frequency)

1. **Background-wait stall** ("Agent stalled: no progress for 600s", usually Miri): the work
   is done; SendMessage the agent: check the background run's output or rerun bounded
   foreground, then gate + describe + report. Works every time.
2. **Delegate-and-idle** (agent spawns a sub-agent then stops "waiting"): SendMessage: verify
   the sub-agent's work yourself, finish the remainder in the foreground, no further
   delegation. If a stray CHILD surfaces asking for scope confirmation, stand it down —
   one actor per workspace.
3. **Hard death** (connection error, token exhaustion, policy false-positive): inspect the
   workspace (`jj st`, `cargo check/test`), then launch a CONTINUATION agent (fresh, usually
   one tier cheaper) whose prompt lists exactly what's on disk, what compiles, what remains.
   The analysis doc is the recovery map — this is why analysis-first is non-negotiable.
4. **Phantom root workspace**: if jj reports stale/divergent state mentioning the repo root,
   someone ran jj/git at `~/local/ghostty-rs` (editors do this). The bad snapshot looks like
   "everything deleted + work/ files added". `jj abandon` it; never run jj at the root.

## Standing invariants

- Trunk (`work/default`) is always green; a red gate blocks the bookmark, full stop.
- `docs/port-status.md` is orchestrator-owned (agents never edit it; prevents 3-way
  conflicts). Same for roadmap checkboxes.
- Every engine change re-runs the differential suite; parity is the product.
- `work/qwertty/` is a drop-box, never a workspace. `work/upstream/` holds issue drafts.
- Chunk agents get the sibling-territory list VERBATIM — most cross-chunk conflicts were
  prevented by this, and the one real collision (osc color duplication) happened where
  territories were fuzzy.
- Perf changes cite the committed bench
  (`cargo test -p vt-diff --features reference --release -- --ignored --nocapture throughput`)
  against the ReleaseFast reference (`mise exec zig@0.15.2 -- zig build -Demit-lib-vt=true
  -Doptimize=ReleaseFast` in `~/local/ghostty`).
