# Handoff — current state

> Read `docs/rewrite-prompt.md` first (mission, phases, working rules), then
> `docs/port-status.md` (file-by-file ledger). This file is the short "where we are"
> pointer, updated at the end of every session. History note: the pre-rewrite spike's
> handoff content was retired 2026-07-06 along with the spike's move to
> `crates/spike` scaffolding; see git history if needed.

## State as of 2026-07-06 (end of autonomous run)

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
