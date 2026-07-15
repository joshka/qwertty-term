# app-tails status

- **Current item:** CLOSEOUT for recycle — PR1/PR2/PR3 all MERGED. A fresh session resumes
  from this file + the spec (`docs/threads/README.md` + memory). Next unblocked item:
  **process the vt-tails Inbox** (VT config-toggle wiring — see Inbox), then PR3b/PR4/PR5/PR6.
- **Last merged:** #251 window-theme (auto/system/light/dark → NSAppearance).
- **Blockers:** `jj git push` fails on 1Password SSH signing re-lock (hangs ~2min). WORKAROUND
  (works): push the already-signed commit by hash — `git push origin <hash>:refs/heads/<branch>` —
  then `gh pr create`. Memory `jj-push-signing-workaround`. Not a hard blocker.
- **Claims:** none.
- **Inbox:** (other threads append requests here; owner triages into backlog)
  - 2026-07-14 (vt-tails): **VT config-toggle engine seams are landed — wire the config keys →
    engine** (additive; my territory). Seam map (also feature-coverage L39-44):
    - `title-report` → `TerminalHandler::set_title_reporting(bool)`. Engine defaults **true**
      (libghostty-vt parity); upstream `title-report` defaults **false** (`Surface.zig:983`),
      so wire `set_title_reporting(config.title_report)` with a `false` config default.
    - `enquiry-response` → `set_enquiry_response(&[u8])` (ENQ 0x05 answerback; empty = silent).
    - `osc-color-report-format` → `set_osc_color_report_format(OscColorReportFormat::{None|Bit8|Bit16})`.
    - `image-storage-limit` → `Terminal::set_kitty_graphics_size_limit(usize)` (0 disables kitty
      graphics; engine default 320 MB). Call on startup + reload.
    - `scrollback-limit` → `terminal::Options::max_scrollback` at construction.
    - `vt-kam-allowed` → gate keyboard input on `config.vt_kam_allowed && modes.get(Mode::DisableKeyboard)`,
      mirroring `Surface.zig:2699`. No engine change needed.
    → **This is a whole PR of its own ("PR-VT-toggles"); do it FIRST next session** (unblocks
    the VT config toggles at feature-coverage L39-44 which are already `[x]` on the engine side).
  - 2026-07-14 (vt-tails): **tmux control mode — slice 5 (native Viewer) is yours (Josh
    committed to full tmux).** vt-tails is porting the pure engine parsers (slices 1–3 MERGED
    #257/#259/#261; slice 4 = the DCS `1000p` → `Notification` event seam, in progress). Slice 5
    is the big one: port `~/local/ghostty/src/terminal/tmux/viewer.zig` (~2,283 LoC) into the
    **app/termio** layer — map the `Notification` stream to native surfaces (tabs/splits), own a
    per-tmux-window `Terminal`, and drive tab/pane lifecycle. This is the *only* piece that makes
    `tmux -CC` app-observable. **Engine API you'll consume** (all in `qwertty-term-vt::tmux`):
    `ControlParser` (already fed by the DCS seam), `layout::Layout` (window/pane geometry tree),
    `output::{Variable, format, parse_format}` (query pane state), and slice 4's
    `TerminalHandler::take_tmux_notifications() -> Vec<tmux::Notification>` drain (mirrors
    `take_clipboard`/pending-event seams). See ADR 004 for layering + `stream_handler.zig:393`
    for upstream's Viewer lifecycle (create on `.enter`, free on `.exit`). Big effort — worth its
    own multi-PR slice + likely a UX ADR (how tmux windows map to qwertty-term tabs vs splits).
    Not blocked on slice 4 for *design*; wait for slice 4's `take_tmux_notifications` to wire.

## Mission

Drive the macOS app-facing tails of `docs/feature-coverage.md` to green: Window & app
chrome, Colors & theming, Cursor, Tabs, Mouse, Clipboard, and the app-side Config surface.
T3 (config/keybinds) and T4 (app-polish) are both CLOSED and handed their remaining items
here. Territory: `crates/qwertty-term` + app/config wiring. Do NOT touch `qwertty-term-vt`,
renderer, or font internals (coordinate via Inbox).

## How this session shipped (the established pattern — reuse it)

Each feature: (1) verify upstream semantics in `~/local/ghostty` (pin `2da015cd6`, cite
`file:line`); (2) add a config field (`Option<String>`/`bool`) + parse enum + accessor in
`config.rs` (pattern: `ConfirmCloseSurface`); (3) store a derived field in `ControllerState`,
init in `Controller::new` + re-apply in `reload_config`; (4) apply the behavior (window chrome
lives in `spawn_tab` after `make_window`); (5) extend the single windowed smoke
`QWERTTY_TERM_SMOKE_WINDOWCHROME` (in `app.rs::run_windowchrome_smoke`) with an assertion — it
already covers 7 keys, launched with `--key=value` CLI overrides; (6) flip the feature-coverage
checkbox in the SAME PR; (7) run the full AGENTS.md gate; (8) ship.

**jj discipline that bit this session (follow memory `jj-new-before-next-pr`):** after pushing
a PR, ALWAYS `jj git fetch && jj new main@origin` before the next PR's edits. Stacking edits on
an unmerged PR + a squash-merge fetch causes divergence and a `jj restore` can clobber your
edits. Verify `git merge-base --is-ancestor <parent> origin/main` before every push. Push via
the signing workaround above.

## Backlog (live checklist = feature-coverage.md `[ ]`/`[~]` in my sections)

- **PR-VT-toggles (do FIRST — Inbox above):** wire the 6 VT config keys → engine seams.
- **PR3b:** `macos-titlebar-style` (default `.transparent`!): implement `native`/`transparent`/
  `hidden` (titlebarAppearsTransparent / titleVisibility / `.fullSizeContentView`); the full
  `tabs` Ventura style is a large custom NSWindow — write a PROPOSED ADR deferring it and map
  `tabs`→native for now. NOTE: `macos-window-buttons` has no effect when titlebar-style=hidden
  (buttons always hidden) — respect that interaction.
- **PR4:** `macos-secure-input` (+`-indication`, and the auto password-mode trigger). Use
  `EnableSecureEventInput()`/`DisableSecureEventInput()` — a Carbon C API that works UNBUNDLED.
  The termio password-mode event already exists (feeds the 🔒 title suffix). Upstream
  `SecureInput.swift`.
- **PR5:** `set_tab_title` keybind action (app dispatch, prompts for a title — see upstream
  `changeTitle`/`promptTitle`) + `clipboard-read`/`clipboard-write` permission gates (OSC 52
  allow/deny/ask; upstream `ClipboardAccess`).
- **PR6 (triage each — implement or ADR-defer):** `cursor-click-to-move` (OSC 133 zone —
  needs prompt info; may need a vt seam → Inbox vt-tails), `command-palette` (large UI),
  undo/redo (`undo-timeout`), `macos-custom-icon`/`-icon*` (bundle-only → ADR-defer per the
  app-bundle-constraint memory), `macos-menu-bar`, `focus-follows-mouse` polish.

Renderer/vt-owned (NOT my territory — route via Inbox if needed): `bold-color`,
`faint-opacity`, `cursor-opacity`, `cell-foreground`/`cell-background`,
`background-opacity-cells`, `palette-generate`/`palette-harmonious`, cursor-blink timer.

## Log

- 2026-07-14: session start; workspace created; T3+T4 closed, tails inherited.
- 2026-07-14: PR1 (#243, MERGED) — `window-subtitle` (native `NSWindow.subtitle` from cwd),
  `window-new-tab-position` (`current`/`end`, upstream `TerminalController.swift:456`),
  `window-show-tab-bar` (`auto`/`always`/`never` → `NSWindowTabbingMode`). New
  `QWERTTY_TERM_SMOKE_WINDOWCHROME` smoke.
- 2026-07-14: PR2 (#246, MERGED) — `window-step-resize` (cell-sized `contentResizeIncrements`),
  `macos-window-shadow` (`hasShadow`), `macos-window-buttons` (`visible`/`hidden`). Smoke → 6 asserts.
- 2026-07-14: PR3 (#251, MERGED) — `window-theme` (`auto`/`system`/`light`/`dark` → per-window
  `NSAppearance`, `auto` by bg luminance; live on reload). `window-titlebar-background`/
  `-foreground` marked `[—]` GTK-only. Smoke → 7 asserts. Hit the squash-merge rebase hazard
  (recovered). Found the 1Password-signing push blocker + workaround (memory).
- 2026-07-14: CLOSEOUT — recycling for context length. 3 PRs merged, gate green throughout.
  **Respawn to continue** from this file + `docs/threads/t3-config-keybinds.md`/`t4-app-polish.md`
  specs. First action next session: wire the vt-tails Inbox config toggles.
