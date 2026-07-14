# app-tails status

- **Current item:** PR2 shipping (window-step-resize / macos-window-shadow /
  macos-window-buttons). Next: PR3 — `macos-titlebar-style` + `window-titlebar-bg/fg` +
  `window-theme`.
- **Last merged:** #243 window-subtitle / window-new-tab-position / window-show-tab-bar
- **Blockers:** none
- **Claims:** none
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Mission

Drive the macOS app-facing tails of `docs/feature-coverage.md` to green: Window & app
chrome, Colors & theming, Cursor, Tabs, Mouse, Clipboard, and the app-side Config surface.
T3 (config/keybinds) and T4 (app-polish) are both CLOSED and handed their remaining items
here. Territory: `crates/qwertty-term` + app/config wiring. Do NOT touch `qwertty-term-vt`,
renderer, or font internals (coordinate via Inbox).

## Backlog (live checklist = feature-coverage.md `[ ]`/`[~]` in my sections)

Planned PR batches (see the task list for detail):

- **PR1 (done):** window-subtitle, window-new-tab-position, window-show-tab-bar.
- **PR2:** window-step-resize, macos-window-shadow, macos-window-buttons.
- **PR3:** macos-titlebar-style variants, window-titlebar-background/-foreground, window-theme.
- **PR4:** macos-secure-input (+indication/auto) — `EnableSecureEventInput` works unbundled.
- **PR5:** set_tab_title keybind action, clipboard-read/clipboard-write permission gates.
- **PR6 (triage):** cursor-click-to-move, command-palette, undo/redo, macos-custom-icon
  (ADR-defer bundle-only), macos-menu-bar.

Renderer/vt-owned (NOT my territory — route via Inbox if picked up): `bold-color`,
`faint-opacity`, `cursor-opacity`, `cell-foreground`/`cell-background`,
`background-opacity-cells`, `palette-generate`/`palette-harmonious`, cursor-blink timer.

## Log

- 2026-07-14: session start; workspace created; T3+T4 closed, tails inherited.
- 2026-07-14: PR1 (#243, MERGED) — `window-subtitle` (native `NSWindow.subtitle` from cwd),
  `window-new-tab-position` (`current`/`end` grouping, upstream
  `TerminalController.swift:456`), `window-show-tab-bar` (`auto`/`always`/`never` →
  `NSWindowTabbingMode`). New `QWERTTY_TERM_SMOKE_WINDOWCHROME` asserts all three; gate green.
- 2026-07-14: PR2 — `window-step-resize` (cell-sized `contentResizeIncrements`,
  upstream `BaseTerminalController.swift:884`), `macos-window-shadow` (`NSWindow.hasShadow`),
  `macos-window-buttons` (`visible`/`hidden` traffic-lights, `TerminalWindow.swift:570`).
  Extended the WINDOWCHROME smoke (now 6 assertions); gate green (release+paranoid, offscreen,
  windowchrome all pass).
