# linux status (Linux port — ADR 003 Wave-1 done; P4 windowed app GREENLIT)

- **Current item:** **P4 — the GTK4 Linux terminal is TYPEABLE (milestone hit).** `cargo run -p
  qwertty-term-gtk` opens a GTK4 window running a real shell: FreeType glyphs via `Engine<OpenGL>`
  → GtkGLArea, keystrokes → pty → echo. Shipped: OpenGL backend (#279), GTK plan (#280),
  coordination (#281), scaffold (#284), **present seam** (#290, into T2 core per Josh's call, T2
  post-hoc note), **terminal render** (#291), **keyboard input** (#294 — GDK keys→pty, echo
  round-trip proven). All Docker+Xvfb-verified.
- **NEXT (toward daily-usable; all additive to the gtk crate = my territory):** per-surface
  **resize** (`TODO(resize)` in `app.rs::connect_resize` — re-grid `Terminal` + `Subprocess::resize`
  TIOCSWINSZ + engine target); **IME/compose** (`GtkIMMulticontext`, `surface.zig:1246-1334`);
  **live encode modes** (thread DECCKM/kitty flags into `EncodeOptions`); **mouse/selection**;
  **dirty-tracked redraw** (drop the 60Hz tick); **DPI/font-config**; later winproto/tabs/splits.
  Also outstanding: T8 CI (headless-GL + `--features fontconfig` + GTK-dev-libs steps — filed);
  T2 post-hoc review of the present seam (#290, T2 thread currently closed).
- **The keyboard chunk (DONE #294):** `EventControllerKey` → GDK keyval →
  headless: a scripted keypress reaches the pty (strongest as a `TabIo::write`→snapshot-echo test).
- **Last merged:** #284 (GTK scaffold). All P4 PRs merged: #270, #279, #280, #281, #284. Wave-1
  (#245/#248/#254/#258/#260/#262/#264/#265) done. Everything Docker-validated on arm64 Linux.
- **Blockers:** none. (Session note: 1Password SSH-signing can lock mid-session — if `jj git
  push` fails with `op-ssh-sign: failed to fill whole buffer`, push the commit object directly:
  `git push origin <sha>:refs/heads/<branch>` bypasses jj's re-sign. See the
  `jj-push-signing-workaround` memory.)
- **Claims:** `docs/adr/005-*` for the ADR PR.
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Local Linux validation harness (proven this session)

Run the full Linux path in Docker, no CI/VM/human needed (arm64 = valid; code is arch-neutral):

```sh
REPO=~/local/ghostty-rs; DST=/tmp/lx && rm -rf $DST && mkdir -p $DST
git -C $REPO archive origin/main | tar -x -C $DST
docker run --rm -v $DST:/src -w /src -e CARGO_TARGET_DIR=/cache -v /tmp/lxcache:/cache \
  rust:1-bookworm bash -c 'apt-get update -q && apt-get install -y -q libfontconfig1 fonts-dejavu-core &&
    cargo test -p qwertty-term-renderer --test software_headless &&
    cargo test -p qwertty-term-renderer --test bold_italic_pixels --test sprite_specimen --test text_baseline &&
    cargo test -p qwertty-term-font --features fontconfig'
# pixel-test PNGs land in $DST/target/*.png (mkdir $DST/target first if you want them)
```

For P4 slice-1's headless GL readback: add Mesa (`apt install -y libgl1-mesa-dri libegl1`) and
run with `LIBGL_ALWAYS_SOFTWARE=1` / EGL surfaceless — llvmpipe software GL, no display server.

## Log (recent)

- 2026-07-15: **P4 slice 2 PR-B — GTK crate scaffold shipped (#284).** Subagent (single writer)
  created `crates/qwertty-term-gtk` (Linux-gated): `adw::Application`→`ApplicationWindow`→
  `GtkGLArea`, realize/render/resize mirroring `apprt/gtk/class/surface.zig` (3247/3347/3365).
  I re-validated in Docker (GTK 4.8.3, Mesa llvmpipe, Xvfb): `--smoke` → realized+rendered,
  `gl_error=0`, center pixel == clear color (`glReadPixels`); smoke test passes; macOS builds the
  crate empty (cfg-gated). `cargo check -p qwertty-term` clean on Linux (no objc2 leak). Two
  render-wiring seams marked in `src/app.rs`. **Workspace hazard:** the earlier concurrent-subagent
  divergence left the local jj view with a spurious `perf.md` conflict (unrelated thread; origin/
  main git-clean); shipped #284 via a `git worktree` off origin/main. **Recycle: tear down
  `work/linux`, re-bootstrap fresh.** Next critical-path item = the **present seam** (T2 core,
  awaiting T2's Inbox reply — do not author until agreed).

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

- 2026-07-14: **FreeType `LoadFlags` plumbing** (`src/freetype.rs`) — the face now carries a
  `LoadFlags { hinting, force_autohint, autohint }` (Default = upstream: hinting+autohint on,
  force-autohint off) and builds the FT load bitset via `glyph_load_flags(constrained)` mirroring
  upstream `glyphLoadFlags` (constrained forces hinting off). `with_load_flags` builder +
  `load_flags()` getter; `try_clone` preserves them. Behavior-preserving by default (the old
  `LoadFlag::DEFAULT` ≈ the default set). `monochrome` deferred (grayscale-only; needs mono-bitmap
  path). **No consumer yet** (macOS CoreText ignores it; Linux apprt is P4) — filed a heads-up to
  **T3's Inbox** so the `freetype-load-flags`/`force-autohint` config keys have a landing spot.
  **Gate:** fmt ✓; macOS default + `--features freetype` + `--features fontconfig` clippy ✓; full
  freetype (67) + fontconfig (73, real libfontconfig) suites ✓; `load_flags_default_and_mapping`
  test ✓.

- 2026-07-15: **Wave-1 validated on native arm64 Linux via Docker** (rust:1-bookworm = rustc
  1.97.0). Exported `origin/main`, ran headless render + all un-gated pixel tests + fontconfig
  suite over real FreeType/fontconfig — **all pass**; FiraCode-dependent tests skip-as-designed;
  bold/italic/sprite/baseline PNGs visually confirmed. Harness recorded above (repeatable, no
  human needed). Then **Josh greenlit P4**; wrote ADR 005 (PROPOSED) — OpenGL-first slicing +
  the GTK4-vs-winit toolkit Open Question. Shipping ADR; slice 1 (OpenGL backend) is next.
- 2026-07-15: **Josh accepted ADR 005; toolkit = GTK4** (winit rejected — known problems). ADR
  merged #270. Kicked off execution with **subagents in parallel**: (a) OpenGL `GpuBackend`
  slice 1 — shipped #279; (b) GTK app plan — shipped #280 (`docs/plans/linux-gtk-app.md`).
  **#279:** ports upstream OpenGL.zig + opengl/* + 8 GLSL (verbatim) as a headless GL 4.3
  backend over surfaceless EGL (`glow`+`khronos-egl`); additive (no T2 core); I re-validated it
  myself in Docker (arm64 Mesa llvmpipe, surfaceless): opengl_headless (2, incl. differential
  parity vs Software) + 12 module tests pass; macOS gate green (2580). **LESSON: running two
  file-mutating subagents concurrently in the SAME jj workspace diverged the working copy**
  (both snapshots landed as divergent `vmxrkots/0` non-empty vs `/2` empty; `update-stale`
  reverted the tree). Recovered via the op log: `jj log -r 'files(<path>)'` found the non-empty
  commit, `jj rebase -r <it> -d main@origin`, `jj edit`, `jj split` (non-interactive:
  `JJ_EDITOR=true jj split <path>`). **RULE: only ONE file-mutating subagent per workspace at a
  time; parallel subagents must be read-only, or use `isolation:"worktree"`.** Filed T8 (headless-
  GL CI step) + T2 (present-seam heads-up) inbox notes.

## Next-item pointers (respawn crib)

**P4 slice 1 — OpenGL `GpuBackend` (the greenlit next chunk; spec = ADR 005):**

- Port upstream `renderer/OpenGL.zig` (461) + `renderer/opengl/*` + the GLSL shaders
  (`shaders/glsl/*`, kept **verbatim**) as a new `OpenGL` backend impl of `renderer/src/gpu.rs`'s
  `GpuBackend`, alongside `Metal`/`Software`. Feed the **frozen wire structs** (T2 invariant) —
  GL just interprets the same uniforms/vertices Metal does.
- GL 4.3 core; loader via the `glow` crate (safe-ish GL) — mirrors upstream's GLAD/min-4.3
  (`OpenGL.zig:36-38,134,141-149`). Swap-chain = 1, always-sync.
- **Toolkit-independent** — needed by any on-screen path (GTK GLArea or winit), so it proceeds
  before the toolkit decision. It's the one diff-provable P4 piece.
- **Evidence (headless, no display/GPU):** an offscreen GL readback test (EGL surfaceless /
  pbuffer under Mesa `llvmpipe`, `LIBGL_ALWAYS_SOFTWARE=1`) that reproduces the same cell grid
  the Software/Metal backends produce for a known input — the ADR-003 differential parity. Runs
  in the Docker harness above. Renderer core (`gpu.rs`/`engine.rs`/`present.rs`) is **T2**'s —
  the `OpenGL` backend module is additive, but file-claim/coordinate any `engine.rs` touch.
- **BLOCKED-on-Josh for slice 2+ only:** the windowing toolkit — GTK4-rs (recommended, parity)
  vs winit+GL (lean standalone). ADR 005 Open Question. Do NOT start slice 2 until confirmed.

**Wave-1 follow-ups (lower priority than P4 now):**

**DONE this session (all merged):** mission #1 = fontconfig discovery (#245 module, #248 wiring),
FreeType `load_by_name` (#260), FreeType `LoadFlags` (#264); #42 Linux pixel coverage (#258
slice 1: bold_italic/sprite/text_baseline/default_fg embedded; #262 slice 2: ligature +
named_family). Plus status closeouts (#254 + this one).

**Remaining backlog (pick top-down):**

- **real wght-variation bold** (font crate, `freetype.rs`). Today `load_embedded_bold` uses
  *synthetic* bold (1px dilation) because FreeType wght-instance selection isn't wired; CoreText
  uses the real `wght=700` axis. Not a correctness gap — synthetic works — but a quality
  refinement. **Deserves fresh context (unsafe cross-crate FFI).** API is scouted:
  - freetype-rs 0.36 has no safe var/MM API, but exposes `Face::raw_mut() -> &mut ffi::FT_FaceRec`
    (`face.rs:349`); cast to `*mut FT_FaceRec` = `FT_Face`.
  - `freetype-sys` 0.20 (add as a direct dep; already in the lock) exposes
    `FT_Set_Var_Design_Coordinates(face, num_coords, coords: *mut FT_Fixed)` (`lib.rs:1195`) and
    `FT_Set_Named_Instance(face, idx)` (`lib.rs:1240`). Coords are 16.16 fixed (`700 * 65536`).
  - Verify freetype-rs's `ffi` types ARE freetype-sys's (same crate) so `raw_mut() as *mut
    FT_FaceRec` typechecks; else the cast won't compile.
  - wght axis index: query `FT_Get_MM_Var`/`FT_Done_MM_Var` to find the `wght` axis robustly
    (don't hardcode index 0 — fine for the single-axis embedded JBMono but not for arbitrary
    fonts). Set coords BEFORE `set_pixel_sizes`/glyph load.
  - Then update `Face::wght()` to return `Some(700.0)` and drop synthetic bold for real-bold
    faces. Test: real-bold 'H' ink > regular 'H' ink AND differs from the synthetic-bold bitmap.
- **Deferred #42 pixel tests** (`emoji_pixels`, `kitty_image_pixels`, cursor tests) — blocked on
  **Software-backend color-glyph + kitty-image compositing** (renderer/`software.rs`, T7-noted
  deferred features). Do those first, then un-gate the tests.
- **Deferred Software-backend features** (renderer/`software.rs`): color/emoji glyph atlas,
  kitty image compositing, `padding_extend` edges, linear-space blending. Larger; renderer core-ish.
- **`monochrome` FreeType load flag** — deferred with the color-glyph work (needs the 1-bit
  bitmap unpack path); the `LoadFlags` struct is ready to grow the field.
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
