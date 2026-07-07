//! Charset state STUB.
//!
//! TODO(chunk:terminal-state): the real charset tables and the `dec_special` /
//! `british` / `ascii` mapping logic live in `src/terminal/charsets.zig`, which
//! is being ported by the sibling `terminal-state` chunk. This module carries
//! only the minimal enums and the `CharsetState` container that `Screen` owns,
//! so that `Screen::reset` and the saved-cursor plumbing compile and round-trip.
//! Do NOT flesh out the tables here — that would collide with the sibling chunk.

/// A graphical charset slot. Port of `charsets.Slots`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slots {
    G0,
    G1,
    G2,
    G3,
}

/// The active-slot mapping (7-bit vs 8-bit). Port of `charsets.ActiveSlot`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveSlot {
    Gl,
    Gr,
}

/// A supported character set. Port of `charsets.Charset` (STUB — no tables).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Charset {
    Utf8,
    Ascii,
    British,
    DecSpecial,
}

/// The per-slot charset assignments. Port of `CharsetState.CharsetArray`.
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
