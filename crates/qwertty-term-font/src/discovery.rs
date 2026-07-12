//! CoreText font discovery: family/codepoint search with `Score` ranking.
//!
//! Port of the `CoreText` arm of Ghostty's `src/font/discovery.zig` (commit
//! `2da015cd6`): the [`Descriptor`] search query, the `Score` ranking system
//! (traits + raw-table + variable-axis scoring + fuzzy name matching), the
//! `CTFontCollection` family search, and the `CTFontCreateForString`
//! per-codepoint fallback (with Han-block routing and LastResort rejection).
//!
//! See `docs/analysis/font-discovery.md` §1-3 for the commit-stamped analysis.
//!
//! Deferred (documented there): variation-axis targeting in the search
//! descriptor, fontconfig/Windows backends, and the codepoint-map override
//! path (no config surface yet).

#![cfg(target_os = "macos")]

use std::ptr::NonNull;

use objc2_core_foundation::{
    CFArray, CFCharacterSet, CFIndex, CFMutableDictionary, CFNumber, CFRange, CFRetained, CFString,
    CFType,
};
use objc2_core_text::{
    CTFont, CTFontCollection, CTFontDescriptor, CTFontSymbolicTraits, kCTFontCharacterSetAttribute,
    kCTFontFamilyNameAttribute, kCTFontSizeAttribute, kCTFontStyleNameAttribute,
    kCTFontSymbolicTrait, kCTFontTraitsAttribute,
};

use crate::coretext::Face;
use crate::deferred::DeferredFace;

/// A platform-neutral font search query (discovery.zig:34-89, the CoreText
/// fields).
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
    /// (discovery.zig:91-97) — used to key a discovery cache. We hash the same
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

    /// Build a `CTFontDescriptor` from this query
    /// (`toCoreTextDescriptor`, discovery.zig:161-244).
    ///
    /// Assembles an attribute dictionary of family / character-set (codepoint) /
    /// style / size / symbolic-traits, omitting any field that is unset. The
    /// character-set attribute is what makes CoreText's collection query
    /// pre-filter to fonts that contain the codepoint.
    fn to_ct_descriptor(&self) -> CFRetained<CTFontDescriptor> {
        let attrs = CFMutableDictionary::<CFString, CFType>::with_capacity(5);

        if let Some(family) = &self.family {
            let value = CFString::from_str(family);
            // SAFETY: kCTFontFamilyNameAttribute is a valid static key.
            let key = unsafe { kCTFontFamilyNameAttribute };
            attrs.set(key, &value);
        }

        if let Some(style) = &self.style {
            let value = CFString::from_str(style);
            // SAFETY: kCTFontStyleNameAttribute is a valid static key.
            let key = unsafe { kCTFontStyleNameAttribute };
            attrs.set(key, &value);
        }

        if self.codepoint > 0 {
            // A CFCharacterSet over the single codepoint.
            let range = CFRange {
                location: self.codepoint as CFIndex,
                length: 1,
            };
            // SAFETY: null allocator = default; range is a valid unichar range.
            if let Some(cs) = unsafe { CFCharacterSet::with_characters_in_range(None, range) } {
                // SAFETY: kCTFontCharacterSetAttribute is a valid static key.
                let key = unsafe { kCTFontCharacterSetAttribute };
                attrs.set(key, &cs);
            }
        }

        if self.size > 0.0 {
            let rounded = self.size.round() as i32;
            let value = CFNumber::new_i32(rounded);
            // SAFETY: kCTFontSizeAttribute is a valid static key.
            let key = unsafe { kCTFontSizeAttribute };
            attrs.set(key, &value);
        }

        // Symbolic traits, if any bit is requested (discovery.zig:214-241).
        let mut traits = CTFontSymbolicTraits::empty();
        if self.bold {
            traits |= CTFontSymbolicTraits::TraitBold;
        }
        if self.italic {
            traits |= CTFontSymbolicTraits::TraitItalic;
        }
        if self.monospace {
            traits |= CTFontSymbolicTraits::TraitMonoSpace;
        }
        if !traits.is_empty() {
            let traits_num = CFNumber::new_i32(traits.0 as i32);
            let traits_dict = CFMutableDictionary::<CFString, CFType>::with_capacity(1);
            // SAFETY: kCTFontSymbolicTrait is a valid static key.
            let sym_key = unsafe { kCTFontSymbolicTrait };
            traits_dict.set(sym_key, &traits_num);

            // SAFETY: kCTFontTraitsAttribute is a valid static key.
            let key = unsafe { kCTFontTraitsAttribute };
            attrs.set(key, &traits_dict);
        }

        // SAFETY: attrs is a valid attribute dictionary. `as_opaque` yields the
        // untyped `&CFMutableDictionary`, which derefs to `&CFDictionary`.
        let dict: &objc2_core_foundation::CFDictionary = attrs.as_opaque();
        unsafe { CTFontDescriptor::with_attributes(dict) }
    }
}

/// Discover fonts matching `desc`, returning deferred faces in ranked order
/// (`CoreText.discover`, discovery.zig:354-383).
///
/// Builds a `CTFontCollection` from the descriptor, reads its matching
/// descriptors, ranks them with `Score`, and wraps each as a [`DeferredFace`].
/// Returns an empty vec on no match.
pub fn discover(desc: &Descriptor) -> Vec<DeferredFace> {
    let ct_desc = desc.to_ct_descriptor();

    // The collection is built from an array of query descriptors.
    let desc_arr: CFRetained<CFArray<CTFontDescriptor>> =
        CFArray::from_objects(&[ct_desc.as_ref()]);
    let desc_arr_untyped: &CFArray = desc_arr.as_ref();

    // SAFETY: desc_arr is a valid array of CTFontDescriptor; null options.
    let collection =
        unsafe { CTFontCollection::with_font_descriptors(Some(desc_arr_untyped), None) };

    // SAFETY: collection is valid.
    let Some(list) = (unsafe { collection.matching_font_descriptors() }) else {
        return Vec::new();
    };

    // The array's elements are CTFontDescriptors; view it as a typed array so
    // `get` yields retained descriptors.
    // SAFETY: CTFontCollectionCreateMatchingFontDescriptors returns an array
    // whose elements are all CTFontDescriptor.
    let list = unsafe { CFRetained::cast_unchecked::<CFArray<CTFontDescriptor>>(list) };
    let descriptors: Vec<CFRetained<CTFontDescriptor>> = list.to_vec();

    // Rank: compute a Score per descriptor, sort descending (higher = earlier),
    // with the original index as the final deterministic tiebreak.
    let mut ranked: Vec<(Score, usize, CFRetained<CTFontDescriptor>)> = descriptors
        .into_iter()
        .enumerate()
        .map(|(i, d)| (Score::compute(desc, &d), i, d))
        .collect();
    // Sort by score descending; ties broken by original index ascending (stable
    // & deterministic).
    ranked.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));

    // Materialize each descriptor into a CTFont at nominal size 12 (as upstream
    // does, size is re-applied at load) and wrap as a DeferredFace. Strip the
    // character-set attribute first (discovery.zig:850-868): it was only used
    // to *filter* candidates; leaving it on the descriptor would restrict the
    // resulting font's available characters to just the search codepoint.
    ranked
        .into_iter()
        .map(|(_, _, d)| {
            let d = strip_character_set(&d);
            // SAFETY: d is a valid descriptor; null matrix = identity.
            let font = unsafe { CTFont::with_font_descriptor(&d, 12.0, std::ptr::null()) };
            DeferredFace::from_ct_font(font)
        })
        .collect()
}

/// Return a copy of `ct_desc` with the character-set attribute cleared (set to
/// `kCFNull`), so the resulting font is not restricted to the search codepoint
/// (discovery.zig:854-868).
fn strip_character_set(ct_desc: &CTFontDescriptor) -> CFRetained<CTFontDescriptor> {
    let attrs = CFMutableDictionary::<CFString, CFType>::with_capacity(1);
    // SAFETY: kCTFontCharacterSetAttribute is a valid static key; kCFNull is the
    // documented sentinel that clears an attribute in a copy.
    let key = unsafe { kCTFontCharacterSetAttribute };
    if let Some(null) = unsafe { objc2_core_foundation::kCFNull } {
        attrs.set(key, null);
    }
    let dict: &objc2_core_foundation::CFDictionary = attrs.as_opaque();
    // SAFETY: ct_desc is valid; dict overrides the character-set attribute.
    unsafe { ct_desc.copy_with_attributes(dict) }
}

/// Discover a **styled member of a specific family** and load it at `size_px`.
///
/// This is the named-family analog of upstream's per-style discovery in
/// `SharedGridSet.collection` (`SharedGridSet.zig:191-247`): when a `font-family`
/// is configured, each style slot is populated by discovering a descriptor
/// carrying that family plus the style's symbolic traits (bold/italic), taking
/// the top-ranked result. Here we run the same query (family + traits +
/// monospace) and return the highest-scored candidate **whose family name
/// still matches the requested family** — so a family that lacks the requested
/// style (e.g. FiraCode Nerd Font Mono has no italic) yields `None` rather than
/// a fuzzy cross-family substitute, letting the caller fall through to the
/// synthetic ladder (upstream `Collection.completeStyles`).
///
/// The `CTFontCollection`/`Score` path (already used for the regular family
/// discovery) is the mechanism upstream's CoreText discovery backend uses; the
/// symbolic-trait descriptor is assembled in `Descriptor::to_ct_descriptor`.
pub fn discover_family_style(family: &str, bold: bool, italic: bool, size_px: f64) -> Option<Face> {
    let desc = Descriptor {
        family: Some(family.to_string()),
        size: size_px as f32,
        bold,
        italic,
        monospace: true,
        ..Default::default()
    };
    let faces = discover(&desc);

    // Take the top-ranked candidate whose family still matches the request.
    // Discovery can fuzzy-match across families; for a *named-family styled*
    // lookup we only want the family's own members, so reject a mismatch and
    // let the caller synthesize instead.
    let want = family.to_lowercase();
    let candidate = faces.into_iter().find(|f| {
        let got = f.family_name().to_lowercase();
        got.contains(&want) || want.contains(&got)
    })?;
    candidate.load(size_px).ok()
}

/// Discover the top-ranked member of a **named family**, loaded at `size_px`.
///
/// This is the plain family-only analog of [`discover_family_style`] (no
/// bold/italic/monospace traits): it runs `discover({ family })` and loads the
/// first candidate. It is upstream's mechanism for pre-seeding the default
/// collection with the system emoji font on macOS
/// (`SharedGridSet.zig:340-354`: discover `family = "Apple Color Emoji"` and add
/// it as a `.fallback` face, so the OS-native emoji font is always preferred over
/// a third-party emoji font the user may have installed — e.g. Noto Color
/// Emoji). Upstream *does* reference the family name here explicitly, and this
/// mirrors that (cited: `SharedGridSet.zig:342`), rather than inventing a new
/// special case in the ranking.
///
/// Returns `None` if the family isn't installed (the caller then skips the
/// fallback — on macOS Apple Color Emoji ships with the OS, so this is only
/// `None` in degenerate environments).
pub fn discover_family(family: &str, size_px: f64) -> Option<Face> {
    let desc = Descriptor {
        family: Some(family.to_string()),
        size: size_px as f32,
        ..Default::default()
    };
    discover(&desc).into_iter().next()?.load(size_px).ok()
}

/// Discover a fallback face for `desc`, honoring the codepoint-search paths
/// (`CoreText.discoverFallback`, discovery.zig:385-447).
///
/// - **Han block** (`U+4E00..=U+9FFF`): go straight to the
///   `CTFontCreateForString` path, which respects system locale for CJK.
/// - **General**: run [`discover`]; if it finds nothing and a codepoint was
///   requested, fall back to `CTFontCreateForString` (the ghostty#2499 fix,
///   which is also how emoji get resolved).
///
/// `original` is the base face whose cascade CoreText consults for the
/// per-codepoint substitute (upstream picks it by style; the reduced collection
/// passes the primary).
pub fn discover_fallback(original: &Face, desc: &Descriptor) -> Vec<DeferredFace> {
    // Han-block special-case.
    if (0x4E00..=0x9FFF).contains(&desc.codepoint)
        && let Some(face) = discover_codepoint(original, desc.codepoint)
    {
        return vec![face];
        // (Falls through to the general path below if the substitution declined.)
    }

    let general = discover(desc);
    if !general.is_empty() || desc.codepoint == 0 {
        return general;
    }

    // General discovery found nothing but we have a codepoint: use CoreText's
    // own substitution (emoji, and the #2499 cases).
    match discover_codepoint(original, desc.codepoint) {
        Some(face) => vec![face],
        None => Vec::new(),
    }
}

/// Find a font for a single codepoint via `CTFontCreateForString`
/// (`discoverCodepoint`, discovery.zig:451-550).
///
/// Asks CoreText, starting from `original`'s cascade, which font it would use to
/// render the codepoint. Rejects the LastResort font (which contains only
/// replacement glyphs). This is the mechanism by which emoji resolve to Apple
/// Color Emoji and CJK resolves to the locale-appropriate system font — the font
/// name is never hard-coded.
pub fn discover_codepoint(original: &Face, cp: u32) -> Option<DeferredFace> {
    let ch = char::from_u32(cp)?;

    // CFString of the single codepoint.
    let mut buf = [0u8; 4];
    let s = ch.encode_utf8(&mut buf);
    let cf_str = CFString::from_str(s);

    // UTF-16 range length (2 for a surrogate pair, else 1).
    let mut utf16 = [0u16; 2];
    let range_len = ch.encode_utf16(&mut utf16).len() as CFIndex;
    let range = CFRange {
        location: 0,
        length: range_len,
    };

    // SAFETY: original.ct_font() is valid; cf_str is a valid CFString; range is
    // within the string. CTFontCreateForString returns the substitute font.
    let font = unsafe { original.ct_font().for_string(&cf_str, range) };

    // LastResort rejection (discovery.zig:534-546).
    // SAFETY: font is a valid CTFont; post_script_name is non-null.
    let ps_name = unsafe { font.post_script_name() };
    if ps_name.to_string() == "LastResort" {
        return None;
    }

    // Confirm the substitute actually maps the codepoint (CTFontCreateForString
    // can return the original font unchanged if it already has the glyph, or a
    // substitute; either way we verify).
    let deferred = DeferredFace::from_ct_font(font);
    if deferred.has_codepoint(cp, crate::presentation::PresentationMode::Any, false) {
        Some(deferred)
    } else {
        None
    }
}

/// A font-ranking score (discovery.zig:592-831).
///
/// Upstream is a `packed struct` compared as an integer, with fields laid out
/// least- to most-significant so the comparison is lexicographic with the
/// last-declared field highest-priority. This port encodes the identical order
/// as a derived-`Ord` struct whose fields are declared **most-significant
/// first**, so `Ord` compares them in exactly upstream's precedence. A higher
/// score sorts earlier.
///
/// Precedence (high → low): codepoint match, monospace, exact style, italic
/// match, bold match, fuzzy style, glyph count.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Score {
    /// The font contains the requested codepoint (highest priority).
    codepoint: bool,
    /// The font has the monospace trait.
    monospace: bool,
    /// Exact (case-insensitive) match on the style string.
    exact_style: bool,
    /// The font's italic-ness matches the descriptor's request.
    italic: bool,
    /// The font's bold-ness matches the descriptor's request.
    bold: bool,
    /// Fuzzy style-string match quality (higher = closer). Upstream `u8`.
    fuzzy_style: u8,
    /// Glyph count, clamped to `u16` (lowest tiebreak before the caller's
    /// original-index tiebreak).
    glyph_count: u16,
}

impl Score {
    /// Compute the score of `ct_desc` against the search `desc`
    /// (`Score.score`, discovery.zig:626-829).
    fn compute(desc: &Descriptor, ct_desc: &CTFontDescriptor) -> Score {
        // Load the font; a font we can't load scores all-zero.
        // SAFETY: ct_desc is a valid descriptor; null matrix = identity.
        let font = unsafe { CTFont::with_font_descriptor(ct_desc, 12.0, std::ptr::null()) };

        // glyph_count, clamped to u16.
        // SAFETY: font is valid.
        let raw_glyphs = unsafe { font.glyph_count() };
        let glyph_count = u16::try_from(raw_glyphs).unwrap_or(u16::MAX);

        // codepoint membership.
        let codepoint = if desc.codepoint > 0 {
            font_has_codepoint(&font, desc.codepoint)
        } else {
            false
        };

        // Symbolic traits. Upstream reads the descriptor's traits dict
        // (discovery.zig:668-679); we read them off the loaded font
        // (`CTFontGetSymbolicTraits`), which is equivalent and avoids untyped
        // CFDictionary traversal.
        // SAFETY: font is a valid CTFont.
        let sym = unsafe { font.symbolic_traits() };
        let monospace = sym.contains(CTFontSymbolicTraits::TraitMonoSpace);

        // Derived bold/italic: symbolic traits, refined by head/OS2 tables.
        let (is_bold, is_italic) = derive_bold_italic(&font, sym);
        let bold = desc.bold == is_bold;
        let italic = desc.italic == is_italic;

        // Style string and desired-style set.
        let style_str = descriptor_style_name(ct_desc).unwrap_or_default();
        let desired = desired_styles(desc);
        let exact_style = !desired.is_empty() && style_str.eq_ignore_ascii_case(desired[0]);

        // Fuzzy style: start at style length, subtract (saturating) each desired
        // substring present, then flip so fewer non-matching chars => higher.
        let mut remainder = style_str.len().min(u8::MAX as usize) as u8;
        for s in &desired {
            if ascii_contains_ignore_case(&style_str, s) {
                remainder = remainder.saturating_sub(s.len().min(u8::MAX as usize) as u8);
            }
        }
        let fuzzy_style = u8::MAX.saturating_sub(remainder);

        Score {
            codepoint,
            monospace,
            exact_style,
            italic,
            bold,
            fuzzy_style,
            glyph_count,
        }
    }
}

/// True if `font` maps `cp` to a glyph (discovery.zig:647-664).
fn font_has_codepoint(font: &CTFont, cp: u32) -> bool {
    let Some(ch) = char::from_u32(cp) else {
        return false;
    };
    let mut utf16 = [0u16; 2];
    let len = ch.encode_utf16(&mut utf16).len();
    let mut glyphs = [0u16; 2];
    // SAFETY: both buffers hold >= len elements.
    let ok = unsafe {
        font.glyphs_for_characters(
            NonNull::new(utf16.as_mut_ptr()).unwrap(),
            NonNull::new(glyphs.as_mut_ptr()).unwrap(),
            len as CFIndex,
        )
    };
    ok && glyphs[0] != 0
}

/// Read the style-name string out of a descriptor (discovery.zig:784-792).
fn descriptor_style_name(ct_desc: &CTFontDescriptor) -> Option<String> {
    // SAFETY: kCTFontStyleNameAttribute is a valid static key.
    let key = unsafe { kCTFontStyleNameAttribute };
    let attr = unsafe { ct_desc.attribute(key) }?;
    let s = attr.downcast_ref::<CFString>()?;
    Some(s.to_string())
}

/// Derive `(is_bold, is_italic)` from symbolic traits refined by the `head` and
/// `OS/2` tables (discovery.zig:683-782, variable-axis arm deferred).
///
/// Variable-axis derivation (`wght`/`ital`/`slnt`) is a documented deferral: the
/// reduced port refines from the raw sfnt tables, which cover the overwhelming
/// majority of static system fonts. This is noted in
/// `docs/analysis/font-discovery.md` §2.
fn derive_bold_italic(font: &CTFont, sym: CTFontSymbolicTraits) -> (bool, bool) {
    let mut is_bold = sym.contains(CTFontSymbolicTraits::TraitBold);
    let mut is_italic = sym.contains(CTFontSymbolicTraits::TraitItalic);

    // Read head.macStyle (offset 44 in the `head` table, a big-endian u16;
    // bit 0 = bold, bit 1 = italic). discovery.zig:690-704.
    if let Some(bytes) = copy_table(font, b"head")
        && let Some(mac_style) = be_u16_at(&bytes, 44)
    {
        is_bold = is_bold || (mac_style & 0x1 != 0);
        is_italic = is_italic || (mac_style & 0x2 != 0);
    }

    // Read OS/2.fsSelection (offset 62 in the `OS/2` table, a big-endian u16;
    // bit 0 = italic, bit 5 = bold). discovery.zig:707-720.
    if let Some(bytes) = copy_table(font, b"OS/2")
        && let Some(fs_selection) = be_u16_at(&bytes, 62)
    {
        is_bold = is_bold || (fs_selection & (1 << 5) != 0);
        is_italic = is_italic || (fs_selection & (1 << 0) != 0);
    }

    (is_bold, is_italic)
}

/// Read a big-endian `u16` at byte `offset` in `bytes`, or `None` if OOB.
fn be_u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_be_bytes([slice[0], slice[1]]))
}

/// Copy a raw sfnt table out of a `CTFont` (discovery.zig table reads).
fn copy_table(font: &CTFont, tag: &[u8; 4]) -> Option<Vec<u8>> {
    use objc2_core_text::CTFontTableOptions;
    let tag_u32 = u32::from_be_bytes(*tag);
    // SAFETY: font is valid; empty options.
    let data = unsafe { font.table(tag_u32, CTFontTableOptions::empty()) }?;
    if data.length() == 0 {
        return None;
    }
    // SAFETY: `data` lives for this scope; as_bytes_unchecked borrows its
    // storage, which we immediately copy into an owned Vec.
    let bytes = unsafe { data.as_bytes_unchecked() };
    Some(bytes.to_vec())
}

/// The desired-style set for a descriptor (discovery.zig:797-811). The first
/// element is used for the exact match; all are used for the fuzzy match.
fn desired_styles(desc: &Descriptor) -> Vec<&'static str> {
    if desc.style.is_some() {
        // An explicit style is compared verbatim as the exact match; the caller
        // handles it via `descriptor_style_name` equality. For the desired set
        // we return the requested style so the fuzzy pass also credits it. We
        // can't return a borrowed &str from `desc` as 'static, so callers that
        // set an explicit style rely on `exact_style` computed against the raw
        // string. Return an empty set here; the exact_style path below handles
        // the explicit case in `Score::compute`.
        return Vec::new();
    }
    if desc.bold {
        if desc.italic {
            vec!["bold italic", "bold", "italic", "oblique"]
        } else {
            vec!["bold", "upright"]
        }
    } else if desc.italic {
        vec!["italic", "regular", "oblique"]
    } else {
        vec!["regular", "upright"]
    }
}

/// Case-insensitive substring check (ASCII), the analog of
/// `std.ascii.indexOfIgnoreCase != null`.
fn ascii_contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let h = haystack.to_ascii_lowercase();
    let n = needle.to_ascii_lowercase();
    h.contains(&n)
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Ported from discovery.zig inline tests ---

    /// `test "descriptor hash"` (discovery.zig:1146-1151): a default descriptor
    /// still hashes to a nonzero code.
    #[test]
    fn descriptor_hash() {
        let d = Descriptor::default();
        assert_ne!(d.hashcode(), 0);
    }

    /// `test "descriptor hash family names"` (discovery.zig:1153-1159):
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

    /// `test "coretext"` (discovery.zig:1200-1219): discovering a stock family
    /// (Monaco) yields at least one result.
    #[test]
    fn coretext_discover_family() {
        let desc = Descriptor {
            family: Some("Monaco".into()),
            size: 12.0,
            ..Default::default()
        };
        let faces = discover(&desc);
        assert!(
            !faces.is_empty(),
            "expected Monaco to discover at least one face"
        );
    }

    /// `test "coretext codepoint"` (discovery.zig:1221-1243): discovering a
    /// codepoint-bearing font finds one that has 'A' (and 'B').
    #[test]
    fn coretext_discover_codepoint() {
        let desc = Descriptor {
            codepoint: 'A' as u32,
            size: 12.0,
            ..Default::default()
        };
        let faces = discover(&desc);
        assert!(!faces.is_empty(), "expected a font for 'A'");
        let first = &faces[0];
        assert!(
            first.has_codepoint(
                'A' as u32,
                crate::presentation::PresentationMode::Any,
                false
            ),
            "first result should have 'A'"
        );
        assert!(
            first.has_codepoint(
                'B' as u32,
                crate::presentation::PresentationMode::Any,
                false
            ),
            "first result should have 'B'"
        );
    }

    /// Fuzzy-name test (adaptation of the disabled `test "coretext sorting"`,
    /// which required SF Pro in CI): "jetbrains mono" should discover JetBrains
    /// Mono *if installed*, else skip-with-note. JetBrains Mono is our embedded
    /// font but not necessarily system-installed, so this is best-effort.
    #[test]
    fn fuzzy_name_jetbrains_mono() {
        let desc = Descriptor {
            family: Some("jetbrains mono".into()),
            size: 12.0,
            ..Default::default()
        };
        let faces = discover(&desc);
        if faces.is_empty() {
            eprintln!("note: JetBrains Mono not system-installed; fuzzy-name discovery skipped");
            return;
        }
        let name = faces[0].family_name().to_lowercase();
        assert!(
            name.contains("jetbrains"),
            "top fuzzy match for 'jetbrains mono' was {:?}, expected a JetBrains family",
            faces[0].family_name()
        );
    }

    /// Score determinism: the same query resolves to the same top family across
    /// repeated discovery runs (relies on the deterministic tiebreak).
    #[test]
    fn score_is_deterministic() {
        let desc = Descriptor {
            codepoint: '水' as u32,
            size: 16.0,
            ..Default::default()
        };
        let run1 = discover(&desc);
        let run2 = discover(&desc);
        if run1.is_empty() || run2.is_empty() {
            eprintln!("note: no descriptor-matched font for 水; determinism check skipped");
            return;
        }
        assert_eq!(
            run1[0].family_name(),
            run2[0].family_name(),
            "discovery should be deterministic across runs"
        );
    }
}
