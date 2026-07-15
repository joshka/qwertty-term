# linux status (Linux port — ADR 003, P2/P3 continuation of T7)

- **Current item:** #42 slice 2 (un-gate `ligature_pixels` + `named_family_default_fg`) —
  gate-green, shipping. Next: `force-autohint`/`freetype-load-flags` (FreeType plumbing; config
  parsing T3); then real wght-variation bold; then deferred pixel tests (emoji/kitty/cursor —
  need Software color/image compositing).
- **Last merged:** #260 (FreeType `load_by_name` via fontconfig) → `882acd8a`.
  (Also merged this session: S1 #245, S2 #248, #254 status, #258 #42-slice1.)
- **Blockers:** none. (Session note: 1Password SSH-signing can lock mid-session — if `jj git
  push` fails with `op-ssh-sign: failed to fill whole buffer`, push the commit object directly:
  `git push origin <sha>:refs/heads/<branch>` bypasses jj's re-sign. See the
  `jj-push-signing-workaround` memory.)
- **Claims:** `crates/qwertty-term-font/src/freetype.rs` (`load_by_name`) for the current PR.
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Log (recent)

- 2026-07-14: **#42 slice 1 — un-gated 4 renderer pixel tests onto `Software`/Linux.**
  `bold_italic_pixels`, `sprite_specimen`, `text_baseline`, and the embedded case of
  `default_fg_ink` now build `Engine<Software>` (dropped `#![cfg(target_os="macos")]`, swapped
  `metal::Metal`→`software::Software` + `coretext::Face`→the `Face` alias), mirroring
  `software_headless.rs` — so they run on macOS (CoreText) and Linux CI (FreeType) alike, giving
  real Linux glyph/sprite/baseline pixel coverage. `named_family_default_fg_inks_on_theme` stays
  `#[cfg(target_os="macos")]` (needs CoreText `load_by_name`). Note: in the *renderer* crate,
  CoreText⟺`target_os="macos"` (it enables the font FreeType backend only off-macOS) — do NOT
  use `feature="freetype"` in renderer cfgs (unknown-cfg warning → `-D warnings`). Deferred:
  `emoji_pixels`/`kitty_image_pixels` (Software color/image are deferred features),
  `ligature_pixels` (needs `load_by_name`), cursor tests; `first_pixels` kept as the Metal
  IOSurface-readback proof. **Gate:** fmt ✓; workspace clippy+test ✓ (2496); vt release ✓ (1570)
  and paranoid ✓ (1545); offscreen-smoke ✓; the 4 tests pass on macOS-Software locally (the
  Linux FreeType path is validated by CI, same mechanism as `software_headless`). Merged #258
  (`23c09cbd`) — Linux CI green (compiled + ran the un-gated tests over FreeType).
- 2026-07-14: **FreeType `Face::load_by_name`** added (`src/freetype.rs`) — mirrors
  `coretext::Face::load_by_name`: `#[cfg(feature="fontconfig")]` discovers the family via
  `fontconfig::discover_family`, keeps it iff the resolved family case-insensitively matches
  (rejecting fontconfig's silent substitute), else embedded fallback; the `not(fontconfig)` arm
  is embedded-only (API parity so callers reach it through the `Face` alias). Unblocks #42 slice
  2 (`ligature_pixels` + `named_family_default_fg`, which need `load_by_name`). Test:
  `load_by_name_bogus_falls_back_to_embedded` (holds on both feature configs; the positive
  discovery path is covered by the fontconfig module tests). **Gate:** fmt ✓; macOS default +
  `--features freetype` + `--features fontconfig` clippy ✓; freetype/fontconfig tests ✓ (bogus
  fallback verified with real libfontconfig); workspace test ✓ (2521); vt release ✓ (1595) +
  paranoid ✓ (1570); offscreen-smoke ✓.

- 2026-07-14: **#42 slice 2 — un-gated `ligature_pixels` + `named_family_default_fg_inks_on_theme`.**
  Both need `Face::load_by_name` (now on FreeType via #260). `ligature_pixels`: dropped the OS
  gate, swapped `coretext::{Face,PixelFormat,Bitmap}` → the crate-root aliases. `default_fg_ink`:
  removed the `#[cfg(target_os="macos")]` on the named-family test. Both skip-if-not-installed on
  Linux (FiraCode/named family usually absent on CI → `load_by_name` embedded-fallback → family
  check misses → SKIP). **Gate:** fmt ✓; renderer clippy all-targets ✓; workspace test ✓ (2521);
  vt release ✓ (1595) + paranoid ✓ (1570); offscreen-smoke ✓; both pass on macOS locally.

## Next-item pointers (respawn crib)

- **`force-autohint`/`freetype-load-flags`:** give `freetype::Face` a load-flags field, apply in
  `rasterize`/`rasterize_constrained` (`FT_LOAD_FORCE_AUTOHINT` etc.), default = upstream
  `face/freetype.zig` load flags. Config-key *parsing* is **T3**; do the FreeType plumbing
  additively + route the config wiring to T3's Inbox (coordination, not a hard block).
- **#42:** un-gate the ~9 `#[cfg(target_os="macos")]` renderer pixel tests
  (`crates/qwertty-term-renderer/tests/*_pixels.rs`) to also run over `Engine<Software>` on
  Linux CI — mirror `tests/software_headless.rs`. Pure coverage, my territory.
- **GATED (P4):** windowed Linux renderer (OpenGL R9 / software-to-window) + GTK4 apprt + OS
  glue. ADR 003 defers behind a separate Josh greenlight — **flag Josh, do not start**.
- **Local fontconfig validation:** libfontconfig isn't on the dev mac by default (tests
  skip-with-note). To run them: symlink `/opt/homebrew/opt/fontconfig/lib/libfontconfig.1.dylib`
  to `DIR/libfontconfig.dylib.1` (note the odd dlopen name order), then run with
  `DYLD_FALLBACK_LIBRARY_PATH=DIR cargo test -p qwertty-term-font --features fontconfig`. Linux
  `--features fontconfig` can't cross-build locally (freetype needs a Linux g++) → rely on CI.
  (See the `fontconfig-local-validation` memory.)

## Backlog (from T7 handoff §b + mission)

1. **fontconfig discovery** (mission #1) — S1 + S2 DONE. Remaining:
   - S1 ✅ `fontconfig.rs` module + feature + `Descriptor` hoist (#245, merged `d84360c6`).
   - S2 ✅ wired into `collection::discover_family_style` + `resolver::discover_fallback`;
     enabled on Linux via renderer. Emoji seed: NO change needed — emoji resolves through the
     now-wired presentation-aware `discover_fallback` (fontconfig handles emoji preference; the
     macOS Apple-emoji pre-seed only fixes a CoreText glyph-count tiebreak). (PR opening.)
   - S3: **T8 CI step** (`cargo clippy/test -p qwertty-term-font --features fontconfig` on the
     Linux lane) — route to T8 Inbox.
2. **`force-autohint` / `freetype-load-flags`** config keys (P2; lightly shared w/ app-tails —
   file-claim/Inbox for config-key overlap).
3. **real wght-variation bold** (FreeType face; deferred synthetic-only today).
4. **#42** — un-gate macOS pixel tests over `Engine<Software>` on Linux CI (coverage).
5. Deferred software-backend features (color/emoji glyphs, kitty image, padding_extend,
   linear-space) — renderer.
6. **P4** (GTK apprt + OpenGL) — needs Josh greenlight; do NOT start.

## Key decisions

- **fontconfig binding = `fontconfig` crate v0.11 with `dlopen`** (loads libfontconfig at
  runtime). Rationale: no link-time system dep → `cargo check`/`clippy` compile on any host,
  cross-target too; only *running* discovery needs libfontconfig (ubuntu-latest has it). Mirrors
  the accepted `--features freetype` validation pattern (bundled C build on the macOS gate + a
  Linux CI step). `fontconfig` feature implies `freetype` (discovery must produce a `Face`).
- **Validation:** macOS default gate unaffected (fontconfig code cfg'd out). `--features
  fontconfig` builds/tests on the macOS host (freetype bundled C + fontconfig dlopen link-free);
  discovery tests skip gracefully when libfontconfig can't init locally. Linux `--features
  fontconfig` cross-check fails locally (freetype needs a Linux cc) → relies on the requested T8
  CI step, same as freetype.

## Bootstrap / workspace notes

- jj `main` bookmark is a **stale divergent** local copy at `a6afe1cf`; real trunk is
  `main@git` = `d4171155` (6 ahead, linear). Base work on the git ref (`jj new d4171155`), do
  NOT touch the global `main` bookmark (cross-workspace hazard).

## Log

- 2026-07-14: session 1 start. Bootstrapped `work/linux` workspace off real trunk d4171155
  (jj main bookmark was stale-divergent). Read ADR 003 + linux.md plan + T7 closeout. P1
  (renderer software backend + `Engine<B>`) and P2 FreeType *face* are DONE on main; remaining P2
  is fontconfig *discovery* + FreeType config flags. Surveyed the discovery seams
  (`collection::discover_family_style`, `resolver::discover_fallback`, emoji seed) and upstream
  `discovery.zig` fontconfig arm (117-339). Chose the `fontconfig`+dlopen binding. Starting S1.
- 2026-07-14: **S1 done — fontconfig discovery module** (additive, not yet wired). New
  `fontconfig` Cargo feature (implies `freetype`; `fontconfig` crate 0.11 in dlopen mode → no
  link-time dep). Hoisted `Descriptor` (+`hashcode`) to a platform-neutral `src/descriptor.rs`
  shared by both backends; `discovery.rs` keeps `to_ct_descriptor`. New `src/fontconfig.rs`:
  `Descriptor::to_fc_pattern` (family/style/charset/size/weight/slant + always spacing=mono, per
  upstream `discovery.zig:117-155`), `discover` via `FcFontSort` (NoTrim), `discover_fallback`
  (= discover), `discover_family_style`/`discover_family`, and `FcDeferredFace` (path+index+family
  with a lazy probe-face for `has_codepoint`/`presentation`, `load` via FreeType). Documented
  reductions: no `FcFontRenderPrepare`, `has_codepoint` loads-to-probe (no charset getter).
  **Gate:** fmt ✓; workspace clippy+test macOS ✓ (2484 pass, Descriptor hoist safe); font default
  linux cross-check ✓; `--features fontconfig` clippy+test ✓. Real end-to-end discovery validated
  by symlinking homebrew libfontconfig onto the dyld path (`libfontconfig.dylib.1`) — 4 fontconfig
  tests genuinely ran (not skipped) against system fonts. `--features fontconfig` linux cross-build
  can't run locally (freetype needs a linux g++) → relies on the requested T8 CI step (same as
  `--features freetype`). Opening PR.
- 2026-07-14: **S1 shipped** — #245 merged (rebase) to `d84360c6`; CI green (incl. Linux core,
  confirming the `Descriptor` hoist compiles on the real Linux target). Rebased S2 onto it;
  cleaned the post-merge divergent local S1 leftover. Hit + fixed the release-plz version skew
  (`0.2.0`→`0.3.0`) my new Linux renderer dep block introduced on rebase.
- 2026-07-14: **S2 done — wired fontconfig discovery into the font stack.** `collection`'s
  `discover_family_style` and `resolver`'s step-6 `discover_fallback` now have a
  `#[cfg(feature="fontconfig")]` arm alongside the CoreText one (mutually exclusive: fontconfig
  implies freetype, CoreText arm requires not-freetype); the loop body is shared since
  `FcDeferredFace` mirrors `DeferredFace`'s `has_codepoint`/`load`. Enabled the `fontconfig`
  feature on Linux via a `cfg(target_os="linux")` renderer dep block (dlopen → Linux CI compiles
  with no libfontconfig; headless/betamax path never triggers discovery so stays lib-free). Added
  `resolver::fontconfig_tests` (emoji + CJK fallback, skip-with-note if libfontconfig absent).
  **Gate:** fmt ✓; workspace clippy+test macOS ✓ (2491); vt release ✓ (1568) + paranoid ✓ (1543);
  offscreen-smoke ✓; `--features fontconfig` clippy+test ✓ (71, real libfontconfig). Probe
  confirmed discovery returns real covering fonts for A/CJK/snowman/Cyrillic/Hebrew and that the
  resolver correctly rejects a text-presentation candidate for an emoji-presentation request.
  Opening PR-2.
