//! The platform-neutral font search query [`Descriptor`].
//!
//! This is the reduced port of the backend-independent fields of Ghostty's
//! `discovery.Descriptor` (`src/font/discovery.zig:34-97`, commit `2da015cd6`):
//! the observable query (family / style / codepoint / size / bold / italic /
//! monospace) plus its `hashcode`. It lives in its own module so **both**
//! discovery backends can share it: the CoreText backend
//! ([`crate::discovery`], macOS) builds a `CTFontDescriptor` from it
//! (`Descriptor::to_ct_descriptor`), and the fontconfig backend
//! ([`crate::fontconfig`], Linux) builds an `FcPattern` from it
//! (`Descriptor::to_fc_pattern`). The backend-specific conversions live with
//! their backends; only the query shape is here.
//!
//! Variation-axis targeting is a documented deferral in both backends (see
//! `docs/analysis/font-discovery.md` §2); it is not part of this reduced
//! surface.

/// A platform-neutral font search query (`discovery.zig:34-89`, the
/// backend-independent fields).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Descriptor {
    /// Font family to search for ("Fira Code", "monospace", …). `None` means
    /// don't constrain by family.
    pub family: Option<String>,
    /// A specific style-name string filter ("Bold Italic", …).
    pub style: Option<String>,
    /// A codepoint the font must be able to render (0 = don't care).
    pub codepoint: u32,
    /// Point size the font should support (for emoji px conversion; may be 0).
    pub size: f32,
    /// Prefer a font with the bold trait.
    pub bold: bool,
    /// Prefer a font with the italic trait.
    pub italic: bool,
    /// Prefer a font with the monospace trait.
    pub monospace: bool,
}

impl Descriptor {
    /// Hash the descriptor. The analog of upstream `Descriptor.hashcode`
    /// (`discovery.zig:91-97`) — used to key a discovery cache. We hash the same
    /// observable fields; variation axes are not in the reduced surface.
    pub fn hashcode(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.family.hash(&mut hasher);
        self.style.hash(&mut hasher);
        self.codepoint.hash(&mut hasher);
        self.size.to_bits().hash(&mut hasher);
        self.bold.hash(&mut hasher);
        self.italic.hash(&mut hasher);
        self.monospace.hash(&mut hasher);
        hasher.finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `test "descriptor hash"` (`discovery.zig:1146-1151`): a default descriptor
    /// still hashes to a nonzero code.
    #[test]
    fn descriptor_hash() {
        let d = Descriptor::default();
        assert_ne!(d.hashcode(), 0);
    }

    /// `test "descriptor hash family names"` (`discovery.zig:1153-1159`):
    /// different families hash differently.
    #[test]
    fn descriptor_hash_family_names() {
        let d1 = Descriptor {
            family: Some("A".into()),
            ..Default::default()
        };
        let d2 = Descriptor {
            family: Some("B".into()),
            ..Default::default()
        };
        assert_ne!(d1.hashcode(), d2.hashcode());
    }
}
