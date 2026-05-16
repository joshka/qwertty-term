# Agent Handoff

This file is the pickup point for a future Codex session. It records current
state, proof, tradeoffs, and the next useful chunks so the next agent does not
need to rediscover the project shape from scratch.

## Current Objective

Build toward a full running terminal emulator on macOS, with Ghostty as the
implementation northstar. The repo should keep delivering visible, validated
milestones instead of wandering through unrelated terminal ideas.

Current state: this is a visible Rust prototype with a PTY-backed terminal core,
a terminal-hosted frontend, an experimental native egui window, a local macOS
`.app` wrapper, and replay fixtures. It is not yet a production terminal.

## What Runs

Use these commands from the repo root:

```bash
cargo run -- --window
cargo run
cargo run -p xtask -- bundle && open target/ghostty-rs.app
cargo run -- --smoke-command 'printf READY'
cargo run -- --font-report
cargo run -- --render-probe
printf 'Hello \033[31mred\033[0m\n' | cargo run --quiet
```

The native window command is the most useful human-visible path. The
terminal-hosted command is useful for quick PTY/core checks. The app bundle is a
development wrapper around the window path, not a signed release app.

If a previous validation left something running, inspect it before killing:

```bash
ps -axo pid,command | rg 'target/(debug|release)/ghostty-rs|ghostty-rs\.app|cargo run'
```

## Validation To Run

Run the full local gate after code or documentation work:

```bash
cargo fmt --check
cargo test --workspace
markdownlint-cli2 "**/*.md"
cargo run --quiet -- --smoke-command 'printf READY'
```

Use narrower checks while iterating. For Markdown-only changes,
`markdownlint-cli2 "**/*.md"` is sufficient until the final handoff.

The most recent full gate passed before this handoff doc was added:

- `cargo fmt --check`
- `cargo test --workspace`
- `markdownlint-cli2 "**/*.md"`
- `cargo run --quiet -- --smoke-command 'printf READY'`

Rerun the relevant gate after editing this file or related docs.

## Source-Control State

This repo uses jj. Keep using `jj`, not Git, for normal source-control state.

The current work is intentionally one early prototype change:

```bash
jj --no-pager status
```

The working-copy description was set to:

```text
Build Rust terminal prototype
```

Do not run broad jj rewrite commands without inspecting state first. Do not use
interactive jj commands from unattended agent work.

## Documentation Context

The shared guidance has been copied into `docs/development/`. Future agents
should read the domains that match their task. The rules that mattered most so
far:

- preserve local human work and jj state
- prefer coherent module boundaries over generic organization
- keep modules small enough to understand locally
- validate claims with commands and fixtures
- use docs as durable handoff, not only final-chat memory
- keep AGENTS and README as maps to deeper context

## Implementation Map

Core terminal state:

- `src/terminal.rs`: `Terminal` fields, public accessors, parser dispatch, and
  reset behavior.
- `src/terminal/edit.rs`: screen editing, cursor movement, tabs, wrapping,
  scroll regions, alternate screen switching, DECALN, and UTF-8 printing.
- `src/terminal/effects.rs`: OSC-driven host-facing side effects such as title
  and clipboard writes.
- `src/terminal/modes.rs`: terminal-specific mode policy for DEC private modes
  and cursor-shape control.
- `src/terminal/report.rs`: terminal-to-PTY report bytes such as DSR and DA
  responses.
- `src/screen.rs`: grid storage, cursor values, scrollback, resize, plain-text
  extraction, and default tab stops.
- `src/cell.rs`, `src/color.rs`, `src/style.rs`: cell and style value types,
  including SGR mutation and colors.
- `src/parser.rs`: parser states and typed CSI sequence parsing.
- `src/mode.rs`: terminal modes and mode value types.
- `src/osc.rs`: pure OSC payload decoding.

I/O and UI:

- `src/pty.rs`: PTY session wrapper around `portable-pty`.
- `src/main.rs`: CLI entrypoint for terminal-hosted mode, replay mode,
  smoke-command mode, and native-window mode.
- `src/window/mod.rs`: egui native terminal window.
- `src/window/app_shell.rs`: macOS-style window shortcuts for new window,
  preferences, quit, and app-owned preference persistence.
- `src/window/font.rs`: native-window font selection, Nerd Font discovery, and
  font-size configuration. An explicit font path is installed first, followed by
  a bounded list of discovered local Nerd Font fallbacks. Loaded font files are
  probed with `ttf-parser` for Powerline and devicon glyph coverage.
- `src/window/input.rs`: keyboard, paste, focus, and mouse reporting encoding.
- `src/window/renderer.rs`: native terminal painting, cursor rendering,
  selection highlighting, scrollback viewport mapping, and error banner
  rendering. It now builds a `RenderPlan` before painting so shaped rows and
  glyph-run caches have a concrete future attachment point. Rows now also group
  adjacent single-width cells into runs when style and selection state match.
  Wide glyphs stay in their own runs so their grid placement remains stable
  until a real shaper owns width and fallback decisions.
- `src/window/theme.rs`: color mapping, including xterm 256-color palette.
- `xtask/src/main.rs`: development `.app` bundle generator.
  It writes the launcher, `Info.plist`, generated `.icns` icon, and `PkgInfo`.

Tests:

- Unit tests live near the relevant modules.
- Replay fixtures live under `tests/fixtures/replay/*`.
- `tests/replay_fixtures.rs` decodes fixture input notation and compares
  `Terminal::screen_dump()`.

## Recently Implemented Ideas

1. Bootstrapped shared guidance into `docs/development/` and added local
   `AGENTS.md` routing.
1. Created a Rust workspace with a root crate and `xtask`.
1. Implemented a VT-like `Terminal` core with grid, cursor, style, scrollback,
   parser dispatch, resize, and side-effect queues.
1. Added C0, ESC, CSI, SGR, OSC, DEC private-mode, DSR, and DA coverage for a
   practical first slice.
1. Added a PTY-backed shell session using `portable-pty`.
1. Added a terminal-hosted frontend, replay mode, and smoke-command mode.
1. Added an experimental native egui window frontend.
1. Added scrollback, selection, copy, focus reporting, mouse reporting, title
   sync, cursor shapes, and child-exit handling in the native window.
1. Added a local macOS `.app` bundle generator.
1. Added replay fixture tests and split terminal behavior into coherent edit,
   effect, report, and mode-policy modules.
1. Added native-window Nerd Font discovery and `GHOSTTY_RS_FONT_PATH` /
   `GHOSTTY_RS_FONT_SIZE` overrides.
1. Added first-pass macOS app-shell behavior and stronger app bundle metadata.
1. Added generated `.icns` app icon support to the app bundle helper.
1. Added app-owned window-size and font-size restore under
   `~/Library/Application Support/ghostty-rs/preferences`.
1. Split native terminal painting into `src/window/renderer.rs`.
1. Added renderer row planning before painting.
1. Added renderer run planning before text painting and basic underline /
   strikethrough rendering.
1. Added explicit-first Nerd Font fallback ordering for local font files.
1. Added Nerd Font glyph coverage diagnostics and surfaced them in the
   preferences window.
1. Added a noninteractive `--font-report` command for the same coverage data.
1. Added a noninteractive `--render-probe` command that shows the renderer run
   plan for the Powerline/devicon probe glyphs.

## Current Gaps

The highest-risk gaps are not isolated missing escape sequences. They are the
larger product and architecture gaps that separate a prototype from a proper
terminal:

- renderer architecture, glyph shaping, ligatures, font fallback, glyph atlas,
  dirty regions, and GPU batching
- Unicode correctness for grapheme clusters, combining marks, emoji, and
  ambiguous-width policy
- input fidelity for macOS Option/Alt, dead keys, IME composition, and Command
  shortcuts
- configuration for fonts, theme, cursor, scrollback, and policy
- complete DEC mode/query coverage and synchronized output
- OSC 8 hyperlinks, OSC palette, shell integration, Kitty graphics, and
  clipboard readback policy
- app shell polish: menu, preferences, multiple windows, lifecycle handling,
  icon, signing-ready bundle metadata, and restore
- compatibility corpus against real app streams and selected Ghostty tests
- structured diagnostics for parser, PTY lifecycle, renderer timing, and
  user-visible failures

## Recommended Next Chunks

Work in small, reviewable, visible milestones:

1. Turn the Nerd Font coverage probe into a renderer-visible fixture or window
   smoke check.
1. Improve the macOS app shell with native menu support if available, stronger
   lifecycle handling, signing-ready metadata, and multi-window session
   management.
1. Expand renderer run planning into shaped text, font fallback runs, and
   glyph-run caching.
1. Add dirty-region repainting and a future glyph atlas boundary.
1. Finish module split around parser dispatch. Keep behavior unchanged and run
   the full gate.
1. Add replay fixtures from real common applications: shell prompt redraw,
   `less`, `vim`, `git diff`, and `tmux`.
1. Improve native text input fidelity, especially Option/Alt and Command
   shortcut handling.
1. Add a small config file or CLI settings boundary for font size, font family,
   theme, and cursor style.
1. Implement OSC 8 hyperlinks in core state, then render clickable regions in
   the native window.
1. Add Unicode grapheme and combining-mark support with focused fixtures.
1. Add DEC mode query support and synchronized output.
1. Verify mouse protocol encodings against real TUIs.
1. Add scrollback search and wrapped-line selection.
1. Build a more macOS-like app shell: menu, new window, preferences, icon, and
   bundle metadata.

## Tradeoffs Made So Far

- The native UI uses egui because it produced visible results quickly. Ghostty
  proper has a much more serious renderer architecture; this should eventually
  be replaced or isolated behind a renderer boundary.
- The VT core is direct Rust state mutation. That keeps early behavior easy to
  inspect, but `src/terminal.rs` still needs more coherent splitting.
- OSC 52 writes are accepted as side effects, but clipboard policy is not
  complete. A real terminal needs user-facing security defaults.
- The app bundle is development-only. It is useful for visible milestones, but
  not evidence of a distributable macOS app.
- Replay fixtures prove deterministic core behavior, not full app
  compatibility. Grow them from real captured streams.
- The implementation prefers current visible behavior over speculative public
  API stability. Avoid freezing public API shapes too early.

## Lessons Learned

- Visible milestones help keep this project grounded. Keep making the app run
  after each meaningful step.
- Module size matters here. `src/terminal.rs` became too large quickly; split by
  owning concept and preserve reader locality.
- Terminal work needs realistic input. Idealized escape tests miss behavior
  exercised by shells and full-screen TUIs.
- Treat terminal-to-host effects as explicit queues or policies. Clipboard,
  title, reports, focus, paste, and mouse behavior cross important boundaries.
- Do not confuse "works in a smoke command" with "proper terminal." The next
  durable progress comes from compatibility, input fidelity, renderer design,
  and app lifecycle.

## Completion Audit Status

The full objective is not complete. Current deliverables satisfy only the early
prototype milestones:

- PTY-backed execution exists.
- Native window execution exists.
- A development `.app` wrapper exists.
- Core parser/screen behavior has tests and replay fixtures.
- Documentation now records run modes, architecture, roadmap, and gaps.

Missing for completion:

- production-grade macOS app shell
- mature rendering and font shaping
- broad terminal compatibility
- configuration and policy surfaces
- robust Unicode and input handling
- release/signing/distribution validation
- comparison against Ghostty behavior beyond selected reference reading
