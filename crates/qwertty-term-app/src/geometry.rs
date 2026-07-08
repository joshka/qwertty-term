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

/// The `(col, row)` grid cell a device-pixel position `(x, y)` falls in,
/// given a `cols × rows` grid of `cell_width × cell_height` cells. `None` if
/// the position is negative or falls outside the grid (used to reject clicks
/// past the last partial cell / above-viewport coordinates for selection
/// mouse handling).
pub fn cell_at(
    x: f32,
    y: f32,
    cols: usize,
    rows: usize,
    cell_width: u32,
    cell_height: u32,
) -> Option<(usize, usize)> {
    if x < 0.0 || y < 0.0 {
        return None;
    }
    let col = (x / cell_width.max(1) as f32) as usize;
    let row = (y / cell_height.max(1) as f32) as usize;
    if col >= cols || row >= rows {
        return None;
    }
    Some((col, row))
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

    #[test]
    fn cell_at_maps_pixel_to_cell() {
        assert_eq!(cell_at(0.0, 0.0, 80, 24, 8, 16), Some((0, 0)));
        assert_eq!(cell_at(9.0, 17.0, 80, 24, 8, 16), Some((1, 1)));
        // Just inside the last cell's bounds.
        assert_eq!(
            cell_at(79.0 * 8.0 + 7.0, 23.0 * 16.0 + 15.0, 80, 24, 8, 16),
            Some((79, 23))
        );
    }

    #[test]
    fn cell_at_rejects_negative_positions() {
        assert_eq!(cell_at(-1.0, 5.0, 80, 24, 8, 16), None);
        assert_eq!(cell_at(5.0, -1.0, 80, 24, 8, 16), None);
    }

    #[test]
    fn cell_at_rejects_positions_past_the_grid() {
        // Exactly at the grid's right/bottom edge is one cell past the last
        // valid column/row.
        assert_eq!(cell_at(80.0 * 8.0, 0.0, 80, 24, 8, 16), None);
        assert_eq!(cell_at(0.0, 24.0 * 16.0, 80, 24, 8, 16), None);
    }

    #[test]
    fn cell_at_zero_cell_size_does_not_divide_by_zero() {
        assert_eq!(cell_at(5.0, 5.0, 10, 10, 0, 0), Some((5, 5)));
    }
}
