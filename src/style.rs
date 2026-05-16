use crate::{
    color::{Color, ansi_color},
    parser::parse_extended_color,
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Style {
    pub bold: bool,
    pub faint: bool,
    pub italic: bool,
    pub underline: bool,
    pub blink: bool,
    pub inverse: bool,
    pub strikethrough: bool,
    pub fg: Option<Color>,
    pub bg: Option<Color>,
}

impl Style {
    pub(crate) fn apply_sgr(&mut self, params: &[Option<usize>]) {
        if params.is_empty() {
            *self = Self::default();
            return;
        }

        let mut idx = 0;
        while idx < params.len() {
            let param = params[idx].unwrap_or(0);
            match param {
                0 => *self = Self::default(),
                1 => self.bold = true,
                2 => self.faint = true,
                3 => self.italic = true,
                4 => self.underline = true,
                5 => self.blink = true,
                7 => self.inverse = true,
                9 => self.strikethrough = true,
                22 => {
                    self.bold = false;
                    self.faint = false;
                }
                23 => self.italic = false,
                24 => self.underline = false,
                25 => self.blink = false,
                27 => self.inverse = false,
                29 => self.strikethrough = false,
                30..=37 => self.fg = Some(Color::Ansi(ansi_color(param - 30, false))),
                39 => self.fg = None,
                40..=47 => self.bg = Some(Color::Ansi(ansi_color(param - 40, false))),
                49 => self.bg = None,
                90..=97 => self.fg = Some(Color::Ansi(ansi_color(param - 90, true))),
                100..=107 => self.bg = Some(Color::Ansi(ansi_color(param - 100, true))),
                38 | 48 => {
                    if let Some((color, consumed)) = parse_extended_color(params, idx + 1) {
                        if param == 38 {
                            self.fg = Some(color);
                        } else {
                            self.bg = Some(color);
                        }
                        idx += consumed;
                    }
                }
                _ => {}
            }
            idx += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::AnsiColor;

    #[test]
    fn applies_sgr_attributes_and_resets() {
        let mut style = Style::default();

        style.apply_sgr(&[Some(1), Some(31), Some(44)]);
        assert!(style.bold);
        assert_eq!(style.fg, Some(Color::Ansi(AnsiColor::Red)));
        assert_eq!(style.bg, Some(Color::Ansi(AnsiColor::Blue)));

        style.apply_sgr(&[Some(22), Some(39), Some(49)]);
        assert!(!style.bold);
        assert_eq!(style.fg, None);
        assert_eq!(style.bg, None);
    }

    #[test]
    fn applies_extended_sgr_colors() {
        let mut style = Style::default();

        style.apply_sgr(&[
            Some(38),
            Some(5),
            Some(196),
            Some(48),
            Some(2),
            Some(1),
            Some(2),
            Some(3),
        ]);

        assert_eq!(style.fg, Some(Color::Indexed(196)));
        assert_eq!(style.bg, Some(Color::Rgb { r: 1, g: 2, b: 3 }));
    }
}
