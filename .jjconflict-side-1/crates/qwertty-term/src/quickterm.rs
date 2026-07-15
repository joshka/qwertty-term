//! Quick-terminal geometry: where the dropdown window sits and how big it is.
//!
//! Pure port of upstream's `QuickTerminalPosition.swift` (initial/final window
//! origin per screen edge) and `QuickTerminalSize.swift` (`calculate`) at
//! `2da015cd6`, extracted as plain math over rects so it is unit-testable
//! off the main thread — the load-bearing part of the feature, and the house
//! style for anything with fiddly geometry.
//!
//! Coordinate space matches AppKit's `NSWindow.setFrame`: screen coordinates
//! with a **bottom-left origin, y increasing upward**. All inputs are the
//! screen's `visibleFrame` (the usable area minus the menu bar and Dock) and
//! the window's own width/height; outputs are the window-frame origin to set.
//! `round`ing mirrors upstream's `round(...)` on the centered axis exactly.
//!
//! The [`crate::app`] window controller consumes these; nothing here touches
//! AppKit, so the whole module is portable and tested in isolation.

/// Which screen edge (or the center) the quick terminal drops from. Port of
/// `QuickTerminalPosition`. Default is [`Position::Top`] (upstream
/// `quick-terminal-position = top`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Position {
    #[default]
    Top,
    Bottom,
    Left,
    Right,
    Center,
}

impl Position {
    /// Parse the config spelling (`top`/`bottom`/`left`/`right`/`center`);
    /// `None` for an unknown value (caller keeps the default).
    pub fn parse(s: &str) -> Option<Position> {
        match s {
            "top" => Some(Position::Top),
            "bottom" => Some(Position::Bottom),
            "left" => Some(Position::Left),
            "right" => Some(Position::Right),
            "center" => Some(Position::Center),
            _ => None,
        }
    }
}

/// One axis of the quick-terminal size: a percentage of the screen dimension
/// or an absolute pixel count. Port of `QuickTerminalSize.Size`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Dim {
    /// Percent of the parent screen dimension (`0.0..=100.0`).
    Percentage(f32),
    /// Absolute device-independent pixels.
    Pixels(u32),
}

impl Dim {
    /// Resolve to concrete pixels against the parent screen dimension. Port of
    /// `Size.toPixels`.
    fn to_pixels(self, parent: f64) -> f64 {
        match self {
            Dim::Percentage(v) => parent * v as f64 / 100.0,
            Dim::Pixels(v) => v as f64,
        }
    }

    /// Parse one axis token: `N%` (percentage) or `Npx` (pixels). A bare value
    /// with no suffix is a config error upstream (`Config.zig:2631`), so
    /// returns `None` here. Whitespace around the token is trimmed.
    fn parse(s: &str) -> Option<Dim> {
        let s = s.trim();
        if let Some(pct) = s.strip_suffix('%') {
            Some(Dim::Percentage(pct.trim().parse().ok()?))
        } else if let Some(px) = s.strip_suffix("px") {
            Some(Dim::Pixels(px.trim().parse().ok()?))
        } else {
            None
        }
    }
}

/// The `quick-terminal-size` value: an optional size along the primary and
/// secondary axis (the axes are defined *by the position* — see
/// [`Size::calculate`]). Port of `QuickTerminalSize`. `None` on an axis means
/// "use the upstream default for that position/axis".
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Size {
    pub primary: Option<Dim>,
    pub secondary: Option<Dim>,
}

impl Size {
    /// Parse the `quick-terminal-size` config value: `<primary>[,<secondary>]`,
    /// each axis a `N%` or `Npx` token (`Config.zig:2626-2646`). An empty
    /// string is the all-defaults [`Size`]; an unparseable axis leaves that
    /// axis at its default (`None`).
    pub fn parse(s: &str) -> Size {
        let s = s.trim();
        if s.is_empty() {
            return Size::default();
        }
        let mut it = s.splitn(2, ',');
        let primary = it.next().and_then(Dim::parse);
        let secondary = it.next().and_then(Dim::parse);
        Size { primary, secondary }
    }

    /// The window `(width, height)` in pixels for `position` on a screen whose
    /// visible area is `screen_w` × `screen_h`. Direct port of
    /// `QuickTerminalSize.calculate` (`QuickTerminalSize.swift:52-83`),
    /// including its per-position axis mapping and default fallbacks
    /// (400 primary edge / full secondary edge; center 800×400 landscape,
    /// 400×800 portrait).
    pub fn calculate(&self, position: Position, screen_w: f64, screen_h: f64) -> (f64, f64) {
        match position {
            Position::Left | Position::Right => {
                let w = self.primary.map_or(400.0, |d| d.to_pixels(screen_w));
                let h = self.secondary.map_or(screen_h, |d| d.to_pixels(screen_h));
                (w, h)
            }
            Position::Top | Position::Bottom => {
                let w = self.secondary.map_or(screen_w, |d| d.to_pixels(screen_w));
                let h = self.primary.map_or(400.0, |d| d.to_pixels(screen_h));
                (w, h)
            }
            Position::Center => {
                if screen_w >= screen_h {
                    // Landscape.
                    let w = self.primary.map_or(800.0, |d| d.to_pixels(screen_w));
                    let h = self.secondary.map_or(400.0, |d| d.to_pixels(screen_h));
                    (w, h)
                } else {
                    // Portrait.
                    let w = self.secondary.map_or(400.0, |d| d.to_pixels(screen_w));
                    let h = self.primary.map_or(800.0, |d| d.to_pixels(screen_h));
                    (w, h)
                }
            }
        }
    }
}

/// A screen rect in AppKit screen coordinates (bottom-left origin). This is the
/// screen's `visibleFrame`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn max_x(&self) -> f64 {
        self.x + self.width
    }
    fn max_y(&self) -> f64 {
        self.y + self.height
    }
}

/// The horizontally-centered x origin within `visible` for a window `win_w`
/// wide (`round`ed, matching upstream).
fn centered_x(visible: &Rect, win_w: f64) -> f64 {
    (visible.x + (visible.width - win_w) / 2.0).round()
}

/// The vertically-centered y origin within `visible` for a window `win_h`
/// tall (`round`ed, matching upstream).
fn centered_y(visible: &Rect, win_h: f64) -> f64 {
    (visible.y + (visible.height - win_h) / 2.0).round()
}

/// The **off-screen start** origin the window animates *from* (and returns to
/// on animate-out). Port of `QuickTerminalPosition.initialOrigin`
/// (`QuickTerminalPosition.swift:66-91`) — note the deliberately quirky
/// bottom/center y values are replicated verbatim.
pub fn initial_origin(position: Position, visible: &Rect, win_w: f64, win_h: f64) -> (f64, f64) {
    match position {
        Position::Top => (centered_x(visible, win_w), visible.max_y()),
        Position::Bottom => (centered_x(visible, win_w), -win_h),
        Position::Left => (visible.x - win_w, centered_y(visible, win_h)),
        Position::Right => (visible.max_x(), centered_y(visible, win_h)),
        // Upstream uses `screen.visibleFrame.height - window.frame.width` for
        // the center start-y (a known quirk — the window slides in from a point
        // offset by its own *width*); replicated for fidelity.
        Position::Center => (centered_x(visible, win_w), visible.height - win_w),
    }
}

/// The **in-position** origin the window animates *to*. Port of
/// `QuickTerminalPosition.finalOrigin` (`QuickTerminalPosition.swift:94-119`).
pub fn final_origin(position: Position, visible: &Rect, win_w: f64, win_h: f64) -> (f64, f64) {
    match position {
        Position::Top => (centered_x(visible, win_w), visible.max_y() - win_h),
        Position::Bottom => (centered_x(visible, win_w), visible.y),
        Position::Left => (visible.x, centered_y(visible, win_h)),
        Position::Right => (visible.max_x() - win_w, centered_y(visible, win_h)),
        Position::Center => (centered_x(visible, win_w), centered_y(visible, win_h)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1440×877 visible frame at the origin (a typical laptop screen minus
    /// the menu bar), used across the geometry tests.
    fn screen() -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 877.0,
        }
    }

    // ---- Position::parse --------------------------------------------------

    #[test]
    fn position_parses_known_and_rejects_unknown() {
        assert_eq!(Position::parse("top"), Some(Position::Top));
        assert_eq!(Position::parse("center"), Some(Position::Center));
        assert_eq!(Position::parse("nonsense"), None);
        assert_eq!(Position::default(), Position::Top);
    }

    // ---- Size::calculate --------------------------------------------------

    #[test]
    fn size_defaults_per_position() {
        let s = Size::default();
        // Top/bottom: full width, 400 tall.
        assert_eq!(s.calculate(Position::Top, 1440.0, 877.0), (1440.0, 400.0));
        assert_eq!(
            s.calculate(Position::Bottom, 1440.0, 877.0),
            (1440.0, 400.0)
        );
        // Left/right: 400 wide, full height.
        assert_eq!(s.calculate(Position::Left, 1440.0, 877.0), (400.0, 877.0));
        assert_eq!(s.calculate(Position::Right, 1440.0, 877.0), (400.0, 877.0));
        // Center landscape: 800×400.
        assert_eq!(s.calculate(Position::Center, 1440.0, 877.0), (800.0, 400.0));
        // Center portrait: 400×800.
        assert_eq!(s.calculate(Position::Center, 800.0, 1440.0), (400.0, 800.0));
    }

    #[test]
    fn size_parse_syntax() {
        // Both axes.
        assert_eq!(
            Size::parse("50%,500px"),
            Size {
                primary: Some(Dim::Percentage(50.0)),
                secondary: Some(Dim::Pixels(500)),
            }
        );
        // Primary only → secondary stays default (maximized).
        assert_eq!(
            Size::parse("300px"),
            Size {
                primary: Some(Dim::Pixels(300)),
                secondary: None,
            }
        );
        // Empty → all defaults.
        assert_eq!(Size::parse(""), Size::default());
        assert_eq!(Size::parse("   "), Size::default());
        // A bare (suffix-less) value is a config error upstream → default axis.
        assert_eq!(Size::parse("500"), Size::default());
        // Whitespace tolerated around the comma.
        assert_eq!(
            Size::parse("20% , 40%"),
            Size {
                primary: Some(Dim::Percentage(20.0)),
                secondary: Some(Dim::Percentage(40.0)),
            }
        );
    }

    #[test]
    fn size_percentage_and_pixels() {
        // Top: primary = height axis, secondary = width axis.
        let half_tall = Size {
            primary: Some(Dim::Percentage(50.0)),
            secondary: Some(Dim::Pixels(1000)),
        };
        // width = 1000px (secondary), height = 50% of 877 = 438.5.
        assert_eq!(
            half_tall.calculate(Position::Top, 1440.0, 877.0),
            (1000.0, 438.5)
        );

        // Left: primary = width axis, secondary = height axis.
        let narrow = Size {
            primary: Some(Dim::Pixels(500)),
            secondary: Some(Dim::Percentage(80.0)),
        };
        // width = 500 (primary), height = 80% of 877 = 701.6.
        let (w, h) = narrow.calculate(Position::Left, 1440.0, 877.0);
        assert_eq!(w, 500.0);
        assert!((h - 701.6).abs() < 1e-9, "h={h}");
    }

    // ---- initial/final origin --------------------------------------------

    #[test]
    fn top_origins() {
        let vis = screen();
        // A full-width, 400-tall window (the top default).
        let (win_w, win_h) = (1440.0, 400.0);
        // Initial: centered x (0 here, full width), y at the top edge (maxY).
        assert_eq!(
            initial_origin(Position::Top, &vis, win_w, win_h),
            (0.0, 877.0)
        );
        // Final: same x, y pulled down by the window height so it sits flush
        // under the top edge.
        assert_eq!(
            final_origin(Position::Top, &vis, win_w, win_h),
            (0.0, 477.0)
        );
    }

    #[test]
    fn bottom_origins() {
        let vis = screen();
        let (win_w, win_h) = (1440.0, 400.0);
        // Initial: below the screen (y = -height).
        assert_eq!(
            initial_origin(Position::Bottom, &vis, win_w, win_h),
            (0.0, -400.0)
        );
        // Final: flush at the bottom (y = visible.y).
        assert_eq!(
            final_origin(Position::Bottom, &vis, win_w, win_h),
            (0.0, 0.0)
        );
    }

    #[test]
    fn left_right_origins_center_vertically() {
        let vis = screen();
        let (win_w, win_h) = (400.0, 877.0);
        // Full-height window → centered y is 0.
        let cy = centered_y(&vis, win_h);
        assert_eq!(cy, 0.0);
        // Left initial is off the left edge (x = -width); final flush at minX.
        assert_eq!(
            initial_origin(Position::Left, &vis, win_w, win_h),
            (-400.0, cy)
        );
        assert_eq!(final_origin(Position::Left, &vis, win_w, win_h), (0.0, cy));
        // Right initial is off the right edge (x = maxX); final flush at maxX-width.
        assert_eq!(
            initial_origin(Position::Right, &vis, win_w, win_h),
            (1440.0, cy)
        );
        assert_eq!(
            final_origin(Position::Right, &vis, win_w, win_h),
            (1040.0, cy)
        );
    }

    #[test]
    fn center_final_is_screen_centered() {
        let vis = screen();
        let (win_w, win_h) = (800.0, 400.0);
        let (fx, fy) = final_origin(Position::Center, &vis, win_w, win_h);
        // Centered on both axes.
        assert_eq!(fx, ((1440.0 - 800.0) / 2.0f64).round());
        assert_eq!(fy, ((877.0 - 400.0) / 2.0f64).round());
    }

    #[test]
    fn centered_axis_is_rounded() {
        // An odd leftover pixel must round (upstream `round(...)`), not truncate.
        let vis = Rect {
            x: 0.0,
            y: 0.0,
            width: 1441.0,
            height: 877.0,
        };
        // (1441 - 800)/2 = 320.5 → rounds to 321 (round-half-away/​to-even both
        // give 320 or 321; Rust f64::round is half-away-from-zero → 321).
        assert_eq!(centered_x(&vis, 800.0), 321.0);
    }

    #[test]
    fn origins_respect_a_nonzero_screen_origin() {
        // A second display offset to the right of the primary: visibleFrame.x
        // is 1440. All origins must be relative to it.
        let vis = Rect {
            x: 1440.0,
            y: 100.0,
            width: 1000.0,
            height: 600.0,
        };
        let (win_w, win_h) = (1000.0, 300.0);
        // Top final: x = centered within the offset frame, y = maxY - height.
        let (fx, fy) = final_origin(Position::Top, &vis, win_w, win_h);
        assert_eq!(fx, 1440.0); // full-width window → centered x is the frame x
        assert_eq!(fy, 100.0 + 600.0 - 300.0); // 400
    }
}
