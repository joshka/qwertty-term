use eframe::egui::Color32;
use ghostty_spike::{CellStyle, SnapshotColor};

/// Resolve a cell's style into `(foreground, background)` egui colors.
pub(super) fn colors(style: &CellStyle) -> (Color32, Color32) {
    let mut fg = to_egui_color(style.fg).unwrap_or(Color32::LIGHT_GRAY);
    let mut bg = to_egui_color(style.bg).unwrap_or(Color32::BLACK);
    if style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if style.faint {
        fg = fg.gamma_multiply(0.6);
    }
    (fg, bg)
}

fn to_egui_color(color: SnapshotColor) -> Option<Color32> {
    match color {
        SnapshotColor::Default => None,
        SnapshotColor::Palette(value) => Some(indexed_color(value)),
        SnapshotColor::Rgb { r, g, b } => Some(Color32::from_rgb(r, g, b)),
    }
}

fn indexed_color(value: u8) -> Color32 {
    match value {
        0..=15 => ansi_color(value),
        16..=231 => {
            let value = value - 16;
            let r = cube_component(value / 36);
            let g = cube_component((value / 6) % 6);
            let b = cube_component(value % 6);
            Color32::from_rgb(r, g, b)
        }
        232..=255 => {
            let gray = 8 + (value - 232) * 10;
            Color32::from_rgb(gray, gray, gray)
        }
    }
}

fn ansi_color(value: u8) -> Color32 {
    match value {
        0 => Color32::BLACK,
        1 => Color32::from_rgb(205, 49, 49),
        2 => Color32::from_rgb(13, 188, 121),
        3 => Color32::from_rgb(229, 229, 16),
        4 => Color32::from_rgb(36, 114, 200),
        5 => Color32::from_rgb(188, 63, 188),
        6 => Color32::from_rgb(17, 168, 205),
        7 => Color32::from_rgb(229, 229, 229),
        8 => Color32::from_rgb(102, 102, 102),
        9 => Color32::from_rgb(241, 76, 76),
        10 => Color32::from_rgb(35, 209, 139),
        11 => Color32::from_rgb(245, 245, 67),
        12 => Color32::from_rgb(59, 142, 234),
        13 => Color32::from_rgb(214, 112, 214),
        14 => Color32::from_rgb(41, 184, 219),
        _ => Color32::WHITE,
    }
}

fn cube_component(value: u8) -> u8 {
    if value == 0 { 0 } else { 55 + value * 40 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_colors_follow_xterm_palette() {
        assert_eq!(indexed_color(1), Color32::from_rgb(205, 49, 49));
        assert_eq!(indexed_color(16), Color32::from_rgb(0, 0, 0));
        assert_eq!(indexed_color(21), Color32::from_rgb(0, 0, 255));
        assert_eq!(indexed_color(196), Color32::from_rgb(255, 0, 0));
        assert_eq!(indexed_color(231), Color32::from_rgb(255, 255, 255));
        assert_eq!(indexed_color(232), Color32::from_rgb(8, 8, 8));
        assert_eq!(indexed_color(255), Color32::from_rgb(238, 238, 238));
    }
}
