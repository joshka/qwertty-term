# Architecture Notes

This document captures the current shape of the Rust prototype and the intended
direction for future splits. It should help a future agent decide where a
change belongs before editing.

## Design Goal

The current architecture optimizes for visible progress and local readability.
It does not attempt structural parity with Ghostty's Zig implementation. Ghostty
is the behavioral and product northstar, while this repo is still proving out a
Rust implementation path.

The most important boundary is:

- core terminal state and parsing stay pure and testable
- PTY, windowing, clipboard, app lifecycle, and host integration stay at edges
- side effects crossing from terminal state to host code should be explicit

## Data Flow

PTY-backed runs follow this flow:

1. `PtySession` spawns the shell and reads bytes from the PTY.
1. The frontend passes PTY bytes into `Terminal::write`.
1. `Terminal` parses bytes and mutates core screen state.
1. Terminal side effects, such as reports or clipboard writes, are drained by
   the frontend.
1. The frontend renders visible cells and sends user input back to the PTY.

Replay mode skips the PTY and frontend. It writes bytes directly into
`Terminal` and prints `screen_dump()`.

## Core State

`Terminal` owns the active state:

- dimensions
- primary and alternate screens
- active screen selector
- current style
- parser state and in-progress CSI/OSC/UTF-8 buffers
- terminal-to-PTY output queue
- clipboard side-effect queue
- title
- modes
- tab stops
- last printed character for REP
- scroll region
- scrollback limit

This is intentionally explicit. Avoid hiding state behind broad helper objects
unless the new object owns a coherent concept and reduces the amount a reader
must hold in memory.

## Module Boundaries

`src/terminal.rs` should eventually be mostly:

- public `Terminal` API
- parser dispatch
- high-level state transitions
- reset behavior
- small delegations to concept-owned modules

`src/terminal/edit.rs` currently owns:

- printable character handling
- UTF-8 fragment handling
- cursor movement
- tabs
- erase/insert/delete operations
- scrolling and scroll regions
- alternate screen switching
- DECALN

`src/terminal/effects.rs` currently owns OSC-driven host-facing side effects:

- OSC 0/2 title updates
- OSC 52 clipboard write requests
- intentionally ignored OSC payloads

`src/terminal/report.rs` currently owns terminal-to-PTY report bytes:

- DSR operating status
- DSR cursor position
- color-scheme query response
- primary and secondary device attributes

`src/terminal/modes.rs` currently owns terminal-specific mode policy:

- applying DEC private mode changes to `TerminalModes`
- switching the alternate screen for mode `1049`
- mapping DECSCUSR parameters into cursor shape state
- preserving unsupported mode shapes for future implementation

Good next split candidates:

- `src/terminal/dispatch.rs`: CSI and ESC dispatch once the action list grows.
- more work in `src/mode.rs`: richer DEC mode values once mode queries and
  synchronized output require more than booleans.
- a future policy boundary for clipboard, title, and color-scheme reports once
  those need host settings rather than fixed defaults.

Do not split just to make files smaller. Split when a future reader can open one
file and understand one concept without jumping through unrelated behavior.

## Frontend Boundaries

The terminal-hosted frontend in `src/main.rs` is intentionally basic. It is a
debugging and smoke path.

The native frontend in `src/window/` currently owns:

- egui window setup
- macOS-style app shortcuts and preferences entry point
- app-owned persistence for window size and font size
- PTY event polling
- font metric to grid-size conversion
- Nerd Font loading and font-size configuration
- styled cell painting
- scrollback viewport
- renderer boundary in `src/window/renderer.rs`
- selection and copy
- paste and bracketed paste
- title sync
- focus reporting
- mouse reporting
- child-exit behavior

This is too much for a production app, but acceptable for the first visible UI.
Future work should separate:

- terminal view model
- renderer
- input encoder
- font discovery and fallback policy
- clipboard policy
- window lifecycle
- preferences/config

## Side Effects

Current explicit side effects:

- `Terminal::take_output()` returns terminal-to-PTY bytes, such as DSR/DA
  responses.
- `Terminal::take_clipboard()` returns OSC 52 clipboard writes.
- `Terminal::title()` exposes OSC title state.

Future side effects should follow the same pattern: make them visible and drain
them at the frontend or policy edge. Do not let the core terminal write to the
OS clipboard, filesystem, or PTY directly.

## Testing Shape

Use local unit tests for pure state transitions and value types.

Use replay fixtures for compatibility-shaped byte streams. They protect the
public behavior of the terminal core better than implementation-detail unit
tests.

Use smoke commands for PTY lifecycle checks. They prove process spawning,
reading, parser flow, and screen dumping enough to catch integration breakage,
but they do not prove native UI behavior.

Native UI behavior still needs stronger validation. Future options:

- Playwright/browser-like screenshots are not directly applicable to egui.
- Add deterministic renderer model tests where possible.
- Add macOS window smoke checks only when they are reliable and noninteractive.
- Prefer captured stream fixtures for core compatibility.

## Known Architecture Debt

- `src/terminal.rs` still carries parser dispatch directly.
- `src/terminal/effects.rs` and `src/terminal/report.rs` are intentionally
  small now, but will need explicit host policy once settings exist.
- `src/terminal/modes.rs` has only the first mode-policy boundary; DEC mode
  query behavior is not modeled yet.
- `src/window/mod.rs` is narrower now but still owns session, input, selection,
  preferences, and lifecycle together.
- `src/window/renderer.rs` is a first renderer boundary, but it is still
  immediate-mode egui painting rather than a shaping/cache/atlas architecture.
  It does now have a `RenderPlan` step that separates visible-row planning from
  paint calls, plus a run-planning step that batches adjacent single-width
  cells with matching style and selection state. Wide glyphs remain in separate
  runs so grid placement stays predictable until real shaping exists.
- `src/window/font.rs` can inspect loaded font files for a small set of Nerd
  Font symbols. This powers the preferences readout and the `--font-report`
  command. It proves table coverage, not final renderer output.
- Clipboard and title policy are too simple.
- PTY lifecycle has a minimal wrapper, not a full session model with structured
  diagnostics.
- App bundle icon generation lives in `xtask`; a production app needs
  reviewed brand assets and signing/notarization workflow.
- Configuration does not exist.
- There is no stable app-state versus UI-state separation yet.

## Ghostty Reference Use

Use `/Users/joshka/local/ghostty` as the local reference checkout. The most
useful files so far:

- `src/terminal/Parser.zig`
- `src/terminal/stream.zig`
- `src/terminal/stream_terminal.zig`
- `src/terminal/Terminal.zig`
- `src/terminal/Screen.zig`
- `src/terminal/PageList.zig`
- `src/terminal/page.zig`
- `src/terminal/style.zig`
- `src/terminal/color.zig`

When consulting Ghostty, extract behavior and invariants. Do not mechanically
translate file shape or implementation details unless they fit the Rust design.
