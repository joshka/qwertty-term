//! Size types carried by termio messages. Local mirrors of the
//! `renderer/size.zig` structs (`Size`, `ScreenSize`, `CellSize`, `Padding`,
//! `GridSize`) — only the plain data, none of the layout math.
//!
//! Upstream `termio/message.zig` imports `renderer.Size` directly. The Rust
//! renderer crate (`ghostty-renderer/src/size.rs`) already ports the same
//! structs, but it is owned by a parallel work chunk, so this crate carries
//! its own mirror for now. **Chunk E reconciles the two** (either termio
//! depends on the renderer crate like upstream, or the size types move to a
//! shared home). Keep these field-for-field identical with the renderer's
//! port in the meantime.

/// The dimensions of a single cell, in pixels. Mirror of `renderer.CellSize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellSize {
    pub width: u32,
    pub height: u32,
}

/// The dimensions of the drawable terminal screen, in pixels. Mirror of
/// `renderer.ScreenSize`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScreenSize {
    pub width: u32,
    pub height: u32,
}

/// The padding around the terminal grid, in pixels. Mirror of
/// `renderer.Padding`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Padding {
    pub top: u32,
    pub bottom: u32,
    pub right: u32,
    pub left: u32,
}

/// The dimensions of the grid in rows/columns. Mirror of `renderer.GridSize`
/// (`Unit = terminal CellCountInt = u16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GridSize {
    pub columns: u16,
    pub rows: u16,
}

/// All size metrics for a rendered terminal. Mirror of `renderer.Size`.
/// Pixel values are already scaled to the screen's DPI.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Size {
    pub screen: ScreenSize,
    pub cell: CellSize,
    pub padding: Padding,
}
