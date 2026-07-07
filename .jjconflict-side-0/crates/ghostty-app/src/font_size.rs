//! Font-size state: the clamped point size driving the render grid, plus the
//! up/down/reset steps the View menu triggers.
//!
//! Pure and platform-independent so the font-size menu actions are unit-tested
//! without AppKit or a font stack. A size *change* triggers the host to rebuild
//! the font grid (new cell metrics → new grid geometry → target rebuild).

/// The smallest and largest point sizes the app allows (matches the spike's
/// clamp range).
pub const MIN_FONT_SIZE: f32 = 6.0;
pub const MAX_FONT_SIZE: f32 = 48.0;
/// The default point size when no config / env override is present.
pub const DEFAULT_FONT_SIZE: f32 = 14.0;
/// The per-step increment for Cmd-+/Cmd--.
pub const FONT_SIZE_STEP: f32 = 1.0;

/// Clamped font-size state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FontSize {
    /// The configured default, returned to by [`FontSize::reset`].
    default: f32,
    /// The current size (always within `[MIN_FONT_SIZE, MAX_FONT_SIZE]`).
    current: f32,
}

impl FontSize {
    /// New state with the given default (itself clamped), current == default.
    pub fn new(default: f32) -> Self {
        let default = clamp(default);
        FontSize {
            default,
            current: default,
        }
    }

    /// The current point size.
    pub fn get(self) -> f32 {
        self.current
    }

    /// Increase by one step (clamped). Returns whether the value changed.
    pub fn increase(&mut self) -> bool {
        self.set(self.current + FONT_SIZE_STEP)
    }

    /// Decrease by one step (clamped). Returns whether the value changed.
    pub fn decrease(&mut self) -> bool {
        self.set(self.current - FONT_SIZE_STEP)
    }

    /// Reset to the configured default. Returns whether the value changed.
    pub fn reset(&mut self) -> bool {
        self.set(self.default)
    }

    fn set(&mut self, value: f32) -> bool {
        let clamped = clamp(value);
        let changed = (clamped - self.current).abs() > f32::EPSILON;
        self.current = clamped;
        changed
    }
}

impl Default for FontSize {
    fn default() -> Self {
        FontSize::new(DEFAULT_FONT_SIZE)
    }
}

fn clamp(value: f32) -> f32 {
    value.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_default() {
        let fs = FontSize::new(14.0);
        assert_eq!(fs.get(), 14.0);
    }

    #[test]
    fn increase_and_decrease_step() {
        let mut fs = FontSize::new(14.0);
        assert!(fs.increase());
        assert_eq!(fs.get(), 15.0);
        assert!(fs.decrease());
        assert_eq!(fs.get(), 14.0);
    }

    #[test]
    fn clamps_at_bounds_and_reports_no_change() {
        let mut fs = FontSize::new(MAX_FONT_SIZE);
        assert!(!fs.increase(), "already at max");
        assert_eq!(fs.get(), MAX_FONT_SIZE);

        let mut fs = FontSize::new(MIN_FONT_SIZE);
        assert!(!fs.decrease(), "already at min");
        assert_eq!(fs.get(), MIN_FONT_SIZE);
    }

    #[test]
    fn reset_returns_to_default() {
        let mut fs = FontSize::new(14.0);
        fs.increase();
        fs.increase();
        assert_eq!(fs.get(), 16.0);
        assert!(fs.reset());
        assert_eq!(fs.get(), 14.0);
        assert!(!fs.reset(), "reset when already default is a no-op");
    }

    #[test]
    fn constructor_clamps_wild_defaults() {
        assert_eq!(FontSize::new(1000.0).get(), MAX_FONT_SIZE);
        assert_eq!(FontSize::new(0.0).get(), MIN_FONT_SIZE);
    }
}
