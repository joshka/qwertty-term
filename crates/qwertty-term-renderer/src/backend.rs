//! GPU backend selection. Stub port of `src/renderer/backend.zig` (commit
//! `2da015cd6`).
//!
//! Upstream's `Backend::default(target, wasm_target)` platform-detection
//! function (Darwin -> Metal, wasm32 -> WebGL, else OpenGL) is deferred:
//! this crate doesn't yet pick or link against any GPU API, and encoding a
//! platform default before any backend exists would be dead code. See
//! `docs/analysis/renderer-r0.md`.

/// Possible renderer backend implementations, used for build options.
///
/// TODO(chunk:R2+ GPU backend): `default(target)`-style platform detection,
/// once a concrete GPU backend exists to default to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    OpenGl,
    Metal,
    WebGl,
}
