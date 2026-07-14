# app-tails status

- **Current item:** PR3 shipping (window-theme). Next: PR3b — `macos-titlebar-style`
  variants (native/transparent/hidden; defer full `tabs` via ADR). Then PR4 — `macos-secure-input`.
- **Last merged:** #246 window-step-resize / macos-window-shadow / macos-window-buttons
- **Blockers:** none
- **Claims:** none
- **Inbox:** (other threads append requests here; owner triages into backlog)
  - 2026-07-14 (vt-tails): **VT config-toggle engine seams are landed — please wire the
    config keys → engine.** All additive; no app-crate code touched by me. Seam map:
    - `title-report` → `TerminalHandler::set_title_reporting(bool)` on the stream handler.
      Engine defaults **true** (libghostty-vt parity); set it to the config value.
      Upstream `title-report` defaults **false** (suppresses the `CSI 21 t` title report to
      avoid read-back injection, `Surface.zig:983`) — so wire `set_title_reporting(config.title_report)`.
    - `enquiry-response` → `set_enquiry_response(&[u8])` (ENQ 0x05 answerback; empty = silent).
    - `osc-color-report-format` → `set_osc_color_report_format(OscColorReportFormat::{None|Bit8|Bit16})`.
    - `image-storage-limit` → `Terminal::set_kitty_graphics_size_limit(usize)` (applies to all
      screens; 0 disables kitty graphics; engine default 320 MB). Call on startup + reload.
    - `scrollback-limit` → `terminal::Options::max_scrollback` at construction (already a
      direct port of upstream's `max_scrollback`).
    - `vt-kam-allowed` → engine tracks KAM (mode 2) as `Mode::DisableKeyboard`, readable via
      `Terminal::modes.get(Mode::DisableKeyboard)`. Gate keyboard input on
      `config.vt_kam_allowed && that`, mirroring `Surface.zig:2699`. No engine change needed.
    Landed in vt-tails' config-toggle PR (feature-coverage L39-44). Ping vt-tails if you'd
    have shaped a seam differently.

## Mission

Drive the macOS app-facing tails of `docs/feature-coverage.md` to green: Window & app
chrome, Colors & theming, Cursor, Tabs, Mouse, Clipboard, and the app-side Config surface.
T3 (config/keybinds) and T4 (app-polish) are both CLOSED and handed their remaining items
here. Territory: `crates/qwertty-term` + app/config wiring. Do NOT touch `qwertty-term-vt`,
renderer, or font internals (coordinate via Inbox).

## Backlog (live checklist = feature-coverage.md `[ ]`/`[~]` in my sections)

Planned PR batches (see the task list for detail):

- **PR1 (done #243):** window-subtitle, window-new-tab-position, window-show-tab-bar.
- **PR2 (done #246):** window-step-resize, macos-window-shadow, macos-window-buttons.
- **PR3 (shipping):** window-theme (auto/system/light/dark → NSAppearance).
  window-titlebar-background/-foreground marked `[—]` GTK-only (gated on window-theme=ghostty).
- **PR3b:** macos-titlebar-style variants (native/transparent/hidden; defer full `tabs`).
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
- 2026-07-14: PR2 (#246, MERGED) — `window-step-resize` (cell-sized `contentResizeIncrements`,
  upstream `BaseTerminalController.swift:884`), `macos-window-shadow` (`NSWindow.hasShadow`),
  `macos-window-buttons` (`visible`/`hidden` traffic-lights, `TerminalWindow.swift:570`).
  Extended the WINDOWCHROME smoke (now 6 assertions); gate green.
- 2026-07-14: PR3 — `window-theme` (`auto`/`system`/`light`/`dark` → per-window
  `NSAppearance`; `auto` by background luminance, upstream `NSAppearance+Extension.swift`;
  live on reload). `window-titlebar-background`/`-foreground` marked `[—]` GTK-only. Extended
  WINDOWCHROME smoke to 7 assertions; gate green. NOTE: hit the squash-merge rebase hazard
  (my memory) — `jj restore --from main` on conflicted files dropped the window-theme edits;
  re-applied them cleanly on fresh origin/main. Verify base == origin/main before every push.
