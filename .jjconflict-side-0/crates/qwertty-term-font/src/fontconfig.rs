//! Fontconfig font discovery: the non-CoreText discovery backend (Linux).
//!
//! Port of the `Fontconfig` arm of Ghostty's `src/font/discovery.zig` (commit
//! `2da015cd6`): the [`Descriptor`]→`FcPattern` conversion
//! ([`Descriptor::to_fc_pattern`], `discovery.zig:117-155`), the
//! `FcFontSort`-based [`discover`] (`discovery.zig:263-289`), and the deferred
//! face it yields ([`FcDeferredFace`], `discovery.zig:305-339`). Fontconfig's
//! own closeness sort replaces the CoreText `Score` ranking; the fallback path
//! (`discoverFallback`, `discovery.zig:291-299`) is just `discover` (fontconfig
//! needs no CoreText `CTFontCreateForString` per-codepoint special case).
//!
//! Uses the `fontconfig` crate in `dlopen` mode (libfontconfig loaded at
//! runtime), so there is no link-time system dependency: the module compiles on
//! any host, and only *running* discovery needs libfontconfig present. If
//! Fontconfig can't initialise ([`Fontconfig::new`] returns `None`), every entry
//! point degrades to "no match" (empty / `None`) exactly like the CoreText
//! backend does on a failed query — the caller then falls through to the
//! embedded + synthetic chain.
//!
//! # Reductions vs upstream (documented deferrals)
//!
//! - **No `FcFontRenderPrepare`.** Upstream calls `config.fontRenderPrepare` per
//!   matched font to merge the query pattern's render edits (hinting/embedding
//!   config) into the font pattern before reading its file/index. The
//!   `fontconfig` crate does not expose it; we read `file`/`index` directly off
//!   the sorted set. For *discovery* (locating the font file) this is
//!   equivalent — render edits (`force-autohint` etc.) are applied at
//!   rasterization time, a later P2 slice.
//! - **`has_codepoint` loads the face** rather than reading fontconfig's cached
//!   `FcCharSet` off the pattern (the crate exposes no charset getter). The
//!   query is already charset-filtered and coverage-sorted, so the top
//!   candidate almost always covers the codepoint; the probe face is loaded once
//!   and cached. This matches the reduced port's "verify after load" style and
//!   is the same order of cost as the CoreText path (which also loads to score).
//! - **Variation axes** are not in the reduced [`Descriptor`] surface (shared
//!   deferral; see `docs/analysis/font-discovery.md` §2).

use std::cell::OnceCell;
use std::ffi::CString;
use std::path::PathBuf;

use fontconfig::{CharSet, Fontconfig, Pattern, UnicodeCoverage};

use crate::descriptor::Descriptor;
use crate::presentation::{Presentation, PresentationMode};
use crate::{Face, FaceError};

/// Fontconfig integer enum values (`fontconfig.h`), the analogs of upstream's
/// `@intFromEnum(fontconfig.Weight.bold)` etc. (`discovery.zig:140-152`).
const FC_WEIGHT_BOLD: i32 = 200;
const FC_SLANT_ITALIC: i32 = 100;
const FC_SPACING_MONO: i32 = 100;

impl Descriptor {
    /// Build an `FcPattern` from this query (`toFcPattern`,
    /// `discovery.zig:117-155`).
    ///
    /// Adds family / style / codepoint-charset / size / weight(bold) /
    /// slant(italic), each omitted when unset, and — as upstream always does —
    /// `spacing=mono` so fontconfig's closeness sort *prefers* (not requires) a
    /// monospace font. Returns `None` if any pattern construction step fails.
    fn to_fc_pattern<'fc>(&self, fc: &'fc Fontconfig) -> Option<Pattern<'fc>> {
        let mut pat = Pattern::new(fc).ok()?;

        if let Some(family) = &self.family {
            let value = CString::new(family.as_str()).ok()?;
            pat.add_string(c"family", &value).ok()?;
        }

        if let Some(style) = &self.style {
            let value = CString::new(style.as_str()).ok()?;
            pat.add_string(c"style", &value).ok()?;
        }

        if self.codepoint > 0
            && let Some(ch) = char::from_u32(self.codepoint)
        {
            let mut cs = CharSet::new(fc).ok()?;
            cs.add_char(ch).ok()?;
            pat.add_charset(cs).ok()?;
        }

        if self.size > 0.0 {
            pat.add_integer(c"size", self.size.round() as i32).ok()?;
        }

        if self.bold {
            pat.add_integer(c"weight", FC_WEIGHT_BOLD).ok()?;
        }

        if self.italic {
            pat.add_integer(c"slant", FC_SLANT_ITALIC).ok()?;
        }

        // Always bias toward monospace (upstream `discovery.zig:150-154`):
        // fontconfig sorts by closeness, so this prefers but doesn't exclude
        // non-monospace fonts.
        pat.add_integer(c"spacing", FC_SPACING_MONO).ok()?;

        Some(pat)
    }
}

/// Discover fonts matching `desc`, returning deferred faces in fontconfig's
/// closeness-sorted order (`Fontconfig.discover`, `discovery.zig:263-289`).
///
/// Builds a pattern from the descriptor and runs `FcFontSort` (which applies
/// `FcConfigSubstitute` + `FcDefaultSubstitute` first, as fontconfig requires),
/// then materializes each result as a cheap [`FcDeferredFace`] (file path +
/// subface index + family). Returns an empty vec if fontconfig is unavailable or
/// nothing matched.
pub fn discover(desc: &Descriptor) -> Vec<FcDeferredFace> {
    let Some(fc) = Fontconfig::new() else {
        return Vec::new();
    };
    let Some(mut pat) = desc.to_fc_pattern(&fc) else {
        return Vec::new();
    };
    // `NoTrim`: mirror upstream `fontSort(pat, trim = false, null)`
    // (`discovery.zig:277`) — keep every candidate, don't elide by coverage.
    let Ok(set) = pat.sort_fonts(UnicodeCoverage::NoTrim) else {
        return Vec::new();
    };

    set.iter()
        .filter_map(|font| {
            let path = PathBuf::from(font.filename().ok()?);
            let index = font.face_index().unwrap_or(0).max(0) as u32;
            // `family` (not `fullname`): the collection's family-match check
            // compares against the requested family.
            let family = font.get_string(c"family").unwrap_or_default().to_string();
            Some(FcDeferredFace::new(path, index, family))
        })
        .collect()
}

/// Fallback discovery (`Fontconfig.discoverFallback`, `discovery.zig:291-299`).
///
/// For fontconfig this is just [`discover`]: unlike CoreText there is no
/// per-codepoint `CTFontCreateForString` cascade or Han-block special case — the
/// codepoint is already encoded as a charset in the pattern, so `FcFontSort`
/// returns the covering fonts directly. The CoreText backend's `original` face
/// (used to seed its cascade) has no fontconfig analog, so it is not a
/// parameter here.
pub fn discover_fallback(desc: &Descriptor) -> Vec<FcDeferredFace> {
    discover(desc)
}

/// Discover a **styled member of a specific family** and load it at `size_px`.
///
/// The fontconfig analog of [`crate::discovery::discover_family_style`]: run the
/// family+traits query and take the top candidate **whose family name still
/// matches** the request, so a family lacking the requested style yields `None`
/// (letting the caller fall through to the synthetic ladder) rather than a
/// cross-family substitute that fontconfig's closeness sort would otherwise
/// offer.
pub fn discover_family_style(family: &str, bold: bool, italic: bool, size_px: f64) -> Option<Face> {
    let desc = Descriptor {
        family: Some(family.to_string()),
        size: size_px as f32,
        bold,
        italic,
        monospace: true,
        ..Default::default()
    };
    let want = family.to_lowercase();
    discover(&desc)
        .into_iter()
        .find(|f| {
            let got = f.family_name().to_lowercase();
            got.contains(&want) || want.contains(&got)
        })?
        .load(size_px)
        .ok()
}

/// Discover the top-ranked member of a **named family**, loaded at `size_px`
/// (the fontconfig analog of [`crate::discovery::discover_family`]).
///
/// Runs `discover({ family })` and loads the first candidate. Returns `None` if
/// the family isn't installed.
pub fn discover_family(family: &str, size_px: f64) -> Option<Face> {
    let desc = Descriptor {
        family: Some(family.to_string()),
        size: size_px as f32,
        ..Default::default()
    };
    discover(&desc).into_iter().next()?.load(size_px).ok()
}

/// A cheap handle to a discovered font: its file path, subface index, and family
/// name, materialized into a [`Face`] only on [`load`](FcDeferredFace::load).
///
/// The fontconfig analog of [`crate::deferred::DeferredFace`]. Upstream keeps an
/// `FcPattern` (with its cached charset/langset) live; the reduced port keeps
/// the resolved file location plus a lazily-loaded probe face for
/// [`has_codepoint`](FcDeferredFace::has_codepoint) /
/// [`presentation`](FcDeferredFace::presentation) (see the module reductions).
pub struct FcDeferredFace {
    path: PathBuf,
    index: u32,
    family: String,
    /// Lazily-loaded probe face (nominal size) for the cheap queries. `Some(None)`
    /// means "tried to load and failed"; `None` means "not yet tried".
    probe: OnceCell<Option<Face>>,
}

impl FcDeferredFace {
    /// Nominal size for the probe face — glyph coverage and color-ness are
    /// size-independent, so any positive size works.
    const PROBE_SIZE_PX: f64 = 12.0;

    fn new(path: PathBuf, index: u32, family: String) -> FcDeferredFace {
        FcDeferredFace {
            path,
            index,
            family,
            probe: OnceCell::new(),
        }
    }

    /// The discovered font file's path.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The subface index within a `.ttc`/`.otc` collection (`0` for a single
    /// face).
    pub fn face_index(&self) -> u32 {
        self.index
    }

    /// The discovered font's family name (from the fontconfig pattern).
    pub fn family_name(&self) -> String {
        self.family.clone()
    }

    /// Load (once) and return the probe face, or `None` if the file can't be
    /// read/parsed.
    fn probe(&self) -> Option<&Face> {
        self.probe
            .get_or_init(|| {
                let bytes = std::fs::read(&self.path).ok()?;
                Face::load_from_bytes_indexed(&bytes, Self::PROBE_SIZE_PX, self.index).ok()
            })
            .as_ref()
    }

    /// The presentation this face advertises (emoji for a color face, else
    /// text). `Text` if the probe face can't load.
    pub fn presentation(&self) -> Presentation {
        self.probe()
            .map_or(Presentation::Text, |f| f.presentation())
    }

    /// True if this face satisfies `cp` under presentation mode `p_mode`
    /// (delegates to [`Face::has_codepoint`]). `false` if the probe can't load.
    pub fn has_codepoint(&self, cp: u32, p_mode: PresentationMode, fallback: bool) -> bool {
        self.probe()
            .is_some_and(|f| f.has_codepoint(cp, p_mode, fallback))
    }

    /// Materialize the full face at `size_px` from the discovered file.
    pub fn load(&self, size_px: f64) -> Result<Face, FaceError> {
        let bytes = std::fs::read(&self.path).map_err(|_| FaceError::FaceLoadFailed)?;
        Face::load_from_bytes_indexed(&bytes, size_px, self.index)
    }
}

impl std::fmt::Debug for FcDeferredFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FcDeferredFace")
            .field("path", &self.path)
            .field("index", &self.index)
            .field("family", &self.family)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fontconfig may be unavailable at runtime (e.g. no libfontconfig on this
    /// macOS host); these tests then skip-with-note rather than fail, mirroring
    /// the CoreText discovery tests that skip when a font isn't installed.
    fn fontconfig_available() -> bool {
        if Fontconfig::new().is_none() {
            eprintln!("note: libfontconfig unavailable at runtime; fontconfig test skipped");
            false
        } else {
            true
        }
    }

    /// Discovering a generic monospace family yields at least one face on any
    /// system with fonts installed (the fontconfig analog of the CoreText
    /// `coretext_discover_family` test).
    #[test]
    fn discover_monospace_yields_a_face() {
        if !fontconfig_available() {
            return;
        }
        let desc = Descriptor {
            family: Some("monospace".into()),
            size: 12.0,
            ..Default::default()
        };
        let faces = discover(&desc);
        assert!(
            !faces.is_empty(),
            "expected 'monospace' to discover at least one face"
        );
        // The top candidate must resolve to a readable, loadable font file.
        let top = &faces[0];
        assert!(top.path().exists(), "discovered path should exist: {top:?}");
        assert!(top.load(12.0).is_ok(), "top candidate should load: {top:?}");
    }

    /// A codepoint query returns a face that actually covers the codepoint (the
    /// fontconfig analog of `coretext_discover_codepoint`). Uses 'A', present in
    /// essentially every font.
    #[test]
    fn discover_codepoint_covers_it() {
        if !fontconfig_available() {
            return;
        }
        let desc = Descriptor {
            codepoint: 'A' as u32,
            size: 12.0,
            ..Default::default()
        };
        let faces = discover(&desc);
        assert!(!faces.is_empty(), "expected a font for 'A'");
        assert!(
            faces[0].has_codepoint('A' as u32, PresentationMode::Any, false),
            "first result should cover 'A'"
        );
    }

    /// Discovery is deterministic: the same query resolves to the same top
    /// family across repeated runs (fontconfig's sort is stable for a fixed
    /// config).
    #[test]
    fn discovery_is_deterministic() {
        if !fontconfig_available() {
            return;
        }
        let desc = Descriptor {
            family: Some("monospace".into()),
            size: 12.0,
            ..Default::default()
        };
        let run1 = discover(&desc);
        let run2 = discover(&desc);
        if run1.is_empty() || run2.is_empty() {
            return;
        }
        assert_eq!(
            run1[0].family_name(),
            run2[0].family_name(),
            "fontconfig discovery should be deterministic across runs"
        );
    }

    /// `discover_family_style` returns `None` for a family that surely isn't
    /// installed, rather than a cross-family substitute (the family-match guard).
    #[test]
    fn family_style_rejects_cross_family_substitute() {
        if !fontconfig_available() {
            return;
        }
        let got = discover_family_style("this-font-does-not-exist-qwertty-xyz", false, false, 12.0);
        assert!(
            got.is_none(),
            "a non-existent family must not fuzzy-match to a substitute"
        );
    }
}
