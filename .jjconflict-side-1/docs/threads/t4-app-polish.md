# T4 — App polish thread

**Model:** Opus (spawn Sonnet sub-agents for the S-sized mechanical items) · **Wave:** 1 ·
**Workspace:** `work/t4` · **Status:** `status/t4.md`
**Territory:** `crates/qwertty-term` (the macOS app) — views, windows, tabs, splits,
menus, clipboard, smokes. Renderer/vt/font are read-only (file-claim for one-line hooks).
Rules: `docs/threads/README.md`.

## Mission

Make the daily-driver *feel* finished: the remaining `[ ]` items in the app-facing
sections of `docs/feature-coverage.md` (window/tabs/clipboard/mouse/process groups).
Josh uses this terminal all day — every item ships with a windowed smoke assertion, the
same way tab-keys/splits/search did (read `smoke.rs` + existing `GHOSTTY_APP_SMOKE_*`
lanes first; that pattern is the house style).

## Backlog

Ordered roughly by user-visible value. Verify each behavior against upstream
`macos/Sources` + `Config.zig` defaults at `2da015cd6`; cite in the PR.

- [ ] **Selection gestures** (M): double-click word select (upstream word chars +
      `selection-word-chars` semantics), triple-click line, shift-click extend, drag past
      edge autoscroll. Evidence: smoke drives NSEvents, asserts selection text.
- [ ] **OSC-synced tab titles** (S/M): live titles from OSC 0/2 (engine already stores;
      surface via snapshot/event), `title-report` gate, `window-subtitle` basics, fall
      back to pwd like upstream. Evidence: smoke feeds OSC 2, asserts tab title.
- [ ] **Quick terminal** (M/L): dropdown window, `quick-terminal-position/size/animation-
      duration/autohide` defaults, global-ish hotkey within app scope (true global hotkey
      needs accessibility perms — implement in-app first, note ADR for global).
- [ ] **Notifications & command-finish** (M): bell (`bell-features` audio/attention),
      OSC 9 desktop notifications, `notify-on-command-finish` via shell-integration marks
      (+ after/action), OSC 9;4 progress in tab/dock per `progress-style`.
- [ ] **Mouse behaviors** (S each, Sonnet-able): `mouse-hide-while-typing`,
      `focus-follows-mouse` (pane-level), `middle-click-action`/`right-click-action` +
      context menu (copy/paste/split/close per upstream's menu), `cursor-click-to-move`
      (prompt-line cursor repositioning via OSC133 zone — check upstream gating).
- [ ] **Clipboard hardening** (S/M): `clipboard-paste-protection` (+bracketed-safe),
      `clipboard-trim-trailing-spaces`, `selection-clear-on-typing`/`-on-copy`,
      OSC52 `clipboard-read`/`-write` permission prompts per upstream policy.
- [ ] **Window state** (M): `window-save-state` (frames/tabs/splits/cwd restore),
      `window-width/height/position-*` initial geometry, `window-inherit-*`,
      `confirm-close-surface` (running-process check), `quit-after-last-window-closed`.
- [ ] **Resize overlay** (S): cols×rows HUD during live resize per `resize-overlay*`.
- [ ] **macOS chrome extras** (S each, lowest): `macos-titlebar-style` variants,
      `macos-window-buttons`, `macos-option-as-alt` per-side, secure-input indication
      (`macos-auto-secure-input` — password-mode signal already exists in termio events).

Config keys: T3 owns the config SYSTEM (Wave 2). For now add options to the existing
TOML config struct only when trivially additive; otherwise implement with internal
defaults + leave the key in T3's `## Inbox`. Don't build option infrastructure.

## Method rules

Every feature: upstream-verified default + cited behavior, windowed smoke lane (extend the
existing pattern; keep every prior smoke green — they're the app's regression net), and a
`docs/feature-coverage.md` checkbox flip in the same PR. Re-entrancy: never hold a
controller borrow across AppKit calls that can synchronously call back (close/makeKey —
two prior crashes). Poison-resilience and per-pane isolation patterns are established;
follow them. Anything needing engine data not in the snapshot: file-claim a minimal
accessor in vt, don't duplicate state.

## Definition of done

Window/tabs/clipboard/mouse/process sections of feature-coverage.md at `[x]` except
explicitly-deferred ADR items; all smokes green; Josh's daily-drive complaints trend to
zero (field reports route here — treat any Josh screenshot as a P0 field-bug item with
the root-cause + missing-test-class discipline).
