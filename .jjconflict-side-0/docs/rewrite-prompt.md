# Prompt: Rewrite Ghostty in Rust

> **How to use this document.** Paste this prompt (or reference this file) at the start of a
> working session. It is self-contained: it embeds the ground-truth map of the Zig codebase so an
> agent does not need to re-explore it from scratch, defines the phase plan, and sets the working
> rules. Each session should (1) read this file, (2) read `docs/roadmap.md` and `docs/handoff.md`
> for current state, (3) pick up the next incomplete milestone, and (4) update those docs before
> ending. Facts below were surveyed 2026-07-06 against the Zig source; spot-check paths that
> matter to your task before relying on them.

---

## Mission

Produce a full, faithful rewrite of the Ghostty terminal emulator in Rust, targeting **macOS
first** (Linux/GTK later), with Ghostty's behavior as the conformance target and Ghostty's
architecture as the default design unless there is a documented reason to deviate.

"Faithful" means: a user switching from Ghostty should not notice behavioral differences in
terminal emulation, rendering quality, font handling, keyboard encoding, or configuration
semantics. It does **not** mean transliterating Zig line-by-line — use idiomatic Rust and the
Rust ecosystem where it genuinely matches Ghostty's quality bar, and port Ghostty's designs where
they are the differentiator.

The reference source is at `~/local/ghostty` (Zig + Swift). The Rust workspace is
`~/local/ghostty-rs`.

**Co-equal goal: be an embeddable library, not just an app.** Upstream ghostty declined to
make its *rendering* embeddable; only the VT layer (`libghostty-vt`) is consumable. The
concrete casualty is betamax (`~/local/betamax`, joshka.net/betamax — a Rust VHS
reimplementation): it uses `libghostty-vt` for terminal semantics but had to rasterize frames
itself with cosmic-text/swash, so its output does not look like ghostty, and its build drags
in a pinned Zig toolchain via `libghostty-vt-sys`. This rewrite fixes both: every layer —
VT, fonts, and **rendering** — must be usable as ordinary Rust crates by applications that
are not this terminal app, with output pixel-identical to the app. Betamax is the named
reference consumer; when making design choices, ask "could betamax call this?" Embeddability
requirements are listed under Architecture and are not deferrable polish — retrofitting them
is exactly what upstream wouldn't do.

## Non-goals (for now — revisit only when the core is done)

- Linux/GTK app shell, Flatpak/Snap packaging, Wayland protocols, cgroups, D-Bus, systemd.
- WebAssembly/browser runtime.
- Windows support.
- i18n/translations, Sparkle auto-update, Sentry crash reporting, AppleScript/App Intents.
- The `+boo` easter egg and other CLI whimsy.

Do keep the *seams* for these (traits/feature flags at the same boundaries ghostty uses) so they
remain addable.

---

## Ground truth: what Ghostty is (surveyed 2026-07-06)

Scale: ~263k LOC Zig in 515 files, ~37k LOC Swift in 160 files (macOS app), plus 25 vendored
dependencies. Major subsystems:

| Subsystem                 | Location                         | ~LOC                            | Notes                                     |
| ------------------------- | -------------------------------- | ------------------------------- | ----------------------------------------- |
| Terminal core             | `src/terminal/`                  | 113k (74k excl. C API/graphics) | The heart; see below                      |
| Fonts                     | `src/font/`                      | 30k                             | Discovery, shaping, atlas, sprites        |
| App runtimes              | `src/apprt/`                     | 26k                             | GTK 16.6k; embedded (libghostty) 2.3k     |
| Config                    | `src/config/`                    | 14.5k                           | `Config.zig` alone is 10.9k               |
| Input                     | `src/input/`                     | 12.8k                           | Bindings, kitty keyboard, mouse encode    |
| Renderer                  | `src/renderer/`                  | 12–14k                          | Generic core + Metal/OpenGL backends      |
| CLI actions               | `src/cli/`                       | 9k                              | `+list-fonts`, `+show-config`, etc.       |
| termio                    | `src/termio/`                    | 6.4k                            | PTY, exec, read thread, shell integration |
| Surface/App orchestration | `src/Surface.zig`, `src/App.zig` | 6.6k                            | Mailbox message passing                   |
| OS abstractions           | `src/os/`                        | 4.7k                            | env, passwd, xdg, locale, macOS bits      |
| Inspector                 | `src/inspector/`                 | 4.9k                            | Dear ImGui debug UI                       |
| macOS app                 | `macos/Sources/`                 | 37k Swift                       | AppKit/SwiftUI over libghostty C API      |

### Signature designs — port these, don't substitute

These are what make Ghostty Ghostty. Port the *design*, in idiomatic Rust:

1. **Page-based scrollback memory** (`src/terminal/PageList.zig` 14.8k, `page.zig` 3.9k).
   Scrollback is an intrusive doubly-linked list of page-aligned, individually mmap-able pages.
   Each page is one contiguous block laid out as
   `[Rows][Cells][Styles][Graphemes][Strings][Hyperlinks]`, addressed by **offsets, not
   pointers** (`Offset(T)`), with internal bitmap allocators for grapheme/string data, a
   ref-counted deduplicating `StyleSet`, and an offset-based hash map. Persistent references
   (viewport, selection, search results) go through tracked **Pins** that survive page
   reallocation. Per-row dirty flags drive rendering. This is the core performance story —
   do not replace it with `Vec<Vec<Cell>>` or a rope.
2. **Threading model**: dedicated threads for termio read, renderer, and app/UI, communicating
   via bounded blocking mailboxes (`BlockingQueue(Message, 64)`), with a mutex-protected shared
   render state read once per frame. Renderer runs its own event loop with a redraw timer and
   cursor-blink timer. **The invariant here is the message-passing architecture and its
   taxonomy** (see `src/apprt/surface.zig` Message union and `src/renderer/Message.zig`) —
   the concurrency substrate (OS threads à la ghostty vs. tokio tasks) is an implementation
   choice, ADR-gated and settled empirically in Phase 2 (see decisions table). Whatever the
   substrate, the renderer keeps a dedicated thread: Metal/AppKit thread affinity and frame
   pacing don't fit a work-stealing executor.
3. **Generic renderer over a GPU backend trait** (`src/renderer/generic.zig` 3.4k). All terminal
   rendering logic is backend-agnostic; backends (Metal, OpenGL) implement
   Target/Frame/RenderPass/Pipeline/Buffer/Texture. Cell model: flat bg-color array + per-row
   fg glyph lists with cursor at index 0; grayscale + color (BGRA) glyph atlases with
   rectangle bin packing and atomic modified counters; triple buffering on Metal via semaphore.
4. **Sprite font subsystem** (`src/font/sprite/` ~3.8k): box drawing, powerline, braille, block
   elements, legacy computing symbols are *rasterized in code*, not loaded from fonts, using
   sprite codepoints above the Unicode range. Also the **nerd-font constraint table**
   (codegen'd `nerd_font_attributes.zig`: per-codepoint-range sizing/alignment/padding rules).
5. **Font resolution pipeline** (`src/font/CodepointResolver.zig`): style fallback → user
   codepoint overrides → sprite check → primary face → fallback chain → on-demand discovery.
   Deferred (lazy-loaded) faces; shared font grid with cached metrics across surfaces.
6. **libghostty layering**: the core is a library (`include/ghostty.h`, ~1,210 lines, 100+
   functions, opaque `app_t`/`surface_t`/`config_t` handles, runtime-callback struct for
   wakeup/action/clipboard) and the macOS app is a *consumer* of it. Preserve this split: the
   Rust workspace must produce a `ghostty-vt`-equivalent crate and a C ABI layer so a
   Swift/AppKit shell (or any other frontend) can embed it.
7. **Kitty protocols**: graphics (`src/terminal/kitty/graphics_*.zig` ~6.3k — chunked transfer,
   placements at three z-layers, virtual placements) and keyboard
   (`kitty/key.zig` + `src/input/key_encode.zig` 2.5k — progressive enhancement flags stack).
8. **Config system semantics** (`src/config/`): the option set, themes, conditional blocks,
   hot reload, list-valued fields, CLI override parity, and
   `+show-config`/`+validate-config`/`+explain-config` actions. **Deliberate deviation: the
   file format is TOML, not ghostty's custom key=value format** (see decisions table). Port
   the *semantics* (what every option means and does), not the syntax.

### Reference file map (highest-value files to read while porting)

- `src/terminal/Parser.zig` (1.1k) — DEC-style VT state machine → `Action` union.
- `src/terminal/stream.zig` (3.7k) + `stream_terminal.zig` — parser actions → terminal ops.
- `src/terminal/Terminal.zig` (14k) — the state machine: modes, cursor, scroll regions,
  charsets, tabstops, screens (primary/alternate).
- `src/terminal/Screen.zig` (10.5k) — viewport over PageList, cursor state, selection,
  kitty image storage, semantic prompts.
- `src/terminal/sgr.zig`, `csi.zig`, `osc.zig` + `osc/parsers/` (6.3k), `dcs.zig`, `apc.zig`.
- `src/terminal/formatter.zig` (6.3k) — screen → VT serialization (copy/paste/export).
- `src/terminal/search/` (5.2k) — sliding-window regex search over PageList, on a thread.
- `src/simd/` (0.7k) — SIMD UTF-8 decode until control seq; scalar fallbacks exist.
- `src/unicode/` (1.7k) — grapheme break FSM + codegen'd width/property tables (uucode).
- `src/termio/Exec.zig`, `Termio.zig`, `shell_integration.zig` + `src/shell-integration/`
  (shell scripts are reusable as-is).
- `src/input/Binding.zig` (4.9k) — trigger parsing, 70+ actions, leader sequences, global binds.
- `src/font/discovery.zig`, `face/coretext.zig`, `shaper/harfbuzz.zig`, `shaper/coretext.zig`,
  `Atlas.zig`, `Glyph.zig`, `SharedGrid.zig`.
- `src/renderer/generic.zig`, `Thread.zig`, `cell.zig`, `image.zig`, `shadertoy.zig`,
  `shaders/` (GLSL is source of truth; MSL generated via glslang→SPIRV→SPIRV-Cross).
- `macos/Sources/Ghostty/Ghostty.App.swift`, `SurfaceView` — how the Swift shell consumes the
  C API.

---

## Architecture: target workspace layout

Cargo workspace in `~/local/ghostty-rs` with crates cut at ghostty's own seams:

```text
crates/
  ghostty-vt         # terminal core: parser, stream, Terminal, Screen, PageList, page,
                     # sgr/csi/osc/dcs/apc, kitty graphics state, search, selection,
                     # formatter, unicode tables. No I/O, no platform deps. Fuzzable.
  ghostty-config     # config parse/validate/format, themes, conditionals, keybind parsing
  ghostty-input      # key/mouse encoding (kitty keyboard, legacy, mouse protocols),
                     # binding trigger matching, actions
  ghostty-termio     # PTY (openpty/fork on macOS), exec backend, read thread,
                     # shell integration injection, flow control
  ghostty-font       # discovery (CoreText now, fontconfig later), face loading, shaping,
                     # atlas, sprite rasterization, nerd-font constraints, resolver
  ghostty-renderer   # generic renderer + backend trait; Metal backend first
  ghostty-core       # App/Surface orchestration, mailboxes, threading, actions
  ghostty-ffi        # C ABI mirroring include/ghostty.h (cbindgen), for the Swift shell
  ghostty            # binary: CLI actions (+list-fonts, +show-config, …) and app entry
xtask/               # build tooling: unicode table codegen, nerd-font codegen, shader
                     # compilation, .app bundling, xcframework
macos/               # Swift shell (later phase; may adapt ghostty's own Swift sources)
```

### Embeddability requirements (first-class, designed in from Phase 0)

These are load-bearing constraints on every crate's API, validated continuously by treating
betamax as the reference consumer:

1. **No global state, no required app shell.** Every crate is constructible as a plain value:
   multiple independent instances in one process, no singletons, no init-order requirements,
   no assumption that a window, event loop, or config file exists. The `ghostty` binary is
   just one consumer of the library crates.
2. **Headless rendering with frame readback.** The renderer must render a terminal state to
   an offscreen target and hand back RGBA pixels without any window or display — the exact
   pipeline the app uses (same shaping, sprites, nerd-font constraints, atlas, cursor styles,
   minimum-contrast), so embedded output is pixel-identical to the app by construction, not
   by imitation. See the decisions table for the offscreen-GPU-vs-software-raster choice.
3. **Injectable clock.** Anything time-dependent (cursor blink phase, custom-shader `iTime`,
   animation uniforms) takes time as a parameter; nothing reads the wall clock internally.
   Frame-capture consumers need "render this state at t=1.25s" to be deterministic and
   reproducible.
4. **Injectable fonts.** Font faces loadable from explicit paths/bytes, bypassing system
   discovery entirely, so renders are hermetic and byte-stable across machines and CI.
   System discovery is one provider behind a trait, not the substrate.
5. **Synchronous, pull-based embedding path.** An embedder must be able to do
   `feed bytes → inspect/step state → render frame` on its own thread and schedule, without
   spawning ghostty's thread topology. The threaded mailbox architecture is how the *app*
   composes the crates, not a requirement baked into them.
6. **Rust API is primary; the C ABI (`ghostty-ffi`) is a wrapper over it** — never the only
   door to a capability, and no capability (especially rendering) is app-private.

An embedding example lives in-tree from Phase 4 onward (`examples/frame-capture`: bytes in,
PNG frames out) and is part of CI, so embeddability regressions fail the build rather than
being discovered by the next betamax.

Rules:

- `ghostty-vt` must stay dependency-light and compile on stable Rust; it is the crown jewel
  and should be independently publishable/fuzzable, like `libghostty-vt`.
- Unsafe is allowed and expected in the page memory model, FFI, and GPU code — isolate it,
  document invariants, run Miri on `ghostty-vt`'s unsafe layer where feasible.
- Cross-thread messages are enums mirroring ghostty's Message unions; no ad-hoc shared state
  beyond the documented render-state mutex.

## Port vs. crate decisions (defaults; deviations need an ADR in docs/adr/)

| Area                                              | Default                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                             | Rationale                                                                                                                                                               |
| ------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| VT parser                                         | **Port `Parser.zig`** (consider `vte` only as test oracle)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | Ghostty's action set (APC, DCS hooks, OSC parser plugins) is bespoke; parser is only 1.1k                                                                               |
| Scrollback/page memory                            | **Port**                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | Signature design                                                                                                                                                        |
| Grapheme/width tables                             | **Port the codegen approach** via `icu4x` datagen or `ucd-parse` in xtask; do NOT hand-roll tables; `unicode-width`/`unicode-segmentation` acceptable as cross-check oracles                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                        | Ghostty codegens custom packed tables for speed                                                                                                                         |
| SIMD UTF-8                                        | `memchr` + `simdutf` bindings or `std::simd` port of `decode_utf8_until_control_seq`; scalar fallback first                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                         | Perf phase, not correctness phase                                                                                                                                       |
| Shaping                                           | **HarfBuzz via `harfbuzz` crate (or rustybuzz — ADR required)**; CoreText shaper later for parity                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   | HarfBuzz is what ghostty uses on the freetype path; rustybuzz is pure-Rust but verify ligature/feature parity                                                           |
| Font loading/raster                               | CoreText via `core-text`/`objc2` crates on macOS (ghostty's default macOS backend is pure CoreText); FreeType later for Linux                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Match platform-native rendering                                                                                                                                         |
| Font discovery                                    | CoreText descriptors on macOS                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Same                                                                                                                                                                    |
| GPU                                               | **Metal directly** via `objc2-metal` (preferred, mirrors ghostty) — wgpu only with an ADR accepting its costs (custom-shader pipeline, IOSurface integration, present timing)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Ghostty's Metal path is IOSurface-based and tightly integrated with AppKit                                                                                              |
| Custom shaders                                    | Keep GLSL-in → SPIRV → MSL pipeline (`shaderc`/`naga` + `spirv-cross`)                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                              | User-facing compat with existing shadertoy shaders                                                                                                                      |
| Regex (URL detection, search)                     | Port against **oniguruma bindings** (`onig` crate) OR `regex`+`fancy-regex` with an ADR documenting semantic differences                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                            | Ghostty uses oniguruma                                                                                                                                                  |
| Image decode (kitty graphics)                     | `image`/`png` crates instead of wuffs                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                               | No user-visible semantics                                                                                                                                               |
| Headless/embedded rendering                       | **Offscreen GPU target + readback first** (render to an offscreen Metal texture with the exact app pipeline, read pixels back) — pixel-identity with the app is free. Then evaluate (ADR) a **software raster backend** implementing the same GPU-backend trait for GPU-less environments (Linux CI, servers): candidates are a scalar/`std::simd` rasterizer of the same cell model, or `naga`-interpreted shaders; it must pass the same golden-image suite as the GPU path or document every divergence                                                                                                                                                                                                                                                                                                                                                          | Betamax-class consumers need frames on machines without a display and ideally without a GPU; two backends behind one trait keeps them honest against each other         |
| PTY                                               | Direct `openpty`/`forkpty` via `nix`/`rustix` (port `pty.c` + `Exec.zig`), not `portable-pty`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Need ghostty-level control (termios polling, flow control, process watching)                                                                                            |
| Concurrency substrate (termio, app orchestration) | **Undecided — settle by measurement in Phase 2 (ADR required).** Candidates: (a) dedicated threads + `mio`/`polling`/kqueue-via-`rustix` (libxev equivalent, mirrors ghostty), (b) tokio (`AsyncFd` for the PTY, `tokio::process`, timers, `select!` replacing hand-rolled wakeup pipes). Build the Phase-2 termio spike so the substrate is swappable behind the mailbox API, then benchmark both: PTY read→screen-update latency (p50/p99), throughput under `cat` flood, wakeup jitter, CPU idle cost, and code complexity (signal handling, kill-pipe, termios polling). Adopt tokio if it wins or ties on the hot path — it likely simplifies process/signal/timer plumbing — but do not let executor scheduling onto the byte-parsing hot path without numbers. Renderer stays a dedicated thread regardless; `ghostty-vt` stays sync/runtime-free regardless | This is exactly the layer where threads-vs-async problems (latency jitter, priority inversion, buffer ownership across `await`) show up — decide with data, not fashion |
| Config format                                     | **TOML** (`toml`/`serde` crates) — a deliberate, owner-approved deviation from ghostty's custom key=value format. Keep hyphenated option names and semantics identical; ship a `+import-ghostty-config` converter (ghostty format → TOML, including keybind strings) and use converted ghostty example configs as fixtures                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                          | Owner prefers a standard format; converter preserves the migration path                                                                                                 |
| Shell integration scripts                         | **Copy verbatim** from `src/shell-integration/`                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                     | They're shell scripts; divergence is pure loss                                                                                                                          |
| Terminfo                                          | Ship ghostty's terminfo (`xterm-ghostty`) unchanged                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                 | Ecosystem compat                                                                                                                                                        |
| Inspector                                         | Defer; later egui or imgui-rs                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                       | Debug tooling, not user-facing                                                                                                                                          |

## Library extraction policy & the qwertty seam

### Extraction policy: shape emulator-agnostic concepts as real libraries

Some of what ghostty implements is not "terminal emulator internals" — it is general
knowledge the Rust ecosystem lacks, currently trapped inside apps. Where a concept is
emulator-agnostic, shape it as a standalone crate from the start: **no ghostty types in its
public API, its own tests and fixtures, documented for a consumer who has never heard of
this project.** This is the same discipline as the embeddability requirements, applied one
level deeper.

Extraction candidates (flag status in `docs/port-status.md`):

| Candidate                                                                                                                                                                                                                              | From (Zig)                           | Standalone value                                                                                                                                    | Confidence                                            |
| -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- |
| **Sprite glyph rasterizer** — box drawing, block elements, braille, powerline, branch/git symbols, geometric shapes, legacy computing (U+1FB00 block), etc.: "given cell metrics + line thickness, what does this codepoint look like" | `src/font/sprite/` (~3.8k)           | Any terminal, TUI screenshot/recording tools (betamax), font tooling; nothing in the ecosystem renders these *correctly and seam-free* as a library | High — extract                                        |
| Nerd-font constraint rules (codepoint-range → sizing/alignment/padding)                                                                                                                                                                | codegen'd `nerd_font_attributes.zig` | Companion to the sprite crate; any renderer drawing nerd fonts at grid metrics                                                                      | High — extract (same crate or sibling)                |
| **ghostty-vt** — the whole terminal core                                                                                                                                                                                               | `src/terminal/`                      | Already planned as the crown-jewel crate; this is the flagship extraction                                                                           | Committed                                             |
| Kitty graphics protocol model (chunked transfer, placement tracking, z-layers) — decoupled from any renderer                                                                                                                           | `src/terminal/kitty/` (~6.3k)        | Any emulator or emulator-adjacent tool implementing the protocol                                                                                    | Medium — design for it, split when a consumer appears |
| Terminal-tuned Unicode tables (width + grapheme FSM, mode-2027-aware)                                                                                                                                                                  | `src/unicode/` + codegen             | Overlaps `unicode-width`/`unicode-segmentation`; only worth it if terminal-specific semantics diverge enough                                        | Low — flag, don't commit                              |
| Glyph atlas / bin packing                                                                                                                                                                                                              | `src/font/Atlas.zig`                 | `etagere`/`guillotiere` already exist                                                                                                               | No — use or wrap ecosystem                            |

Discipline: design the *seams* now (ghostty-free APIs), but do the actual crate splits when a
second consumer exists or at publication time — over-fragmenting mid-port slows the primary
mission. When in doubt, keep it a module with a clean boundary and a `docs/port-status.md`
note.

### The qwertty seam: same wire, opposite ends

**qwertty** (`~/local/qwertty`, driving prompt `work/prompt.md`) is Josh's app-side terminal
library — the layer that replaces crossterm/termina/termwiz for Rust *applications talking
to* a terminal. It owns: session lifecycle, command encoding (host→terminal), input/report
decoding (terminal→host), query correlation, and — critically — a **canonical
machine-readable sequence database** (every sequence family, with citations and byte
fixtures, generating its docs) plus a **conformance runner** producing a "caniuse for
terminals" support matrix. ghostty-rs is the emulator side of the same wire: it decodes what
qwertty encodes and encodes what qwertty decodes.

Division of labor — do not duplicate:

- **qwertty owns the sequence database, citations, and conformance tooling.** ghostty-rs
  must NOT build a rival sequence registry or protocol-documentation corpus. The
  authoritative statement of what is currently consumable is
  `work/qwertty/protocol-status.md` (maintained by the qwertty thread, commit-stamped) —
  read it before relying on anything qwertty-shaped. As of its 2026-07-06 stamp: the
  database exists only on qwertty's prototype branch
  (`joshka/qwertty-reference-prototype:registry/`); its **host→terminal command fixtures
  are usable as a `ghostty-vt` parser corpus today** (unescape `\e`/`\xNN`, trailing LF is
  noise, skip the named known-bad groups in the status doc's trust map), its terminal→host
  report fixtures are **quarantined — never use as encoder ground truth**, and its sequence
  IDs are **not yet citable** — `docs/analysis/` docs cite primary specs directly until
  qwertty's Phase 2 stabilizes an ID scheme (mapping can be added mechanically later).
- **ghostty-rs owns emulator-side semantics** (state machine, memory model, rendering) —
  nothing session- or app-side-shaped belongs here.
- **Mutual oracle testing**: qwertty's report/event decoders verify ghostty-rs's encoders
  (kitty keyboard, mouse reports, DSR/DA replies) and vice versa; each project's fixtures
  are test vectors for the other. (Today qwertty can verify only CPR/DSR replies and
  general CSI well-formedness — kitty keyboard/mouse decode is deferred on their side, so
  don't plan Phase 5 encoder tests around it; check `protocol-status.md` for current
  coverage. Pin any qwertty git dependency by rev — their input event model is explicitly
  high-churn.)
- **Headless ghostty-rs is a conformance asset for qwertty**: an embeddable, deterministic,
  CI-runnable emulator is exactly what qwertty's conformance runner and its quarantined
  terminal→host fixture regeneration need. This strengthens the embeddability requirements —
  qwertty joins betamax as a named reference consumer.

Coordination follows the pattern proven with rabbitui: `work/qwertty/` in this repo is a
shared drop-box (NOT a jj workspace) where the qwertty thread maintains a commit-stamped
protocol/status doc and dispositions this project's requests; this project's asks/offers
live in `work/qwertty/ghostty-rs-collab.md`. The prompt to hand the qwertty thread is
`work/qwertty-thread-prompt.md`.

## Conformance & testing strategy (non-negotiable, built alongside each phase)

1. **Port ghostty's inline tests.** Terminal.zig alone has 50+ `test` blocks; Screen, PageList,
   Parser, sgr/osc/csi all have dense inline tests. Every ported module ports its tests. Track
   coverage in a checklist doc (`docs/port-status.md`): file → ported? tests ported? count.
2. **Differential/oracle testing.** Build a harness that feeds identical byte streams to
   (a) ghostty's `libghostty-vt` (build it: `zig build` in `~/local/ghostty`; C API in
   `include/`) and (b) `ghostty-vt`, then diffs final screen state (text, styles, cursor,
   modes). Corpus: the existing replay fixtures in `tests/fixtures/replay/`, vttest/esctest
   sequences, captured real-app sessions (nvim, tmux, htop, fzf startup), and qwertty's
   audited host→terminal fixture corpus (see the qwertty seam section for the trust map and
   unescaping convention).
3. **Fuzzing**: `cargo-fuzz` on the parser + stream from day one of Phase 1. `ghostty-vt` must
   never panic on arbitrary bytes.
4. **Snapshot/replay fixtures** (already in repo) grow with every feature; keep them
   deterministic.
5. **Benchmarks**: port the spirit of `src/main_bench.zig`. Criterion benches for: plain-text
   throughput, SGR-heavy stream, scrollback pressure, `cat` of a large file. Gate: within 2× of
   ghostty at Phase-1 exit, parity or better by Phase 6.
6. **Rendering verification**: `--render-probe`-style golden-image tests for sprite glyphs
   (box drawing must be pixel-perfect and seam-free at multiple sizes/DPIs) and cell layout.

## Phase plan

Each phase ends with: tests green, fixtures added, `docs/analysis/` docs written for the
subsystems ported, `docs/port-status.md` and `docs/roadmap.md` updated, an ADR for any
deviation taken, and a bookmarked jj change.

- **Phase 0 — Workspace + harness.** Restructure the repo into the workspace above (see
  "existing prototype" below). Build libghostty-vt from the Zig tree and stand up the
  differential harness + fuzz targets + criterion skeleton. Set up xtask codegen for unicode
  tables (verify output against ghostty's `props_table.zig` semantics).
- **Phase 1 — VT core (`ghostty-vt`).** Parser → stream → Terminal → Screen → PageList/page,
  in dependency order but with the page memory model FIRST (everything sits on it). Full
  sgr/csi/osc/dcs/apc coverage, charsets, tabstops, modes, primary/alternate screens, scroll
  regions, wide chars + grapheme clustering, hyperlinks, styles dedup, dirty tracking, resize
  with reflow, selection data model, formatter (screen→VT). Exit: differential harness agrees
  with libghostty-vt on the full corpus; vim/tmux/htop replay fixtures byte-identical.
- **Phase 2 — termio + real shell.** PTY/exec port, read path with the SIMD-less UTF-8
  decoder, write path with flow control, process lifecycle, shell integration injection,
  termios polling (password detection). Build the I/O layer swappable behind the mailbox API
  and run the **threads-vs-tokio evaluation** (see decisions table): benchmark both
  substrates, write the ADR, commit to one. Wire into a minimal debug frontend (the existing
  crossterm host is fine as scaffolding). Exit: daily-drivable inside another terminal;
  interactive vim/tmux sessions correct; concurrency-substrate ADR merged with numbers.
- **Phase 3 — fonts.** CoreText discovery + face loading, HarfBuzz shaping (with ghostty's
  shaping-break rules), fallback resolver, deferred faces, shared grid + metrics, atlas with
  bin packing, sprite rasterization (box/powerline/braille/blocks), nerd-font constraint
  codegen, emoji (color bitmap via CoreText) + VS15/VS16 handling. Exit: `+list-fonts` and
  `+show-face` CLI parity; golden-image sprite tests pass.
- **Phase 4 — renderer + native window.** Generic renderer core (cell building from dirty
  rows, bg array + fg rows, cursor styles, underline/strikethrough/overline decorations,
  minimum-contrast), Metal backend (IOSurface target, triple buffering), **offscreen target +
  readback and the `examples/frame-capture` embedding example**, render thread with timers
  (clock injectable per the embeddability requirements), kitty graphics rendering layers,
  background image, custom shader pipeline. Replace the egui window with a thin
  winit-or-AppKit window driving the Metal renderer. Exit: 120fps scroll on a 4k window;
  ghostty's custom shaders load and run; frame-capture example renders a scripted session to
  PNG frames deterministically (same bytes in → same pixels out, twice), and those frames are
  the golden-image test substrate.
- **Phase 5 — input + config + surface orchestration.** Full Binding.zig port (leader
  sequences, global/all/performable flags, 70+ actions), kitty keyboard protocol encode with
  progressive enhancement, mouse reporting modes + SGR/pixel encoding, IME plumbing, full
  config system in TOML (all option semantics — enumerate fields from Config.zig, don't trust
  this doc's counts), themes, hot reload, conditional config, and the
  `+import-ghostty-config` converter. App/Surface mailboxes and action dispatch
  (`ghostty-core`). Exit: a ghostty user's config, run through the converter, produces
  identical effective settings (`+show-config` diff vs ghostty).
- **Phase 6 — macOS app shell.** `ghostty-ffi` C ABI (cbindgen, mirror `include/ghostty.h`
  closely enough that ghostty's Swift sources can be adapted rather than rewritten), then the
  Swift app: window/tab/split management, quick terminal, secure input, clipboard
  confirmation, menu sync with keybinds. Strongly prefer adapting `macos/Sources/` from the
  Zig repo over greenfield Swift. Exit: .app that a Ghostty user can switch to.
- **Phase 7 — long tail.** Search (sliding-window regex over pages, on a thread), CLI action
  parity, inspector, perf work (SIMD UTF-8, benchmarks to parity), the software-raster
  backend ADR/implementation if the GPU-readback path proved insufficient for headless
  consumers, a **betamax integration spike** (port betamax's renderer to these crates and
  diff its output against the app — the acid test for embeddability), Linux/GTK spike ADR.

## Existing prototype disposition

`~/local/ghostty-rs` currently holds a ~4.4k-line spike (single crate, egui window, crossterm
host, `Vec<Vec<Cell>>` grid, 1000-row scrollback, partial VT coverage). Verdict: **the spike is
scaffolding, not a foundation.** Keep and carry forward: the replay-fixture harness and
fixtures, the xtask bundling approach, `docs/` process files, and the crossterm debug host
(as a `ghostty-vt` consumer for Phase 2). The grid/screen/parser will be superseded by the
Phase-1 port — don't incrementally mutate the spike's screen model into PageList; build
`ghostty-vt` clean against the Zig reference and port the spike's passing fixtures over as
acceptance tests. Salvage individual functions (OSC parsing, CSI param handling) only where
they match ghostty semantics.

## Parallel execution model: jj workspaces, one per chunk

Version control is **jj** (jujutsu), colocated with git. Parallel work uses jj workspaces —
one workspace per parallel chunk, integrated in the default workspace.

One-time setup (from the repo root, after `jj git init --colocate` if not already a jj repo):

```sh
jj workspace forget default
mkdir work
jj workspace add work/default
```

All work then happens under `work/`: `work/default` is the integration workspace; each
parallel chunk gets its own sibling:

```sh
jj workspace add work/vt-osc        # e.g. porting the OSC parser family
jj workspace add work/vt-pagelist   # e.g. the page memory model
jj workspace add work/font-sprite   # e.g. sprite rasterization
```

Rules:

- `work/qwertty/` is a shared drop-box for cross-thread coordination docs with the qwertty
  project — it is NOT a jj workspace; never `jj workspace add` over it.
- **One workspace per parallel chunk; one subagent per workspace.** The orchestrating session
  creates the workspace, tells the subagent its absolute path (`work/<chunk>/`), and the
  subagent does all its work there — it never touches `work/default` or sibling workspaces.
  Chunk names are short kebab-case matching the subsystem being ported.
- **Integrate in `work/default` only.** The orchestrator merges/rebases chunk changes there
  (`jj new <chunk-change> <trunk-change>` or rebase), resolves conflicts, runs the full gate
  (`cargo check --workspace`, then tests), and abandons/retires the chunk workspace
  (`jj workspace forget <name>`) when its work has landed.
- **Chunks must be genuinely independent.** Before fanning out, land the shared skeleton in
  `work/default` first: crate layout, module stubs, shared types/traits, `todo!()` bodies —
  so every chunk compiles against the same interfaces and merges are additive, not
  structural. If two chunks would edit the same files, they are one chunk.
- **Keep everything compilable, liberally `cargo check`.** `work/default` must pass
  `cargo check --workspace` at every integration point — treat a red trunk as a
  stop-the-line event. Chunk agents run `cargo check -p <crate>` after every edit burst, not
  just at the end. Prefer landing compiling stubs over long-lived broken branches. Each
  workspace has its own `target/` (don't share `CARGO_TARGET_DIR` across parallel builds —
  lock contention); disk is the accepted cost.
- **Right-size the model per chunk — default DOWN, escalate on evidence.** Concrete tiers:
  **Sonnet** for mechanical, well-specified work (porting an enumerated list of OSC parsers,
  transcribing inline tests, codegen plumbing, doc write-ups from an existing analysis,
  exploration/survey agents). **Opus** is the default for ordinary porting chunks, including
  design-y ones (parser, stream, selection, config, most of the renderer). **Top-tier
  (Fable-class) only by exception**, for the small set of chunks where subtle invariants
  make failure expensive (page/PageList unsafe memory core, FFI boundary design, threading
  redesign) — and even there, prefer Opus-first with a top-tier *verification* pass over
  top-tier doing the whole chunk. When a cheaper chunk fails its gate twice, escalate one
  tier. The orchestrating session does integration, conflict resolution, ADRs, and phase
  decisions itself; it should not burn top-tier budget on work a delegated Opus/Sonnet
  agent can do. Rationale: top-tier session budget is the scarce resource — spend it on
  judgment, not transcription.

## Working rules for agents

1. **The Zig source is the spec.** Before implementing any behavior, read the corresponding
   Zig file(s) and port their inline tests. When ghostty's behavior surprises you, it is
   almost certainly deliberate — check git blame/comments in the Zig repo before "fixing" it.
2. **Use subagents for scale.** Fan out Explore agents to map a subsystem before porting it;
   use parallel agents (one jj workspace each — see the parallel execution model above) for
   mechanical porting of independent OSC parsers/test blocks; use a verifying agent to diff
   ported behavior against the Zig original. Keep synthesis and design decisions in the main
   session.
3. **Analysis-first porting: write the analysis doc before the port.** For every subsystem
   chunk, the first artifact is `docs/analysis/<area>.md` — a maintainer-grade explanation of
   how the *Zig* implementation works: data structures and their invariants, memory layout,
   threading/ownership, the escape-sequence or API surface handled, edge cases the inline
   tests pin down, and file/line references into the Zig tree. Stamp each doc with the
   ghostty commit hash surveyed. These docs serve three audiences at once: the porting agent
   (forced understanding before code), future rewrite sessions (no re-exploration), and
   upstream ghostty maintainers (human or agent). Keep them utilitarian — accurate and dense
   beats polished; do not spend integration time wordsmithing them. The exploration cost is
   already sunk by the port itself; the doc is how that cost is not thrown away. Primary
   focus stays the rewrite: never block porting progress on documentation of areas not being
   ported. (Note: if these are ever offered upstream, ghostty's CONTRIBUTING.md has an AI
   disclosure policy — disclose accordingly.)
4. **ADRs for deviations.** Any departure from ghostty's design or from the defaults table
   above gets a short ADR in `docs/adr/NNN-title.md`: context, ghostty's approach, chosen
   approach, cost of switching back. (ADR 001 should record the TOML config decision.)
5. **Maintain the ledgers.** `docs/port-status.md` (file-by-file port/test/analysis-doc
   status), `docs/roadmap.md` (phase progress), `docs/handoff.md` (state for the next
   session). Update before ending every session.
6. **Quality gates every session**: `cargo fmt --check`, `cargo clippy --workspace`,
   `cargo test --workspace`, fuzz smoke (60s) when parser/stream changed, benches when the
   hot path changed. Between gates, `cargo check` early and often — a workspace that doesn't
   compile is debt accruing interest.
7. **Small, revertible changes** at subsystem boundaries; jj change descriptions reference
   the Zig files ported (e.g. `port: terminal/sgr.zig → ghostty-vt::sgr (49 tests)`).
8. **Don't trust this document's numbers over the source.** LOC counts and field counts here
   are survey estimates; enumerate from the Zig source when exactness matters (config fields,
   action list, mode list, OSC command set).

## Definition of done

A macOS .app, built from this workspace, that: passes the differential corpus against
libghostty-vt; imports an existing Ghostty user's config via `+import-ghostty-config` and
honors it — themes, custom shaders, and keybinds — with identical effective behavior; runs
vim/tmux/nvim/htop/fzf indistinguishably from Ghostty; renders nerd fonts, emoji, box
drawing, and kitty graphics at parity; and matches Ghostty's throughput benchmarks within
noise. Alongside it: a `docs/analysis/` corpus covering every ported subsystem, accurate to a
stamped ghostty commit; and the library goal proven — a betamax-class consumer can depend on
these crates alone (no Zig toolchain, no window, no app shell) and produce frames
pixel-identical to the app, deterministically.
