//! Pixel-viewport → terminal-grid geometry.
//!
//! Pure integer math (unit-tested without AppKit) mapping a device-pixel
//! viewport and the font's cell size to a `(cols, rows)` grid, and back. The
//! reduced R5 cut uses no window padding, so this is a straight floor-division
//! with a one-cell floor (the engine panics on a zero dimension).

/// The grid size that fits in a `width_px × height_px` device-pixel viewport
/// with `cell_width × cell_height` cells. Always at least 1×1.
pub fn grid_size(
    width_px: usize,
    height_px: usize,
    cell_width: u32,
    cell_height: u32,
) -> (usize, usize) {
    let cols = (width_px / cell_width.max(1) as usize).max(1);
    let rows = (height_px / cell_height.max(1) as usize).max(1);
    (cols, rows)
}

/// The device-pixel size a grid of `cols × rows` occupies (the render target
/// size the engine produces for that grid — no padding in the reduced cut).
pub fn pixel_size(cols: usize, rows: usize, cell_width: u32, cell_height: u32) -> (usize, usize) {
    (cols * cell_width as usize, rows * cell_height as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_fit() {
        assert_eq!(grid_size(800, 600, 8, 16), (100, 37));
    }

    #[test]
    fn floors_partial_cells() {
        // 803 / 8 = 100 (remainder dropped).
        assert_eq!(grid_size(803, 607, 8, 16), (100, 37));
    }

    #[test]
    fn floors_to_at_least_one_cell() {
        assert_eq!(grid_size(3, 3, 8, 16), (1, 1));
        assert_eq!(grid_size(0, 0, 8, 16), (1, 1));
    }

    #[test]
    fn zero_cell_size_does_not_divide_by_zero() {
        assert_eq!(grid_size(100, 100, 0, 0), (100, 100));
    }

    #[test]
    fn pixel_size_round_trips_exact_grids() {
        let (cw, ch) = (8, 16);
        let (cols, rows) = grid_size(800, 640, cw, ch);
        assert_eq!(pixel_size(cols, rows, cw, ch), (800, 640));
    }
}
