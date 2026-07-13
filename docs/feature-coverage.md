# Feature coverage — qwertty-term vs Ghostty

Module-by-module feature matrix, built from Ghostty's own feature catalog at commit
`2da015cd6` (its ~230 `Config.zig` options, `Binding.zig` keybind actions, and terminal
modes) cross-referenced against what qwertty-term has shipped. Curated, not
per-sequence-exhaustive — each section can be deepened to individual-sequence granularity
by a dedicated audit thread.

Legend: `[x]` parity / working · `[~]` partial or reduced · `[ ]` not yet · `[—]`
deliberately not planned (deviation / non-goal). macOS is the target platform; Linux/GTK
items are `[ ]` wholesale unless noted.

## Terminal / VT engine (`src/terminal`, ~85% — certified, differential-proven)

- [x] Parser: CSI/OSC/DCS/APC/ESC state machine, UTF-8 decode, param overflow policy
- [x] Screen/grid: pages, scrollback, wide chars, graphemes, styles (ref-counted)
- [x] Cursor movement (CUP/CUU/CUD/CUF/CUB/CHA/VPA/HVP), scroll regions (DECSTBM/DECSLRM)
- [x] Erase/insert/delete (ED/EL/ICH/DCH/IL/DL/ECH), REP
- [x] SGR: bold/faint/italic/underline(+styles)/blink/inverse/invisible/strike, 16/256/truecolor
- [x] Modes: DECCKM, DECAWM autowrap, origin, insert, reverse, alt-screen 1047/1049
- [x] Mouse modes 1000/1002/1003/1006/1015, focus 1004, bracketed paste 2004
- [x] Synchronized output 2026, DECSCUSR cursor shapes
- [x] Charsets (G0–G3, DEC special graphics), DECALN
- [x] Kitty graphics protocol (transmit/place/delete; exec path)
- [x] Kitty keyboard protocol (progressive enhancement flags)
- [x] Kitty unicode placeholders (U=1)
- [x] OSC 0/1/2 title, 4/104 palette, 7 cwd, 8 hyperlink, 10/11/12 fg/bg/cursor, 52 clipboard
- [x] OSC 133 shell-integration marks, 22 pointer shape
- [x] DCS: DECRQSS (partial), XTGETTCAP (partial)
- [x] Scrollback engine + viewport pins; Unicode grapheme break + width (UAX #29/#11, exact)
- [x] Selection model + literal-substring search (no regex, matches upstream)
- [x] Snapshot/formatter (owned styled grid + reply queue) — the embeddability seam
- [~] XTWINOPS / title stack (core reports done; some ops stubbed)
- [~] XTGETTCAP / DECRQSS full capability set
- [ ] tmux control mode (`4.3k` Zig, deferred)
- [ ] OSC 21 color query reply (upstream finding filed in `work/upstream/`)
- [ ] VT config toggles: `title-report`, `enquiry-response`, `vt-kam-allowed` (KAM),
      `osc-color-report-format`, `scrollback-limit`, `image-storage-limit`

## Fonts & text shaping (`src/font`, ~72%)

- [x] `font-family` (+ discovery of styled members: bold/italic/bold-italic)
- [x] `font-family-bold`/`-italic`/`-bold-italic` explicit overrides
- [x] Bold/italic via variable-font `wght` axis + synthetic fallback ladder
- [x] Ligatures (rustybuzz shaping, run-based, live in the render engine)
- [x] Emoji (Apple Color Emoji discovery, pre-seeded like upstream)
- [x] Nerd-font glyphs + per-icon constraint sizing (codegen'd table, byte-exact)
- [x] Procedural sprites: box drawing, blocks, braille, powerline, legacy computing
- [x] `font-size`, embedded default fonts (JetBrains Mono + symbols, vendored)
- [x] CoreText face + fallback discovery, byte-backed named faces
- [~] `font-feature` (OpenType features passthrough — shaper supports, config unwired)
- [~] `font-variation*` (axes settable internally; config keys unwired)
- [ ] `font-thicken` / `font-thicken-strength` (config flags; default-off path matches)
- [x] `adjust-*` metric overrides (13 keys: cell width/height, font-baseline,
      underline/strikethrough/overline pos+thickness, cursor thickness/height,
      box-thickness, icon-height → font `Metrics::apply`; imports + live on reload)
- [ ] `font-codepoint-map`, `font-style*` name overrides, `grapheme-width-method` config
- [ ] FreeType/fontconfig backend (Linux), `force-autohint`, `freetype-load-flags`

## Rendering (`src/renderer`, ~60%)

- [x] Metal backend, IOSurface-on-CALayer presentation, retina/contentsScale
- [x] Upstream shaders verbatim; frozen wire structs; grayscale + color atlases
- [x] Per-row dirty tracking (equality-proven vs full redraw)
- [x] Run-based shaping cache; `alpha-blending` native, `minimum-contrast`
- [x] `background-opacity`, `display-p3` / `window-colorspace`
- [~] Timer-based frame pacing (CVDisplayLink not yet wired)
- [ ] `background-image` (+ fit/position/repeat/opacity)
- [ ] `background-blur`, `background-opacity-cells`
- [ ] `custom-shader` (shadertoy) + animation
- [x] Kitty image *rendering* (R6 COMPLETE — RGBA transmit→texture→placement quads incl.
      unicode placeholders, scrollback tracking + viewport clip/cull, delete/eviction +
      storage-limit texture reclaim, live-app rendering (kitty data through `SnapshotWindow`),
      and z-order buckets (below-bg / below-text / above-text); offscreen readback +
      dirty-equality-proven. Follow-up perf note in #19: `Image.data`→`Arc` for copy-free)
- [ ] Link detection/underline overlay (R7), `link-url`, `link-previews`
- [ ] `resize-overlay`, OpenGL backend (R9, Linux)

## Colors & theming

- [x] `theme` (Ghostty theme-file format loaded), 256-color palette + dynamic palette
- [x] `background`/`foreground`/`cursor-color` config overrides (seed startup Colors,
      live on reload; program OSC 10/11/12 still win); imports
- [x] `selection-background`/`-foreground` (theme + config override, per-channel);
      `cursor-text` (theme only — no Colors slot yet)
- [x] `split-divider` (implicit), search highlight colors
- [~] `bold-color`, `cursor-opacity`, `faint-opacity` (some wired, some not)
- [ ] `palette-generate`/`palette-harmonious`, `window-theme` auto light/dark
- [ ] `cell-foreground`/`cell-background`, `background-opacity-cells`

## Cursor

- [x] Styles: block, bar, underline (+ `cursor-style`), hollow when unfocused
- [x] Bar-at-prompt via shell integration (DECSCUSR)
- [x] Hidden when scrolled into history
- [~] `cursor-style-blink` (style set; blink timer not implemented)
- [x] `cursor-color` config override (seeds startup `Colors.cursor`, live on reload; imports)
- [ ] `cursor-click-to-move`, `cursor-opacity`

## Window & app chrome

- [x] Native AppKit window, `window-padding-x/y`, content-flush layout
- [x] `window-decoration`, native + non-native fullscreen
- [x] Menu bar (basic), key-window activation
- [x] `window-height`/`-width`/`-position` (initial geometry: cells → first window)
- [x] `title` (fixed window/tab title override; forces over OSC 0/2, live on reload; imports)
- [~] `window-save-state` (default/never/always): config-gates macOS native restoration
      (`NSQuitAlwaysKeepsWindows` + per-window `isRestorable`; savestate smoke). Content restore
      core landed — a live tab's split tree + per-pane cwd captures to a serializable
      `WindowSession`, round-trips through JSON, and rebuilds a single-pane session into a fresh
      tab (session unit tests + smoke). Remaining slice-2b: multi-pane tree rebuild + the
      `NSWindowRestoration`/`NSCoder` OS wiring (only exercisable by a genuine quit+relaunch)
- [ ] `window-step-resize`, `window-subtitle`
- [ ] `window-titlebar-background`/`-foreground`, `window-new-tab-position`
- [x] `resize-overlay` (+ `-position`, `-duration`): `cols ⨯ rows` HUD (NSTextField overlay)
      on live resize, positioned per config, auto-hiding after the duration (resize smoke)
- [ ] `command-palette`, undo/redo (`undo-timeout`)

## Tabs

- [x] Native NSWindow tabs, `new_tab`/`close_tab`/`goto_tab` (Cmd+1–9)/`move_tab`
- [x] `tab-inherit-working-directory` (OSC 7 pwd)
- [x] Tab bar visible only at 2+ tabs; Ctrl+Tab cycling
- [x] Live tab titles from OSC 0/2 (per-tab window/tab-label sync, ghost-emoji
      fallback after the 500ms grace — title smoke)
- [ ] `set_tab_title` keybind action (needs the Binding.zig system — T3)
- [ ] `window-show-tab-bar` policy, `gtk-tabs-location`/`gtk-wide-tabs` (Linux)

## Splits (`src/apprt` + Splits, slice 1+2 done)

- [x] `new_split` (Cmd+D / Cmd+Shift+D), `goto_split` directional + prev/next
- [x] `resize_split` chords, `toggle_split_zoom`, equalize
- [x] Divider drag, close-collapse, per-pane io/focus/scrollback isolation
- [x] Unfocused-split dimming (`unfocused-split-opacity`, `-fill`)
- [x] `split-inherit-working-directory`
- [ ] `split-preserve-zoom`, `split-divider-color` config, drag-to-reparent

## Quick terminal & extra surfaces

- [x] Quick terminal (dropdown): borderless key window, `quick-terminal-position`
      (top/bottom/left/right/center), `-size` (%/px per axis), `-animation-duration`,
      `-autohide`; in-app toggle (Cmd-`, View menu). Global hotkey deferred (ADR 0002).
- [ ] `new-window`/`new-tab` from CLI/AppleScript beyond the default first window

## Process, launch & lifecycle

- [x] Spawn `$SHELL`, inherit env/cwd; command override seam (`GHOSTTY_RS_COMMAND`)
- [x] `working-directory` / cwd inheritance (OSC 7)
- [~] `command` / `-e` initial command (env-override path exists; full `-e` CLI parse partial)
- [ ] `initial-command`, `initial-window`, `wait-after-command`
- [x] `quit-after-last-window-closed` (default false on macOS) + `confirm-close-surface`
      (false/true/always; running-process decided by OSC 133 prompt state, confirm modal on
      Cmd-W / context-menu Close Pane / windowShouldClose — confirmclose smoke)
- [ ] `abnormal-command-exit-runtime`, `window-inherit-working-directory`/`-font-size`

## Input & keybindings (`src/input`, ~78% encoders / ~10% bind system)

- [x] Kitty keyboard encoding, full legacy encoder, 117-entry macOS keymap
- [x] Bracketed paste, `macos-option-as-alt`
- [x] Byte-emitting keybinds `text:` / `esc:` / `csi:` (e.g. shift+enter, the
      default `alt+left`=esc:b word-motion) — dispatched through the ported
      `Binding.zig` `Set` (`crate::keybind::build_set` / `resolve_text_bytes`)
- [~] Font-size binds (increase/decrease/set) — actions exist, not all wired to config
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
      macOS split/search/tab chords are now upstream's exact defaults. Remaining: wire
      the remaining action categories (font-size, scroll, clipboard, window, …) + the
      runtime sequence/chain (leader-key) dispatch.
- [~] `Binding.zig` runtime: **leader-key sequences** (`ctrl+a>c`) **and `chain=`
      multi-action bindings dispatched** (`handle_key_sequence` + `resolve_actions`
      over the `Set`'s `Leader`/`Leaf`/`LeafChained` storage). Remaining: sequence
      idle-timeout + flush-on-abort, key tables, `global` binds, `performable`
      fallthrough.
- [ ] Most keybind *actions* that need new behavior: `jump_to_prompt`,
      `write_scrollback_file`, `inspector`, `scroll_page_*`, `adjust_selection`,
      `select_all`, `clear_screen`, `crash`, config-reload
- [x] `keybind` config parsing — the full trigger/action grammar (not just `text:`)
      parses and, for the wired action categories, dispatches
- [ ] `key-remap` (`RemapSet` ported but unwired — issue #23)

## Mouse

- [x] Reporting (5 formats), wheel → scrollback / alternate-scroll ladder
- [x] `mouse-scroll-multiplier`, shift-to-select over reporting
- [x] `context-menu` (right-click menu): Copy (on selection)/Paste/Split ×4/Close,
      per-pane; `right-click-action` (context-menu/paste/copy/copy-or-paste/ignore)
- [x] `mouse-hide-while-typing` (hide on keystroke, reveal on move)
- [x] `focus-follows-mouse` (per-pane NSTrackingArea → `mouseEntered:` focuses the pane)
- [x] `middle-click-action` (`primary-paste` pastes the selection / `ignore`) — mouse2 smoke
- [ ] `cursor-click-to-move` (OSC133 zone), `mouse-shift-capture` config

## Clipboard & selection

- [x] `copy-on-select`, OSC 52 read/write, selection string extraction
- [x] Double-click-drag select (basic); per-pane selection
- [x] Double-click *word* / triple-click *line* gestures (+ shift-click extend,
      ctrl/cmd-triple-click output select, option rectangle select, drag-past-
      edge viewport autoscroll — `SelectionGesture.zig` port, selection smoke)
- [x] `clipboard-paste-protection`/`-bracketed-safe` (confirm unsafe/multiline
      pastes), `clipboard-trim-trailing-spaces`, `selection-clear-on-typing`
- [ ] `selection-word-chars` (T3 config), `selection-clear-on-copy`
- [ ] `clipboard-read`/`clipboard-write` permission gates
- [ ] Primary selection / `primary-paste` (Linux)

## Shell integration (`src/shell-integration`)

- [x] Vendored bash/zsh/fish scripts (byte-identical, sha256 manifest)
- [x] OSC 133 prompt marks, OSC 7 cwd, bar-cursor-at-prompt
- [x] `shell-integration` auto-detect + injection (ZDOTDIR indirection)
- [~] `shell-integration-features` granular toggles
- [ ] `ssh-env` / `ssh-terminfo` propagation, `jump_to_prompt` navigation

## Config system (`src/config`, ~5% — deliberate deviation)

- [x] Minimal TOML config (theme, copy-on-select, font-family, font-size, keybind subset)
- [x] `theme` resolution via Ghostty theme files
- [—] Ghostty's custom config format (replaced by TOML — ADR)
- [ ] `+import-ghostty-config` converter
- [~] `config-reload` action (default `cmd+shift+,`) — re-reads config and re-applies
      keybinds, copy-on-select, scroll-multiplier, **and the theme live** (palette + fg/bg/
      cursor + selection colors pushed into every surface's engine, forced full repaint
      via `PageList::mark_all_dirty`). Fonts/cursor-style/padding re-apply deferred
      (need font-grid/window rebuild, config-core.md §7)
- [x] `config-file` includes — deferred breadth-first queue, cycle detection,
      `?optional` prefix, relative-to-including-file resolution; generic TOML merge
      (last-wins scalars, append arrays)
- [ ] `config-default-files`, two-location (XDG + App Support) merge, CLI overrides
- [ ] Full option surface (~200 keys) — most map to features listed elsewhere here

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
- [x] `macos-option-as-alt`, retina, key-window activation
- [~] `macos-titlebar-style` (tabbed layout works; style variants partial)
- [ ] `macos-secure-input` (+ indication/auto), `macos-custom-icon`/`-icon*`
- [ ] `macos-menu-bar`, `macos-applescript`, `macos-shortcuts`, `macos-dock-drop-behavior`
- [ ] `macos-window-buttons`, `-window-shadow`, `-glass-*`, `-titlebar-proxy-icon`
- [ ] Sparkle `auto-update`, `macos-hidden`

## Platform: Linux / GTK (0% — not started)

- [ ] GTK apprt, Wayland/X11, all `gtk-*` (~10 keys), `linux-cgroup*`, FreeType/fontconfig

## Embeddability / library (a qwertty-term goal beyond Ghostty)

- [x] Headless offscreen render + RGBA/PNG readback (`examples/frame-capture`)
- [x] VT / fonts / renderer as plain Rust crates, no global state
- [x] Injectable fonts; deterministic output (betamax reference consumer)
- [x] Embedding guide + one-call render API (`docs/embedding.md`; `Engine::render` →
      `Frame`, `FullSnapshot::capture_live`, `Engine::for_grid`)
- [x] MB4: betamax's offscreen render path (via `qwertty-term-renderer`) is exercised by
      `examples/frame-capture`; betamax's own adoption tracked in the betamax repo
- [~] Injectable clock (deterministic render proven; cursor-blink phase injected via
      `FrameOptions`, but the blink *mode* isn't threaded through the snapshot yet — #57)
- [x] **crates.io publish — all 8 crates at 0.1.0** (`qwertty-term` + `-vt`/`-font`/
      `-renderer`/`-termio`/`-input`/`-sprite`/`-ffi`; published 2026-07-08, docs.rs built)
- [x] MB5 API polish (Display/Error on font errors [already in 0.1.0]; matched `Engine::for_grid`;
      typed `Frame` RGBA readback; one-call `Engine::render`; `Stream::terminal()`;
      `capture_live`) — shipped in #5; docs.rs full-API + quickstart in #51

## Advanced / tooling

- [x] Differential testing vs `libghostty-vt`, resize-interleaved fuzzing, Miri
- [x] vtebench lane (ahead of Ghostty 1.3.1 on most suites; behind Ghostty `main` — see
      `docs/benchmarks/`; no dedicated perf tuning done yet, that's the T1 thread)
- [ ] Inspector / debug overlay, `write_screen_file`/`write_scrollback_file` actions
- [ ] `command-palette`, `resize-overlay`, glyph APC protocol, animation
