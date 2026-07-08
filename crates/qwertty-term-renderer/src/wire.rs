//! GPU wire structs — the CPU↔shader data contract.
//!
//! # THESE LAYOUTS ARE FROZEN
//!
//! Every struct in this module is a bit-for-bit port of the `extern struct`
//! definitions in Ghostty's `src/renderer/metal/shaders.zig` (commit
//! `2da015cd6`), which are themselves the mirror of the argument structs in
//! `src/renderer/shaders/shaders.metal`. Chunk R1 freezes them; chunk R3
//! ports the MSL that reads them and chunk R4 emits into them. **Do not
//! change a field, its type, its order, or any `repr`/`align` attribute
//! without an ADR** — the layout tests below are the executable form of that
//! freeze (including upstream's own `sizeof(CellText) == 32` assertion).
//!
//! Buffer index convention (plan decision 5, `docs/plans/m3-first-pixels.md`):
//! index 0 = vertex/instance data, index 1 = uniforms, 2+ = extras.
//!
//! These are plain data types with no GPU-API dependency, so they live in a
//! backend-agnostic module (upstream duplicates them per backend in
//! `metal/shaders.zig` / `opengl/shaders.zig`; the Rust port shares one
//! definition — a future OpenGL backend must keep matching these layouts,
//! which upstream's GLSL mirrors already do by construction).
//!
//! Zig→Rust layout notes:
//!
//! - Zig `math.Mat` is `[4]@Vector(4, f32)` (16-byte-aligned vectors); Rust
//!   models that as [`Mat`], a `#[repr(C, align(16))]` wrapper over
//!   `[[f32; 4]; 4]`.
//! - Zig `grid_padding: [4]f32 align(16)` needs an explicit aligned wrapper
//!   ([`AlignedF32x4`]) in Rust, since bare `[f32; 4]` is only 4-aligned and
//!   would land at offset 84 instead of 96 inside [`Uniforms`].
//! - Zig `packed struct(u8)` bitfields become `#[repr(transparent)]` newtypes
//!   over `u8` with LSB-first bit constants ([`PaddingExtend`],
//!   [`CellTextBools`], [`BgImageInfo`]).
//! - Zig `bool` in an `extern struct` is 1 byte; Rust `bool` matches.

/// 4x4 `f32` matrix, column-major as consumed by MSL `float4x4`.
///
/// Port of `math.Mat` = `[4]@Vector(4, f32)`: each column is a 16-byte
/// aligned `float4`, giving the whole matrix 16-byte alignment and 64-byte
/// size.
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Mat(pub [[f32; 4]; 4]);

impl Mat {
    /// Identity matrix.
    pub const IDENTITY: Self = Self([
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]);

    /// 2D orthographic projection matrix. Port of `math.ortho2d`.
    #[must_use]
    pub fn ortho2d(left: f32, right: f32, bottom: f32, top: f32) -> Self {
        let w = right - left;
        let h = top - bottom;
        Self([
            [2.0 / w, 0.0, 0.0, 0.0],
            [0.0, 2.0 / h, 0.0, 0.0],
            [0.0, 0.0, -1.0, 0.0],
            [-(right + left) / w, -(top + bottom) / h, 0.0, 1.0],
        ])
    }
}

/// `[4]f32` with 16-byte alignment, for `Uniforms::grid_padding`
/// (`align(16)` in the Zig source, matching MSL `float4`).
#[repr(C, align(16))]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct AlignedF32x4(pub [f32; 4]);

/// Bit mask defining which directions to extend cell colors into the
/// padding. Port of `Uniforms.PaddingExtend` (`packed struct(u8)`,
/// LSB first: left, right, up, down).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PaddingExtend(pub u8);

impl PaddingExtend {
    pub const LEFT: u8 = 1 << 0;
    pub const RIGHT: u8 = 1 << 1;
    pub const UP: u8 = 1 << 2;
    pub const DOWN: u8 = 1 << 3;
}

/// The four shader booleans at the tail of [`Uniforms`]. Port of the
/// anonymous `bools: extern struct { ... }` in `shaders.zig` (four 1-byte
/// bools, no bit packing).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UniformBools {
    /// Whether the cursor is 2 cells wide.
    pub cursor_wide: bool,
    /// Colors provided to the shader are already in the P3 color space, so
    /// they don't need to be converted from sRGB.
    pub use_display_p3: bool,
    /// The color attachments have an `*_srgb` pixel format, so the shaders
    /// must output linear RGB (blending happens in linear space and Metal
    /// re-encodes on store).
    pub use_linear_blending: bool,
    /// Enables the weight correction step that makes linear-blended text
    /// match the apparent thickness of gamma-incorrect blending.
    pub use_linear_correction: bool,
}

/// The uniforms passed to every shader. Port of `shaders.zig Uniforms`
/// (`extern struct` with explicit MSL-reference alignments).
///
/// Layout (asserted by tests): size 144, align 16; field offsets
/// 0/64/72/80/96/112/116/120/124/128/132.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Uniforms {
    /// Projection matrix turning world coordinates into normalized device
    /// coordinates; calculated from the screen size.
    pub projection_matrix: Mat,
    /// Size of the screen (render target) in pixels.
    pub screen_size: [f32; 2],
    /// Size of a single cell in pixels, unscaled.
    pub cell_size: [f32; 2],
    /// Size of the grid in columns and rows.
    pub grid_size: [u16; 2],
    /// Padding around the terminal grid in pixels. Order: top, right,
    /// bottom, left.
    pub grid_padding: AlignedF32x4,
    /// Which directions to extend cell colors into the padding.
    pub padding_extend: PaddingExtend,
    /// Minimum contrast ratio for text (WCAG 2.0 formula).
    pub min_contrast: f32,
    /// Cursor position (grid cells) and color.
    pub cursor_pos: [u16; 2],
    pub cursor_color: [u8; 4],
    /// Background color for the whole surface.
    pub bg_color: [u8; 4],
    /// Various booleans.
    pub bools: UniformBools,
}

/// Which atlas a glyph belongs to. Port of `CellText.Atlas` (`enum(u8)`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Atlas {
    #[default]
    Grayscale = 0,
    Color = 1,
}

/// Per-glyph shader booleans. Port of the `packed struct(u8)` bitfield in
/// `CellText` (LSB first: `no_min_contrast`, `is_cursor_glyph`).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CellTextBools(pub u8);

impl CellTextBools {
    pub const NO_MIN_CONTRAST: u8 = 1 << 0;
    pub const IS_CURSOR_GLYPH: u8 = 1 << 1;
}

/// A single instance for the cell text shader. Port of `shaders.zig
/// CellText` (`extern struct`, struct alignment 8 via `glyph_pos`'s
/// `align(8)`).
///
/// Upstream asserts `@sizeOf(CellText) == 32` in an inline test ("minimizing
/// the size of this struct is important"); the layout tests below carry that
/// assertion forward.
#[repr(C, align(8))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellText {
    /// Position of the glyph in the texture atlas (pixels).
    pub glyph_pos: [u32; 2],
    /// Size of the glyph in the texture atlas (pixels).
    pub glyph_size: [u32; 2],
    /// Left and top bearing of the glyph (pixels).
    pub bearings: [i16; 2],
    /// Grid cell position (columns, rows).
    pub grid_pos: [u16; 2],
    /// Foreground color (RGBA bytes).
    pub color: [u8; 4],
    /// Which atlas texture to sample.
    pub atlas: Atlas,
    /// Per-glyph booleans.
    pub bools: CellTextBools,
}

impl CellText {
    /// Mirrors the Zig field defaults (`glyph_pos`/`glyph_size` zeroed,
    /// `bools` empty); `grid_pos`/`color`/`atlas` have no upstream default
    /// and are required here.
    #[must_use]
    pub fn new(grid_pos: [u16; 2], color: [u8; 4], atlas: Atlas) -> Self {
        Self {
            glyph_pos: [0, 0],
            glyph_size: [0, 0],
            bearings: [0, 0],
            grid_pos,
            color,
            atlas,
            bools: CellTextBools::default(),
        }
    }
}

/// A single instance for the cell background shader: one RGBA color per
/// cell. Port of `pub const CellBg = [4]u8`.
pub type CellBg = [u8; 4];

/// A single instance for the (kitty) image shader. Port of `shaders.zig
/// Image` (`extern struct`, all-`f32`, natural alignment).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Image {
    /// Grid position (top-left cell) the image is placed at.
    pub grid_pos: [f32; 2],
    /// Offset within that cell, in pixels.
    pub cell_offset: [f32; 2],
    /// Source rectangle within the image texture (x, y, w, h).
    pub source_rect: [f32; 4],
    /// Final destination size in pixels.
    pub dest_size: [f32; 2],
}

/// Background image placement. Port of `BgImage.Info.Position` (`enum(u4)`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BgImagePosition {
    TopLeft = 0,
    TopCenter = 1,
    TopRight = 2,
    MiddleLeft = 3,
    #[default]
    MiddleCenter = 4,
    MiddleRight = 5,
    BottomLeft = 6,
    BottomCenter = 7,
    BottomRight = 8,
}

/// Background image fit mode. Port of `BgImage.Info.Fit` (`enum(u2)`).
#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum BgImageFit {
    #[default]
    Contain = 0,
    Cover = 1,
    Stretch = 2,
    None = 3,
}

/// Packed background-image info byte. Port of `BgImage.Info`
/// (`packed struct(u8)`, LSB first: position `u4`, fit `u2`, repeat `bool`,
/// 1 bit padding).
#[repr(transparent)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BgImageInfo(pub u8);

impl BgImageInfo {
    /// Pack position (bits 0-3), fit (bits 4-5) and repeat (bit 6) exactly
    /// like the Zig packed struct.
    #[must_use]
    pub fn new(position: BgImagePosition, fit: BgImageFit, repeat: bool) -> Self {
        Self((position as u8) | ((fit as u8) << 4) | (u8::from(repeat) << 6))
    }
}

/// A single instance for the background image shader. Port of `shaders.zig
/// BgImage` (`extern struct`: `opacity: f32 align(4)`, `info: Info
/// align(1)`; size rounds up to 8).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BgImage {
    pub opacity: f32,
    pub info: BgImageInfo,
}

#[cfg(test)]
mod tests {
    use std::mem::{align_of, offset_of, size_of};

    use super::*;

    /// Upstream's own inline test: `expectEqual(32, @sizeOf(CellText))`.
    #[test]
    fn cell_text_size_matches_upstream_assertion() {
        assert_eq!(size_of::<CellText>(), 32);
    }

    #[test]
    fn cell_text_layout_frozen() {
        assert_eq!(align_of::<CellText>(), 8);
        assert_eq!(offset_of!(CellText, glyph_pos), 0);
        assert_eq!(offset_of!(CellText, glyph_size), 8);
        assert_eq!(offset_of!(CellText, bearings), 16);
        assert_eq!(offset_of!(CellText, grid_pos), 20);
        assert_eq!(offset_of!(CellText, color), 24);
        assert_eq!(offset_of!(CellText, atlas), 28);
        assert_eq!(offset_of!(CellText, bools), 29);
    }

    #[test]
    fn uniforms_layout_frozen() {
        assert_eq!(size_of::<Uniforms>(), 144);
        assert_eq!(align_of::<Uniforms>(), 16);
        assert_eq!(offset_of!(Uniforms, projection_matrix), 0);
        assert_eq!(offset_of!(Uniforms, screen_size), 64);
        assert_eq!(offset_of!(Uniforms, cell_size), 72);
        assert_eq!(offset_of!(Uniforms, grid_size), 80);
        assert_eq!(offset_of!(Uniforms, grid_padding), 96);
        assert_eq!(offset_of!(Uniforms, padding_extend), 112);
        assert_eq!(offset_of!(Uniforms, min_contrast), 116);
        assert_eq!(offset_of!(Uniforms, cursor_pos), 120);
        assert_eq!(offset_of!(Uniforms, cursor_color), 124);
        assert_eq!(offset_of!(Uniforms, bg_color), 128);
        assert_eq!(offset_of!(Uniforms, bools), 132);
        assert_eq!(size_of::<UniformBools>(), 4);
        assert_eq!(align_of::<UniformBools>(), 1);
    }

    #[test]
    fn mat_layout_frozen() {
        assert_eq!(size_of::<Mat>(), 64);
        assert_eq!(align_of::<Mat>(), 16);
    }

    #[test]
    fn cell_bg_layout_frozen() {
        assert_eq!(size_of::<CellBg>(), 4);
        assert_eq!(align_of::<CellBg>(), 1);
    }

    #[test]
    fn image_layout_frozen() {
        assert_eq!(size_of::<Image>(), 40);
        assert_eq!(align_of::<Image>(), 4);
        assert_eq!(offset_of!(Image, grid_pos), 0);
        assert_eq!(offset_of!(Image, cell_offset), 8);
        assert_eq!(offset_of!(Image, source_rect), 16);
        assert_eq!(offset_of!(Image, dest_size), 32);
    }

    #[test]
    fn bg_image_layout_frozen() {
        assert_eq!(size_of::<BgImage>(), 8);
        assert_eq!(align_of::<BgImage>(), 4);
        assert_eq!(offset_of!(BgImage, opacity), 0);
        assert_eq!(offset_of!(BgImage, info), 4);
    }

    #[test]
    fn bg_image_info_packs_lsb_first() {
        // position bits 0-3, fit bits 4-5, repeat bit 6.
        let info = BgImageInfo::new(BgImagePosition::BottomRight, BgImageFit::None, true);
        assert_eq!(info.0, 8 | (3 << 4) | (1 << 6));
        let info = BgImageInfo::new(BgImagePosition::TopLeft, BgImageFit::Contain, false);
        assert_eq!(info.0, 0);
    }

    #[test]
    fn ortho2d_matches_upstream_formula() {
        let m = Mat::ortho2d(0.0, 800.0, 600.0, 0.0);
        assert_eq!(m.0[0][0], 2.0 / 800.0);
        assert_eq!(m.0[1][1], 2.0 / -600.0);
        assert_eq!(m.0[2][2], -1.0);
        assert_eq!(m.0[3], [-1.0, 1.0, 0.0, 1.0]);
        assert_eq!(Mat::IDENTITY.0[0][0], 1.0);
    }
}
