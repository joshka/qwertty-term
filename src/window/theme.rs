use eframe::egui::Color32;
use ghostty_rs::{AnsiColor, Color, Style};

pub(super) fn colors(style: Style) -> (Color32, Color32) {
    let mut fg = style
        .fg
        .and_then(to_egui_color)
        .unwrap_or(Color32::LIGHT_GRAY);
    let mut bg = style.bg.and_then(to_egui_color).unwrap_or(Color32::BLACK);
    if style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if style.faint {
        fg = fg.gamma_multiply(0.6);
    }
    (fg, bg)
}

fn to_egui_color(color: Color) -> Option<Color32> {
    match color {
        Color::Ansi(color) => Some(match color {
            AnsiColor::Black => Color32::BLACK,
            AnsiColor::Red => Color32::from_rgb(205, 49, 49),
            AnsiColor::Green => Color32::from_rgb(13, 188, 121),
            AnsiColor::Yellow => Color32::from_rgb(229, 229, 16),
            AnsiColor::Blue => Color32::from_rgb(36, 114, 200),
            AnsiColor::Magenta => Color32::from_rgb(188, 63, 188),
            AnsiColor::Cyan => Color32::from_rgb(17, 168, 205),
            AnsiColor::White => Color32::from_rgb(229, 229, 229),
            AnsiColor::BrightBlack => Color32::from_rgb(102, 102, 102),
            AnsiColor::BrightRed => Color32::from_rgb(241, 76, 76),
            AnsiColor::BrightGreen => Color32::from_rgb(35, 209, 139),
            AnsiColor::BrightYellow => Color32::from_rgb(245, 245, 67),
            AnsiColor::BrightBlue => Color32::from_rgb(59, 142, 234),
            AnsiColor::BrightMagenta => Color32::from_rgb(214, 112, 214),
            AnsiColor::BrightCyan => Color32::from_rgb(41, 184, 219),
            AnsiColor::BrightWhite => Color32::WHITE,
        }),
        Color::Indexed(value) => Some(indexed_color(value)),
        Color::Rgb { r, g, b } => Some(Color32::from_rgb(r, g, b)),
    }
}

fn indexed_color(value: u8) -> Color32 {
    match value {
        0..=15 => to_egui_color(Color::Ansi(match value {
            0 => AnsiColor::Black,
            1 => AnsiColor::Red,
            2 => AnsiColor::Green,
            3 => AnsiColor::Yellow,
            4 => AnsiColor::Blue,
            5 => AnsiColor::Magenta,
            6 => AnsiColor::Cyan,
            7 => AnsiColor::White,
            8 => AnsiColor::BrightBlack,
            9 => AnsiColor::BrightRed,
            10 => AnsiColor::BrightGreen,
            11 => AnsiColor::BrightYellow,
            12 => AnsiColor::BrightBlue,
            13 => AnsiColor::BrightMagenta,
            14 => AnsiColor::BrightCyan,
            _ => AnsiColor::BrightWhite,
        }))
        .expect("ANSI palette entries are supported"),
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
