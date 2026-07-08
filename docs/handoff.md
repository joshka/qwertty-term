# Handoff — current state

> Read `docs/rewrite-prompt.md` first (mission, phases, working rules), then
> `docs/port-status.md` (file-by-file ledger). This file is the short "where we are"
> pointer, updated at the end of every session. History note: the pre-rewrite spike's
> handoff content was retired 2026-07-06 along with the spike's move to
> `crates/spike` scaffolding; see git history if needed.

## State as of 2026-07-08 (evening)

**The app runs the REAL stack end-to-end**: `cargo run -p ghostty-app` = native AppKit
window (tabs/menu/theme/selection/IME), Metal rendering (contentsScale fixed — presented-
pixel assertions now in the smoke), and the genuine termio architecture (rustix PTY,
two-stage Exec pipeline, ADR-002 thread loop, 135.8 MiB/s into the live engine).
M2 spine A/B/C/D/E+J DONE. **The M2 exit test (maintainer drives it for an hour) is OPEN.**
Remaining M2: G shell-integration, F stream-handler delta, M/N Surface-completeness items
absorbed-or-pending (see roadmap). Two field regressions were found by the maintainer and
fixed same-day (app activation/key-window; missing contentsScale) — both now covered by
smokes that assert what the user actually experiences.

## Earlier 2026-07-08 state

**M1 CERTIFIED** (see port-status Milestones). **M3 essentially complete**: `ghostty-app`
runs — native AppKit window, Metal via the full ported stack, native tabs with OSC7 pwd
inheritance, menu, kitty+legacy key encoding, IME. All three de-risk spike decisions
RATIFIED (ADR-002 threads+polling; FFI Swift-adaptation GO; raw AppKit). In flight:
app theme+selection chunk; termio A+B (ghostty-termio). Next: M2 spine (Exec D, hub E),
M3 completeness (emoji/color atlas, kitty image render R6, links R7, CVDisplayLink,
F5-full discovery), then M5 via the proven FFI path.

## Historical: state as of 2026-07-06

**Phase 1 core loop CLOSED and demo live**: parser → stream → Terminal → Screen → PageList all
ported; differential parity proven (zero divergences vs libghostty-vt across fixtures + 8
hand-written streams); both spike frontends now run on the ghostty-vt engine
(`cargo run -p ghostty-spike -- --window`). Trunk: 880 lib tests + differential + E2E PTY
tests, all green. Ledger + milestones in `docs/port-status.md`; 14 analysis docs in
`docs/analysis/`.

**Phase 1 remaining tail** (parallelizable): search/, kitty graphics exec + unicode
placeholders, stream seams (kitty keyboard, XTWINOPS/title stack, mouse reporting,
XTGETTCAP tail, REP), snapshot gaps (OSC 52 read-back, dynamic palette into color
resolution, underline styles), promptClickMove (OSC133 click plumbing), StringMap pin-map,
Terminal edge-permutation tests, resize-permutation tests deferred from PageList,
SelectionGesture (input phase). Selection and formatter are DONE (2026-07-06 late);
formatter differential vs ghostty_formatter_* is clean; trunk ~993 lib tests.

## Prior state notes

- **Phase 0 essentially complete.** jj `work/` layout live (integrate in `work/default`,
  one workspace per chunk). Cargo workspace: `crates/ghostty-vt` (core, real code now),
  `crates/vt-diff` (differential harness vs Zig-built libghostty-vt, feature `reference`),
  `crates/spike` (old prototype, kept as Phase-2 debug frontend), `xtask`
  (`gen-unicode` codegen). Remaining Phase 0 items (fuzz targets, criterion skeleton)
  ride along with the Parser.zig chunk.
- **Reference library**: build with
  `cd ~/local/ghostty && mise exec zig@0.15.2 -- zig build -Demit-lib-vt=true`; then
  `cargo test -p vt-diff --features reference`. Ghostty commit ported against: `2da015cd6`.
- **Unicode done at exact parity** (0 mismatches over all codepoints vs ghostty's
  generated table); regenerate tables with `cargo xtask gen-unicode`.
- **Analysis docs so far**: `docs/analysis/libghostty-vt-c-api.md`, `docs/analysis/unicode.md`.
- **qwertty coordination**: `work/qwertty/protocol-status.md` (theirs) and
  `ghostty-rs-collab.md` (ours); their conformance-target sketch is expected during their
  Phase 2 — shape the Phase 4 headless API against it.

## Plans and playbook (written 2026-07-07, Fable-era)

`docs/orchestration.md` is the operations manual (chunk prompts, integration recipe,
failure recoveries — follow it mechanically). `docs/plans/` holds locked design decisions
for the big remaining work: `m3-first-pixels.md`, `m2-termio.md`, `m5-ffi-spike.md`.
Execute against these; record an ADR before deviating from a locked decision.

## In flight / next

- Phase 1 opening chunks: `page.zig` memory-model port (analysis + Page/bitmap
  allocator/style set; PageList is the follow-on chunk) and `Parser.zig` port
  (+ UTF8Decoder, fuzz target, bench skeleton; OSC accumulates raw until the osc.zig
  chunk lands).
- After those: PageList.zig, then stream.zig/Terminal.zig, then the osc parser family
  (highly parallelizable, one agent per parser).

## Session-end checklist

Update this file, `docs/port-status.md`, and (at phase boundaries) `docs/roadmap.md`;
gate: `cargo check --workspace && cargo fmt --check && cargo clippy --workspace && cargo
test --workspace`; advance `main` with `jj bookmark move main --to <rev>`.
