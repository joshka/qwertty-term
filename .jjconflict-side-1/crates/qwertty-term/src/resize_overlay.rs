//! Resize-overlay model: the small `cols ⨯ rows` HUD shown over a pane while
//! the window is being live-resized (`resize-overlay*`).
//!
//! Upstream draws this purely in the apprt layer (a SwiftUI text overlay over
//! the Metal surface), never in the renderer — so the app hosts an AppKit
//! overlay view and the renderer stays untouched. This module is the pure part:
//! the config enums, the HUD text, the show/suppress decision, and the position
//! → frame-origin mapping — all unit-testable without AppKit.

use std::time::Duration;

/// Default overlay lifetime with no further resize (upstream 750ms).
pub const DEFAULT_DURATION_MS: f64 = 750.0;
/// Inset from the container edge for non-centered positions, in points.
pub const MARGIN: f64 = 12.0;

/// When to show the resize overlay (`resize-overlay`, upstream default
/// `after-first`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResizeOverlayMode {
    /// Show on every resize, including the initial window size.
    Always,
    /// Never show.
    Never,
    /// Show on resizes *after* the initial size (the default).
    #[default]
    AfterFirst,
}

impl ResizeOverlayMode {
    /// Parse the config value; unknown values fall back to the default.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "always" => Self::Always,
            "never" => Self::Never,
            _ => Self::AfterFirst,
        }
    }

    /// Whether to show the overlay for a sizing event. `first_size` is true for
    /// the initial window size (creation), false for a later resize.
    pub fn should_show(self, first_size: bool) -> bool {
        match self {
            Self::Never => false,
            Self::Always => true,
            Self::AfterFirst => !first_size,
        }
    }
}

/// Where in the pane the overlay sits (`resize-overlay-position`, default
/// `center`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResizeOverlayPosition {
    #[default]
    Center,
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl ResizeOverlayPosition {
    /// Parse the config value; unknown values fall back to `center`.
    pub fn parse(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "top-left" => Self::TopLeft,
            "top-center" => Self::TopCenter,
            "top-right" => Self::TopRight,
            "bottom-left" => Self::BottomLeft,
            "bottom-center" => Self::BottomCenter,
            "bottom-right" => Self::BottomRight,
            _ => Self::Center,
        }
    }

    /// The overlay's bottom-left origin `(x, y)` within a `container` of size
    /// `(cw, ch)` for a HUD of size `(hw, hh)`, in AppKit (bottom-left origin)
    /// content-view coordinates. Non-centered axes inset by [`MARGIN`].
    pub fn origin(self, container: (f64, f64), hud: (f64, f64)) -> (f64, f64) {
        let (cw, ch) = container;
        let (hw, hh) = hud;
        let left = MARGIN;
        let right = (cw - hw - MARGIN).max(0.0);
        let x_center = ((cw - hw) / 2.0).max(0.0);
        let bottom = MARGIN;
        let top = (ch - hh - MARGIN).max(0.0);
        let y_center = ((ch - hh) / 2.0).max(0.0);
        match self {
            Self::Center => (x_center, y_center),
            Self::TopLeft => (left, top),
            Self::TopCenter => (x_center, top),
            Self::TopRight => (right, top),
            Self::BottomLeft => (left, bottom),
            Self::BottomCenter => (x_center, bottom),
            Self::BottomRight => (right, bottom),
        }
    }
}

/// The HUD text for a grid: `"{cols} ⨯ {rows}"` (U+2A2F), matching upstream.
pub fn overlay_text(cols: usize, rows: usize) -> String {
    format!("{cols} \u{2A2F} {rows}")
}

/// Parse a `resize-overlay-duration` in milliseconds into a `Duration`
/// (non-negative; `0` disables the auto-hide delay — the overlay hides on the
/// next tick).
pub fn duration_from_ms(ms: f64) -> Duration {
    Duration::from_secs_f64((ms / 1000.0).max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parses_with_after_first_default() {
        assert_eq!(
            ResizeOverlayMode::parse("always"),
            ResizeOverlayMode::Always
        );
        assert_eq!(ResizeOverlayMode::parse("never"), ResizeOverlayMode::Never);
        assert_eq!(
            ResizeOverlayMode::parse("after-first"),
            ResizeOverlayMode::AfterFirst
        );
        assert_eq!(
            ResizeOverlayMode::parse("garbage"),
            ResizeOverlayMode::AfterFirst
        );
    }

    #[test]
    fn should_show_respects_mode_and_first_size() {
        // after-first: suppressed on the initial size, shown on later resizes.
        assert!(!ResizeOverlayMode::AfterFirst.should_show(true));
        assert!(ResizeOverlayMode::AfterFirst.should_show(false));
        // always/never are unconditional.
        assert!(ResizeOverlayMode::Always.should_show(true));
        assert!(ResizeOverlayMode::Always.should_show(false));
        assert!(!ResizeOverlayMode::Never.should_show(true));
        assert!(!ResizeOverlayMode::Never.should_show(false));
    }

    #[test]
    fn text_uses_the_cross_glyph() {
        assert_eq!(overlay_text(80, 24), "80 \u{2A2F} 24");
    }

    #[test]
    fn center_is_middle_of_the_container() {
        let (x, y) = ResizeOverlayPosition::Center.origin((200.0, 100.0), (80.0, 20.0));
        assert!((x - 60.0).abs() < 1e-9);
        assert!((y - 40.0).abs() < 1e-9);
    }

    #[test]
    fn corners_inset_by_margin_in_bottom_left_space() {
        let container = (200.0, 100.0);
        let hud = (80.0, 20.0);
        // top-left: x = margin, y = ch - hh - margin.
        assert_eq!(
            ResizeOverlayPosition::TopLeft.origin(container, hud),
            (MARGIN, 100.0 - 20.0 - MARGIN)
        );
        // bottom-right: x = cw - hw - margin, y = margin.
        assert_eq!(
            ResizeOverlayPosition::BottomRight.origin(container, hud),
            (200.0 - 80.0 - MARGIN, MARGIN)
        );
    }

    #[test]
    fn duration_parses_ms() {
        assert_eq!(duration_from_ms(750.0), Duration::from_millis(750));
        assert_eq!(duration_from_ms(-5.0), Duration::ZERO);
    }
}
