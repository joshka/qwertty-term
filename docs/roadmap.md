# Roadmap — milestone ladder

The organizing principle, learned from the spike and re-proven by the demo milestone: **every
phase exits with a user-visible artifact**, and depth-vs-breadth trades resolve toward
standing the whole pipeline up early. Phase 1 de-risked VT semantics (differential parity
with upstream); the remaining unknowns are concentrated where ghostty meets the platform —
fonts, GPU, FFI, app shell. Get there sooner; backfill completeness behind the moving front.

Detailed phase definitions live in `docs/rewrite-prompt.md`; per-file status in
`docs/port-status.md`. This file tracks the milestone ladder and what "done" means for each.

## M1 — Engine certified (Phase 1 exit) — IN PROGRESS

Functional core is done (parity on fixtures + hand streams, formatter differential clean,
demo window live). Remaining exit items:

- [ ] **Perf gate: ≥0.5x of ReleaseFast libghostty-vt** on the committed throughput bench
      (currently 0.14x–0.31x). Levers, in order: decode-until-control-seq fast path (the
      ascii 4x), print-path row caching, dispatch inlining. Perf pass now also hardens the
      architecture before more weight lands on it.
- [ ] **Corpus expansion**: esctest/vttest-derived sequences + captured real-app sessions
      (vim/tmux/htop/fzf) run differentially. Cheap, parallel, Sonnet-class.
- [ ] Fuzz campaign actually run (cargo-fuzz install is the only blocker) + Miri full pass.
- [ ] Tail modules: search (sliding-window core), kitty graphics exec + unicode placeholders,
      stream seams (kitty keyboard encode, XTWINOPS/title stack, mouse reporting, REP),
      snapshot gaps (OSC 52 read-back, dynamic palette), promptClickMove.
- [ ] Deferred-test backfill to agreed thresholds (Terminal 381, resize permutations).

Artifact: a "certified" report in the ledger — corpus size, perf table, fuzz/Miri evidence.

## M2 — Daily-drivable terminal (Phase 2: termio)

PTY/exec port (openpty/fork via rustix, not portable-pty), read path, write path with flow
control, process lifecycle, termios polling, shell-integration injection (scripts copied
verbatim). **Threads-vs-tokio settled by benchmark behind the mailbox seam (ADR).**

Artifact: the spike window is your terminal for an hour without pain. Exit checkpoint:
maintainer actually does this.

## M3 — Real rendering + embeddable frames (Phases 3+4, overlapped)

Fonts (CoreText discovery/rasterization, HarfBuzz shaping, fallback resolver, sprite
rasterizer — extraction candidate built as a lib from day one) and the Metal renderer
(generic core, IOSurface, triple buffering, damage tracking), replacing egui. Headless
offscreen target + RGBA readback with injectable clock/fonts lands WITH the renderer, not
after — `examples/frame-capture` in CI.

Artifact: pixel-identical-to-app frame capture, deterministic. **Pull the betamax spike
forward to here** (it was Phase 7): port betamax's rasterizer onto these crates as the first
external consumer — it validates embeddability while the API is still cheap to change, and
gives the sprite/nerd-font extraction its second consumer.

## M4 — Your config works (Phase 5: input + config + orchestration)

TOML config with full option semantics + `+import-ghostty-config`; Binding.zig port (leader
sequences, 70+ actions); kitty keyboard encode; mouse reporting; IME plumbing; App/Surface
mailboxes.

Artifact: maintainer's real ghostty config imported; keybinds and mouse behave identically
(`+show-config` diff vs ghostty clean).

## M5 — The .app (Phase 6: ghostty-ffi + macOS shell)

C ABI mirroring include/ghostty.h closely enough to adapt (not rewrite) ghostty's Swift
sources; window/tab/split management, quick terminal, secure input, clipboard confirmation.
De-risk earlier: a thin ghostty-ffi spike (app/surface/key/draw round-trip) can start any
time after M2 — it's the least-explored seam left.

Artifact: a signed-enough .app a Ghostty user can switch to.

## M6 — Long tail & ecosystem (Phase 7)

Perf to parity (SIMD UTF-8 port), search thread + UI hooks, inspector, resize-permutation
completeness, qwertty conformance-runner target + fixture regeneration (their Phase 2
interface sketch should have arrived by then), Linux/GTK spike ADR, extraction-candidate
crate splits (sprite rasterizer first), upstream findings reported (max_scrollback docs,
OSC 21 reply gap, Flattened.init field bug).

## Standing process

Chunk cadence as today (workspace-per-chunk, Opus/Sonnet by tier, analysis-first, ledger at
every landing). Maintainer checkpoints at each M-exit; M2's "use it" test and M4's "your
config" test are personal, not automatable. Perf bench + differential suite run at every
M-boundary; regressions block the milestone.
