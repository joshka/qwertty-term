# ADR 0003 — Desktop notifications: parse + throttle + fallback now, bundled delivery deferred

- **Status:** ACCEPTED (OSC 9/777 pipeline + dock-attention fallback landed; real OS notification deferred)
- **Date:** 2026-07-13
- **Thread:** T4 (app polish)
- **Context commit:** upstream `2da015cd6`

## Context

Terminal applications request a desktop notification with either
`OSC 9 ; <body> ST` (iTerm2 form, empty title) or
`OSC 777 ; notify ; <title> ; <body> ST` (rxvt form). Upstream Ghostty parses
both, applies a rate limit in its **core** `Surface.showDesktopNotification`
(one notification per second globally + suppression of an identical
`(title, body)` within 5 seconds), gates them on the `desktop-notifications`
config key (default true), and then hands the notification to the macOS apprt,
which delivers it via **`UNUserNotificationCenter`** (`.alert, .sound`, with the
surface title as the notification subtitle).

`UNUserNotificationCenter.current()` requires the process to be a **properly
signed `.app` bundle with a `CFBundleIdentifier`** and a runtime authorization
grant (the first call prompts the user). Calling it from an unbundled binary
throws/crashes. This app is currently launched as a bare CLI binary
(`target/debug/qwertty-term --window`, which is also how every windowed smoke
drives it) — it has no bundle, no identifier, and no signing, exactly the same
packaging gap that deferred the quick-terminal global hotkey (ADR 0002).

## Decision

Ship the **full notification pipeline up to the delivery seam now**, with an
unbundled-safe delivery fallback:

1. **VT parse + hook** (`crates/qwertty-term-vt`): OSC 9/777 already parse to
   `Command::ShowDesktopNotification { title, body }`; add a
   `show_desktop_notification` handler that latches the latest `(title, body)`
   (mirroring the bell hook), drained via `take_notification`.
2. **Core gate + throttle** (`crates/qwertty-term`): the pace tick drains each
   surface's pending notification, drops it when `desktop-notifications` is off
   (the core-level gate, matching upstream), and rate-limits the rest through a
   pure, unit-tested `NotificationThrottle` that reproduces upstream's 1/sec +
   5s-identical policy.
3. **Delivery fallback**: because the app is unbundled, deliver by bouncing the
   Dock (`requestUserAttention(.informational)`) and logging the title/body —
   the same "works today, no new permissions" posture as ADR 0002. The admitted
   `(title, body)` is recorded so the windowed smoke can assert the pipeline.

**Defer the real OS notification.** Wiring `UNUserNotificationCenter` now would
add an untestable, permission- and packaging-dependent surface that crashes in
the current dev/smoke workflow. When the app is bundled and signed (the same
milestone ADR 0002 waits on), swap the fallback body of
`Controller::deliver_notification` for a `UNMutableNotificationContent` +
`UNUserNotificationCenter.add` call (title, subtitle = surface title, body,
`.default` sound), guarded by `requestAuthorization`/`authorizationStatus` —
no changes to the parse, gate, or throttle stages.

## Consequences

- Apps' OSC 9/777 notifications are parsed, gated, throttled, and surfaced
  (as a Dock bounce + log) today, with zero new permissions.
- The `desktop-notifications` config key and the upstream-faithful rate limiting
  are live and testable now, independent of packaging.
- Real banner/alert notifications wait for the bundling milestone; the swap is
  localized to one method behind a single delivery seam, tracked as a follow-up.
- OSC 9;4 progress reports and `notify-on-command-finish` (which needs OSC 133
  command-boundary tracking not yet in the VT engine) are separate follow-up
  slices, out of scope here.
