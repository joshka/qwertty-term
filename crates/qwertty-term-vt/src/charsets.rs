//! Character set slots and tables. Port of `src/terminal/charsets.zig`
//! (115 lines, 1 inline test).
//!
//! Ghostty's `Terminal` keeps four charset "slots" (G0-G3) plus a notion of
//! which slot is active for GL (left, 7-bit codes) and GR (right, 8-bit
//! codes) — set via `ESC (`, `ESC )`, `ESC *`, `ESC +` and switched with
//! `SI`/`SO`/locking shifts. Three of the four selectable charsets
//! (`ascii`, `british`, `dec_special`) are simple 256-entry remap tables from
//! an input byte to a Unicode codepoint; `utf8` is not a table at all (it's
//! the "pass bytes through the UTF-8 decoder" charset) and callers must check
//! for it before calling [`table`].

/// The available charset slots for a terminal (G0-G3). Port of `charsets.zig`
/// `Slots`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slots {
    G0,
    G1,
    G2,
    G3,
}

/// The name of the active slots (GL = left/7-bit, GR = right/8-bit). Port of
/// `charsets.zig` `ActiveSlot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveSlot {
    Gl,
    Gr,
}

/// The list of supported character sets. Port of `charsets.zig` `Charset`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    Ascii,
    British,
    DecSpecial,
}

/// The per-slot charset assignments (G0-G3). Port of the `CharsetArray`
/// enum-array inside `Screen.zig`'s `CharsetState`; hoisted here so both
/// `Screen` and `Terminal` share the one definition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharsetArray {
    pub g0: Charset,
    pub g1: Charset,
    pub g2: Charset,
    pub g3: Charset,
}

impl Default for CharsetArray {
    fn default() -> Self {
        CharsetArray {
            g0: Charset::Utf8,
            g1: Charset::Utf8,
            g2: Charset::Utf8,
            g3: Charset::Utf8,
        }
    }
}

impl CharsetArray {
    pub fn get(&self, slot: Slots) -> Charset {
        match slot {
            Slots::G0 => self.g0,
            Slots::G1 => self.g1,
            Slots::G2 => self.g2,
            Slots::G3 => self.g3,
        }
    }

    pub fn set(&mut self, slot: Slots, charset: Charset) {
        match slot {
            Slots::G0 => self.g0 = charset,
            Slots::G1 => self.g1 = charset,
            Slots::G2 => self.g2 = charset,
            Slots::G3 => self.g3 = charset,
        }
    }
}

/// State required for all charset operations. Port of `Screen.CharsetState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharsetState {
    pub charsets: CharsetArray,
    /// GL is the slot for 7-bit printable chars (up to 127).
    pub gl: Slots,
    /// GR is the slot for 8-bit printable chars.
    pub gr: Slots,
    /// Single shift where a slot is used for exactly one char.
    pub single_shift: Option<Slots>,
}

impl Default for CharsetState {
    fn default() -> Self {
        CharsetState {
            charsets: CharsetArray::default(),
            gl: Slots::G0,
            gr: Slots::G2,
            single_shift: None,
        }
    }
}

/// Our table length is 256 so we can contain all ASCII/8-bit chars. Port of
/// `charsets.zig` `table_len`.
const TABLE_LEN: usize = 256;

/// The table for the given charset: a 256-entry map from input byte to
/// Unicode codepoint. Port of `charsets.zig` `table`.
///
/// # Panics
///
/// Panics if `set` is [`Charset::Utf8`], mirroring the Zig `unreachable`:
/// UTF-8 is not a remap table and callers must check for it first.
pub fn table(set: Charset) -> &'static [u16; TABLE_LEN] {
    match set {
        Charset::British => &BRITISH,
        Charset::DecSpecial => &DEC_SPECIAL,
        Charset::Utf8 => unreachable!("utf8 is not a table; callers must check for it first"),
        Charset::Ascii => &ASCII,
    }
}

/// Creates a table that maps a byte to itself as a starting point. Port of
/// `charsets.zig` `initTable`.
const fn init_table() -> [u16; TABLE_LEN] {
    let mut result = [0u16; TABLE_LEN];
    let mut i = 0;
    while i < TABLE_LEN {
        result[i] = i as u16;
        i += 1;
    }
    result
}

/// Just a basic c => c ascii table. Port of `charsets.zig` `ascii`.
const ASCII: [u16; TABLE_LEN] = init_table();

/// <https://vt100.net/docs/vt220-rm/chapter2.html>. Port of `charsets.zig`
/// `british`.
const BRITISH: [u16; TABLE_LEN] = {
    let mut tbl = init_table();
    tbl[0x23] = 0x00a3;
    tbl
};

/// <https://en.wikipedia.org/wiki/DEC_Special_Graphics>. Port of
/// `charsets.zig` `dec_special`.
const DEC_SPECIAL: [u16; TABLE_LEN] = {
    let mut tbl = init_table();
    tbl[0x60] = 0x25C6;
    tbl[0x61] = 0x2592;
    tbl[0x62] = 0x2409;
    tbl[0x63] = 0x240C;
    tbl[0x64] = 0x240D;
    tbl[0x65] = 0x240A;
    tbl[0x66] = 0x00B0;
    tbl[0x67] = 0x00B1;
    tbl[0x68] = 0x2424;
    tbl[0x69] = 0x240B;
    tbl[0x6a] = 0x2518;
    tbl[0x6b] = 0x2510;
    tbl[0x6c] = 0x250C;
    tbl[0x6d] = 0x2514;
    tbl[0x6e] = 0x253C;
    tbl[0x6f] = 0x23BA;
    tbl[0x70] = 0x23BB;
    tbl[0x71] = 0x2500;
    tbl[0x72] = 0x23BC;
    tbl[0x73] = 0x23BD;
    tbl[0x74] = 0x251C;
    tbl[0x75] = 0x2524;
    tbl[0x76] = 0x2534;
    tbl[0x77] = 0x252C;
    tbl[0x78] = 0x2502;
    tbl[0x79] = 0x2264;
    tbl[0x7a] = 0x2265;
    tbl[0x7b] = 0x03C0;
    tbl[0x7c] = 0x2260;
    tbl[0x7d] = 0x00A3;
    tbl[0x7e] = 0x00B7;
    tbl
};

#[cfg(test)]
mod tests {
    use super::*;

    // Port of charsets.zig's single unnamed `test { ... }` block: it checks
    // every non-utf8 charset's table is exactly 256 entries.
    #[test]
    fn all_non_utf8_charsets_have_256_entry_tables() {
        for &set in &[Charset::Ascii, Charset::British, Charset::DecSpecial] {
            assert_eq!(table(set).len(), 256);
        }
    }

    #[test]
    #[should_panic]
    fn utf8_table_panics() {
        let _ = table(Charset::Utf8);
    }

    #[test]
    fn dec_special_box_drawing_spot_check() {
        // Sanity beyond the ported test: a couple of well-known DEC special
        // graphics mappings used by box-drawing programs.
        assert_eq!(table(Charset::DecSpecial)[0x71], 0x2500); // horizontal line
        assert_eq!(table(Charset::DecSpecial)[0x78], 0x2502); // vertical line
    }
}
