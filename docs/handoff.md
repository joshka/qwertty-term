# Handoff — current state

> Read `docs/rewrite-prompt.md` first (mission, phases, working rules), then
> `docs/port-status.md` (file-by-file ledger). This file is the short "where we are"
> pointer, updated at the end of every session. History note: the pre-rewrite spike's
> handoff content was retired 2026-07-06 along with the spike's move to
> `crates/spike` scaffolding; see git history if needed.

## IN FLIGHT at session pause (2026-07-10, usage limits)

One chunk workspace has an agent finishing independently; each was told to gate +
`jj describe`. Next session: integrate each per the recipe (rebase -d main, gate incl.
release lane + smokes, bookmark move, docs), in any order; check `jj log` for their
described commits even if the workspace looks idle.

- `work/bench-upstream` — "Bench: Ghostty main@91f66da24 lane — three-way vtebench
  comparison" (scripts + docs/benchmarks; prebuilt app at
  `~/local/ghostty-main/macos/build/ReleaseLocal/Ghostty.app`).

Queued next (user-approved): selection gestures (double-click word / triple line) +
OSC-synced tab titles (one qwertty-term chunk, launch after splits-2 integrates);
CVDisplayLink pacing; dense_cells re-bench after simd-perf lands. Also pending: user
pastes `work/betamax-thread-prompt.md` (MB1) and the jj thread
(`~/local/jj/work/qwertty-term-jj-failures-thread.md`).

## State as of 2026-07-10 (third batch: perf + search + embeddability)

Four lanes landed: **per-row dirty tracking** (upstream render.zig full-rebuild conditions
mirrored; equality-proven vs full redraw across 6 scenarios; 2x faster single-row frames;
scrolled-back cursor now hidden), **Cmd+F search UI** (overlay + incremental PageListSearch,
3.2ms/10k lines synchronous, upstream highlight colors, per-pane; one additive vt
constructor), **vtebench lane** (`scripts/bench-vtebench.sh`, pinned v0.3.1;
`docs/benchmarks/vtebench-baseline.md`: **qwertty-term faster in 9/10 suites vs Ghostty
1.3.1**, dense_cells the one loss at 1.36x; pty-drain caveat documented), **MB2
frame-capture example** (public-APIs-only bytes→PNG, deterministic; 6-item API-polish list
for MB5 in its report; MB1 unblocked — paste `work/betamax-thread-prompt.md` into a
betamax session). Restart-audit also recovered 3 orphaned commits (font-shaping analysis
doc, workspace-lifecycle + SendMessage-scoping playbook rules) — landed. jj-failure-modes
thread prompt written to `~/local/jj/work/qwertty-term-jj-failures-thread.md`.

## Earlier 2026-07-10 (second sweep)

**Field-quality sweep complete except lig-engine (in flight).** Landed on main since the
splits entry below: **wheel scrolling** (upstream ladder: reporting bytes unchanged /
alternate-scroll arrows per DECCKM / per-pane scrollback viewport with precision-delta
accumulation, snap-on-keystroke; known gap: cursor renders at stale position when scrolled
back — upstream hides it); **fuzz-resize** (resize-interleaved cargo-fuzz target + seeded
10k-interleaving property test in the release lane; 242k execs clean); **app hardening**
(engine poison → dead pane + banner, app survives — proven in the splits smoke; minimal
`keybind = text:` subset incl. Josh's shift+enter; per-pane mode-1004 focus reporting);
**family-styles** (FiraCode's real Bold via discovery, synthetic ladder behind);
**font-fidelity** (Apple Color Emoji pre-seeded fallback exactly like upstream
SharedGridSet.zig:335-354; byte-backed named faces via kCTFontURLAttribute so rustybuzz
shapes them; nerd-font constraint table codegen'd via `cargo xtask gen-nerd-constraints`,
math byte-exact vs upstream's Glyph.zig oracle). **In flight: lig-engine** — run-based
shaping in the render engine so multi-cell ligatures display live (font layer proven).

## Earlier 2026-07-10

**Splits slice 1 landed** (`docs/analysis/splits.md`): Tab refactored to a Surface tree
(engine+TabIo+view per pane); cmd+d / cmd+shift+d (+ upstream's ctrl+shift+o/e aliases),
ctrl+alt+arrows / ctrl+cmd+brackets navigation, divider drag with per-pane WINCH,
close-collapse, input isolation via first-responder, hollow cursor on unfocused panes.
Deferred: zoom, equalize, resize chords, dimming. **Tab-nav keybinds landed** (ctrl+tab,
cmd+1-9 physical, cmd+shift+brackets; close_tab re-entrancy fix). **Two release-only field
bugs fixed** (first --release run): cursor_absolute pin-walk panic (alt-1049+resize desync;
corpus case) and grapheme debug_assert side-effect (Zig assert ALWAYS evaluates — see
memory + orchestration release lane, now part of the standard gate). Config import
corrected: real ghostty loads TWO files; Josh actually runs FiraCode NFM @16pt (now in
qwertty-term config). Invisible-FiraCode-text fixed (byteless named faces: unshaped
fallback; ligatures for named faces still pending). In flight: family-styles (FiraCode
real Bold). Queued: emoji-discovery (we pick Noto, ghostty picks Apple), named-face
ligatures, nerd-font constraint sizing.

## Earlier: 2026-07-09 (later)

**"Font feels thin" root-caused and fixed: bold/italic never rendered.** A text-weight
chunk first PROVED rasterization flags, alpha-blending mode, pixel formats, and the
cell-text shader byte-match upstream's macOS defaults (fixing one real find: color atlas
now `Bgra8UnormSrgb` like upstream — emoji were washed out; regression pin added). Triage
then found the render path dropped SnapshotCell bold/italic entirely and the collection
had only a Regular face. Now: full style table per upstream's default mechanism
(`SharedGridSet.zig` — Bold = variable font at `wght=700`, Italic = embedded italic
variable, BoldItalic = italic@700; synthetic-bold stroke kept as documented fallback),
style-aware glyph cache + shaping, (bold,italic)→Style threaded through the engine.
Evidence: bold ink coverage 1.335× regular; offscreen test `bold_italic_pixels.rs`.

## Earlier 2026-07-09

Visual-parity sweep landed on `main` (all field-reported): **glyph baseline fix** (text was
`cell_baseline` px too low — cursor was never wrong), **top-band root cause** (sub-cell
surface remainder exposed at the visual top by flipped-layer gravity; surface now pinned
under the titlebar + window bg painted terminal-colored; `QWERTTY_TERM_SMOKE_GEOMETRY`
asserts the 1→2→1-tab transition), and **default-font parity** (vendored upstream's exact
JetBrainsMono variable + italic-variable + SymbolsNerdFontMono, hash-manifested like the
shell scripts; nerd-symbols is an explicit fallback slot ahead of discovery; metrics
unchanged). Known gap: nerd-font `constrain()` sizing table unported — PUA icons render at
natural size. Emoji, tabs-only-at-2+, bar cursor: confirmed good by maintainer.

## Earlier state (2026-07-08 evening)

**The app runs the REAL stack end-to-end**: `cargo run -p qwertty-term` = native AppKit
window (tabs/menu/theme/selection/IME), Metal rendering (contentsScale fixed — presented-
pixel assertions now in the smoke), and the genuine termio architecture (rustix PTY,
two-stage Exec pipeline, ADR-002 thread loop, 135.8 MiB/s into the live engine).
M2 spine A/B/C/D/E+J DONE. **The M2 exit test (maintainer drives it for an hour) is OPEN.**
Remaining M2: G shell-integration, F stream-handler delta, M/N Surface-completeness items
absorbed-or-pending (see roadmap). Two field regressions were found by the maintainer and
fixed same-day (app activation/key-window; missing contentsScale) — both now covered by
smokes that assert what the user actually experiences.

## Earlier 2026-07-08 state

**M1 CERTIFIED** (see port-status Milestones). **M3 essentially complete**: `qwertty-term`
runs — native AppKit window, Metal via the full ported stack, native tabs with OSC7 pwd
inheritance, menu, kitty+legacy key encoding, IME. All three de-risk spike decisions
RATIFIED (ADR-002 threads+polling; FFI Swift-adaptation GO; raw AppKit). In flight:
app theme+selection chunk; termio A+B (qwertty-term-termio). Next: M2 spine (Exec D, hub E),
M3 completeness (emoji/color atlas, kitty image render R6, links R7, CVDisplayLink,
F5-full discovery), then M5 via the proven FFI path.

## Historical: state as of 2026-07-06

**Phase 1 core loop CLOSED and demo live**: parser → stream → Terminal → Screen → PageList all
ported; differential parity proven (zero divergences vs libghostty-vt across fixtures + 8
hand-written streams); both spike frontends now run on the qwertty-term-vt engine
(`cargo run -p qwertty-term-spike -- --window`). Trunk: 880 lib tests + differential + E2E PTY
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
  one workspace per chunk). Cargo workspace: `crates/qwertty-term-vt` (core, real code now),
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
  `qwertty-term-collab.md` (ours); their conformance-target sketch is expected during their
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
