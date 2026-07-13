# Changelog

<!-- Release sections below are generated/updated by release-plz. Disable
markdownlint on this file — machine-generated changelog content routinely trips
line-length, bare-url, list-style, and duplicate-heading rules, and shouldn't
block CI. -->
<!-- markdownlint-disable -->

All notable changes to the qwertty-term crate family. The crates share one
workspace version and release together. This project adheres to
[Semantic Versioning](https://semver.org/spec/v2.0.0.html); pre-1.0, a minor
bump (`0.x.0`) may carry breaking changes and a patch bump (`0.x.y`) is
additive.

## Unreleased — 0.2.0

The next release is **0.2.0** (not 0.1.1): it carries one small breaking change
to a `qwertty-term-vt` snapshot type (see Breaking), so per SemVer the minor
version bumps. Everything else is additive.

### Breaking

- **`qwertty-term-vt`:** `snapshot::SnapshotCursor` gained a `blinking: bool`
  field. Code that *reads* a `SnapshotCursor` is unaffected; code that
  *constructs* one via a struct literal must add `blinking` (typically
  `blinking: false`). The type is an engine output, so most consumers only
  read it.

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
- Kitty graphics image rendering (R6): transmit → texture → placement quads,
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
  actions), a config-reload action, OSC-synced tab titles, bell + desktop
  notifications, clipboard hardening, mouse behaviors (context menu,
  hide-while-typing), the quick terminal, and selection gestures.

### Fixed

- **`qwertty-term-vt`:** reject non-ASCII OSC color specs instead of panicking;
  preserve aliased selection pins in `Screen::select`; handle stored grapheme
  breaks on a mode-2027 toggle; zero-capacity / growth-doubling latent bugs.

### Documentation

- Per-crate READMEs; an embedding guide (`docs/embedding.md`); docs.rs built for
  a darwin target (with the `freetype` feature) so the macOS-only renderer and
  font API is documented; intra-doc link fixes.

## 0.1.0 — 2026-07-08

Initial release of all eight crates: `qwertty-term`, `qwertty-term-vt`,
`qwertty-term-font`, `qwertty-term-renderer`, `qwertty-term-termio`,
`qwertty-term-input`, `qwertty-term-sprite`, `qwertty-term-ffi`.
