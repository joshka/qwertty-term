use eframe::egui::Color32;
use qwertty_term_spike::{CellStyle, Rgb, Snapshot, SnapshotColor, SnapshotWindow};

/// The dynamic color state a snapshot variant carries (256-color palette +
/// optional OSC 10/11 default fg/bg overrides). Both the full [`Snapshot`]
/// and the windowed [`SnapshotWindow`] carry the same three fields, so color
/// resolution doesn't need to care which one rendered a given frame.
pub(super) trait ColorSource {
    fn palette_color(&self, index: u8) -> Rgb;
    fn default_fg(&self) -> Option<Rgb>;
    fn default_bg(&self) -> Option<Rgb>;
}

impl ColorSource for Snapshot {
    fn palette_color(&self, index: u8) -> Rgb {
        self.palette[index as usize]
    }
    fn default_fg(&self) -> Option<Rgb> {
        self.default_fg
    }
    fn default_bg(&self) -> Option<Rgb> {
        self.default_bg
    }
}

impl ColorSource for SnapshotWindow {
    fn palette_color(&self, index: u8) -> Rgb {
        self.palette[index as usize]
    }
    fn default_fg(&self) -> Option<Rgb> {
        self.default_fg
    }
    fn default_bg(&self) -> Option<Rgb> {
        self.default_bg
    }
}

/// Resolve a cell's style into `(foreground, background)` egui colors,
/// looking indexed/default colors up through the snapshot's *dynamic* color
/// state (`palette`/`default_fg`/`default_bg`, mutated by OSC
/// 4/10/11/104/110/111/112) rather than a fixed xterm table, so palette and
/// default-color changes made by the running program are reflected live.
pub(super) fn colors(source: &impl ColorSource, style: &CellStyle) -> (Color32, Color32) {
    let mut fg = to_egui_color(source, style.fg).unwrap_or(default_fg(source));
    let mut bg = to_egui_color(source, style.bg).unwrap_or(default_bg(source));
    if style.inverse {
        std::mem::swap(&mut fg, &mut bg);
    }
    if style.faint {
        fg = fg.gamma_multiply(0.6);
    }
    (fg, bg)
}

fn default_fg(source: &impl ColorSource) -> Color32 {
    match source.default_fg() {
        Some(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        None => Color32::LIGHT_GRAY,
    }
}

/// The resolved default background: the OSC 11/111-controlled dynamic
/// background if the terminal has set one, else the frontend's black
/// fallback. Also used as the whole-viewport backdrop color (see
/// `renderer::paint_terminal`) so a program that sets a light background
/// doesn't paint on top of a black canvas.
pub(super) fn default_bg(source: &impl ColorSource) -> Color32 {
    match source.default_bg() {
        Some(rgb) => Color32::from_rgb(rgb.r, rgb.g, rgb.b),
        None => Color32::BLACK,
    }
}

fn to_egui_color(source: &impl ColorSource, color: SnapshotColor) -> Option<Color32> {
    match color {
        SnapshotColor::Default => None,
        SnapshotColor::Palette(value) => {
            let rgb = source.palette_color(value);
            Some(Color32::from_rgb(rgb.r, rgb.g, rgb.b))
        }
        SnapshotColor::Rgb { r, g, b } => Some(Color32::from_rgb(r, g, b)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qwertty_term_spike::Engine;

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
                qwertty_term_vt::color::DEFAULT[1].r,
                qwertty_term_vt::color::DEFAULT[1].g,
                qwertty_term_vt::color::DEFAULT[1].b,
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
