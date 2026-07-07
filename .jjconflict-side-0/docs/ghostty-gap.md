# Ghostty Gap Notes

Ghostty is the product and behavior northstar for this repo. This document
names the gap between the current Rust prototype and Ghostty proper so future
work can prioritize the differences that matter.

## Current Prototype Strengths

The current repo has enough to demonstrate a real path:

- PTY-backed shell execution
- terminal-hosted renderer for quick checks
- native macOS window path through egui
- development `.app` wrapper
- first-pass macOS app-shell shortcuts and preferences entry point
- generated app bundle icon
- app-owned window-size and font-size restore
- first native renderer module boundary
- renderer row and run planning before paint
- basic underline and strikethrough rendering
- local Nerd Font auto-discovery, explicit font override, and bounded fallback
  ordering
- font coverage probes for Powerline separators and a small devicon set
- noninteractive font coverage reporting
- core grid, cursor, style, scrollback, alternate screen, and resize behavior
- useful C0, ESC, CSI, SGR, OSC, DEC private mode, DSR, and DA subset
- basic wide-character handling and split UTF-8 handling
- xterm 256-color palette rendering
- selection, copy, paste, focus reporting, mouse reporting, and title sync in
  the native frontend
- replay fixture harness for deterministic compatibility checks

This is enough for visible milestones. It is not enough to be called a full
terminal emulator.

## Major Gap Areas

### Renderer

Ghostty has a serious renderer architecture. This prototype paints text through
egui. It now has a small row/run planning model, but missing pieces include:

- glyph shaping
- ligatures
- font fallback
- emoji presentation policy
- glyph atlas management
- GPU batching
- dirty-region repainting
- high-DPI polish
- renderer diagnostics
- Kitty graphics rendering

Nearest useful work: expand the renderer boundary before adding features that
depend on shaped text or graphics. Nerd Font selection now has explicit-first
fallback ordering, but it still needs visible verification that Powerline and
devicon glyphs render from the intended local files. The preferences window can
show glyph coverage from font tables, and `--font-report` can print the same
data without opening a window. That is not the same as proving the renderer
shaped and painted the glyphs correctly.

### Unicode

The prototype handles split UTF-8 and basic double-width code points. It does
not correctly model:

- grapheme clusters
- combining marks
- emoji sequences
- ambiguous-width policy
- zero-width overwrite behavior
- invalid-width overwrite behavior
- locale-sensitive width choices

Nearest useful work: add fixtures for combining marks and emoji, then introduce
a text-cell model that can store clusters rather than only one `char`.

### Terminal Protocol

The prototype has a pragmatic subset. Missing or incomplete areas include:

- complete C0/C1 controls
- charset designation and locking shifts
- broad DEC private mode behavior
- DECRQM and other mode queries
- synchronized output
- OSC 8 hyperlinks
- OSC palette queries and updates
- OSC 52 readback policy
- shell integration OSCs
- Kitty keyboard protocol
- Kitty graphics protocol
- bracketed paste edge cases
- full mouse protocol verification

Nearest useful work: grow replay fixtures from real applications before adding
large amounts of protocol surface.

### Input

The native frontend sends useful basic input, but macOS terminal input fidelity
is not done. Missing areas include:

- Option/Alt behavior
- dead keys
- IME composition
- Command shortcuts that should stay in the app
- application keypad behavior
- richer modifier encodings
- Kitty keyboard protocol
- configurable keybindings

Nearest useful work: decide the input policy and add tests around
`src/window/input.rs` before wiring more UI shortcuts.

### App Shell

The `.app` is a development wrapper, not a real macOS app. Missing pieces:

- app menu
- preferences window
- new windows and sessions
- lifecycle handling
- dock/icon metadata
- signing-ready bundle metadata
- window restore
- release-mode bundle validation
- crash and error reporting

Nearest useful work: add a small app-shell milestone after input and config
boundaries are clearer.

### Configuration And Policy

The prototype has no durable config. Missing configuration/policy surfaces:

- font family
- font size
- theme palette
- cursor style
- scrollback limits
- clipboard permissions
- OSC 52 read/write behavior
- shell command and environment
- mouse/input policies
- logging level

Nearest useful work: add a small configuration type and wire font size/theme
before adding many user-facing toggles.

### Compatibility Evidence

The current tests are useful but small. Ghostty-level confidence needs:

- real captured shell streams
- real `less`, `vim`, `git`, and `tmux` streams
- reference comparisons for selected Ghostty tests
- renderer-visible checks
- PTY lifecycle tests
- app bundle launch smoke
- documented accepted divergences

Nearest useful work: add fixtures from common apps, one app at a time, and keep
the expected screen dump readable.

## What To Avoid

- Do not add random escape sequences without a fixture or visible app reason.
- Do not make `AGENTS.md` the full rule book; keep deeper material in docs.
- Do not freeze a broad public API yet.
- Do not treat the egui renderer as the final architecture.
- Do not claim Ghostty compatibility without a specific behavior comparison.
- Do not let terminal core code perform host side effects directly.

## Prioritized Gap Closure

1. Turn Nerd Font coverage into renderer-visible verification for devicons,
   Powerline glyphs, and fallback behavior.
1. App shell lifecycle, native menu, signing metadata, and multi-window session
   management.
1. Expand renderer run planning toward shaped text, font fallback, and glyph
   caching.
1. Add dirty-region repainting and a glyph atlas boundary.
1. Compatibility fixtures from real applications.
1. More coherent terminal modules for parser dispatch.
1. Native input fidelity.
1. Config for font, theme, cursor, and scrollback.
1. OSC 8 hyperlink model and native rendering.
1. Unicode cluster model.
1. DEC mode query and synchronized output support.
1. Release bundle validation.

Each step should leave the app runnable with `cargo run -- --window` and the
full validation gate green.
