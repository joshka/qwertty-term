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
- [x] Search core — DONE 2026-07-07: literal-substring matcher (upstream has NO regex —
      zero deps), 38/38 tests; Thread wrapper + ScreenSearch cache → M2
- [x] Deferred-test backfill — DONE 2026-07-08: Terminal 419 tests (of Zig 381 semantic set;
      only 2 blocked on seams), Screen cursorCopy ported, 1428 vt lib tests total; found+fixed
      grapheme-OOM infinite-loop bug; PageList resize permutations confirmed intentionally
      reduced
- [x] Certification note — DONE 2026-07-08 (see docs/port-status.md Milestones)

## M2 — Daily-drivable terminal (termio + minimal input) — ~13k LoC

Dependency spine: A → B/C → D → E → M/N; input track H → I/J/K/L independent, joins at M.

| #   | Chunk                                                   | Zig LoC | Cx  | Status / notes                                                                                                            |
| --- | ------------------------------------------------------- | ------- | --- | ------------------------------------------------------------------------------------------------------------------------- |
| A   | PTY primitive (pty.zig+pty.c → rustix)                  | 546     | M   | **DONE 2026-07-08 (rustix pty + fork-child helpers; real-shell tests)**                                                   |
| B   | termio plumbing (Options/message/mailbox/backend)       | 384     | S   | **DONE 2026-07-08 (message 16/16 @40B, ADR-contract mailbox)**                                                            |
| C   | **threads-vs-tokio spike + ADR** (Thread.zig semantics) | 531     | M   | **DONE; ADR-002 ACCEPTED 2026-07-08 (threads+polling)**                                                                   |
| D   | Exec: fork/exec, 2-stage read pipeline, termios poll    | 2,143   | XL  | **DONE 2026-07-08 (two-stage pipeline verbatim, 106 MiB/s; waitpid watcher; teardown-under-flood proven)**                |
| E   | Termio integration hub                                  | 800     | L   | **DONE 2026-07-08 (Termio hub + Thread loop; app on real stack @135.8 MiB/s live-engine; portable-pty retired from app)** |
| F   | stream_handler glue (VT actions → mailboxes)            | 1,577   | L   | much already ported in ghostty-vt stream; delta only                                                                      |
| G   | shell integration (bash/zsh/fish RC injection)          | 1,032   | M   | scripts copy verbatim; soon-after                                                                                         |
| H   | input models (key/mods/keycodes/…)                      | 3,745   | M/L | **partial DONE 2026-07-07** (ghostty-input crate: key/mods/mouse/function-keys models; keycodes/KeymapDarwin remain)      |
| I   | kitty keyboard encode                                   | ~400    | S   | **DONE 2026-07-07** (window emits kitty sequences when apps enable them)                                                  |
| K   | mouse reporting encode (5 formats)                      | 781     | M   | **DONE 2026-07-07** (wired into window)                                                                                   |
| L   | bracketed paste                                         | 228     | M   | **DONE 2026-07-07** (control-byte stripping now active)                                                                   |
| J   | legacy key encode (remainder of key_encode.zig)         | ~2,100  | XL  | **DONE 2026-07-08 (full legacy encoder + 117-entry keymap; 172 input tests)**                                             |
| M   | Surface.zig single-surface core                         | 6,036   | XL  | the join point; last                                                                                                      |
| N   | App single-surface slice + surface_mouse                | ~860    | S/M | parallel with M                                                                                                           |

Exit artifact: you use the window as your terminal for an hour. Deferred: Binding.zig
keybinds (4.9k XL), multi-surface App, tmux control mode.

## M3 — Real rendering (fonts + Metal) — ~44k LoC total, ~11k on critical path

**First-pixels critical path (~5.3k renderer + ~5.5k font):** R0→R1→{R2∥R3∥R4}, fonts
F1→F4(partial)→F5(reduced)→F6(reduced)→F7(rustybuzz-first). Everything else is
completeness behind it.

Font chunks (30.3k total; 27.7k macOS-relevant):

| #   | Chunk                                             | Zig LoC     | Cx   | Notes                                                                                                  |
| --- | ------------------------------------------------- | ----------- | ---- | ------------------------------------------------------------------------------------------------------ |
| F3  | **sprite rasterizer → ghostty-sprite crate**      | 6,239       | M/L  | — IN FLIGHT; fully independent                                                                         |
| F1  | opentype tables                                   | 2,457       | M    | **DONE 2026-07-07 (ttf-parser adopted; skyline atlas ported — etagere rejected: no counter protocol)** |
| F4  | Metrics + Atlas + backend + embedded fonts        | 3,981       | S/M  | **DONE 2026-07-07 (with F1: Metrics verified, Atlas, embedded fonts)**                                 |
| F5  | CoreText face + discovery                         | 3,197       | XL   | **DONE 2026-07-08 (CoreText face+rasterize; F1 metrics VERIFIED exact)**                               |
| F6  | Collection + CodepointResolver + SharedGrid(-Set) | 3,459       | L    | **DONE 2026-07-08 (reduced: slotmap Collection, sprite-dispatch resolver)**                            |
| F7  | run segmentation + shaping                        | 5,769       | L/XL | **DONE 2026-07-08 (reduced: rustybuzz, cluster==cell mapping, atlas proof)**                           |
| F2  | glyf rasterizer (glyph protocol)                  | 1,756       | L    | not first-pixels; after F1                                                                             |
| F8  | HarfBuzz/CoreText shaper parity passes            | 2,172+2,678 | M/XL | deferred fidelity                                                                                      |

Renderer chunks (13.9k total; Metal-first):

| #   | Chunk                                                                                           | Zig LoC | Cx  | Notes                                                                                                               |
| --- | ----------------------------------------------------------------------------------------------- | ------- | --- | ------------------------------------------------------------------------------------------------------------------- |
| R0  | geometry + RenderSnapshot trait (size/State/cursor/row)                                         | 883     | S/M | **DONE 2026-07-07 (RenderSnapshot trait + full-copy impl)**                                                         |
| R1  | GpuBackend trait + Metal context (objc2-metal deletes 452-LoC bindings)                         | ~1,470  | L   | **DONE 2026-07-08 (objc2-metal; wire structs FROZEN; IOSurface readback proven; fixed upstream buffer over-alloc)** |
| R2  | frame/present/pacing (IOSurface-on-CALayer, NOT CAMetalLayer; CVDisplayLink later, timer first) | ~1,050  | L   | **DONE 2026-07-08 (Frame/RenderPass/Pipeline/IOSurfaceLayer/SwapChain; clear+triangle readback proven)**            |
| R3  | shaders: cell_text/cell_bg/bg_color MSL + wire structs bit-exact                                | ~1,300  | L   | **DONE 2026-07-08 (MSL verbatim, compiles on Metal; layouts pinned to wire offsets; color-math goldens)**           |
| R4  | cell engine (rebuildCells family; full-redraw mode day one)                                     | ~2,600  | XL  | **DONE 2026-07-08 (first pixels: offscreen readback acceptance, all 5 assertions)**                                 |
| R5  | render thread + mailbox (replace egui shell)                                                    | ~840    | L   | **DONE 2026-07-08 (ghostty-app: native window, tabs w/ pwd inheritance, menu, IME; smoke green)**                   |
| R6  | kitty image + bg-image rendering                                                                | ~1,400  | L   | completeness                                                                                                        |
| R7  | links (regex crate) + overlay + min-contrast polish                                             | ~740    | M   | completeness                                                                                                        |
| R8  | shadertoy custom shaders (naga/shaderc)                                                         | ~1,100  | M   | completeness                                                                                                        |
| R9  | OpenGL backend                                                                                  | ~1,870  | L   | Linux-later                                                                                                         |

Exit artifact: the window renders with real CoreText glyphs, sprites, emoji; egui retired.
Per-row dirty tracking slots in behind the RenderSnapshot trait when wired to PageList's
existing row-dirty flags.

## M4 — Input/config completeness

- [x] Minimal TOML config — DONE 2026-07-07: ~/.config/ghostty-rs/config.toml (theme via
      ghostty theme files, copy-on-select, font-size, font-family preference)
- [ ] Legacy key encoder (chunk J above, if not landed in M2)
- [ ] Binding.zig keybind system + actions (4,882 XL) — only when wanted; near-default user
- [ ] Full config port (Config.zig 10.9k + subsystems ~14.5k) + `+import-ghostty-config` — LOW
  priority per Josh
- [ ] IME/preedit plumbing (renderer already models preedit)
- [x] Minimal `keybind = text:` subset — DONE 2026-07-10 (trigger grammar subset from
      Binding.zig, `text:` action only; structured for the full port to absorb)
- [x] Tab-nav keybinds — DONE 2026-07-09 (ctrl+tab, cmd+1-9 physical, cmd+shift+brackets)
- [x] Minimal `keybind = text:` subset — DONE 2026-07-10 (trigger grammar subset from
      Binding.zig, `text:` action only; structured for the full port to absorb)
- [x] Tab-nav keybinds — DONE 2026-07-09 (ctrl+tab, cmd+1-9 physical, cmd+shift+brackets)

## M5 — The .app (ghostty-ffi + macOS shell)

- [ ] Thin ghostty-ffi spike (app/surface/key/draw round-trip) — can start after M2;
      de-risks the least-explored seam
- [ ] C ABI mirroring include/ghostty.h (~1.2k header; c/ bindings 14.8k Zig as reference)
- [ ] Adapt macos/Sources Swift (37k; single-window subset first: SurfaceView, window,
      clipboard, secure input)
- [x] Splits slice 1 — DONE 2026-07-10 in the native Rust app (surface tree, cmd+d /
      cmd+shift+d, focus nav, divider resize, close-collapse, per-pane io/focus/scrollback;
      `docs/analysis/splits.md`; slice 2 = zoom/equalize/resize-chords)
- [ ] Quick terminal, full menu-keybind sync, OSC-synced tab titles — after single
      window works (NOTE: basic native NSWindow tabs + minimal menu land EARLY, in M3's R5
      window swap — see docs/plans/m3-first-pixels.md)

## MB — betamax pure-Rust track (elevated 2026-07-08)

Goal: betamax (~/local/betamax) drops `libghostty-vt-sys` (the pinned-Zig build) and, on
macOS, renders THROUGH this stack for ghostty-identical output. Work splits across repos:

- [ ] MB1 (betamax repo): swap `libghostty-vt-sys` -> `ghostty-vt` path/git dep. The VT
      surface it consumes exists (Terminal/Stream/snapshot/formatter). Prompt for the
      betamax thread: `work/betamax-thread-prompt.md`
- [x] MB2 — DONE 2026-07-10: headless frame-capture example (`examples/frame-capture`: bytes in, PNG
      frames out, injectable font+size, deterministic) — the rewrite-prompt's embeddability
      artifact, now unblocked since the offscreen stack is proven (specimen/first-pixels
      tests ARE this flow already; the example packages it)
- [x] MB3 — DONE 2026-07-08: pixel-identical on all integer-path glyphs (braille/octants
      0.00%); AA fringes on curves within per-family budgets, permanently gated (36 upstream
      golden atlases vendored)
- [ ] MB4 (betamax): render via ghostty-renderer offscreen on macOS (Metal readback);
      Linux CI stays on betamax's cosmic-text path until the software-raster ADR (M6)
- [ ] MB5 (here): publishing prep for ghostty-vt/-sprite/-font/-renderer (versioning,
      README, docs.rs) when Josh wants crates.io

## M6 — Long tail & deferred

- [ ] Perf to parity (SIMD utf8/decode, wide-run batching; currently 0.52–0.63x)
- [ ] Search thread wrapper + window search UI
- [ ] Glyph APC protocol (2.2k, needs F2) · kitty unicode placeholders (U=1) · file/shm media ·
  animation
- [ ] tmux control mode (4.3k) · XTGETTCAP/DECRQSS full · OSC 21 effects
- [x] ~~Embeddability deprioritized~~ REVERSED 2026-07-08 (Josh): betamax-pure-Rust is now
      an active track — see the new M-track below
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
