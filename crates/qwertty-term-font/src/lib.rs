//! Font table parsing, cell-metrics derivation, and texture atlas allocation
//! for qwertty-term.
//!
//! This is a standalone port of Ghostty's `src/font/` opentype table layer,
//! `Metrics.zig`, and `Atlas.zig` (commit `2da015cd6`). See
//! `docs/analysis/font-foundations.md` for the full analysis: what each
//! OpenType table provides and who consumes it, the `Metrics` derivation
//! algorithm (rounding/centering rules, the modifier redistribution system),
//! the `Atlas` skyline bin-packer, and the decisions to adopt `ttf-parser`
//! for table parsing and to port (rather than adopt `etagere` for) the
//! atlas.
//!
//! # Modules
//!
//! - [`metrics`] — [`metrics::Metrics`] and [`metrics::FaceMetrics`]: the
//!   cell-dimension/decoration-placement derivation and its modifier system.
//! - [`atlas`] — [`atlas::Atlas`]: the CPU-side texture atlas and its
//!   skyline bin-packer.
//! - [`tables`] — [`tables::face_metrics`]: extracts a [`metrics::FaceMetrics`]
//!   from a `ttf_parser::Face`, replicating Ghostty's CoreText-backend
//!   fallback logic against portable table/glyph queries.
//! - [`embedded`] — embedded fallback fonts.
//! - [`backend`] — font backend enumeration (currently a CoreText-only
//!   stub).
//! - [`coretext`] (macOS only) — [`coretext::Face`]: CoreText face loading
//!   (load-by-name discovery + embedded fallback), CoreGraphics glyph
//!   rasterization to a [`coretext::Bitmap`], and `FaceMetrics` extraction
//!   reconciled with the table-derived layer. See
//!   `docs/analysis/font-coretext.md`.
//! - [`collection`] (macOS only) — [`collection::Collection`] and
//!   [`collection::FontIndex`]: faces grouped by [`collection::Style`], with a
//!   slotmap-style index (decision 8).
//! - [`resolver`] (macOS only) — [`resolver::CodepointResolver`]: the reduced
//!   codepoint → font chain (sprite dispatch + primary face).
//! - [`shaper`] (macOS only) — [`shaper::Shaper`]: rustybuzz run shaping with
//!   upstream cluster→cell mapping semantics.
//! - [`grid`] (macOS only) — [`grid::Grid`]: SharedGrid-reduced glyph render
//!   cache + grayscale atlas upload. See `docs/analysis/font-shaping.md`.
//!
//! # Example
//!
//! ```
//! use qwertty_term_font::{embedded, metrics::Metrics, tables};
//! use ttf_parser::Face;
//!
//! let face = Face::parse(embedded::JETBRAINS_MONO_VARIABLE, 0).unwrap();
//! let face_metrics = tables::face_metrics(&face, 16.0);
//! let metrics = Metrics::calc(face_metrics);
//! assert!(metrics.cell_width > 0);
//! assert!(metrics.cell_height > 0);
//! ```

pub mod atlas;
pub mod backend;
/// Font collection: faces grouped by style with per-style fallback lists.
/// Available with either face backend (CoreText on macOS, FreeType under the
/// `freetype` feature); system-font *discovery* fallback is macOS-only, so on
/// Linux the collection uses the embedded + synthetic style chain only.
#[cfg(any(target_os = "macos", feature = "freetype"))]
pub mod collection;
pub mod constraint;
#[cfg(target_os = "macos")]
pub mod coretext;
#[cfg(target_os = "macos")]
pub mod deferred;
#[cfg(target_os = "macos")]
pub mod discovery;
pub mod embedded;
/// FreeType glyph rasterization: [`freetype::Face`], the non-CoreText face
/// backend (Linux/software render path, ADR 003 P2). Behind the `freetype`
/// Cargo feature; FreeType is cross-platform, so this builds on macOS too (for
/// testing the FreeType face against the CoreText one).
#[cfg(feature = "freetype")]
pub mod freetype;
/// Glyph render cache + atlas upload. Available with either face backend.
#[cfg(any(target_os = "macos", feature = "freetype"))]
pub mod grid;
pub mod metrics;
pub mod nerd_font_constraints;
pub mod presentation;
/// Platform-neutral rasterized-glyph output ([`raster::Bitmap`] /
/// [`raster::PixelFormat`]), shared by every face backend.
pub mod raster;
/// Codepoint → font resolution (sprite + primary + style/fallback chain).
/// Available with either face backend; the discovery-fallback step (step 6) is
/// macOS-only, so on Linux resolution stops at the collection's own faces.
#[cfg(any(target_os = "macos", feature = "freetype"))]
pub mod resolver;
/// rustybuzz run shaping ([`shaper::Shaper`]) over any [`shaper::ShapeFace`] —
/// platform-agnostic (pure Rust); both the CoreText and FreeType faces
/// implement `ShapeFace`, so shaping works on Linux too.
pub mod shaper;
pub mod tables;

pub use atlas::Atlas;
pub use backend::Backend;
pub use metrics::{FaceMetrics, Metrics};
pub use presentation::{Presentation, PresentationMode};
pub use raster::{Bitmap, PixelFormat};

/// The platform font face and its error type: CoreText (`coretext::Face`) on
/// macOS by default, FreeType (`freetype::Face`) whenever the `freetype` feature
/// is enabled. The font stack (`collection`/`resolver`/`grid`) is written against
/// these aliases so it compiles and renders on both backends.
///
/// Enabling `freetype` selects FreeType **even on macOS** — since FreeType is
/// cross-platform, `--features freetype` on macOS builds and tests the exact
/// Linux font stack (the only place the bundled C FreeType links locally). So
/// "the CoreText backend" means `macos && !feature=freetype`.
#[cfg(all(target_os = "macos", not(feature = "freetype")))]
pub use coretext::{Error as FaceError, Face};
#[cfg(feature = "freetype")]
pub use freetype::{Error as FaceError, Face};

#[cfg(any(target_os = "macos", feature = "freetype"))]
pub use collection::{Collection, FontIndex, Style};
#[cfg(target_os = "macos")]
pub use deferred::DeferredFace;
#[cfg(target_os = "macos")]
pub use discovery::Descriptor;
#[cfg(any(target_os = "macos", feature = "freetype"))]
pub use grid::{AtlasKind, CachedGlyph, Grid};
#[cfg(any(target_os = "macos", feature = "freetype"))]
pub use resolver::CodepointResolver;
pub use shaper::{ShapeFace, ShapedCell, Shaper};

#[cfg(test)]
mod smoke_test {
    //! Smoke test: load the embedded JetBrains Mono **variable** font (the
    //! `wght` axis's default instance, `wght=400`, since neither ttf-parser
    //! nor this test set explicit variation coordinates) via ttf-parser,
    //! derive `Metrics`, and assert plausible + pinned cell
    //! width/height/baseline.
    //!
    //! Values below were computed by running this exact derivation
    //! (`tables::face_metrics` + `Metrics::calc`) against
    //! `embedded::JETBRAINS_MONO_VARIABLE` at 16px; they are pinned here as a
    //! regression guard. **Old vs new (default-font-parity re-vendor):** the
    //! previously-pinned values (against the static, non-variable
    //! `JetBrainsMonoNoNF-Regular.ttf`) were byte-for-byte identical to the
    //! ones below — `cell_width: 10, cell_height: 21, cell_baseline: 5,
    //! underline_position: 18, underline_thickness: 1,
    //! strikethrough_position: 11, strikethrough_thickness: 1` — because the
    //! static regular weight and the variable font's default `wght=400`
    //! instance share the same hhea/OS/2 metrics and glyph outlines at that
    //! instance. So this pin did **not** need to change; it is re-derived
    //! here against the new embedded bytes purely to keep the regression
    //! guard honest about which file it covers.
    //!
    //! They are **not** cross-checked against ghostty's own CoreText-derived
    //! output for this exact font/size combination — doing so would require
    //! running the Zig build with CoreText on this machine's font-rendering
    //! stack, which is out of scope for this chunk. Documented as
    //! unverified-vs-upstream per the task brief; the derivation *logic*
    //! (this crate's `tables::face_metrics` and `metrics::Metrics::calc`) is
    //! a line-for-line port of `coretext.zig::getMetrics` +
    //! `Metrics.zig::calc`, so any divergence from upstream would have to
    //! come from ttf-parser's measurement of this specific font differing
    //! from CoreText's, not from the derivation math.

    use super::*;
    use ttf_parser::Face;

    #[test]
    fn jetbrains_mono_smoke_test() {
        let face = Face::parse(embedded::JETBRAINS_MONO_VARIABLE, 0).expect("parse embedded font");

        let face_metrics = tables::face_metrics(&face, 16.0);
        let metrics = Metrics::calc(face_metrics);

        // Plausibility bounds: for a 16px monospace font, cell width should
        // be roughly half the point size to a bit more, cell height should
        // exceed the point size (to fit ascent+descent+linegap), and the
        // baseline should sit strictly inside the cell.
        assert!(
            (6..=16).contains(&metrics.cell_width),
            "implausible cell_width: {}",
            metrics.cell_width
        );
        assert!(
            (12..=32).contains(&metrics.cell_height),
            "implausible cell_height: {}",
            metrics.cell_height
        );
        assert!(
            metrics.cell_baseline > 0 && metrics.cell_baseline < metrics.cell_height,
            "baseline {} not within cell height {}",
            metrics.cell_baseline,
            metrics.cell_height
        );

        // Pinned regression values (see doc comment above for provenance),
        // computed by this exact derivation at 16px:
        //   FaceMetrics { cell_width: 9.6, ascent: 16.32, descent: -4.8,
        //     line_gap: 0.0, underline_position: -2.48,
        //     underline_thickness: 0.8, strikethrough_position: 5.12,
        //     strikethrough_thickness: 0.8, cap_height: 11.68,
        //     ex_height: 8.8, ascii_height: 16.8, ic_width: None }
        assert_eq!(metrics.cell_width, 10);
        assert_eq!(metrics.cell_height, 21);
        assert_eq!(metrics.cell_baseline, 5);
        assert_eq!(metrics.underline_position, 18);
        assert_eq!(metrics.underline_thickness, 1);
        assert_eq!(metrics.strikethrough_position, 11);
        assert_eq!(metrics.strikethrough_thickness, 1);
    }
}
