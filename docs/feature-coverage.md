# Feature coverage — qwertty-term vs Ghostty

Module-by-module feature matrix, built from Ghostty's own feature catalog at commit
`2da015cd6` (its ~230 `Config.zig` options, `Binding.zig` keybind actions, and terminal
modes) cross-referenced against what qwertty-term has shipped. Curated, not
per-sequence-exhaustive — each section can be deepened to individual-sequence granularity
by a dedicated audit thread.

Legend: `[x]` parity / working · `[~]` partial or reduced · `[ ]` not yet · `[—]`
deliberately not planned (deviation / non-goal).

**Read the percentages honestly — three caveats that make them smaller than they look:**

1. **The denominator is a 2026-07-06 pin, not today's Ghostty.** Upstream has landed **156
   commits** since `2da015cd6`. Coverage here means "vs the pin"; the live gap is larger.
   Drift is tracked separately in `docs/upstream/drift.md` (T8) — read both before planning.
2. **A single number per section averages two platforms that have diverged.** macOS (Metal +
   CoreText + AppKit) and Linux (OpenGL + FreeType + GTK) are now materially different
   completeness stories. Sections where that matters carry split figures. **Unless a line says
   otherwise, assume a `[x]` was built and verified on macOS**; the Linux column of the same
   feature may be absent, partial, or (for emoji) not working at all.
3. **`[x]` means the behavior works, not that it is configurable.** Several features are ported
   and hardcoded to upstream's default with no config key (`tab-inherit-working-directory`,
   `macos-option-as-alt`, `shell-integration-features`). Those are `[~]` and say so — but the
   distinction is easy to miss when planning "what's left".

Last audited **2026-07-16** (per-section adversarial audit against the code; corrections
below). Sections are otherwise updated by each thread at closeout, so freshness varies —
a section not listed in that audit may lag its code.

## At a glance

| Area                | macOS | Linux | Notes                                                  |
| ------------------- | ----- | ----- | ------------------------------------------------------ |
| VT engine           | ~85%  | ~85%  | Platform-free; the strongest area, differential-proven |
| Fonts & shaping     | ~70%  | ~55%  | Linux: no emoji/color, glyph constraints no-op         |
| Rendering           | ~70%  | ~50%  | Linux: no kitty images; `display-p3` would mis-render  |
| Input encoders      | ~78%  | ~78%  | Platform-free                                          |
| Keybind system      | ~85%  | ~10%  | GTK doesn't use the `Set` yet — bespoke table          |
| Config surface      | ~35%  | ~0%   | 73 keys wired; **the GTK app reads none of them**      |
| App chrome / window | ~50%  | ~25%  | Linux has tabs + headerbar; no splits, no IME          |
| Embeddability       | ~90%  | ~90%  | A qwertty-term goal beyond Ghostty; 8 crates at 0.4.0  |

**The honest one-liner:** the engine is close to done and platform-free; the *app* is much
further along on macOS than Linux; and config is the thinnest layer everywhere — most ported
behavior is reachable only at its default.

**Biggest gaps, in the order they hurt:**

1. **The GTK app ignores user config entirely** — font family/size, theme, and keybinds are
   hardcoded (`Face::load_embedded`, no theme wired, bespoke key table). Every config key
   listed as `[x]` below is macOS-only in practice. This is the single largest honesty gap
   between "what the doc says" and "what a Linux user gets".
2. **Emoji do not render on Linux** at all (four independent causes — see Fonts).
3. **Linux app chrome**: no splits (the *model* is portable and done — only a GTK view is
   missing), no IME/compose, no HiDPI scale.
4. **Kitty images** work on Metal only — not Software, not OpenGL.
5. **Config breadth** (~200 keys upstream, 73 wired) — and each new key needs wiring twice
   now that a second apprt exists.

**Cheap wins hiding in here:** `splits.rs`, `tabs.rs`, `quickterm.rs`, `selection.rs`,
`paste.rs`, `gesture.rs`, `context_menu.rs`, and `session.rs` are all **un-gated,
platform-free model code** the GTK app can consume today — Linux needs view layers, not ports.

## Terminal / VT engine (`src/terminal`, ~85% — certified, differential-proven)

- [x] Parser: CSI/OSC/DCS/APC/ESC state machine, UTF-8 decode, param overflow policy
- [x] Screen/grid: pages, scrollback, wide chars, graphemes, styles (ref-counted)
- [x] Cursor movement (CUP/CUU/CUD/CUF/CUB/CHA/VPA/HVP), scroll regions (DECSTBM/DECSLRM)
- [x] Erase/insert/delete (ED/EL/ICH/DCH/IL/DL/ECH), REP
- [x] SGR: bold/faint/italic/underline(+styles)/blink/inverse/invisible/strike, 16/256/truecolor
- [x] Modes: DECCKM, DECAWM autowrap, origin, insert, reverse, alt-screen 1047/1049
- [x] Mouse modes 1000/1002/1003/1005/1006/1015/1016, focus 1004, bracketed paste 2004
      (41-entry mode table — matches upstream `modes.zig`'s 41 exactly)
- [x] Synchronized output 2026, DECSCUSR cursor shapes
- [x] Charsets (G0–G3, DEC special graphics), DECALN
- [x] Kitty graphics protocol (transmit/place/delete; exec path)
- [x] Kitty keyboard protocol (progressive enhancement flags)
- [x] Kitty unicode placeholders (U=1)
- [x] OSC 0/1/2 title, 4/104 palette, 7 cwd, 8 hyperlink, 10/11/12 fg/bg/cursor, 52 clipboard
- [x] OSC 133 shell-integration marks, 22 pointer shape
- [x] DCS: DECRQSS (SGR/DECSCUSR/DECSTBM/DECSLRM), XTGETTCAP (full terminfo cap set)
- [x] Scrollback engine + viewport pins; Unicode grapheme break + width (UAX #29/#11, exact)
- [x] Selection model + literal-substring search (no regex, matches upstream)
- [x] Snapshot/formatter (owned styled grid + reply queue) — the embeddability seam
- [x] XTWINOPS size/title reports (14/16/18/21 t, extra-param-guarded); title stack
      push/pop (22/23 t) validated as upstream's apprt-level no-op seam
- [x] XTGETTCAP full terminfo capability set (268 caps + TN/Co/RGB) / DECRQSS at parity
- [x] OSC 9 / 9;4 (ConEmu progress) / 777 desktop notifications, iTerm2 OSC, kitty
      text-sizing / DnD / clipboard protocols, OSC 133 context signals — ~2.3k lines of
      shipped OSC parsers (`src/osc/parsers/`) that this matrix previously left uncredited
- [x] XTVERSION (`CSI > q`), XTSHIFTESCAPE, DECSCA, DECRQM; x11 color names; tabstops;
      hyperlink pages; pagelist reflow
- [ ] **APC glyph protocol** (`apc/glyph.zig` + `glyph/`, ~2.2k Zig LoC — seam only:
      `src/apc/mod.rs:19`, `src/stream.rs:3199`). The largest single unported VT feature and
      most of the residual ~15%; depends on the font subsystem
- [~] tmux control mode — engine parsers + DCS wiring done (slices 1–4); the native Viewer
      **core landed** (slice 5a: `qwertty-term/src/tmux_viewer.rs` — session→windows→panes
      tree, per-pane `Terminal`, `%output` routing, + `tmux_session.rs`/`tmux_reconcile.rs`).
      Remaining: the AppKit half (slice 5b, app-tails)
- [x] OSC 21 kitty color protocol (set/reset/query, 8-bit `rgb:` replies) — the query
      side is our forward-port of upstream `14c829883` (postdates the pin)
- [x] VT config toggles — wired end-to-end (TOML config key → `ControllerState` →
      engine setter, applied at surface build + live on reload): `title-report`
      (`set_title_reporting`, app default false overrides the engine's lib-parity true),
      `enquiry-response` (`set_enquiry_response`), `osc-color-report-format`
      (`set_osc_color_report_format`), `image-storage-limit`
      (`Engine::set_kitty_graphics_size_limit`, which delegates), `scrollback-limit`
      (`Options::max_scrollback`, construction-only per upstream), `vt-kam-allowed`
      (KAM mode 2 gates the key-input path in `encode_key_to_surface`/`send_text_to_surface`)

## Fonts & text shaping (`src/font`, ~70% macOS / ~55% Linux)

Checkmarks are CoreText/macOS unless stated. The FreeType+fontconfig stack that ships on
Linux is materially behind — emoji don't render and glyph constraints are a no-op.

- [x] `font-family` (+ discovery of styled members: bold/italic/bold-italic)
- [x] `font-family-bold`/`-italic`/`-bold-italic` explicit overrides
- [x] Bold/italic via variable-font `wght` axis + synthetic fallback ladder
- [x] Ligatures (rustybuzz shaping, run-based, live in the render engine)
- [~] Emoji — **macOS only** (Apple Color Emoji discovery, pre-seeded like upstream, color
      atlas). **Not working on Linux/FreeType**, at four independent layers, any one of which
      alone would break it: rasterization hardcodes `RenderMode::Normal` and returns
      `PixelFormat::Alpha8` with no `FT_LOAD_COLOR` (`freetype.rs:401,429,479` — "a later
      slice"); the pre-seed is `cfg`'d off whenever `freetype` is on (`collection.rs:195`);
      upstream's non-Darwin strategy — an embedded NotoColorEmoji fallback
      (`SharedGridSet.zig:357`) — is unported (we vendor only NotoEmoji-Regular, referenced
      solely by a manifest test); and `set_pixel_sizes` (`freetype.rs:139`) fails on
      CBDT strike-only faces, so a discovered emoji face wouldn't even load. Note the
      *resolution* logic (presentation-aware fallback) is well ported — only rasterization
      is missing. The existing `resolver.rs:510` test asserts cmap coverage, never
      rasterization, so it passes while the feature is entirely broken
- [~] Nerd-font glyphs + per-icon constraint sizing (codegen'd table, byte-exact) — the table
      and macOS path are real, but FreeType's `rasterize_constrained` (`freetype.rs:496`)
      **discards the constraint**, so on Linux Nerd icons render unscaled and overflow the cell
- [x] Procedural sprites: box drawing, blocks, braille, powerline, legacy computing
- [x] `font-size`, embedded default fonts (JetBrains Mono + symbols, vendored)
- [x] CoreText face + fallback discovery, byte-backed named faces
- [ ] `font-feature` (OpenType features passthrough) — **the shaper does not support it**:
      `shaper.rs:198` calls `rustybuzz::shape(&self.face, &[], buf)` with features hardcoded
      empty. rustybuzz can do it; nothing is plumbed. (Was `[~] shaper supports, config
      unwired`, which implied only the last mile was missing)
- [~] `font-variation*` (axes settable internally; config keys unwired) — **macOS only**
      (`coretext.rs:118`); nothing on FreeType, so 0% on Linux
- [ ] `font-thicken` / `font-thicken-strength` (config flags; default-off path matches)
- [x] `adjust-*` metric overrides (13 keys: cell width/height, font-baseline,
      underline/strikethrough/overline pos+thickness, cursor thickness/height,
      box-thickness, icon-height → font `Metrics::apply`; imports + live on reload)
- [ ] `font-codepoint-map`, `font-style*` name overrides, `grapheme-width-method` config
- [~] FreeType **face** backend (Linux) shipped — load/shape/rasterize/metrics/synthetic
      bold+italic + query parity, cfg-selected `Face` alias (ADR 003 P2). fontconfig
      **discovery wired** (`fontconfig` feature, dlopen; enabled on Linux via the renderer):
      `Descriptor`→`FcPattern`, `FcFontSort`-ranked `discover`/`discover_fallback`/
      `discover_family_style`, `FcDeferredFace`, now consumed by `collection` (styled-family
      members) and `resolver` (codepoint fallback, presentation-aware) — so installed system
      fonts resolve on Linux, not just the embedded + synthetic chain. Verified end-to-end against
      real system fonts. FreeType `Face::load_by_name` (named-family lookup) added via
      `fontconfig::discover_family` with the CoreText family-match + embedded-fallback semantics.
      FreeType `LoadFlags` (`hinting`/`force-autohint`/`autohint`) honored during rasterization
      (upstream `glyphLoadFlags` semantics; config-key parsing awaits the Linux apprt/T3 — FYI
      filed). Deferred: `monochrome` load flag, real wght-variation bold
- [x] Uncredited until now, but shipped and load-bearing: `atlas.rs` (full `Atlas.zig` port —
      reserve/set/grow + generation counters, grayscale **and** BGRA), `constraint.rs` (the
      general constraint engine), `discovery.rs` `Score` ranking, `metrics.rs` cell-metrics
      derivation, and presentation-aware resolution (`PresentationMode`)
- [ ] `font.rs` `Backend` enum is stale: its doc still says "Only `Backend::CoreText` is
      implemented today" and `default_for_platform` is macOS-gated — it contradicts the whole
      FreeType/fontconfig stack above and won't compile on Linux

## Rendering (`src/renderer`, ~70% Metal / ~50% OpenGL)

Checkmarks are Metal unless stated. Three `GpuBackend` implementors now exist behind the
generic `Engine<B>` (`gpu.rs:35`, `engine.rs:168`; default `Metal` on macOS, `Software`
elsewhere):

| Backend  | State                                                                                 |
| -------- | ------------------------------------------------------------------------------------- |
| Metal    | Complete; the only one with a windowed present path on macOS (IOSurface→CALayer)      |
| Software | Partial — kitty images are a no-op, **color/emoji glyphs skipped**, no padding-extend |
| OpenGL   | Near-complete for text (colour/emoji atlas upload works); **kitty images do not**     |

- [x] Metal backend, IOSurface-on-CALayer presentation, retina/contentsScale
- [x] Upstream shaders verbatim; frozen wire structs; grayscale + color atlases
- [x] Per-row dirty tracking (equality-proven vs full redraw)
- [x] Run-based shaping cache; `alpha-blending` native, `minimum-contrast`
- [x] `background-opacity`, `display-p3` / `window-colorspace` — **macOS/Metal only**. On
      OpenGL this is a live trap, not merely absent: the `Uniforms.bools` wire packs four
      one-byte bools while the GLSL reads a bit-flag `uint` (`opengl/mod.rs:27-38`). It works
      today *only* because the engine hardcodes three of them to `false`; wiring `display-p3`
      or `linear-blending` would make GL render **wrong**, not fail loudly
- [~] Timer-based frame pacing (CVDisplayLink not yet wired)
- [ ] `background-image` (+ fit/position/repeat/opacity)
- [ ] `background-blur`, `background-opacity-cells`
- [ ] `custom-shader` (shadertoy) + animation
- [x] Kitty image *rendering* (R6 COMPLETE — RGBA transmit→texture→placement quads incl.
      unicode placeholders, scrollback tracking + viewport clip/cull, delete/eviction +
      storage-limit texture reclaim, live-app rendering (kitty data through `SnapshotWindow`),
      and z-order buckets (below-bg / below-text / above-text); offscreen readback +
      dirty-equality-proven. Follow-up perf note in #19: `Image.data`→`Arc` for copy-free)
- [x] Link detection + hover underline + cmd-click open (R7 COMPLETE, T2): OSC8 hyperlinks
      **and** regex-detected URLs underline on hover; cmd(super)+click opens via `open`
      (#181/#184/#189 OSC8, #194/#210 regex, #220 click). Deferred: `link-url` config key
      (T3 wiring) + `link-previews` (hover-preview popups) — separate features, not started
- [x] **OpenGL backend (GL 4.3 core, Linux)** — `impl GpuBackend for OpenGL`
      (`opengl/mod.rs:363`), upstream GLSL vendored verbatim, driving the same generic
      `Engine<B>`. Both present paths work: headless (surfaceless EGL) and on-screen (GTK4
      GLArea `glBlitFramebuffer`), with differential pixel-parity against the Software backend
      (`tests/opengl_headless.rs`). Requires **desktop GL 4.3 core** — the shaders are
      `#version 430 core` and bind SSBOs — matching upstream's own floor (`OpenGL.zig:36-38`);
      no GLES path, no fallback
- [ ] Kitty images on **OpenGL / Software** — Metal-only. Not a clean unimplemented path on
      GL: every texture is created `GL_TEXTURE_RECTANGLE` (`opengl/texture.rs:5`) while
      `image.f.glsl:3` declares `sampler2D`, so the image pipeline compiles and the generic
      engine will happily encode image draws into a mismatched sampler. Treat kitty graphics
      as **not done** on Linux
- [ ] `Backend` enum doc (`backend.rs:1-19`) still claims the crate "doesn't yet pick or link
      against any GPU API" with a `TODO(chunk:R2+ GPU backend)` — false since Metal/OpenGL
      landed

## Colors & theming

- [x] `theme` (Ghostty theme-file format loaded), 256-color palette + dynamic palette
- [x] `background`/`foreground`/`cursor-color` config overrides (seed startup Colors,
      live on reload; program OSC 10/11/12 still win); imports
- [x] `selection-background`/`-foreground` (theme + config override, per-channel);
      `cursor-text` (theme only — no Colors slot yet)
- [x] `split-divider` (implicit), search highlight colors
- [x] `palette` per-index overrides (`N=color`, on top of theme; OSC 4 still wins; imports)
- [~] `bold-color`, `cursor-opacity`, `faint-opacity` (some wired, some not)
- [x] `window-theme` (`auto`/`system`/`light`/`dark`): maps to each window's
      `NSAppearance` (`auto` picks light/dark by terminal-background luminance,
      matching upstream `NSAppearance+Extension.swift`); live on reload; `ghostty`
      (config-colored titlebar) is Linux-only → `system` on macOS (windowchrome smoke)
- [ ] `palette-generate`/`palette-harmonious`
- [ ] `cell-foreground`/`cell-background`, `background-opacity-cells`

## Cursor

- [x] Styles: block, bar, underline (+ `cursor-style`), hollow when unfocused
- [x] Bar-at-prompt via shell integration (DECSCUSR)
- [x] Hidden when scrolled into history
- [~] `cursor-style-blink` (style set; blink *mode* DEC 12 now threads through
      `SnapshotCursor.blinking` + gates via `FrameOptions.cursor_blink_visible` (#57, T2);
      blink *timer* animation still not implemented)
- [x] `cursor-color` config override (seeds startup `Colors.cursor`, live on reload; imports)
- [ ] `cursor-click-to-move`, `cursor-opacity`

## Window & app chrome

- [x] Native AppKit window, content-flush layout
- [ ] `window-padding-x/y` — **not implemented at all** (no config key; `geometry.rs:5`
      states the reduced R5 cut "uses no window padding", and `padding_left` is hardcoded 0).
      Previously listed `[x]`, which was simply wrong
- [ ] `window-decoration` — no config key; the style mask is hardcoded
      `Titled|Closable|Miniaturizable|Resizable` (`app.rs:5799`)
- [x] Native fullscreen (`toggleFullScreen:`)
- [ ] Non-native fullscreen (`macos-non-native-fullscreen`) — no such path exists
- [x] Menu bar (basic), key-window activation
- [x] `window-height`/`-width`/`-position-x`/`-position-y` (initial geometry: cells → first
      window; note upstream's position is two keys, not one)
- [x] `title` (fixed window/tab title override; forces over OSC 0/2, live on reload; imports)
- [x] `window-save-state` (default/never/always): config-gates macOS native restoration
      (`NSQuitAlwaysKeepsWindows` + per-window `isRestorable`; savestate smoke). Content restore
      complete — a tab's split tree + per-pane cwd captures to a serializable `WindowSession`,
      round-trips through JSON, and rebuilds into a tab: single-pane and multi-pane (full
      structure + per-split ratios, one shell per leaf in its saved cwd). OS wiring in place:
      each restorable window carries a restoration identifier + names the app delegate as its
      `restorationClass`, and the session encodes/decodes through the real `NSCoder` path
      (`willEncodeRestorableState` → JSON `NSString`; `didDecodeRestorableState` → rebuild).
      Session unit tests + smoke cover the tree round-trip and a live `NSKeyedArchiver` coder
      round-trip; macOS actually firing restoration on a real quit+relaunch is manual-verify only
- [x] `window-step-resize`: when true, `NSWindow.contentResizeIncrements` is set
      to the focused cell size so the window resizes in whole-cell steps (upstream
      `BaseTerminalController.swift:884`; windowchrome smoke)
- [x] `window-subtitle` (`false`/`working-directory`): upstream ships this on GTK
      only; mapped natively onto `NSWindow.subtitle`, tracking the focused pane's
      cwd, re-applied on the pace tick (windowchrome smoke)
- [x] `window-new-tab-position` (`current`/`end`): a new tab groups against the
      active tab (`current`) or the last tab in the group (`end`), matching
      upstream `TerminalController.swift:456` (windowchrome smoke)
- [—] `window-titlebar-background`/`-foreground`: GTK-only upstream — both take
      effect only when `window-theme = ghostty`, itself a Linux-only mode
      (`Config.zig:2272`/`2279`); not applicable to the macOS titlebar
- [x] `resize-overlay` (+ `-position`, `-duration`): `cols ⨯ rows` HUD (NSTextField overlay)
      on live resize, positioned per config, auto-hiding after the duration (resize smoke)
- [ ] `command-palette`, undo/redo (`undo-timeout`)

## Tabs

- [x] Native NSWindow tabs, `new_tab`/`close_tab`/`goto_tab` (Cmd+1–9)
- [ ] `move_tab` — `Action::MoveTab` parses (`binding/action.rs:847`) but nothing dispatches
      it in the app crate. Was listed inside an `[x]` line
- [~] `tab-inherit-working-directory` (OSC 7 pwd) — behavior works (`tabs.rs:109`) but is
      **hardcoded on**; upstream's key is a settable bool, so `false` is a no-op here
- [x] Tab bar visible only at 2+ tabs; Ctrl+Tab cycling
- [x] Live tab titles from OSC 0/2 (per-tab window/tab-label sync, ghost-emoji
      fallback after the 500ms grace — title smoke)
- [ ] `set_tab_title` keybind action (needs the Binding.zig system — T3)
- [x] `window-show-tab-bar` policy (`auto`/`always`/`never`): mapped onto
      `NSWindowTabbingMode` (`.automatic`/`.preferred`/`.disallowed`) — upstream is
      a GTK feature; macOS gets the same config surface (windowchrome smoke)
- [ ] `gtk-tabs-location`/`gtk-wide-tabs` (Linux)

## Splits (`src/apprt` + Splits, slice 1+2 done)

**Portable:** the whole split *model* (`splits.rs`, 1386 lines — split/close/neighbor/
adjacent/hit_test/resize_dir/toggle_zoom/equalize/layout) is un-gated pure geometry with zero
objc2. Only `splitview.rs` (the NSView container + divider chrome) is AppKit. GTK needs a view
layer, not a port. The same holds for `tabs.rs` and `session.rs` (split-tree save-state).

- [x] `new_split` (Cmd+D / Cmd+Shift+D), `goto_split` directional + prev/next
- [x] `resize_split` chords, `toggle_split_zoom`, equalize
- [x] Divider drag, close-collapse, per-pane io/focus/scrollback isolation
- [x] Unfocused-split dimming (`unfocused-split-opacity`, `-fill`)
- [~] `split-inherit-working-directory` — hardcoded on (`app.rs:5231` calls `inherit_pwd`
      unconditionally); no config key, so upstream's settable bool is not honored
- [ ] `split-preserve-zoom`, `split-divider-color` config, drag-to-reparent

## Quick terminal & extra surfaces

Portability note: `quickterm.rs` (position/size parsing + origin geometry) is un-gated and
reusable, but the *placement* half won't transfer — it assumes AppKit's Y-up
`NSWindow.setFrame` coords, and Wayland has no global window positioning at all.

- [x] Quick terminal (dropdown): borderless key window, `quick-terminal-position`
      (top/bottom/left/right/center), `-size` (%/px per axis), `-animation-duration`,
      `-autohide`; in-app toggle (Cmd-`, View menu). Global hotkey deferred (ADR 0002).
- [ ] `new-window`/`new-tab` from CLI/AppleScript beyond the default first window

## Process, launch & lifecycle

- [x] Spawn `$SHELL`, inherit env/cwd; command override seam (**`QWERTTY_TERM_COMMAND`** —
      the doc previously named `GHOSTTY_RS_COMMAND`, which no longer exists)
- [x] cwd inheritance (OSC 7)
- [ ] `working-directory` config key — no such key exists; only OSC 7 inheritance
- [~] `command` / `-e` initial command (env-override path exists; full `-e` CLI parse partial)
- [ ] `initial-command`, `initial-window`, `wait-after-command`
- [x] `quit-after-last-window-closed` (default false on macOS) + `confirm-close-surface`
      (false/true/always; running-process decided by OSC 133 prompt state, confirm modal on
      Cmd-W / context-menu Close Pane / windowShouldClose — confirmclose smoke)
- [ ] `abnormal-command-exit-runtime`, `window-inherit-working-directory`/`-font-size`

## Input & keybindings (`src/input`, ~78% encoders / ~85% bind system, ~90% of actions dispatched)

The old `~10% bind system` headline was a fossil from before the b1–b2 slices: the port is
3,712 lines across `binding/` — 85-action enum, 10-rule `Trigger::parse`, the runtime `Set`,
and 93 upstream-verified macOS defaults (asserted exactly by a test). The real shortfall is
*dispatch* (8 actions still inert) and the gaps named below, not the port.

**Linux caveat:** the GTK app does **not** use the `Set` — it has its own bespoke shortcut
table (`qwertty-term-gtk/src/app.rs:719`). The "all four bespoke key tables are retired"
claim below is true of the **macOS app only**; Linux has since introduced a fifth.

- [x] Kitty keyboard encoding, full legacy encoder, 117-entry macOS keymap
- [x] Bracketed paste, `macos-option-as-alt`
- [x] Byte-emitting keybinds `text:` / `esc:` / `csi:` (e.g. shift+enter, the
      default `alt+left`=esc:b word-motion) — dispatched through the ported
      `Binding.zig` `Set` (`crate::keybind::build_set` / `resolve_text_bytes`)
- [x] Font-size + clipboard keybind actions (`increase`/`decrease`/`reset_font_size`,
      `copy_to_clipboard`/`paste_from_clipboard`) dispatched through the `Set` (rebindable;
      default cmd shortcuts still also fire via their menu items). Font-size folds the
      point delta to a fixed step; copy is plain-text (no primary selection on macOS)
- [~] `Binding.zig` port in `qwertty-term-input::binding`: trigger/action/flags model
      + parse layer (10-rule `Trigger::parse`, 85-action enum + `Action::parse`, flag
      prefixes, `=`-splitter, chain + sequence parsing, compat table) **and** the
      runtime `Set` (case-folded `mods.binding()` lookup, 5-probe `get_event`, `put`
      overwrite, **reverse action→trigger map** `get_trigger` for menu accelerators)
      **and** the full macOS `default_set()` (93 upstream-verified default binds)
      **and** `parse_and_put` (config-string application: `>`-sequences/leaders,
      `chain=`, `unbind`, with validate-before-mutate + empty-leader pruning). The
      whole config→`Set` build path is done. **App-crate dispatch (slices b1–b2):**
      all four bespoke key tables are retired — the user `keybind` text seam (b1) and
      the tab/split/search chords (b2) now resolve through one unified `Set`
      (`default_set()` + user config) at the `keyDown:`/`performKeyEquivalent:` seam.
      macOS split/search/tab chords are now upstream's exact defaults. Scroll, font-size,
      clipboard, and window/tab-lifecycle (`new_window`/`new_tab`/`close_surface`/
      `close_tab`/`toggle_quick_terminal`/`toggle_fullscreen`) action categories are now
      dispatched too (e.g. the default ctrl+Enter → fullscreen, a non-menu bind, now works),
      plus the `write_*_file` family. Remaining: actions needing new engine/selection
      behavior (`clear_screen`, `select_all`, `jump_to_prompt`) + `performable` menu
      fallthrough.
- [~] `Binding.zig` runtime: **leader-key sequences** (`ctrl+a>c`) **and `chain=`
      multi-action bindings dispatched** (`handle_key_sequence` + `resolve_actions`
      over the `Set`'s `Leader`/`Leaf`/`LeafChained` storage). Remaining: sequence
      idle-timeout + flush-on-abort, key tables, `global` binds, `performable`
      fallthrough.
- [x] Scroll keybind actions: `scroll_to_top`/`_to_bottom`/`_page_up`/`_page_down`/
      `_page_lines`/`_page_fractional` move the focused pane's scrollback viewport
      (default Cmd/Shift + Home/End/PageUp/PageDown); `config-reload` wired
- [x] `write_scrollback_file`/`write_screen_file`/`write_selection_file` (copy/paste/
      open): dump the focused pane's scrollback / viewport / selection to a temp file,
      then copy/paste/open the path (plain format only; `vt`/`html` fall back to plain)
- [ ] Keybind *actions* still needing new behavior: `jump_to_prompt`, `inspector`,
      `adjust_selection`, `select_all`, `clear_screen`, `scroll_to_selection`/`_to_row`,
      `crash`
- [x] `keybind` config parsing — the full trigger/action grammar (not just `text:`)
      parses and, for the wired action categories, dispatches
- [~] Non-macOS `default_set()` — `build_other()` (`binding/defaults.rs:459`) ships 35 default
      binds for Linux/other, dispatched via the `cfg(not(macos))` branch. Partial vs macOS's
      93, and unused by the GTK app (see the caveat above); previously uncredited
- [ ] `key-remap` (`RemapSet` ported but unwired — issue #23)

## Mouse

- [x] Reporting (5 formats), wheel → scrollback / alternate-scroll ladder
- [x] `mouse-scroll-multiplier`, shift-to-select over reporting
- [x] `context-menu` (right-click menu): Copy (on selection)/Paste/Split ×4/Close,
      per-pane; `right-click-action` (context-menu/paste/copy/copy-or-paste/ignore)
- [x] `mouse-hide-while-typing` (hide on keystroke, reveal on move)
- [x] `focus-follows-mouse` (per-pane NSTrackingArea → `mouseEntered:` focuses the pane)
- [x] `middle-click-action` (`primary-paste` pastes the selection / `ignore`) — mouse2 smoke
- [x] `mouse-shift-capture` (`false`/`true`/`always`/`never`): gates whether shift overrides
      mouse reporting for selection, combined with the program's runtime XTSHIFTESCAPE request
      (`Surface.mouseShiftCapture` port; config unit tests + mouse-shift smoke)
- [ ] `cursor-click-to-move` (OSC133 zone)

## Clipboard & selection

- [x] `copy-on-select`, OSC 52 read/write, selection string extraction
- [x] Double-click-drag select (basic); per-pane selection
- [x] Double-click *word* / triple-click *line* gestures (+ shift-click extend,
      ctrl/cmd-triple-click output select, option rectangle select, drag-past-
      edge viewport autoscroll — `SelectionGesture.zig` port, selection smoke)
- [x] `clipboard-paste-protection`/`-bracketed-safe` (confirm unsafe/multiline
      pastes), `clipboard-trim-trailing-spaces`, `selection-clear-on-typing`
- [x] `selection-word-chars` (per-config word-boundary set) + `click-repeat-interval`
      (double/triple-click window) — parsed to codepoints/`Duration` and threaded into the
      gesture layer's `selection_press`/`selection_drag` + click interval (#30, wordchars smoke)
- [x] `selection-clear-on-copy` (clear the selection after an *explicit* copy — the
      `copy_to_clipboard` action / Cmd-C / menu — but not after copy-on-select, matching
      upstream; config unit test + clear-copy smoke)
- [ ] `clipboard-read`/`clipboard-write` permission gates
- [~] Primary selection / `primary-paste` (Linux) — the GTK app copies to **both** CLIPBOARD
      and PRIMARY on an explicit copy, and middle-click pastes PRIMARY
      (`qwertty-term-gtk/src/app.rs:775-936`). Missing: `copy-on-select` → PRIMARY isn't wired
      on Linux, which is the single most expected PRIMARY behavior on X11/Wayland
- Portability: `selection.rs` (669 lines), `paste.rs`, `context_menu.rs`'s item model and
  `gesture.rs` are all un-gated and **already consumed by the GTK app** — for Linux this is
  wiring, not porting. Only `clipboard.rs` (NSPasteboard) is macOS-only, and
  `gesture::click_interval()` is macOS-gated with **no non-macOS fallback** (GTK hardcodes its
  own 500ms) — a small real gap

## Shell integration (`src/shell-integration`)

- [x] Vendored bash/zsh/fish scripts (byte-identical, sha256 manifest)
- [x] OSC 133 prompt marks, OSC 7 cwd, bar-cursor-at-prompt
- [x] `shell-integration` auto-detect + injection (ZDOTDIR indirection)
- [~] `shell-integration-features` granular toggles — parsed and emitted into
      `GHOSTTY_SHELL_FEATURES` (`shell_integration.rs:60-90,250`), but **not exposed as a
      config key**: `termio.rs:130` hardcodes upstream defaults, so the toggles are
      unreachable for users
- [ ] `ssh-env` / `ssh-terminfo` propagation, `jump_to_prompt` navigation

## Config system (`src/config`, ~35% of the key surface — the *format* is a deliberate deviation)

The old `~5%` conflated two things: the TOML-instead-of-Ghostty-format decision (a real
non-goal — 100% is not the target) and the breadth of keys wired (which has grown ~7× since).
**The GTK/Linux app reads none of this** — every key below is macOS-only in practice.

- [x] TOML config — **73 Ghostty-named keys** wired (`config.rs`, 2422 lines): theme,
      font + 17 `adjust-*`, palette/colors, 4 `quick-terminal-*`, clipboard/selection,
      bell/notify/progress, 9 `window-*`, macOS chrome, `scrollback-limit`, keybinds
- [x] `theme` resolution via Ghostty theme files
- [—] Ghostty's custom config format (replaced by TOML — ADR)
- [x] `+import-ghostty-config` converter — Ghostty `key = value` → qwertty-term TOML;
      data-driven mapping of every real `Config` field (with a drift-guard test),
      format-mismatch keys flagged as `# needs manual conversion`, unknown keys preserved
      as comments; the maintainer's real config is the acceptance test
- [~] `config-reload` action (default `cmd+shift+,`) — re-reads config and re-applies
      keybinds, copy-on-select, scroll-multiplier, **and the theme live** (palette + fg/bg/
      cursor + selection colors pushed into every surface's engine, forced full repaint
      via `PageList::mark_all_dirty`). Fonts/cursor-style/padding re-apply deferred
      (need font-grid/window rebuild, config-core.md §7)
- [x] `config-file` includes — deferred breadth-first queue, cycle detection,
      `?optional` prefix, relative-to-including-file resolution; generic TOML merge
      (last-wins scalars, append arrays)
- [x] Two-location merge (XDG + macOS App Support, last-wins) + CLI `--key=value`
      overrides (highest precedence; captured in a `OnceLock` so they replay on reload;
      invalid overrides dropped, never fatal)
- [ ] `config-default-files`
- [~] Full option surface (~200 keys) — **73 wired**; the rest map to features listed
      elsewhere here. Note each new key now needs wiring twice (AppKit + GTK)

## Notifications, bell, progress

- [~] Bell (`bell-features`): system beep + dock attention + 🔔 title indicator
      on BEL, cleared on refocus (bell smoke). Deferred: `audio`
      (`bell-audio-path`/`-volume`) + `border` flash
- [~] `desktop-notifications`: OSC 9 / OSC 777 parsed → gated → throttled (1/sec +
      5s-identical, upstream core policy) → delivered (dock attention + log; notify
      smoke). Real `UNUserNotificationCenter` banner deferred to the bundling
      milestone (ADR 0003). `app-notifications` still open
- [~] `notify-on-command-finish` (+ `-action` bell/notify, `-after` threshold): OSC 133
      `C`/`D` boundary tracking in the VT engine → per-surface timing → mode/threshold gate
      → bell and/or notification (notifycmd smoke). `abnormal-command-exit-runtime` still open
- [~] `progress-style` (OSC 9;4 progress bar): ConEmu report → vt hook → gated state
      (set/error/indeterminate/pause/remove, 0–100, 15s auto-clear) → CALayer bottom-strip
      overlay over the pane (progress smoke). Reduced to an on/off toggle (upstream is a style enum)

## Platform: macOS (`macos/Sources`, ~45% reimplemented natively in Rust)

- [x] Native window/tabs/splits/menu/IME (NSTextInputClient), theme, selection
- [x] Retina, key-window activation
- [~] `macos-option-as-alt` — the encoder logic is real (`key_mods.rs:194`), but there is **no
      config key**; `translate.rs:75` hardcodes `OptionAsAlt::False`, so users can't set it
- Scope note: this section's `~45%` is against upstream's *app layer* (window/tabs/splits/
  menu/IME), which is defensible by its own checkbox census. It is **not** 45% of macOS
  *options*: upstream has 20 `macos-*` config keys and we wire 2 (10%)
- [~] `macos-titlebar-style` (tabbed layout works; style variants partial)
- [ ] `macos-secure-input` (+ indication/auto), `macos-custom-icon`/`-icon*`
- [ ] `macos-menu-bar`, `macos-applescript`, `macos-shortcuts`, `macos-dock-drop-behavior`
- [x] `macos-window-buttons` (`visible`/`hidden`): `hidden` hides the close/
      miniaturize/zoom traffic-light buttons (upstream `TerminalWindow.swift:570`);
      `macos-window-shadow` (default true): drives `NSWindow.hasShadow`
      (upstream `TerminalWindow.swift:476`) — both windowchrome smoke
- [ ] `-glass-*`, `-titlebar-proxy-icon`
- [ ] Sparkle `auto-update`, `macos-hidden`

## Platform: Linux / GTK (windowed app shipped and usable; config unwired)

`cargo run -p qwertty-term-gtk` is a real GTK4/libadwaita terminal — not a scaffold. The
honest summary: **it works as a terminal, and ignores your configuration completely.**

Requires **desktop GL 4.3 core** (`#version 430 core` + SSBOs) — upstream's own floor
(`OpenGL.zig:36-38`), no GLES path, no fallback. In a Mac VM: VMware Fusion supplies 4.3
(software); QEMU/virgl cannot (no core profile) — use `LIBGL_ALWAYS_SOFTWARE=1` (llvmpipe =
GL 4.5). Build floor is GTK 4.6 / libadwaita 1.0 (Debian bookworm = 4.8 / 1.2).

- [x] GTK4 + libadwaita app (`crates/qwertty-term-gtk`, ADR 005 P4): `adw::Application` →
      window → `GLArea` presenting `Engine<OpenGL>`, real shell via termio, FreeType glyphs
- [x] Keyboard input + shell env; mouse; **selection + clipboard copy/paste + PRIMARY +
      right-click context menu**; window resize (re-grid + `TIOCSWINSZ`)
- [x] Tabs (`AdwTabView`: new/close/next/prev/goto, cwd inheritance); headerbar + hamburger
      menu + live terminal titles (OSC 0/2)
- [x] Headless CPU render backend + FreeType font stack (ADR 003 P1/P2) — `Engine<Software>`
      renders terminal frames with no GPU / no CoreText (see Embeddability)
- [ ] **Config — nothing is wired.** Font family/size are hardcoded (`Face::load_embedded`),
      no theme is applied (selection uses inverse video as a stand-in), and keybinds use a
      bespoke table rather than the `Set`. All 73 config keys are macOS-only in practice.
      This is the largest gap between this document and a Linux user's experience
- [ ] Splits — the model (`splits.rs`) is portable and done; only a `gtk::Paned` view is missing
- [ ] IME / compose (`TODO(ime)`: needs `GtkIMMulticontext`), HiDPI scale (`TODO(scale)`:
      widget px == device px), live encode modes (`TODO(modes)`: DECCKM/kitty flags aren't
      threaded, so arrows misbehave in vim), preferences UI (`TODO(prefs)`)
- [ ] Emoji / color glyphs (blocked in the font layer — see Fonts), kitty images (blocked in
      the GL backend — see Rendering), dirty-tracked redraw (a 60Hz tick redraws unconditionally)
- [ ] `gtk-*` config keys (~10), `linux-cgroup*`, Wayland/X11-specific glue, notifications/portals
- [~] **fontconfig discovery** wired (ADR 003 P2): styled-family members + codepoint fallback
      resolve from installed system fonts via fontconfig (dlopen), enabled on Linux through the
      renderer. `force-autohint`/`freetype-load-flags` FreeType flags still deferred.
- [~] **Linux pixel-test coverage** (#42): the renderer's acceptance pixel tests run on the Linux
      CI lane over the `Software` backend + FreeType — real glyph/sprite/baseline/ligature coverage
      beyond the headless smoke. Un-gated: `bold_italic_pixels`, `sprite_specimen`, `text_baseline`,
      `default_fg_ink` (both cases), `ligature_pixels` (via FreeType `load_by_name`). Deferred
      (need Software color/image compositing): `emoji_pixels`, `kitty_image_pixels`, cursor tests;
      `first_pixels` stays the Metal IOSurface-readback proof.

## Embeddability / library (a qwertty-term goal beyond Ghostty)

- [x] Headless offscreen render + RGBA/PNG readback (`examples/frame-capture`)
- [x] Headless render on **Linux** — no GPU, no window, no CoreText, no Zig (pure cargo):
      `Engine<Software>` (CPU compositor) over the FreeType font stack, run on the Linux CI lane
      (`tests/software_headless.rs`). The betamax headless-Linux artifact — ADR 003 P1/P2
      (PR-1 #135 trait → PR-2 #172 `Software` backend → PR-3 #187 `Engine<B>` → PR-4 #209 un-gate)
- [x] VT / fonts / renderer as plain Rust crates, no global state
- [x] Injectable fonts; deterministic output (betamax reference consumer)
- [x] Embedding guide + one-call render API (`docs/embedding.md`; `Engine::render` →
      `Frame`, `FullSnapshot::capture_live`, `Engine::for_grid`)
- [x] MB4: betamax's offscreen render path (via `qwertty-term-renderer`) is exercised by
      `examples/frame-capture`; betamax's own adoption tracked in the betamax repo
- [x] Injectable clock: deterministic render proven; cursor-blink *phase* injected via
      `FrameOptions.cursor_blink_visible`, and the blink *mode* (DEC 12) now threads through
      `SnapshotCursor.blinking` (#57, T2)
- [x] **crates.io publish — all 8 crates, latest 0.4.0** (`qwertty-term` + `-vt`/`-font`/
      `-renderer`/`-termio`/`-input`/`-sprite`/`-ffi`; 0.1.0 2026-07-08, 0.2.0 2026-07-13,
      0.3.0 2026-07-14, 0.4.0 2026-07-15 via release-plz + Trusted Publishing, docs.rs built).
      **0.5.0 is queued in an unmerged release-plz PR, not released**
- [ ] **DECISION NEEDED — `qwertty-term-gtk` publishing.** The new GTK crate has **no
      `publish = false`** and inherits `version.workspace`, so merging the queued 0.5.0
      release-plz PR would **first-publish it to crates.io**, irreversibly claiming the name
      (yanking never frees a name). Meanwhile `release-plz.toml`, `CHANGELOG.md` and
      `docs/embedding.md` all still say "eight crates". Decide *before* that PR merges:
      publish a GTK app crate deliberately, or add `publish = false`
- [x] MB5 API polish (Display/Error on font errors [already in 0.1.0]; matched `Engine::for_grid`;
      typed `Frame` RGBA readback; one-call `Engine::render`; `Stream::terminal()`;
      `capture_live`) — shipped in #5; docs.rs full-API + quickstart in #51

## Advanced / tooling

- [x] Differential testing vs `libghostty-vt`, resize-interleaved fuzzing, Miri
- [x] vtebench lane — **note the scoreboard predates the #305/#307 print-scan NEON wins**
      (light_cells ~+14%, medium_cells ~+11%, dense_cells ~+9%, redraw ~+24%), so the 6/10
      figure below is a floor; a refresh is the open perf deliverable (`status/perf.md`).
      (T1 perf tuning landed: wins every suite vs Ghostty 1.3.1, and
      win/tie 6/10 vs Ghostty `main` — region scrolls closed from 1.27–1.47× to 1.13–1.20×
      by #204. The `unicode` 0.50× is a whole-app render-pipeline artifact, not an engine
      lead — our wide *engine* is still ~2.6× behind main. See `docs/benchmarks/vtebench-baseline.md`
      + `docs/analysis/stream-throughput-vs-upstream.md`)
- [x] `write_scrollback_file`/`write_screen_file`/`write_selection_file` actions
      (copy/paste/open the dumped path)
- [ ] Inspector / debug overlay
- [ ] `command-palette`, APC glyph protocol, animation
      (`resize-overlay` was listed here as `[ ]` a third time — it is done; see Window chrome)
