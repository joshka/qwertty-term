//! Desktop-notification model + rate limiting for OSC 9 / OSC 777.
//!
//! A terminal application requests a desktop notification via `OSC 9 ; body ST`
//! (iTerm2 form, empty title) or `OSC 777 ; notify ; title ; body ST` (rxvt
//! form). The VT engine parses these and latches the most recent one; the app
//! drains it each pace tick (`Engine::take_notification`).
//!
//! Before delivering an OS notification we apply the same rate limiting
//! upstream Ghostty enforces in its *core* `Surface.showDesktopNotification`
//! (not the platform layer), so the policy is identical regardless of frontend:
//!
//! - **Global throttle**: at most one notification per second across all
//!   surfaces (upstream `Surface.zig:5977`).
//! - **Identical dedup**: a notification with the same `(title, body)` as the
//!   last one is suppressed if it arrives within 5 seconds (upstream
//!   `Surface.zig:5991`, digest compare).
//!
//! Actual delivery is the app's job and is platform-gated: real macOS
//! notifications require a signed `.app` bundle + `UNUserNotificationCenter`
//! authorization (see ADR 0003). When unbundled the app falls back to a dock
//! attention request. This module is pure (no AppKit) so the throttle is
//! unit-testable without a GUI; the caller passes an explicit `now`.

use std::time::{Duration, Instant};

/// A desktop notification parsed from OSC 9 / OSC 777. `title` is empty for the
/// OSC 9 (iTerm2) form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub title: String,
    pub body: String,
}

impl Notification {
    pub fn new(title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
        }
    }
}

/// At most one notification per second, globally (upstream policy).
const GLOBAL_INTERVAL: Duration = Duration::from_secs(1);
/// Identical `(title, body)` is suppressed within this window (upstream policy).
const IDENTICAL_WINDOW: Duration = Duration::from_secs(5);

/// Rate limiter mirroring upstream's core notification throttle. Holds the last
/// delivery time and the last delivered `(title, body)`; [`Self::admit`]
/// decides whether a fresh request should be delivered *now*.
#[derive(Debug, Default)]
pub struct NotificationThrottle {
    last_delivered: Option<Instant>,
    last_notification: Option<Notification>,
}

impl NotificationThrottle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decide whether `notification` should be delivered at `now`, updating
    /// internal state when it is admitted. Returns `true` to deliver.
    ///
    /// Order matches upstream: the identical-dedup check is only meaningful
    /// alongside the global throttle — an identical repeat inside the 5s window
    /// is dropped, and anything (identical or not) inside the 1s window is
    /// dropped. A distinct notification after ≥1s is admitted.
    pub fn admit(&mut self, notification: &Notification, now: Instant) -> bool {
        // Global 1/sec throttle.
        if let Some(last) = self.last_delivered
            && now.duration_since(last) < GLOBAL_INTERVAL
        {
            return false;
        }
        // Identical-within-5s dedup.
        if let (Some(last), Some(prev)) = (self.last_delivered, self.last_notification.as_ref())
            && prev == notification
            && now.duration_since(last) < IDENTICAL_WINDOW
        {
            return false;
        }
        self.last_delivered = Some(now);
        self.last_notification = Some(notification.clone());
        true
    }
}

// ---- notify-on-command-finish ------------------------------------------

/// When to notify on an OSC 133 command completion (`notify-on-command-finish`,
/// upstream `NotifyOnCommandFinish`, default `Never`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NotifyOnCommandFinish {
    /// Never notify (default).
    #[default]
    Never,
    /// Notify only when the surface/window is not focused.
    Unfocused,
    /// Always notify.
    Always,
}

impl NotifyOnCommandFinish {
    /// Parse the config value; unknown values fall back to the default `Never`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "unfocused" => Self::Unfocused,
            "always" => Self::Always,
            _ => Self::Never,
        }
    }
}

/// Which effects fire when a command finishes (`notify-on-command-finish-action`,
/// upstream default `bell` on, `notify` off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CommandFinishAction {
    pub bell: bool,
    pub notify: bool,
}

impl Default for CommandFinishAction {
    fn default() -> Self {
        // Upstream default: ring the bell, no desktop notification.
        Self {
            bell: true,
            notify: false,
        }
    }
}

impl CommandFinishAction {
    /// Parse a comma-separated flag list (`bell`, `notify`, each optionally
    /// `no-`-prefixed; `true`/`false`/`none` shortcuts). Empty/absent keeps the
    /// upstream default.
    pub fn parse(s: &str) -> Self {
        let mut action = Self::default();
        for raw in s.split(',') {
            let tok = raw.trim().to_ascii_lowercase();
            if tok.is_empty() {
                continue;
            }
            match tok.as_str() {
                "true" | "all" => {
                    action = Self {
                        bell: true,
                        notify: true,
                    }
                }
                "false" | "none" => {
                    action = Self {
                        bell: false,
                        notify: false,
                    }
                }
                "bell" => action.bell = true,
                "no-bell" => action.bell = false,
                "notify" => action.notify = true,
                "no-notify" => action.notify = false,
                _ => {}
            }
        }
        action
    }

    /// Whether any effect is active.
    pub fn any(&self) -> bool {
        self.bell || self.notify
    }
}

/// Decide whether a finished command should notify, given the configured mode,
/// whether its surface is focused, how long it ran, and the minimum-duration
/// threshold (`notify-on-command-finish-after`). Mirrors upstream: the mode
/// gates first, then the elapsed time must reach the threshold.
pub fn should_notify_command_finish(
    mode: NotifyOnCommandFinish,
    focused: bool,
    elapsed: Duration,
    after: Duration,
) -> bool {
    let mode_ok = match mode {
        NotifyOnCommandFinish::Never => false,
        NotifyOnCommandFinish::Always => true,
        NotifyOnCommandFinish::Unfocused => !focused,
    };
    mode_ok && elapsed >= after
}

/// Build the command-finish notification `(title, body)`, mirroring upstream's
/// wording: the title reflects the exit status and the body reports the elapsed
/// time and (when known) the exit code.
pub fn command_finish_notification(exit_code: Option<i32>, elapsed: Duration) -> Notification {
    let title = match exit_code {
        Some(0) => "Command Succeeded",
        Some(_) => "Command Failed",
        None => "Command Finished",
    };
    let body = match exit_code {
        Some(code) => format!(
            "Command took {} and exited with code {code}.",
            humanize_duration(elapsed)
        ),
        None => format!("Command took {}.", humanize_duration(elapsed)),
    };
    Notification::new(title, body)
}

/// Human-friendly elapsed-time string: `450ms`, `3.2s`, `1m05s`.
pub fn humanize_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms < 1000 {
        return format!("{ms}ms");
    }
    let total_secs = d.as_secs();
    if total_secs < 60 {
        // One decimal of seconds for sub-minute durations.
        let tenths = (d.as_millis() % 1000) / 100;
        return format!("{total_secs}.{tenths}s");
    }
    let mins = total_secs / 60;
    let secs = total_secs % 60;
    format!("{mins}m{secs:02}s")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn first_notification_is_admitted() {
        let base = Instant::now();
        let mut t = NotificationThrottle::new();
        assert!(t.admit(&Notification::new("", "hello"), base));
    }

    #[test]
    fn second_within_one_second_is_throttled_even_if_distinct() {
        let base = Instant::now();
        let mut t = NotificationThrottle::new();
        assert!(t.admit(&Notification::new("", "a"), base));
        // A *different* notification 500ms later is still throttled (1/sec).
        assert!(!t.admit(&Notification::new("", "b"), at(base, 500)));
        // ...but admitted once a full second has passed.
        assert!(t.admit(&Notification::new("", "b"), at(base, 1000)));
    }

    #[test]
    fn identical_within_five_seconds_is_deduped() {
        let base = Instant::now();
        let mut t = NotificationThrottle::new();
        let n = Notification::new("Alert", "deploy done");
        assert!(t.admit(&n, base));
        // Same content 2s later: past the 1s throttle but inside the 5s dedup.
        assert!(!t.admit(&n, at(base, 2000)));
        // Same content 6s later: dedup window elapsed → admitted again.
        assert!(t.admit(&n, at(base, 6000)));
    }

    #[test]
    fn distinct_after_one_second_is_admitted() {
        let base = Instant::now();
        let mut t = NotificationThrottle::new();
        assert!(t.admit(&Notification::new("A", "1"), base));
        assert!(t.admit(&Notification::new("B", "2"), at(base, 1500)));
        assert!(t.admit(&Notification::new("C", "3"), at(base, 3000)));
    }

    #[test]
    fn notify_mode_parses_with_never_fallback() {
        assert_eq!(
            NotifyOnCommandFinish::parse("always"),
            NotifyOnCommandFinish::Always
        );
        assert_eq!(
            NotifyOnCommandFinish::parse("Unfocused"),
            NotifyOnCommandFinish::Unfocused
        );
        assert_eq!(
            NotifyOnCommandFinish::parse("never"),
            NotifyOnCommandFinish::Never
        );
        assert_eq!(
            NotifyOnCommandFinish::parse("garbage"),
            NotifyOnCommandFinish::Never
        );
    }

    #[test]
    fn command_finish_action_parses_over_defaults() {
        // Default: bell on, notify off (upstream).
        assert_eq!(
            CommandFinishAction::parse(""),
            CommandFinishAction {
                bell: true,
                notify: false
            }
        );
        assert_eq!(
            CommandFinishAction::parse("notify, no-bell"),
            CommandFinishAction {
                bell: false,
                notify: true
            }
        );
        assert_eq!(
            CommandFinishAction::parse("true"),
            CommandFinishAction {
                bell: true,
                notify: true
            }
        );
        assert!(!CommandFinishAction::parse("none").any());
    }

    #[test]
    fn mode_and_threshold_gate_command_finish() {
        let after = Duration::from_secs(5);
        // Never: never, regardless of duration/focus.
        assert!(!should_notify_command_finish(
            NotifyOnCommandFinish::Never,
            false,
            Duration::from_secs(10),
            after
        ));
        // Always but under threshold: no.
        assert!(!should_notify_command_finish(
            NotifyOnCommandFinish::Always,
            false,
            Duration::from_secs(2),
            after
        ));
        // Always, over threshold: yes.
        assert!(should_notify_command_finish(
            NotifyOnCommandFinish::Always,
            true,
            Duration::from_secs(6),
            after
        ));
        // Unfocused: only when not focused.
        assert!(should_notify_command_finish(
            NotifyOnCommandFinish::Unfocused,
            false,
            Duration::from_secs(6),
            after
        ));
        assert!(!should_notify_command_finish(
            NotifyOnCommandFinish::Unfocused,
            true,
            Duration::from_secs(6),
            after
        ));
    }

    #[test]
    fn command_finish_notification_wording() {
        let ok = command_finish_notification(Some(0), Duration::from_secs(3));
        assert_eq!(ok.title, "Command Succeeded");
        assert!(ok.body.contains("exited with code 0"));

        let fail = command_finish_notification(Some(7), Duration::from_millis(450));
        assert_eq!(fail.title, "Command Failed");
        assert!(fail.body.contains("exited with code 7"));
        assert!(fail.body.contains("450ms"));

        let unknown = command_finish_notification(None, Duration::from_secs(75));
        assert_eq!(unknown.title, "Command Finished");
        assert!(!unknown.body.contains("code"));
        assert!(unknown.body.contains("1m15s"));
    }

    #[test]
    fn humanize_duration_buckets() {
        assert_eq!(humanize_duration(Duration::from_millis(999)), "999ms");
        assert_eq!(humanize_duration(Duration::from_millis(3200)), "3.2s");
        assert_eq!(humanize_duration(Duration::from_secs(65)), "1m05s");
    }
}
