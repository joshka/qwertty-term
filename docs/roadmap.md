# Roadmap — full work breakdown

Priorities (Josh, 2026-07-07): **user-facing first**; real rendering soon; config minimal
(theme + copy-on-select suffice for a long time); library/embeddability deferred for the
first cut (seams kept, not gated on); ALL known work visible here with sizes. LoC figures
are upstream Zig at commit `2da015cd6` (what gets ported/replaced); Cx = S/M/L/XL. Detailed
per-file tables live in the three sizing reports (fonts, renderer, termio/input) summarized
into chunks below. `work/default` is the integration point: always green, gate before every
`main` move.

## M1 — Engine certified (Phase 1 exit) — nearly done

- [x] Differential parity: fixtures + hand streams + 114-case corpus, zero divergences
- [x] Perf gate ≥0.5x ReleaseFast reference (0.52–0.63x across streams)
- [x] Fuzz campaign (7.9M runs clean) · Miri clean per module
- [x] Kitty graphics exec + keyboard dispatch + REP/XTWINOPS/XTSHIFTESCAPE seams
- [x] Snapshot gaps: dynamic palette, OSC 52 read-back, underline styles
- [ ] Search core port (~2.6k of 5.2k; Thread.zig wrapper → M2) — IN FLIGHT
- [ ] Deferred-test backfill: Terminal edge permutations (~300 of 381), resize permutations,
      formatter raw-Page permutations — batchable Sonnet chunks, S/M each
- [ ] Certification note in ledger (corpus size, perf table, fuzz/Miri evidence)

## M2 — Daily-drivable terminal (termio + minimal input) — ~13k LoC

Dependency spine: A → B/C → D → E → M/N; input track H → I/J/K/L independent, joins at M.

| #   | Chunk                                                   | Zig LoC | Cx  | Status / notes                                       |
| --- | ------------------------------------------------------- | ------- | --- | ---------------------------------------------------- |
| A   | PTY primitive (pty.zig+pty.c → rustix)                  | 546     | M   | quick-win class                                      |
| B   | termio plumbing (Options/message/mailbox/backend)       | 384     | S   | preserve mailbox backpressure-unlock trick           |
| C   | **threads-vs-tokio spike + ADR** (Thread.zig semantics) | 531     | M   | gates D; timeboxed                                   |
| D   | Exec: fork/exec, 2-stage read pipeline, termios poll    | 2,143   | XL  | highest-stakes concurrency                           |
| E   | Termio integration hub                                  | 800     | L   | after B+D                                            |
| F   | stream_handler glue (VT actions → mailboxes)            | 1,577   | L   | much already ported in ghostty-vt stream; delta only |
| G   | shell integration (bash/zsh/fish RC injection)          | 1,032   | M   | scripts copy verbatim; soon-after                    |
| H   | input models (key/mods/keycodes/KeymapDarwin/…)         | 3,745   | M/L | — IN FLIGHT (partial, input-encode chunk)            |
| I   | kitty keyboard encode                                   | ~400    | S   | — IN FLIGHT (input-encode)                           |
| K   | mouse reporting encode (5 formats)                      | 781     | M   | — IN FLIGHT (input-encode)                           |
| L   | bracketed paste                                         | 228     | M   | — IN FLIGHT (input-encode)                           |
| J   | legacy key encode (remainder of key_encode.zig)         | ~2,100  | XL  | after I; seam designed                               |
| M   | Surface.zig single-surface core                         | 6,036   | XL  | the join point; last                                 |
| N   | App single-surface slice + surface_mouse                | ~860    | S/M | parallel with M                                      |

Exit artifact: you use the window as your terminal for an hour. Deferred: Binding.zig
keybinds (4.9k XL), multi-surface App, tmux control mode.

## M3 — Real rendering (fonts + Metal) — ~44k LoC total, ~11k on critical path

**First-pixels critical path (~5.3k renderer + ~5.5k font):** R0→R1→{R2∥R3∥R4}, fonts
F1→F4(partial)→F5(reduced)→F6(reduced)→F7(rustybuzz-first). Everything else is
completeness behind it.

Font chunks (30.3k total; 27.7k macOS-relevant):

| #   | Chunk                                             | Zig LoC     | Cx   | Notes                                                                                                                 |
| --- | ------------------------------------------------- | ----------- | ---- | --------------------------------------------------------------------------------------------------------------------- |
| F3  | **sprite rasterizer → ghostty-sprite crate**      | 6,239       | M/L  | — IN FLIGHT; fully independent                                                                                        |
| F1  | opentype tables                                   | 2,457       | M    | ttf-parser replaces most; verify bare-glyf                                                                            |
| F4  | Metrics + Atlas + backend + embedded fonts        | 3,981       | S/M  | atlas → etagere-class crate                                                                                           |
| F5  | CoreText face + discovery                         | 3,197       | XL   | first pixels needs face + load-one-font only; Score ranking deferred                                                  |
| F6  | Collection + CodepointResolver + SharedGrid(-Set) | 3,459       | L    | reduced single-style first; Index bitfield → slotmap decision                                                         |
| F7  | run segmentation + shaping                        | 5,769       | L/XL | **rustybuzz-first** (upstream's coretext_harfbuzz variant proves viability); XL CoreText shaper = fidelity pass later |
| F2  | glyf rasterizer (glyph protocol)                  | 1,756       | L    | not first-pixels; after F1                                                                                            |
| F8  | HarfBuzz/CoreText shaper parity passes            | 2,172+2,678 | M/XL | deferred fidelity                                                                                                     |

Renderer chunks (13.9k total; Metal-first):

| #   | Chunk                                                                                           | Zig LoC | Cx  | Notes                                        |
| --- | ----------------------------------------------------------------------------------------------- | ------- | --- | -------------------------------------------- |
| R0  | geometry + RenderSnapshot trait (size/State/cursor/row)                                         | 883     | S/M | first, solo; full-copy snapshot impl day one |
| R1  | GpuBackend trait + Metal context (objc2-metal deletes 452-LoC bindings)                         | ~1,470  | L   | freezes trait + wire structs                 |
| R2  | frame/present/pacing (IOSurface-on-CALayer, NOT CAMetalLayer; CVDisplayLink later, timer first) | ~1,050  | L   | ∥ R3/R4                                      |
| R3  | shaders: cell_text/cell_bg/bg_color MSL + wire structs bit-exact                                | ~1,300  | L   | ∥ R2/R4                                      |
| R4  | cell engine (rebuildCells family; full-redraw mode day one)                                     | ~2,600  | XL  | critical path                                |
| R5  | render thread + mailbox (replace egui shell)                                                    | ~840    | L   | after pixels; sync loop OK first             |
| R6  | kitty image + bg-image rendering                                                                | ~1,400  | L   | completeness                                 |
| R7  | links (regex crate) + overlay + min-contrast polish                                             | ~740    | M   | completeness                                 |
| R8  | shadertoy custom shaders (naga/shaderc)                                                         | ~1,100  | M   | completeness                                 |
| R9  | OpenGL backend                                                                                  | ~1,870  | L   | Linux-later                                  |

Exit artifact: the window renders with real CoreText glyphs, sprites, emoji; egui retired.
Per-row dirty tracking slots in behind the RenderSnapshot trait when wired to PageList's
existing row-dirty flags.

## M4 — Input/config completeness

- [x] Minimal TOML config: theme (ghostty theme files), copy-on-select, font-size — IN FLIGHT
- [ ] Legacy key encoder (chunk J above, if not landed in M2)
- [ ] Binding.zig keybind system + actions (4,882 XL) — only when wanted; near-default user
- [ ] Full config port (Config.zig 10.9k + subsystems ~14.5k) + `+import-ghostty-config` — LOW
  priority per Josh
- [ ] IME/preedit plumbing (renderer already models preedit)

## M5 — The .app (ghostty-ffi + macOS shell)

- [ ] Thin ghostty-ffi spike (app/surface/key/draw round-trip) — can start after M2;
      de-risks the least-explored seam
- [ ] C ABI mirroring include/ghostty.h (~1.2k header; c/ bindings 14.8k Zig as reference)
- [ ] Adapt macos/Sources Swift (37k; single-window subset first: SurfaceView, window,
      clipboard, secure input)
- [ ] Quick terminal, tabs/splits, menu sync — after single window works

## M6 — Long tail & deferred

- [ ] Perf to parity (SIMD utf8/decode, wide-run batching; currently 0.52–0.63x)
- [ ] Search thread wrapper + window search UI
- [ ] Glyph APC protocol (2.2k, needs F2) · kitty unicode placeholders (U=1) · file/shm media ·
  animation
- [ ] tmux control mode (4.3k) · XTGETTCAP/DECRQSS full · OSC 21 effects
- [ ] Embeddability/betamax (DEPRIORITIZED per Josh): frame-capture example, injectable
      clock/fonts audit, betamax port spike, ghostty-sprite/vt crates.io publishing
- [ ] qwertty conformance target + fixture regeneration (their Phase-2 sketch pending)
- [ ] Linux: GTK spike ADR, FreeType/fontconfig (F-deferred), OpenGL (R9)
- [ ] Inspector · i18n · Sparkle · Sentry (per rewrite-prompt non-goals until wanted)
- [ ] Upstream findings filed (drafts ready in work/upstream/)

## Standing process

Chunk cadence: workspace-per-chunk, Opus default / Sonnet mechanical, analysis-first,
1:1 tests, gates before every main move; Miri foreground-bounded-last (background waits are
the known failure mode). Maintainer checkpoints at M-exits; bench + differential suites at
every M boundary. Keep 3–6 chunks in flight; discovery agents refresh sizing before each
new milestone opens.
