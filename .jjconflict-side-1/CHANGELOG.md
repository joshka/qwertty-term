# Changelog

<!-- This file is hand-maintained (release-plz `changelog_update = false`).
Disable markdownlint here — changelog content routinely trips line-length,
bare-url, list-style, and duplicate-heading rules, and shouldn't block CI. -->
<!-- markdownlint-disable -->

All notable changes to the qwertty-term crate family. The eight crates share one
workspace version and release together. This project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html); pre-1.0, a minor
bump (`0.x.0`) may carry breaking changes and a patch bump (`0.x.y`) is
additive.

## [0.4.0](https://github.com/joshka/qwertty-term/compare/qwertty-term-v0.3.0...qwertty-term-v0.4.0) - 2026-07-15

### Added

- **Linux font discovery (ADR 003 P2):** a fontconfig-backed system-font
  discovery module, wired into the font collection and resolver, plus a FreeType
  `Face::load_by_name` path and configurable glyph load flags
  (`freetype-load-flags` / `force-autohint`). Linux can now find and load system
  fonts by name and tune hinting.
  ([#245](https://github.com/joshka/qwertty-term/pull/245),
  [#248](https://github.com/joshka/qwertty-term/pull/248),
  [#260](https://github.com/joshka/qwertty-term/pull/260),
  [#264](https://github.com/joshka/qwertty-term/pull/264))
- **Window appearance & layout options (app):**
  - `window-theme` — `auto`/`system`/`light`/`dark`, mapped to `NSAppearance`.
    ([#251](https://github.com/joshka/qwertty-term/pull/251))
  - `window-step-resize`, `macos-window-shadow`, and `macos-window-buttons`.
    ([#246](https://github.com/joshka/qwertty-term/pull/246))
  - `window-subtitle`, `window-new-tab-position`, and `window-show-tab-bar`.
    ([#243](https://github.com/joshka/qwertty-term/pull/243))
- **tmux control mode — parser groundwork (ADR 004):** the pure control-mode
  protocol parsers land in `qwertty-term-vt` — the notification state machine,
  the window-layout parser + pane checksum, and format-variable parse/encode.
  These are compiled-in but runtime-inert until a `tmux -CC` session begins; the
  native viewer that renders tmux windows as tabs/splits is still to come.
  ([#257](https://github.com/joshka/qwertty-term/pull/257),
  [#259](https://github.com/joshka/qwertty-term/pull/259),
  [#261](https://github.com/joshka/qwertty-term/pull/261))
- **VT config seams:** `title-report` and `image-storage-limit` engine seams so
  the app can wire those configuration keys through to the terminal.
  ([#249](https://github.com/joshka/qwertty-term/pull/249))

### Changed

- **XTGETTCAP:** terminfo capability queries now answer the full ghostty
  terminfo table (268 capabilities plus the `TN`/`Co`/`RGB` query specials),
  up from 6 hardcoded answers.
  ([#241](https://github.com/joshka/qwertty-term/pull/241))
- **Performance:** scrolling within a DECSTBM scroll region now uses an in-place
  fast path (ported from upstream Ghostty `cursorScrollRegionUp`), avoiding a
  pin walk, cursor re-resolution, and per-cell clear on every region scroll.
  ([#266](https://github.com/joshka/qwertty-term/pull/266))

### Fixed

- **Font zoom:** re-upload the glyph atlas and adopt fresh cell metrics on a font
  rebuild (`cmd`-`+`/`-` zoom, display-scale, or `font-family` change), fixing
  garbled/doubled glyphs in the live window.
  ([#253](https://github.com/joshka/qwertty-term/pull/253))
- **XTWINOPS:** size and title report ops (`CSI 14/16/18/21 t`) are gated on
  exactly one parameter, so `CSI 14;2 t` no longer emits a spurious report
  (matching upstream).
  ([#244](https://github.com/joshka/qwertty-term/pull/244))

## [0.3.0](https://github.com/joshka/qwertty-term/compare/qwertty-term-v0.2.0...qwertty-term-v0.3.0) - 2026-07-14

### Added

- **Linux headless rendering:** the renderer now builds and renders on Linux via
  `Engine<Software>` over a FreeType backend — the first non-macOS render path
  (ADR 003 P1). ([#209](https://github.com/joshka/qwertty-term/pull/209))
- **Clickable links:** URLs — both OSC 8 hyperlinks and regex-detected links —
  are underlined on hover and open on `cmd`-click.
  ([#210](https://github.com/joshka/qwertty-term/pull/210),
  [#220](https://github.com/joshka/qwertty-term/pull/220))
- **`write_screen_file` / `write_selection_file` keybind actions** to dump the
  visible screen or the current selection to a file.
  ([#214](https://github.com/joshka/qwertty-term/pull/214))
- **Configurable selection:** `selection-word-chars` and the click-repeat
  interval are now honored, driving word/line selection gestures.
  ([#205](https://github.com/joshka/qwertty-term/pull/205))

### Changed

- The active text selection is cleared after a copy, matching common terminal
  behavior. ([#225](https://github.com/joshka/qwertty-term/pull/225))
- **Performance:** bulk multibyte UTF-8 is decoded in a scalar fast path,
  improving wide/CJK throughput.
  ([#227](https://github.com/joshka/qwertty-term/pull/227))
- **Performance:** scrolling within a scroll region is optimized (ported from
  upstream Ghostty). ([#204](https://github.com/joshka/qwertty-term/pull/204))

### Fixed

- **`qwertty-term-vt`:** reject pages with a stale width when reusing them in
  `grow_prune`, closing a latent grid-corruption path.
  ([#222](https://github.com/joshka/qwertty-term/pull/222))

## [0.2.0](https://github.com/joshka/qwertty-term/releases/tag/qwertty-term-v0.2.0) - 2026-07-13

The big feature release: kitty image rendering, the full keybind system, the
config surface with live reload, hyperlinks and terminal queries, and the
embeddability API. One small breaking change to a `qwertty-term-vt` snapshot
type bumps the minor version (see Breaking); everything else is additive.

<!-- No compare link vs 0.1.0: the initial release predates crate tags, so there
is no `qwertty-term-v0.1.0` to diff against. This heading links to the 0.2.0 tag. -->

### Breaking

- **`qwertty-term-vt`:** `snapshot::SnapshotCursor` gained a `blinking: bool`
  field. Code that *reads* a `SnapshotCursor` is unaffected; code that
  *constructs* one via a struct literal must add `blinking` (typically
  `blinking: false`). The type is an engine output, so most consumers only
  read it. This addition is why the release is `0.2.0` rather than `0.1.1`.

### Added

#### qwertty-term-vt

- `Stream<TerminalHandler>::terminal()`, `terminal_mut()`, and `into_terminal()`
  accessors, replacing the `stream.handler.terminal` reach-through (still works).
- `SnapshotCursor::blinking` (DEC private mode 12) so a renderer can gate the
  cursor blink phase; the phase itself is injected renderer-side, keeping the
  snapshot deterministic.
- OSC 8 hyperlinks; OSC 4/10/11/12 + kitty OSC 21 color-query replies; complete
  DECRQSS (DECSCUSR, DECSLRM gating); DSR strictness (reject `CSI ? 6 n`, add
  `CSI ? 996 n`); XTWINOPS size reports (`CSI 14/16/18/21 t`); XTGETTCAP
  terminfo replies; mouse tracking flags (`mouse_event`/`mouse_format`) and
  OSC 22 `mouse_shape`.

#### qwertty-term-renderer

- `Engine::render(snapshot, grid, opts) -> Frame` — the one-call render path;
  typed `engine::Frame` readback (`bgra()`/`into_bgra()`/`to_rgba()`);
  `Engine::for_grid`/`with_backend_for_grid` (cell geometry read from the grid);
  and `FullSnapshot::capture_live`.
- Kitty graphics image rendering: transmit → texture → placement quads,
  scrollback tracking + viewport clip/cull, delete/eviction + storage-limit,
  z-order buckets, and live-app rendering via `SnapshotWindow`.

#### qwertty-term-font

- Optional FreeType face path (ADR 003) behind the `freetype` feature; the
  CoreText backend remains the macOS default.

#### qwertty-term-input

- Keyboard binding system ported from `Binding.zig`: the trigger/action model,
  `Set::parse_and_put` (sequences, chains, unbind), and the default keymap.

#### qwertty-term (app)

- Keybind dispatch (leader sequences, `chain=` multi-action, `esc:`/`csi:` byte
  actions), a config-reload action, `+import-ghostty-config`, OSC-synced tab
  titles, bell + desktop notifications, clipboard hardening, mouse behaviors
  (context menu, hide-while-typing), the quick terminal, splits and
  window-save-state, and selection gestures.

### Fixed

- **`qwertty-term-vt`:** reject non-ASCII OSC color specs instead of panicking;
  preserve aliased selection pins in `Screen::select`; handle stored grapheme
  breaks on a mode-2027 toggle; zero-capacity / growth-doubling latent bugs.
- **`qwertty-term-sprite`:** correct the cursor-height regression in
  `adjust-cursor-height`. ([#158](https://github.com/joshka/qwertty-term/pull/158))

### Documentation

- Per-crate READMEs; an embedding guide (`docs/embedding.md`); docs.rs built for
  a darwin target (with the `freetype` feature) so the macOS-only renderer and
  font API is documented; intra-doc link fixes.

## 0.1.0 - 2026-07-08

Initial release of all eight crates: `qwertty-term`, `qwertty-term-vt`,
`qwertty-term-font`, `qwertty-term-renderer`, `qwertty-term-termio`,
`qwertty-term-input`, `qwertty-term-sprite`, `qwertty-term-ffi`.
