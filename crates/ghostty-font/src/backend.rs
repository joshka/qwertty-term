//! Font backend enumeration.
//!
//! Port of (a subset of) Ghostty's `src/font/backend.zig` (commit
//! `2da015cd6`). Ghostty's `Backend` enum spans discovery + rendering +
//! shaping combinations across FreeType/Fontconfig/CoreText/HarfBuzz/
//! web-canvas, selected at Zig comptime based on target OS. `ghostty-font`
//! only stubs the backend this chunk is scoped to: CoreText (macOS). Other
//! backends are deferred to whichever future chunk implements font
//! discovery/rendering for those platforms; the enum is intentionally left
//! extensible (non-exhaustive) rather than guessing their shape now.

/// Font backend used for discovery, rendering, and/or shaping.
///
/// Only [`Backend::CoreText`] is implemented today; other variants will be
/// added as later chunks bring up their platforms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Backend {
    /// CoreText for font discovery, rendering, and shaping (macOS).
    CoreText,
}

impl Backend {
    /// Returns the default backend for the current platform.
    ///
    /// Only macOS is supported today (`CoreText`); other platforms will
    /// gain a default once their backend variant exists.
    #[cfg(target_os = "macos")]
    pub fn default_for_platform() -> Backend {
        Backend::CoreText
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(target_os = "macos")]
    fn default_is_coretext_on_macos() {
        assert_eq!(Backend::default_for_platform(), Backend::CoreText);
    }
}
