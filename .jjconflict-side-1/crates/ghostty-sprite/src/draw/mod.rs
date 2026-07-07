//! Per-block glyph drawing modules, one file per Unicode block (mirroring
//! `src/font/sprite/draw/`). Each `draw<HEX>` / `draw<MIN>_<MAX>` function is
//! wired into the dispatch table in [`crate::dispatch`].
//!
//! Drawing functions are infallible: upstream's `try`/`catch {}` all reduce to
//! "skip on error", and the tiny-skia operations we use don't fail (an empty
//! path simply no-ops), so we drop the error channel entirely.

use crate::{Canvas, Metrics};

pub(crate) mod block;
pub(crate) mod box_drawing;
pub(crate) mod braille;
pub(crate) mod branch;
pub(crate) mod geometric_shapes;
pub(crate) mod legacy_computing;
pub(crate) mod legacy_computing_supplement;
pub(crate) mod powerline;
pub(crate) mod special;

/// The signature every glyph drawing function shares.
///
/// `width`/`height` are the drawing surface size (usually the cell size, but
/// wider for wide cells and shorter/taller for cursors); `metrics` carries the
/// rest of the cell geometry.
pub(crate) type DrawFn =
    fn(cp: u32, canvas: &mut Canvas, width: u32, height: u32, metrics: &Metrics);
