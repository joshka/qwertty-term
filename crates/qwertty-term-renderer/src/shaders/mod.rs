//! First-pixels MSL shader source + pipeline descriptions (chunk R3).
//!
//! Port of Ghostty's `src/renderer/shaders/shaders.metal` (embedded here as
//! [`SOURCE`]) and the first-pixels subset of the pipeline table in
//! `src/renderer/metal/shaders.zig` (`pipeline_descs`), commit `2da015cd6`.
//! See `docs/analysis/renderer-r3.md` for the full survey (what was ported
//! vs skipped, the color-math explanation, the buffer-index/vertex-layout
//! contract).
//!
//! Scope: `bg_color`, `cell_bg`, `cell_text` — the three pipelines that
//! don't need image textures. `image` and `bg_image` are deferred (R6).
//!
//! The vertex attribute layouts below are *derived from*, and pinned to,
//! the frozen wire structs in [`crate::wire`] (`Uniforms`/`CellText`) — see
//! the `layout_pins_match_wire_offsets` test, which asserts each attribute's
//! `offset` equals the corresponding `memoffset`-style `offset_of!` on the
//! wire struct. If wire.rs's layout ever moves (it shouldn't — it's frozen),
//! this test catches the drift here rather than as a silent GPU garbling.

use crate::wire::{CellText, Image};

#[cfg(test)]
mod color_math;
// The smoke test compiles the embedded MSL through a live Metal device
// (`objc2-metal`), which only exists on macOS — where `objc2*` is even a
// dependency (see Cargo.toml's `cfg(target_os = "macos")` deps). Gate it so
// `cargo test` builds on Linux (ADR 003, P1).
#[cfg(all(test, target_os = "macos"))]
mod smoke;

/// The embedded MSL source for the first-pixels shader subset. Compiled at
/// runtime via `newLibraryWithSource:options:error:` (plan decision 6: no
/// build-time metallib yet).
pub const SOURCE: &str = include_str!("ghostty.metal");

/// Metal vertex format, mirroring the subset of `MTLVertexFormat` upstream's
/// `autoAttribute` (`metal/Pipeline.zig`) maps Zig field types to. Kept as a
/// local enum (rather than depending on `objc2-metal` from this
/// backend-agnostic module) so the pipeline table can be asserted against in
/// plain `cargo test` without a Metal device; the Metal backend (R2+) maps
/// these 1:1 onto `objc2_metal::MTLVertexFormat` constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VertexFormat {
    /// `MTLVertexFormatUChar4` — four `u8` lanes, untouched (not normalized).
    UChar4,
    /// `MTLVertexFormatUShort2` — two `u16` lanes.
    UShort2,
    /// `MTLVertexFormatShort2` — two `i16` lanes.
    Short2,
    /// `MTLVertexFormatUInt2` — two `u32` lanes.
    UInt2,
    /// `MTLVertexFormatUChar` — one `u8` lane.
    UChar,
    /// `MTLVertexFormatFloat2` — two `f32` lanes (kitty `Image` grid_pos/
    /// cell_offset/dest_size).
    Float2,
    /// `MTLVertexFormatFloat4` — four `f32` lanes (kitty `Image` source_rect).
    Float4,
}

/// One entry in a pipeline's vertex descriptor: which shader attribute index
/// this field binds to (`[[attribute(N)]]` in the MSL), its Metal format,
/// and its byte offset within the per-instance struct. Port of
/// `autoAttribute`'s per-field `(format, offset, bufferIndex)` triple;
/// `buffer_index` is always 0 per the frozen convention (vertex/instance
/// data), so it's not repeated per-attribute here — see
/// [`PipelineDescription::vertex_buffer_index`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VertexAttribute {
    /// Shader-side attribute index (`[[attribute(N)]]`).
    pub index: u32,
    pub format: VertexFormat,
    /// Byte offset within the instance struct (`@offsetOf` upstream).
    pub offset: usize,
}

/// Which buffer index a vertex descriptor's per-instance data is fetched
/// from. Frozen convention (plan decision 5, `wire.rs`): index 0 = vertex/
/// instance data.
pub const VERTEX_BUFFER_INDEX: u32 = 0;

/// Vertex step function for a pipeline's instance data: per-vertex (the two
/// full-screen-triangle pipelines have no instance data at all) or
/// per-instance (`cell_text`, one `CellText` struct per glyph instance).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepFunction {
    PerVertex,
    PerInstance,
}

/// Blending config for a pipeline's single color attachment. Port of
/// upstream `Pipeline.init`'s attachment setup: when enabled, upstream
/// always uses the same premultiplied-alpha "over" blend (`add` operation,
/// `one` source factor, `one_minus_source_alpha` destination factor) for
/// both RGB and alpha — there's no per-pipeline variation to model, so this
/// is a single bool rather than a full blend-descriptor struct.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Blending {
    pub enabled: bool,
}

impl Blending {
    /// `blending_enabled = false` (upstream `bg_color`'s entry): the
    /// background-color pass draws first and opaquely, nothing to blend
    /// with yet.
    pub const DISABLED: Self = Self { enabled: false };
    /// `blending_enabled = true`, upstream's fixed premultiplied "over"
    /// blend (rgb/alpha both: op=add, src=one, dst=one_minus_source_alpha).
    pub const PREMULTIPLIED_OVER: Self = Self { enabled: true };
}

/// A single pipeline's shader function names, optional per-instance vertex
/// layout, step function, and blending — the Rust mirror of one entry in
/// upstream's `pipeline_descs` array (`shaders.zig`). Field names match
/// `PipelineDescription` there for easy cross-reference.
#[derive(Debug, Clone, Copy)]
pub struct PipelineDescription {
    pub name: &'static str,
    pub vertex_fn: &'static str,
    pub fragment_fn: &'static str,
    /// `None` for the two full-screen-triangle pipelines (`bg_color`,
    /// `cell_bg`): they have no per-instance vertex buffer, only
    /// `[[vertex_id]]`-driven full-screen-triangle math.
    pub vertex_attributes: Option<&'static [VertexAttribute]>,
    /// Byte stride of one instance record, i.e. `@sizeOf(V)` upstream. `0`
    /// when `vertex_attributes` is `None`.
    pub stride: usize,
    pub step_fn: StepFunction,
    pub blending: Blending,
}

/// `CellText`'s vertex attribute layout (`autoAttribute(CellText, ...)`
/// upstream), one entry per struct field in declaration order (attribute
/// index == field position), each offset cross-referenced against the
/// frozen [`CellText`] layout in `wire.rs` by the test below.
///
/// | attribute | field       | format    | offset (wire.rs) |
/// | --------- | ----------- | --------- | ----------------- |
/// | 0         | glyph_pos   | UInt2     | 0                  |
/// | 1         | glyph_size  | UInt2     | 8                  |
/// | 2         | bearings    | Short2    | 16                 |
/// | 3         | grid_pos    | UShort2   | 20                 |
/// | 4         | color       | UChar4    | 24                 |
/// | 5         | atlas       | UChar     | 28                 |
/// | 6         | bools       | UChar     | 29                 |
pub const CELL_TEXT_ATTRIBUTES: &[VertexAttribute] = &[
    VertexAttribute {
        index: 0,
        format: VertexFormat::UInt2,
        offset: 0,
    },
    VertexAttribute {
        index: 1,
        format: VertexFormat::UInt2,
        offset: 8,
    },
    VertexAttribute {
        index: 2,
        format: VertexFormat::Short2,
        offset: 16,
    },
    VertexAttribute {
        index: 3,
        format: VertexFormat::UShort2,
        offset: 20,
    },
    VertexAttribute {
        index: 4,
        format: VertexFormat::UChar4,
        offset: 24,
    },
    VertexAttribute {
        index: 5,
        format: VertexFormat::UChar,
        offset: 28,
    },
    VertexAttribute {
        index: 6,
        format: VertexFormat::UChar,
        offset: 29,
    },
];

/// `Image`'s vertex attribute layout (`autoAttribute(Image, ...)` upstream),
/// one entry per struct field in declaration order (attribute index == field
/// position), each offset cross-referenced against the frozen [`Image`] layout
/// in `wire.rs` by the test below.
///
/// | attribute | field       | format | offset (wire.rs) |
/// | --------- | ----------- | ------ | ----------------- |
/// | 0         | grid_pos    | Float2 | 0                  |
/// | 1         | cell_offset | Float2 | 8                  |
/// | 2         | source_rect | Float4 | 16                 |
/// | 3         | dest_size   | Float2 | 32                 |
pub const IMAGE_ATTRIBUTES: &[VertexAttribute] = &[
    VertexAttribute {
        index: 0,
        format: VertexFormat::Float2,
        offset: 0,
    },
    VertexAttribute {
        index: 1,
        format: VertexFormat::Float2,
        offset: 8,
    },
    VertexAttribute {
        index: 2,
        format: VertexFormat::Float4,
        offset: 16,
    },
    VertexAttribute {
        index: 3,
        format: VertexFormat::Float2,
        offset: 32,
    },
];

/// The first-pixels subset of upstream's `pipeline_descs` table plus the R6
/// `image` pipeline: `bg_color`, `cell_bg`, `cell_text`, `image`. `bg_image` is
/// still skipped — see the module doc and `docs/analysis/renderer-r3.md`.
pub const PIPELINE_DESCRIPTIONS: &[PipelineDescription] = &[
    // Upstream: `.{ "bg_color", .{ .vertex_fn = "full_screen_vertex",
    // .fragment_fn = "bg_color_fragment", .blending_enabled = false } }`.
    PipelineDescription {
        name: "bg_color",
        vertex_fn: "full_screen_vertex",
        fragment_fn: "bg_color_fragment",
        vertex_attributes: None,
        stride: 0,
        step_fn: StepFunction::PerVertex,
        blending: Blending::DISABLED,
    },
    // Upstream: `.{ "cell_bg", .{ .vertex_fn = "full_screen_vertex",
    // .fragment_fn = "cell_bg_fragment", .blending_enabled = true } }`.
    PipelineDescription {
        name: "cell_bg",
        vertex_fn: "full_screen_vertex",
        fragment_fn: "cell_bg_fragment",
        vertex_attributes: None,
        stride: 0,
        step_fn: StepFunction::PerVertex,
        blending: Blending::PREMULTIPLIED_OVER,
    },
    // Upstream: `.{ "cell_text", .{ .vertex_attributes = CellText,
    // .vertex_fn = "cell_text_vertex", .fragment_fn = "cell_text_fragment",
    // .step_fn = .per_instance, .blending_enabled = true } }`.
    PipelineDescription {
        name: "cell_text",
        vertex_fn: "cell_text_vertex",
        fragment_fn: "cell_text_fragment",
        vertex_attributes: Some(CELL_TEXT_ATTRIBUTES),
        stride: size_of::<CellText>(),
        step_fn: StepFunction::PerInstance,
        blending: Blending::PREMULTIPLIED_OVER,
    },
    // Upstream: `.{ "image", .{ .vertex_attributes = Image,
    // .vertex_fn = "image_vertex", .fragment_fn = "image_fragment",
    // .step_fn = .per_instance, .blending_enabled = true } }`. One `Image`
    // instance per placement (drawn as a 4-vertex triangle-strip quad).
    PipelineDescription {
        name: "image",
        vertex_fn: "image_vertex",
        fragment_fn: "image_fragment",
        vertex_attributes: Some(IMAGE_ATTRIBUTES),
        stride: size_of::<Image>(),
        step_fn: StepFunction::PerInstance,
        blending: Blending::PREMULTIPLIED_OVER,
    },
];

/// The 5 shader function names that must resolve out of the compiled
/// library for the first-pixels subset: the shared full-screen vertex
/// function plus the two fragment functions per pipeline that has one
/// (`bg_color`, `cell_bg` share `full_screen_vertex`; `cell_text` has its own
/// vertex function). Named explicitly (rather than derived from
/// [`PIPELINE_DESCRIPTIONS`]) so the smoke test's expectations are legible
/// on their own.
pub const PORTED_FUNCTION_NAMES: &[&str] = &[
    "full_screen_vertex",
    "bg_color_fragment",
    "cell_bg_fragment",
    "cell_text_vertex",
    "cell_text_fragment",
    "image_vertex",
    "image_fragment",
];

#[cfg(test)]
mod tests {
    use std::mem::offset_of;

    use super::*;
    use crate::wire::Uniforms;

    #[test]
    fn source_embeds_all_ported_function_names() {
        for name in PORTED_FUNCTION_NAMES {
            assert!(
                SOURCE.contains(name),
                "embedded MSL source is missing expected function `{name}`"
            );
        }
    }

    #[test]
    fn source_skips_bg_image_pipeline() {
        // `bg_image` (a distinct backlog item) is still deferred; assert its
        // shader pair didn't sneak in via a careless copy-paste of the full
        // upstream file. The `image` pair *is* now ported (R6 slice 1).
        for name in ["bg_image_vertex", "bg_image_fragment"] {
            assert!(
                !SOURCE.contains(name),
                "embedded MSL source unexpectedly contains skipped function `{name}`"
            );
        }
    }

    #[test]
    fn pipeline_descriptions_cover_ported_subset() {
        let names: Vec<&str> = PIPELINE_DESCRIPTIONS.iter().map(|p| p.name).collect();
        assert_eq!(names, ["bg_color", "cell_bg", "cell_text", "image"]);
    }

    #[test]
    fn image_uses_per_instance_image_layout() {
        let desc = PIPELINE_DESCRIPTIONS
            .iter()
            .find(|p| p.name == "image")
            .unwrap();
        assert_eq!(desc.vertex_fn, "image_vertex");
        assert_eq!(desc.fragment_fn, "image_fragment");
        assert_eq!(desc.step_fn, StepFunction::PerInstance);
        assert_eq!(desc.blending, Blending::PREMULTIPLIED_OVER);
        assert_eq!(desc.vertex_attributes, Some(IMAGE_ATTRIBUTES));
        assert_eq!(desc.stride, size_of::<Image>());
    }

    /// Every [`IMAGE_ATTRIBUTES`] offset must equal the corresponding
    /// `offset_of!` on the frozen `wire::Image` — the same layout-pinning
    /// guard the cell_text path has, so image-quad garbling can't slip in via
    /// a wire.rs layout drift.
    #[test]
    fn image_layout_pins_match_wire_offsets() {
        let expected: [(u32, usize); 4] = [
            (0, offset_of!(Image, grid_pos)),
            (1, offset_of!(Image, cell_offset)),
            (2, offset_of!(Image, source_rect)),
            (3, offset_of!(Image, dest_size)),
        ];
        assert_eq!(IMAGE_ATTRIBUTES.len(), expected.len());
        for (attr, (expected_index, expected_offset)) in IMAGE_ATTRIBUTES.iter().zip(expected) {
            assert_eq!(attr.index, expected_index);
            assert_eq!(
                attr.offset, expected_offset,
                "attribute {expected_index} offset mismatch vs wire::Image"
            );
        }
        assert_eq!(size_of::<Image>(), 40);
    }

    #[test]
    fn bg_color_and_cell_bg_use_full_screen_triangle_no_vertex_buffer() {
        for name in ["bg_color", "cell_bg"] {
            let desc = PIPELINE_DESCRIPTIONS
                .iter()
                .find(|p| p.name == name)
                .unwrap();
            assert_eq!(desc.vertex_fn, "full_screen_vertex");
            assert!(desc.vertex_attributes.is_none());
            assert_eq!(desc.stride, 0);
            assert_eq!(desc.step_fn, StepFunction::PerVertex);
        }
        // Upstream: bg_color draws first onto a cleared/undefined target
        // (no blending needed), cell_bg blends per-cell colors over it.
        let bg_color = PIPELINE_DESCRIPTIONS[0];
        assert_eq!(bg_color.blending, Blending::DISABLED);
        let cell_bg = PIPELINE_DESCRIPTIONS[1];
        assert_eq!(cell_bg.blending, Blending::PREMULTIPLIED_OVER);
    }

    #[test]
    fn cell_text_uses_per_instance_cell_text_layout() {
        let desc = PIPELINE_DESCRIPTIONS
            .iter()
            .find(|p| p.name == "cell_text")
            .unwrap();
        assert_eq!(desc.vertex_fn, "cell_text_vertex");
        assert_eq!(desc.fragment_fn, "cell_text_fragment");
        assert_eq!(desc.step_fn, StepFunction::PerInstance);
        assert_eq!(desc.blending, Blending::PREMULTIPLIED_OVER);
        assert_eq!(desc.vertex_attributes, Some(CELL_TEXT_ATTRIBUTES));
        // Upstream `Pipeline.init` sets `layout.stride = @sizeOf(V)`; must
        // match the frozen wire struct's actual size (32, per wire.rs's own
        // upstream-mirroring assertion), not a hardcoded literal here.
        assert_eq!(desc.stride, size_of::<CellText>());
    }

    /// The layout-pinning test required by the R3 task: every
    /// [`CELL_TEXT_ATTRIBUTES`] offset must equal the corresponding
    /// `offset_of!` on `wire::CellText` — the frozen struct R1 already
    /// pins with its own tests. If `wire.rs` ever changes (it shouldn't
    /// without an ADR), this test fails here rather than silently
    /// misdescribing the vertex buffer to Metal.
    #[test]
    fn layout_pins_match_wire_offsets() {
        let expected: [(u32, usize); 7] = [
            (0, offset_of!(CellText, glyph_pos)),
            (1, offset_of!(CellText, glyph_size)),
            (2, offset_of!(CellText, bearings)),
            (3, offset_of!(CellText, grid_pos)),
            (4, offset_of!(CellText, color)),
            (5, offset_of!(CellText, atlas)),
            (6, offset_of!(CellText, bools)),
        ];
        assert_eq!(CELL_TEXT_ATTRIBUTES.len(), expected.len());
        for (attr, (expected_index, expected_offset)) in CELL_TEXT_ATTRIBUTES.iter().zip(expected) {
            assert_eq!(attr.index, expected_index);
            assert_eq!(
                attr.offset, expected_offset,
                "attribute {expected_index} offset mismatch vs wire::CellText"
            );
        }
        // Belt-and-suspenders on the whole-struct size, since the stride
        // above depends on it and `wire.rs` already carries upstream's own
        // `sizeOf(CellText) == 32` assertion.
        assert_eq!(size_of::<CellText>(), 32);
    }

    /// Cross-reference the `Uniforms` buffer-1 contract: this module embeds
    /// no offsets for `Uniforms` fields (the MSL indexes them by name, not
    /// by explicit `[[attribute(N)]]`), but the frozen size/align must
    /// still match what the MSL `struct Uniforms` lays out to, or every
    /// field read past the first would silently shift.
    #[test]
    fn uniforms_size_matches_msl_struct_layout() {
        // wire.rs's own tests already assert size 144 / align 16 in detail;
        // this test exists purely so a reader of shaders/mod.rs sees the
        // cross-reference without having to go find wire.rs.
        assert_eq!(size_of::<Uniforms>(), 144);
    }
}
