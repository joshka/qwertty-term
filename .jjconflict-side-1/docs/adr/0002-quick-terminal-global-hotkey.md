# ADR 0002 — Quick-terminal trigger: in-app hotkey now, global hotkey deferred

- **Status:** ACCEPTED (in-app slice landed; global hotkey deferred)
- **Date:** 2026-07-12
- **Thread:** T4 (app polish)
- **Context commit:** upstream `2da015cd6`

## Context

The quick terminal (dropdown) needs a trigger. Upstream Ghostty exposes it two
ways on macOS:

1. A **menu item** (View → "Toggle Quick Terminal") and any normal `keybind`,
   which fire only while Ghostty is the frontmost app.
2. A **global hotkey** via `keybind = global:<chord>`, which fires no matter
   which app is frontmost — the defining feature of a "drop-down from anywhere"
   terminal (think iTerm2's hotkey window / Guake / yakuake).

A true global hotkey on macOS requires registering a system-wide handler. The
two mechanisms are:

- **Carbon `RegisterEventHotKey`** — works without special permission for a
  fixed chord, but is a deprecated Carbon API and only supports a static key
  combo (no arbitrary `keybind` chords).
- **`CGEventTap` / `NSEvent.addGlobalMonitorForEvents`** — flexible, but a
  global *monitor* can only observe (not consume) events unless the app is
  granted **Accessibility** permission (`AXIsProcessTrusted`), which needs a
  signed/bundled app and an explicit user grant in System Settings → Privacy &
  Security → Accessibility. A raw `cargo run` binary (how this app is driven in
  development and in the smokes) cannot get that grant.

## Decision

Ship the **in-app trigger first**: a View-menu item "Toggle Quick Terminal"
with a `Cmd-`` ` `` key equivalent, dispatching `Controller::toggle_quick_terminal`.
This delivers the whole dropdown feature — window, position/size geometry,
slide animation, autohide — usable whenever the app is frontmost, with **zero
new permissions** and a testable path (the windowed smoke drives the toggle).

**Defer the global hotkey.** It is gated on app packaging + an Accessibility
grant that the current dev/smoke workflow doesn't have, so wiring it now would
add an untestable, permission-dependent surface for marginal additional value
over the in-app chord. When the app is bundled and signed (a later milestone),
revisit with `RegisterEventHotKey` for a fixed default chord and/or a
CGEventTap behind an Accessibility-permission prompt, wired to the same
`toggle_quick_terminal` entry point — no changes to the dropdown itself.

## Consequences

- Users get the dropdown today, triggered while Ghostty is focused.
- "Drop down over another app" (the frontmost-agnostic behavior) waits for the
  packaging milestone; tracked as a follow-up issue.
- The trigger is a single funnel (`toggle_quick_terminal`), so adding the
  global path later is additive.
- `Cmd-`` ` `` can collide with the macOS "cycle windows" shortcut when the app
  has multiple windows; a menu key-equivalent generally wins within the app,
  and the chord becomes user-configurable once T3's keybind system lands.
