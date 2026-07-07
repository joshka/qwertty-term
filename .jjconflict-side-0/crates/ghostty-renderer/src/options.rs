//! Renderer construction options. Stub port of `src/renderer/Options.zig`
//! (commit `2da015cd6`).
//!
//! Upstream bundles everything a concrete renderer implementation needs at
//! construction: derived config, a font grid handle, [`crate::size::Size`],
//! an apprt mailbox/surface pointer, and a thread handle. None of font
//! loading, apprt, or threading exist in this codebase yet, so this is a
//! stub carrying only the one field this chunk can actually type. See
//! `docs/analysis/renderer-r0.md` for the full deferral list.

use crate::size::Size;

/// The options that are used to configure a renderer.
///
/// TODO(chunk:fonts): `font_grid` — the shared font grid handle, once a font
/// chunk exists.
/// TODO(chunk:apprt): `surface_mailbox` / `rt_surface` — apprt surface
/// integration, once an app-shell chunk exists.
/// TODO(chunk:threading): `thread` — the renderer thread handle, once a
/// threading model chunk exists.
/// TODO(chunk:config): `config` — the derived renderer configuration, once
/// config plumbing reaches the renderer.
#[derive(Debug, Clone, Copy, Default)]
pub struct RendererOptions {
    /// The size of everything: screen, cell, and padding.
    pub size: Size,
}
