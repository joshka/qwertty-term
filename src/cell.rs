use crate::style::Style;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Cell {
    pub(crate) ch: char,
    pub(crate) style: Style,
    pub(crate) wide_continuation: bool,
}

impl Cell {
    pub fn ch(&self) -> char {
        self.ch
    }

    pub fn style(&self) -> Style {
        self.style
    }

    pub fn is_wide_continuation(&self) -> bool {
        self.wide_continuation
    }

    pub(crate) fn blank(style: Style) -> Self {
        Self {
            ch: ' ',
            style,
            wide_continuation: false,
        }
    }

    pub(crate) fn printable(ch: char, style: Style) -> Self {
        Self {
            ch,
            style,
            wide_continuation: false,
        }
    }

    pub(crate) fn wide_continuation(style: Style) -> Self {
        Self {
            ch: ' ',
            style,
            wide_continuation: true,
        }
    }

    pub(crate) fn is_blank(&self) -> bool {
        self.ch == ' ' && !self.wide_continuation
    }
}
