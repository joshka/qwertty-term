# Roadmap

This project is working toward a macOS terminal emulator that can run as a real
terminal app. Ghostty is the implementation northstar, but this port should
advance through visible, validated milestones rather than attempting a broad
rewrite all at once.

## Recently Implemented

1. Added a native egui window frontend with styled rendering and dynamic grid
   sizing from monospace font metrics.
1. Added scrollback viewing, text selection, copy, title sync, focus reporting,
   and basic mouse reporting in the native window.
1. Added a macOS `.app` bundle generator through `xtask`.
1. Added wide-character handling, UTF-8 split-write handling, and xterm
   256-color palette rendering.
1. Added DECSCUSR cursor shape support for block, underline, and bar cursors.
1. Added OSC 52 clipboard writes as an explicit terminal side-effect queue.
1. Added native window closure when the PTY child exits.
1. Added a visible native-window error banner for unsuccessful PTY child exits.
1. Split terminal mode value types and pure OSC decoding out of
   `src/terminal.rs`.
1. Added a typed CSI sequence parser boundary with focused tests.
1. Moved SGR style mutation from `src/terminal.rs` into `src/style.rs`.
1. Grouped terminal mode flags and DEC private-mode mutation in
   `src/mode.rs`.
1. Added replay fixture compatibility tests under `tests/fixtures/replay/`.
1. Split screen editing, cursor movement, tabs, wrapping, scroll regions,
   alternate screen switching, and DECALN into `src/terminal/edit.rs`.
1. Expanded README run docs for terminal-hosted, native window, app bundle,
   replay, and smoke-check modes.
1. Split OSC side effects and terminal-to-PTY reports into
   `src/terminal/effects.rs` and `src/terminal/report.rs`.
1. Split terminal-specific DEC private-mode policy into
   `src/terminal/modes.rs`.
1. Added native-window Nerd Font loading with local auto-discovery,
   `GHOSTTY_RS_FONT_PATH`, and `GHOSTTY_RS_FONT_SIZE`.
1. Added first-pass macOS app-shell behavior: `Command-N` new window,
   `Command-,` preferences, `Command-Q` close, and stronger bundle metadata.
1. Added generated `.icns` app icon support to the app bundle helper.
1. Added app-owned window-size and font-size restore under
   `~/Library/Application Support/ghostty-rs/preferences`.
1. Split native terminal painting into `src/window/renderer.rs` as the first
   renderer boundary.
1. Added renderer row planning that materializes visible rows and cells before
   painting.
1. Added renderer run planning that batches adjacent single-width cells with
   matching style and selection state, while keeping wide glyphs grid-pinned.
1. Added native rendering for underline and strikethrough text decorations.
1. Hardened Nerd Font installation to keep an explicit font path first while
   adding discovered local Nerd Fonts as bounded fallbacks.
1. Added Nerd Font glyph coverage probes for Powerline separators and a small
   devicon set, surfaced in the preferences window.
1. Added `cargo run -- --font-report` for noninteractive local Nerd Font
   coverage checks.
1. Added `cargo run -- --render-probe` for deterministic renderer-run checks
   over the same Powerline/devicon probe glyphs.

## Prioritized Next Work

1. Turn the Nerd Font coverage probe into a renderer-visible fixture or window
   smoke check.
1. Improve the macOS app shell with a native menu if the framework supports it,
   stronger lifecycle handling, signing-ready metadata, and multi-window
   session management.
1. Expand renderer run planning into shaped text, font fallback runs, and
   glyph-run caching.
1. Add dirty-region repainting and a future glyph atlas boundary.
1. Continue splitting `src/terminal.rs` around coherent parser dispatch without
   weakening reader locality.
1. Grow replay fixtures with captured streams from common apps such as shells,
   Vim, less, tmux, and git.
1. Improve text input fidelity: Option/Alt handling, dead keys, IME composition,
   and macOS command shortcuts that should stay in the app.
1. Implement OSC 8 hyperlinks in the core and render clickable link regions in
   the native frontend.
1. Add configurable font family, font size, theme palette, and cursor style.
1. Improve Unicode correctness with grapheme clusters, combining marks, emoji,
   ambiguous-width policy, and proper invalid-width overwrite behavior.
1. Add copy and paste policy controls, including OSC 52 readback decisions and
   user-facing security defaults.
1. Implement more DEC private modes and reports used by full-screen TUIs,
   including mode queries and synchronized output.
1. Expand mouse protocol coverage and verify SGR, X10, button, drag, any-motion,
   wheel, and modifier encodings against real apps.
1. Add scrollback persistence limits, scrollback search, and selection across
   wrapped logical lines.
1. Improve renderer performance by caching shaped glyphs/rows and repainting
   only dirty regions.
1. Add Kitty graphics protocol support after the renderer boundary is ready.
1. Add shell integration and working-directory reporting once OSC handling and
   policy are better structured.
1. Add structured logging and diagnostics for parser errors, PTY lifecycle, and
   renderer timing.
1. Add release-mode bundle validation and a smoke test that launches the `.app`
   and verifies the window process stays alive.
1. Compare behavior against selected Ghostty reference tests and document each
   accepted divergence.
1. Decide whether this remains a Rust proof-of-concept or moves toward a
   production-quality port with a stronger module and crate boundary.

## Current Gap To Ghostty Proper

The current implementation is useful and visible, but it is still a prototype.
Ghostty has mature renderer architecture, font shaping, configuration, app
lifecycle, platform integration, terminal protocol breadth, graphics protocols,
shell integration, and extensive compatibility work. The nearest high-leverage
work is to stabilize lifecycle, modularity, fixture-based compatibility testing,
and native input/rendering fidelity before adding many more protocol features.
The renderer now has row and run planning, but it still relies on egui text
painting rather than a shaped glyph pipeline, atlas, or GPU batches.

## Continuation Notes

- `docs/handoff.md`: agent pickup packet with current state, validation,
  tradeoffs, lessons learned, and recommended next chunks.
- `docs/architecture.md`: current architecture, boundaries, data flow, and
  architecture debt.
- `docs/ghostty-gap.md`: detailed gap analysis against Ghostty proper.
