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
}
