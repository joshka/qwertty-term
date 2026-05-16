#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnsiColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Color {
    Ansi(AnsiColor),
    Indexed(u8),
    Rgb { r: u8, g: u8, b: u8 },
}

pub(crate) fn ansi_color(idx: usize, bright: bool) -> AnsiColor {
    match (idx, bright) {
        (0, false) => AnsiColor::Black,
        (1, false) => AnsiColor::Red,
        (2, false) => AnsiColor::Green,
        (3, false) => AnsiColor::Yellow,
        (4, false) => AnsiColor::Blue,
        (5, false) => AnsiColor::Magenta,
        (6, false) => AnsiColor::Cyan,
        (7, false) => AnsiColor::White,
        (0, true) => AnsiColor::BrightBlack,
        (1, true) => AnsiColor::BrightRed,
        (2, true) => AnsiColor::BrightGreen,
        (3, true) => AnsiColor::BrightYellow,
        (4, true) => AnsiColor::BrightBlue,
        (5, true) => AnsiColor::BrightMagenta,
        (6, true) => AnsiColor::BrightCyan,
        (7, true) => AnsiColor::BrightWhite,
        _ => unreachable!("ANSI color index is in range"),
    }
}
