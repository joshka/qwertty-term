use eframe::egui::Color32;
use ghostty_spike::{CellStyle, Snapshot, SnapshotColor};

/// Resolve a cell's style into `(foreground, background)` egui colors,
/// looking indexed/default colors up through the snapshot's *dynamic* color
/// state (`snapshot.palette`/`default_fg`/`default_bg`, mutated by OSC
/// 4/10/11/104/110/111/112) rather than a fixed xterm table, so palette and
/// default-color changes made by the running program are reflected live.
pub(super) fn colors(snapshot: &Snapshot, style: &CellStyle) -> (Color32, Color32) {
    let mut fg = to_egui_color(snapshot, style.fg).unwrap_or(default_fg(snapshot));
    let mut bg = to_egui_color(snapshot, style.bg).unwrap_or(default_bg(snapshot));
    if style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if style.faint {
        fg = fg.gamma_multiply(0.6);
    }
    (fg, bg)
}

fn default_fg(snapshot: &Snapshot) -> Color32 {
    match snapshot.default_fg {
        Some(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        None => Color32::LIGHT_GRAY,
    }
}

/// The resolved default background: the OSC 11/111-controlled dynamic
/// background if the terminal has set one, else the frontend's black
/// fallback. Also used as the whole-viewport backdrop color (see
/// `renderer::paint_terminal`) so a program that sets a light background
/// doesn't paint on top of a black canvas.
pub(super) fn default_bg(snapshot: &Snapshot) -> Color32 {
    match snapshot.default_bg {
        Some(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        None => Color32::BLACK,
    }
}

fn to_egui_color(snapshot: &Snapshot, color: SnapshotColor) -> Option<Color32> {
    match color {
        SnapshotColor::Default => None,
        SnapshotColor::Palette(value) => {
            let rgb = snapshot.palette[value as usize];
            Some(Color32::from_rgb(rgb.r, rgb.g, rgb.b))
        }
        SnapshotColor::Rgb { r, g, b } => Some(Color32::from_rgb(r, g, b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ghostty_spike::Engine;

    fn snapshot_of(bytes: &[u8]) -> Snapshot {
        let mut engine = Engine::new(10, 2);
        engine.write(bytes);
        engine.snapshot()
    }

    #[test]
    fn indexed_colors_follow_default_palette() {
        let snapshot = snapshot_of(b"");
        assert_eq!(
            to_egui_color(&snapshot, SnapshotColor::Palette(1)),
            Some(Color32::from_rgb(
                ghostty_vt::color::DEFAULT[1].r,
                ghostty_vt::color::DEFAULT[1].g,
                ghostty_vt::color::DEFAULT[1].b,
            ))
        );
    }

    #[test]
    fn osc_4_palette_override_is_reflected_in_resolved_color() {
        // OSC 4: palette index 1 (normally red) set to a custom color.
        let snapshot = snapshot_of(b"\x1b]4;1;#112233\x1b\\");
        assert_eq!(
            to_egui_color(&snapshot, SnapshotColor::Palette(1)),
            Some(Color32::from_rgb(0x11, 0x22, 0x33))
        );
    }

    #[test]
    fn osc_10_11_default_fg_bg_override_resolved_colors() {
        let snapshot = snapshot_of(b"\x1b]10;#aabbcc\x1b\\\x1b]11;#001122\x1b\\");
        let style = CellStyle::default();
        assert_eq!(
            colors(&snapshot, &style),
            (
                Color32::from_rgb(0xaa, 0xbb, 0xcc),
                Color32::from_rgb(0x00, 0x11, 0x22)
            )
        );
    }
}
