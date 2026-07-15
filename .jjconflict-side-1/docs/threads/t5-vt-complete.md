# T5 — VT completeness thread

**Model:** Opus · **Wave:** 2 (starts when T1's vt perf work drains — you inherit the vt
crate) · **Workspace:** `work/t5` · **Status:** `status/t5.md`
**Territory:** `crates/qwertty-term-vt` (feature semantics), `crates/vt-diff` (corpus).
App/renderer hooks via file-claim. Rules: `docs/threads/README.md`.

## Mission

Finish the engine's behavioral long tail so the VT layer is a complete, certifiable
Ghostty-semantics implementation — every remaining `[~]`/`[ ]` in feature-coverage.md's
terminal section, each landed with differential-corpus evidence against the reference
oracle (build: `cd ~/local/ghostty && mise exec zig@0.15.2 -- zig build -Demit-lib-vt=true
-Doptimize=ReleaseFast`; then `cargo test -p vt-diff --features reference`).

## Backlog

- [ ] **stream_handler delta audit (M2-F)** (L, FIRST — it gates the rest): diff upstream
      `src/termio/stream_handler.zig` (1,577 LoC) action-by-action against our stream/
      TerminalHandler; table every action (done / partial / missing) in
      `docs/analysis/stream-handler-delta.md`; port the missing ones. This audit likely
      surfaces most items below — reconcile the backlog after it.
- [ ] **XTWINOPS complete** (M): all ops upstream implements (report/resize/title stack
      push-pop with its 10-deep semantics); reply-byte differential cases.
- [ ] **XTGETTCAP + DECRQSS full** (M): complete capability/setting tables per upstream;
      corpus cases for each reply.
- [ ] **VT config toggles** (S/M): `title-report`, `enquiry-response` (ENQ answerback),
      `vt-kam-allowed` (KAM mode 2), `osc-color-report-format`, `scrollback-limit`,
      `image-storage-limit` — engine-side options with plumbing seams (config keys arrive
      via T3; expose setters + Inbox them).
- [ ] **OSC gaps** (S each): OSC 21 query reply (kitty color protocol — our own upstream
      finding issue-3 documents the gap; implement OUR side per kitty spec + note
      divergence-from-upstream-bug in corpus), OSC 22 pointer-shape surfacing to app,
      any OSC 104/110-119 reset edge cases the delta audit finds.
- [ ] **Selection/word semantics** (S): `selection-word-chars`-driven word boundaries
      exposed for T4's double-click (file-claim or Inbox coordination).
- [ ] **promptClickMove + jump_to_prompt support** (M): OSC133 zone navigation primitives
      (T3's actions and T4's click-to-move consume them).
- [ ] **Surface.zig mining (M2-M/N)** (L, ongoing): upstream `src/Surface.zig` (6k) —
      mine for engine-adjacent behaviors we lack (mouse shape protocol, size reports,
      preedit interplay); table findings, port engine-side pieces, Inbox app-side ones.
- [ ] **tmux control mode** (XL, LAST, Josh-gated): 4.3k — confirm Josh wants it before
      starting; analysis doc first if so.
- [ ] **Corpus growth** (recurring): every item lands corpus cases; target zero
      known-uncovered reply paths by thread end.

## Method rules

Differential oracle is the referee for every change; new behavior without a corpus case
doesn't merge. Zig-port rules apply hard here (assert-evaluates, numeric truncation —
`docs/orchestration.md` memory notes). Release lane + resize property tests always. Fuzz
targets get new dictionary tokens when new sequence families land. Perf: don't regress
T1's fence (`scripts/bench-quick.sh`) — coordinate via status if a semantic fix costs.

## Definition of done

feature-coverage.md terminal section fully `[x]`/`[—]` except tmux (Josh's call);
stream-handler delta table shows zero MISSING; recertification note (M1-style) appended
to `docs/port-status.md` with the new totals.
