//! Vendored GLSL shader source for the OpenGL backend (P4 slice 1).
//!
//! The `.glsl` files in this directory are **verbatim** copies of upstream
//! Ghostty's `src/renderer/shaders/glsl/*` (commit `2da015cd6`) — the same
//! files the upstream OpenGL renderer (`opengl/shaders.zig`) embeds. They read
//! the *frozen* wire structs ([`crate::wire`]) exactly as the Metal MSL does;
//! only the GPU API interpreting them differs. Do not edit them.
//!
//! Upstream assembles each shader at comptime by resolving the single
//! `#include "common.glsl"` directive ([`shaders.zig`'s `processIncludes`]).
//! We do the same here with a tiny runtime substitution ([`resolve_includes`]):
//! the first-pixels shaders each begin with `#include "common.glsl"`, which
//! carries the `#version 430 core` directive and the shared `Globals` UBO /
//! color helpers. `full_screen.v.glsl` has its own `#version 330 core` and no
//! include.
//!
//! [`ShaderSet`] pairs the vendored vertex+fragment source for one pipeline,
//! selected by the backend-agnostic [`crate::shaders::PipelineDescription::name`]
//! (the OpenGL `build_pipeline` keys off `name` the way the Software backend
//! does, since the engine hands every backend the *Metal* `ShaderSource` — the
//! GL backend supplies its own GLSL instead).

/// `common.glsl` — the shared `#include` target (Globals UBO, unpack/color
/// helpers, `#version 430 core`). Upstream `shaders/glsl/common.glsl`.
const COMMON: &str = include_str!("common.glsl");

/// `full_screen.v.glsl` — the full-screen-triangle vertex shader shared by
/// `bg_color` and `cell_bg`. Upstream `shaders/glsl/full_screen.v.glsl`.
const FULL_SCREEN_V: &str = include_str!("full_screen.v.glsl");
const BG_COLOR_F: &str = include_str!("bg_color.f.glsl");
const CELL_BG_F: &str = include_str!("cell_bg.f.glsl");
const CELL_TEXT_V: &str = include_str!("cell_text.v.glsl");
const CELL_TEXT_F: &str = include_str!("cell_text.f.glsl");
const IMAGE_V: &str = include_str!("image.v.glsl");
const IMAGE_F: &str = include_str!("image.f.glsl");

/// Resolve the single `#include "common.glsl"` directive upstream's shaders
/// use, returning fully-expanded GLSL ready for `glShaderSource`. Simplified
/// port of `opengl/shaders.zig`'s `processIncludes`: our vendored shaders only
/// ever include `common.glsl` and always as their very first line, so a plain
/// substitution is exact (there is no nested/relative include to resolve).
fn resolve_includes(src: &str) -> String {
    src.replace("#include \"common.glsl\"", COMMON)
}

/// The vendored vertex+fragment GLSL for one pipeline, with includes resolved.
pub struct ShaderSet {
    pub vertex: String,
    pub fragment: String,
}

/// Look up the vendored GLSL pair for a pipeline by its backend-agnostic
/// `PipelineDescription::name`. Mirrors upstream `opengl/shaders.zig`'s
/// `pipeline_descs` file mapping (`bg_color`/`cell_bg`/`cell_text`/`image`;
/// `bg_image` is deferred exactly as in [`crate::shaders`]).
pub fn shader_set(name: &str) -> Option<ShaderSet> {
    let (vertex, fragment) = match name {
        "bg_color" => (FULL_SCREEN_V, BG_COLOR_F),
        "cell_bg" => (FULL_SCREEN_V, CELL_BG_F),
        "cell_text" => (CELL_TEXT_V, CELL_TEXT_F),
        "image" => (IMAGE_V, IMAGE_F),
        _ => return None,
    };
    Some(ShaderSet {
        vertex: resolve_includes(vertex),
        fragment: resolve_includes(fragment),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn common_include_is_resolved() {
        let set = shader_set("cell_bg").expect("cell_bg set");
        // The shared UBO block + version directive from common.glsl are spliced
        // in. (We don't assert the absence of the literal `#include` text:
        // common.glsl's own header *comment* documents the directive, so it
        // legitimately survives the substitution — harmlessly, inside a `//`
        // comment the GLSL compiler ignores.)
        assert!(set.fragment.contains("uniform Globals"));
        assert!(set.fragment.contains("#version 430 core"));
        // The original directive line itself is gone (only the commented
        // mention from common.glsl remains, which is never at column 0).
        assert!(!set.fragment.starts_with("#include"));
    }

    #[test]
    fn full_screen_vertex_keeps_own_version() {
        let set = shader_set("bg_color").expect("bg_color set");
        assert!(set.vertex.contains("#version 330 core"));
        assert!(!set.vertex.contains("#include"));
    }

    #[test]
    fn all_first_pixels_pipelines_present() {
        for name in ["bg_color", "cell_bg", "cell_text", "image"] {
            assert!(shader_set(name).is_some(), "missing GLSL for {name}");
        }
        assert!(shader_set("bg_image").is_none(), "bg_image is deferred");
    }
}
