//! Key event encoding — turns a [`KeyEvent`] into the PTY bytes a terminal
//! program expects. Port of `input/key_encode.zig` (2540 lines, 92 tests).
//!
//! ## Scope of this port
//!
//! Ghostty's `key_encode.zig` implements two encoders behind one dispatch:
//! the modern **kitty keyboard protocol** (`CSI … u`) and the **legacy**
//! encoder (PC-style function keys + xterm `modifyOtherKeys` + Paul Evans's
//! "fixterms" CSI-u extension for ctrl+letter etc). Per this chunk's scope,
//! only the kitty-protocol path is ported in full here (`Options`, `encode`'s
//! dispatch, `kitty`, `KittySequence`, `KittyMods`, and their ~38 relevant
//! inline tests). The legacy encoder (the bulk of the Zig file — PC-style
//! function key matching, ctrl-seq mapping, `CsiUMods`/fixterms, alt-esc-prefix,
//! `modifyOtherKeys` state 2) is a **later chunk**.
//!
//! ## The legacy seam
//!
//! [`encode`] dispatches on `opts.kitty_flags` exactly like the Zig source
//! (`if (opts.kitty_flags.int() != 0) kitty(...) else legacy(...)`), via the
//! [`Encoder`] enum below. [`Encoder::Legacy`] currently delegates to
//! [`legacy_stub`], a narrow placeholder that reproduces the *simple* fixed
//! encoding the `spike` window used before this port (arrow keys, enter, tab,
//! backspace, escape, home/end/delete/page up/down, ctrl+letter) — enough to
//! keep the window's non-kitty path behaviorally unchanged. When the legacy
//! chunk lands, replace [`legacy_stub`]'s body (and ideally rename it) with a
//! full port of Zig's `legacy()`; the [`Encoder`]/[`encode`] entry point is
//! designed so that swap is a one-function change with no call-site impact.

use crate::key::{Action, Key, KeyEvent};
use crate::key_mods::{Mods, OptionAsAlt};
use crate::kitty_keymap::{self, Entry as KittyEntry};

/// Options that affect key encoding behavior. Port of `key_encode.Options`.
/// The Zig `fromTerminal` constructor is skipped (freestanding crate — the
/// caller reads terminal-mode state itself and fills this in).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Options {
    /// Terminal DEC mode 1 (application cursor keys).
    pub cursor_key_application: bool,

    /// Terminal DEC mode 66 (application keypad keys).
    pub keypad_key_application: bool,

    /// DEC Backarrow Key Mode (DECBKM). If `false` (the default),
    /// `backspace` emits `0x7f`; if `true`, it emits `0x08`.
    pub backarrow_key_mode: bool,

    /// Terminal DEC mode 1035 (ignore keypad state with numlock).
    pub ignore_keypad_with_numlock: bool,

    /// Terminal DEC mode 1036 (send ESC prefix for alt-pressed keys).
    pub alt_esc_prefix: bool,

    /// xterm "modifyOtherKeys mode 2". See
    /// <https://invisible-island.net/xterm/modified-keys.html>.
    pub modify_other_keys_state_2: bool,

    /// Kitty keyboard protocol flags for the active screen. When this is
    /// all-zero (`.int() == 0`), [`encode`] dispatches to the legacy encoder;
    /// otherwise to the kitty encoder. This is a plain bitmask struct rather
    /// than `ghostty_vt::screen::kitty_key::Flags` directly, since this crate
    /// cannot depend on `ghostty-vt`; see [`KittyFlags`].
    pub kitty_flags: KittyFlags,

    /// Determines whether the "option" key on macOS is treated as "alt" or
    /// not. See Ghostty's `macos-option-as-alt` config docs.
    pub macos_option_as_alt: OptionAsAlt,
}

/// Kitty keyboard protocol flags, redefined locally since this crate cannot
/// depend on `ghostty-vt` (whose `screen::kitty_key::FlagStack`/`Flags` own
/// the same shape, keyed by the CSI `>`/`<`/`=` `u` sequences). Callers
/// construct this from `ghostty_vt`'s `Flags::int()`/`from_int()` via
/// [`KittyFlags::from_bits`]/[`KittyFlags::to_bits`], keeping the two crates
/// in sync without a dependency edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KittyFlags {
    pub disambiguate: bool,
    pub report_events: bool,
    pub report_alternates: bool,
    pub report_all: bool,
    pub report_associated: bool,
}

impl KittyFlags {
    /// All flags disabled (legacy encoding).
    pub const DISABLED: KittyFlags = KittyFlags {
        disambiguate: false,
        report_events: false,
        report_alternates: false,
        report_all: false,
        report_associated: false,
    };

    /// All flags enabled.
    pub const ALL: KittyFlags = KittyFlags {
        disambiguate: true,
        report_events: true,
        report_alternates: true,
        report_all: true,
        report_associated: true,
    };

    /// The u5 bit representation, matching `ghostty_vt`'s `Flags::int()`
    /// (LSB-first: disambiguate, report_events, report_alternates,
    /// report_all, report_associated).
    pub fn to_bits(self) -> u8 {
        (self.disambiguate as u8)
            | (self.report_events as u8) << 1
            | (self.report_alternates as u8) << 2
            | (self.report_all as u8) << 3
            | (self.report_associated as u8) << 4
    }

    /// Inverse of [`KittyFlags::to_bits`].
    pub fn from_bits(v: u8) -> KittyFlags {
        KittyFlags {
            disambiguate: v & 0b0_0001 != 0,
            report_events: v & 0b0_0010 != 0,
            report_alternates: v & 0b0_0100 != 0,
            report_all: v & 0b0_1000 != 0,
            report_associated: v & 0b1_0000 != 0,
        }
    }

    fn is_disabled(self) -> bool {
        self.to_bits() == 0
    }
}

/// Which encoder to dispatch to. Exists so the legacy path has a well-defined
/// seam to swap in a full port later without touching [`encode`]'s callers.
enum Encoder {
    Kitty,
    Legacy,
}

/// Encode the key event into PTY bytes (a `Vec<u8>`, since this crate has no
/// `std::io::Write`-alike dependency), in the proper format given the
/// options. Port of `key_encode.encode`.
///
/// Not all key events result in output; an empty `Vec` means nothing should
/// be written to the pty.
pub fn encode(event: &KeyEvent, opts: &Options) -> Vec<u8> {
    let encoder = if opts.kitty_flags.is_disabled() {
        Encoder::Legacy
    } else {
        Encoder::Kitty
    };
    let mut out = Vec::new();
    match encoder {
        Encoder::Kitty => kitty(&mut out, event, opts),
        Encoder::Legacy => legacy_stub(&mut out, event, opts),
    }
    out
}

/// Returns true if this is an ASCII control character, matching libc's
/// `iscntrl`. Port of `key_encode.isControl`.
fn is_control(cp: u32) -> bool {
    cp < 0x20 || cp == 0x7F
}

/// Returns true if this string is comprised of a single control character.
/// Returns false for multi-byte strings. Port of `key_encode.isControlUtf8`.
fn is_control_utf8(s: &str) -> bool {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => is_control(c as u32),
        _ => false,
    }
}

/// Kitty modifier bitfield (`CSI…u`'s modifier parameter, minus the `+1`
/// applied by [`KittyMods::seq_int`]). Port of `key_encode.KittyMods`
/// (`packed struct(u8)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct KittyMods {
    shift: bool,
    alt: bool,
    ctrl: bool,
    super_: bool,
    hyper: bool,
    meta: bool,
    caps_lock: bool,
    num_lock: bool,
}

impl KittyMods {
    /// Convert an input mods value into the Kitty mods value. Port of
    /// `KittyMods.fromInput` (the Zig version also takes `action`/`key`
    /// parameters but ignores them (`_ = action; _ = k;`), so they are
    /// omitted here).
    fn from_input(mods: Mods) -> KittyMods {
        KittyMods {
            shift: mods.shift,
            alt: mods.alt,
            ctrl: mods.ctrl,
            super_: mods.super_,
            caps_lock: mods.caps_lock,
            num_lock: mods.num_lock,
            hyper: false,
            meta: false,
        }
    }

    /// Returns true if the modifiers prevent printable text. Port of
    /// `KittyMods.preventsText`. `alt_prevents_text` is `true` on Linux, and
    /// on macOS only true if `macos-option-as-alt` treats this event's Alt as
    /// a real Alt (rather than the macOS Option key, which does not prevent
    /// associated text).
    fn prevents_text(self, alt_prevents_text: bool) -> bool {
        (self.alt && alt_prevents_text) || self.ctrl || self.super_ || self.hyper || self.meta
    }

    /// Raw int value of this bitfield. Port of `KittyMods.int`.
    fn int(self) -> u8 {
        (self.shift as u8)
            | (self.alt as u8) << 1
            | (self.ctrl as u8) << 2
            | (self.super_ as u8) << 3
            | (self.hyper as u8) << 4
            | (self.meta as u8) << 5
            | (self.caps_lock as u8) << 6
            | (self.num_lock as u8) << 7
    }

    /// The integer value sent as part of the Kitty sequence (adds 1 to the
    /// bitmask, per the kitty keyboard protocol spec). Port of
    /// `KittyMods.seqInt`.
    fn seq_int(self) -> u16 {
        self.int() as u16 + 1
    }
}

/// The event-type field of a kitty sequence (`event-type` in the CSI…u
/// syntax comment on [`KittySequence`]). Port of `KittySequence.Event`.
/// Kitty omits `:1` for press; this port includes it, matching upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum KittyEventType {
    #[default]
    None,
    Press,
    Repeat,
    Release,
}

impl KittyEventType {
    fn as_u8(self) -> u8 {
        match self {
            KittyEventType::None => 0,
            KittyEventType::Press => 1,
            KittyEventType::Repeat => 2,
            KittyEventType::Release => 3,
        }
    }
}

/// Represents a kitty key sequence and encodes it. Port of
/// `key_encode.KittySequence`.
///
/// The sequence from the kitty keyboard protocol spec:
/// `CSI unicode-key-code:alternate-key-codes ; modifiers:event-type ; text-as-codepoints u`
#[derive(Debug, Clone, Default)]
struct KittySequence {
    key: u32,
    final_byte: u8,
    mods: KittyMods,
    event: KittyEventType,
    /// `[shifted, base-layout]` alternates, matching the Zig
    /// `alternates: [2]?u21` field order.
    alternates: [Option<u32>; 2],
    text: String,
}

impl KittySequence {
    fn encode(&self, out: &mut Vec<u8>) {
        if self.final_byte == b'u' || self.final_byte == b'~' {
            self.encode_full(out);
        } else {
            self.encode_special(out);
        }
    }

    fn encode_full(&self, out: &mut Vec<u8>) {
        // Key section.
        out.extend_from_slice(format!("\x1B[{}", self.key).as_bytes());

        // Alternates.
        if let Some(shifted) = self.alternates[0] {
            out.extend_from_slice(format!(":{shifted}").as_bytes());
        }
        if let Some(base) = self.alternates[1] {
            if self.alternates[0].is_none() {
                out.extend_from_slice(format!("::{base}").as_bytes());
            } else {
                out.extend_from_slice(format!(":{base}").as_bytes());
            }
        }

        // Mods and events section.
        let mods = self.mods.seq_int();
        let mut emit_prior = false;
        if self.event != KittyEventType::None && self.event != KittyEventType::Press {
            out.extend_from_slice(format!(";{}:{}", mods, self.event.as_u8()).as_bytes());
            emit_prior = true;
        } else if mods > 1 {
            out.extend_from_slice(format!(";{mods}").as_bytes());
            emit_prior = true;
        }

        // Text section: codepoints of `text`, skipping non-printable ASCII
        // control characters, separated by `:` after an initial `;` (or
        // `;;` if the mods/event section above wasn't emitted).
        if !self.text.is_empty() {
            let mut count = 0usize;
            for cp in self.text.chars() {
                if is_control(cp as u32) {
                    continue;
                }
                if count == 0 {
                    if !emit_prior {
                        out.push(b';');
                    }
                    out.push(b';');
                } else {
                    out.push(b':');
                }
                out.extend_from_slice(format!("{}", cp as u32).as_bytes());
                count += 1;
            }
        }

        out.push(self.final_byte);
    }

    fn encode_special(&self, out: &mut Vec<u8>) {
        let mods = self.mods.seq_int();
        if self.event != KittyEventType::None {
            out.extend_from_slice(
                format!(
                    "\x1B[1;{}:{}{}",
                    mods,
                    self.event.as_u8(),
                    self.final_byte as char
                )
                .as_bytes(),
            );
            return;
        }

        if mods > 1 {
            out.extend_from_slice(format!("\x1B[1;{}{}", mods, self.final_byte as char).as_bytes());
            return;
        }

        out.extend_from_slice(format!("\x1B[{}", self.final_byte as char).as_bytes());
    }
}

/// Perform kitty keyboard protocol encoding of the key event. Port of
/// `key_encode.kitty`.
fn kitty(out: &mut Vec<u8>, event: &KeyEvent, opts: &Options) {
    // This should never happen (callers dispatch on `is_disabled` first) but
    // mirror the Zig source's defensive fallback anyway.
    if opts.kitty_flags.is_disabled() {
        legacy_stub(out, event, opts);
        return;
    }

    // We only process "press" events unless report_events is active.
    if event.action == Action::Release {
        if !opts.kitty_flags.report_events {
            return;
        }
        // Enter, backspace, and tab do not report release events unless
        // "report all" is set.
        if !opts.kitty_flags.report_all
            && matches!(event.key, Key::Enter | Key::Backspace | Key::Tab)
        {
            return;
        }
    }

    let all_mods = event.mods;
    let effective_mods = event.effective_mods();
    let binding_mods = effective_mods.binding();

    // Find the entry for this key in the kitty table.
    let entry: Option<KittyEntry> = kitty_keymap::ENTRIES
        .iter()
        .find(|e| e.key == event.key)
        .copied()
        .or({
            // Otherwise, use the unicode codepoint from UTF-8. Always use
            // the unshifted value.
            if event.unshifted_codepoint > 0 {
                Some(KittyEntry {
                    key: event.key,
                    code: event.unshifted_codepoint,
                    final_byte: b'u',
                    modifier: false,
                })
            } else {
                None
            }
        });

    // Preprocessing block (`preprocessing:` in Zig). Returns early (no
    // output, or output-then-return) in several cases; falls through to the
    // main encoding path otherwise.
    enum Preprocess {
        /// Stop; nothing more to do (possibly having already written output).
        Done,
        /// Fall through to the main encoding path.
        Continue,
    }
    let preprocess = 'preprocessing: {
        // When composing, the only keys sent are plain modifiers.
        if event.composing {
            if let Some(e) = &entry
                && e.modifier
            {
                break 'preprocessing Preprocess::Continue;
            }
            break 'preprocessing Preprocess::Done;
        }

        // IME confirmation still sends an enter key, so if we have enter and
        // UTF-8 text we just send it directly since we assume that's what's
        // happening. See `legacy`'s similar logic (once ported) for more
        // details on how to verify this.
        if !event.utf8.is_empty() && matches!(event.key, Key::Enter | Key::Backspace) {
            let is_backspace = event.key == Key::Backspace;
            if !is_control_utf8(&event.utf8) {
                if is_backspace {
                    break 'preprocessing Preprocess::Done;
                }
                out.extend_from_slice(event.utf8.as_bytes());
                break 'preprocessing Preprocess::Done;
            }
        }

        // If we're reporting all then we always send CSI sequences.
        if !opts.kitty_flags.report_all {
            // The Enter, Tab, and Backspace keys still generate the same
            // bytes as legacy mode (so users can type/execute commands like
            // `reset` in the shell after a crashed program leaves this mode
            // set), UNLESS "report all" is set.
            if binding_mods.empty() {
                match event.key {
                    Key::Enter => {
                        out.push(b'\r');
                        break 'preprocessing Preprocess::Done;
                    }
                    Key::Tab => {
                        out.push(b'\t');
                        break 'preprocessing Preprocess::Done;
                    }
                    Key::Backspace => {
                        out.push(0x7F);
                        break 'preprocessing Preprocess::Done;
                    }
                    _ => {}
                }
            }

            // Send plain-text non-modified text directly to the terminal.
            // Release events are excluded since those are specially encoded.
            if !event.utf8.is_empty() && binding_mods.empty() && event.action != Action::Release {
                // Only do this for printable characters (approximated here,
                // as in Zig, via "no control characters").
                let all_printable = event.utf8.chars().all(|cp| !is_control(cp as u32));
                if all_printable {
                    out.extend_from_slice(event.utf8.as_bytes());
                    break 'preprocessing Preprocess::Done;
                }
            }
        }

        Preprocess::Continue
    };

    if matches!(preprocess, Preprocess::Done) {
        return;
    }

    let Some(entry) = entry else {
        // No entry found. If we have UTF-8 text this is a pure text event
        // (e.g. composed/IME text), so send it as-is so programs can still
        // receive it.
        if !event.utf8.is_empty() {
            out.extend_from_slice(event.utf8.as_bytes());
        }
        return;
    };

    // If this is just a modifier we require "report all" to send the
    // sequence.
    if entry.modifier && !opts.kitty_flags.report_all {
        return;
    }

    let mut seq = KittySequence {
        key: entry.code,
        final_byte: entry.final_byte,
        mods: KittyMods::from_input(all_mods),
        ..KittySequence::default()
    };

    if opts.kitty_flags.report_events {
        seq.event = match event.action {
            Action::Press => KittyEventType::Press,
            Action::Release => KittyEventType::Release,
            Action::Repeat => KittyEventType::Repeat,
        };
    }

    if opts.kitty_flags.report_alternates && !is_control(seq.key) {
        let mut chars = event.utf8.chars();
        if let Some(cp1) = chars.next() {
            let cp1 = cp1 as u32;
            // Set the first alternate (shifted version) if it differs from
            // the pressed key and shift is active.
            if cp1 != seq.key && seq.mods.shift {
                seq.alternates[0] = Some(cp1);
            }

            // We want to know if there are additional codepoints because the
            // logic below depends on utf8 being a single codepoint.
            let has_cp2 = chars.next().is_some();

            // Set the base layout key. Only reported if it differs from the
            // pressed key.
            if let Some(base) = event.key.codepoint()
                && base != seq.key
                && (cp1 != base && !has_cp2)
            {
                seq.alternates[1] = Some(base);
            }
        } else {
            // No UTF-8 so we can't report a shifted key, but we can still
            // report a base layout key.
            if let Some(base) = event.key.codepoint()
                && base != seq.key
            {
                seq.alternates[1] = Some(base);
            }
        }
    }

    if opts.kitty_flags.report_associated && seq.event != KittyEventType::Release {
        'associated: {
            // Determine if Alt should be treated as an actual modifier (which
            // prevents associated text) or as the macOS Option key (which
            // does not).
            let alt_prevents_text = if cfg!(target_os = "macos") {
                match opts.macos_option_as_alt {
                    OptionAsAlt::Left => all_mods.sides.alt == crate::key_mods::ModSide::Left,
                    OptionAsAlt::Right => all_mods.sides.alt == crate::key_mods::ModSide::Right,
                    OptionAsAlt::True => true,
                    OptionAsAlt::False => false,
                }
            } else {
                true
            };

            if seq.mods.prevents_text(alt_prevents_text) {
                break 'associated;
            }

            seq.text = event.utf8.clone();
        }
    }

    seq.encode(out);
}

/// Narrow legacy-encoder placeholder. See the module doc comment for the seam
/// design this exists to preserve. Reproduces the spike window's pre-port
/// fixed encoding table (not a port of Zig's `legacy()` — that is a later
/// chunk's scope).
fn legacy_stub(out: &mut Vec<u8>, event: &KeyEvent, opts: &Options) {
    // Legacy encoding only handles press/repeat, matching Zig's `legacy()`
    // (and matching the spike window's prior behavior, which only ever saw
    // press events from egui).
    if event.action != Action::Press && event.action != Action::Repeat {
        return;
    }
    if event.composing {
        return;
    }

    let bytes: Option<Vec<u8>> = match event.key {
        Key::Enter => Some(b"\r".to_vec()),
        Key::Backspace => Some(vec![0x7f]),
        Key::Tab => Some(b"\t".to_vec()),
        Key::Escape => Some(vec![0x1b]),
        Key::ArrowLeft => Some(cursor_key(b'D', opts.cursor_key_application)),
        Key::ArrowRight => Some(cursor_key(b'C', opts.cursor_key_application)),
        Key::ArrowUp => Some(cursor_key(b'A', opts.cursor_key_application)),
        Key::ArrowDown => Some(cursor_key(b'B', opts.cursor_key_application)),
        Key::Home => Some(b"\x1b[H".to_vec()),
        Key::End => Some(b"\x1b[F".to_vec()),
        Key::Delete => Some(b"\x1b[3~".to_vec()),
        Key::PageUp => Some(b"\x1b[5~".to_vec()),
        Key::PageDown => Some(b"\x1b[6~".to_vec()),
        other => {
            if event.mods.ctrl {
                control_key(other).map(|b| vec![b])
            } else {
                None
            }
        }
    };

    if let Some(bytes) = bytes {
        out.extend_from_slice(&bytes);
    }
}

fn cursor_key(final_byte: u8, application_cursor_keys: bool) -> Vec<u8> {
    if application_cursor_keys {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn control_key(key: Key) -> Option<u8> {
    let ch = match key {
        Key::KeyA => b'A',
        Key::KeyB => b'B',
        Key::KeyC => b'C',
        Key::KeyD => b'D',
        Key::KeyE => b'E',
        Key::KeyF => b'F',
        Key::KeyG => b'G',
        Key::KeyH => b'H',
        Key::KeyI => b'I',
        Key::KeyJ => b'J',
        Key::KeyK => b'K',
        Key::KeyL => b'L',
        Key::KeyM => b'M',
        Key::KeyN => b'N',
        Key::KeyO => b'O',
        Key::KeyP => b'P',
        Key::KeyQ => b'Q',
        Key::KeyR => b'R',
        Key::KeyS => b'S',
        Key::KeyT => b'T',
        Key::KeyU => b'U',
        Key::KeyV => b'V',
        Key::KeyW => b'W',
        Key::KeyX => b'X',
        Key::KeyY => b'Y',
        Key::KeyZ => b'Z',
        _ => return None,
    };
    Some(ch - b'@')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kitty_encode(event: KeyEvent, kitty_flags: KittyFlags) -> Vec<u8> {
        let opts = Options {
            kitty_flags,
            ..Options::default()
        };
        encode(&event, &opts)
    }

    fn ev(key: Key) -> KeyEvent {
        KeyEvent {
            key,
            ..KeyEvent::default()
        }
    }

    // Port of test "KittySequence: backspace".
    #[test]
    fn kitty_sequence_backspace() {
        // Plain.
        let mut out = Vec::new();
        let seq = KittySequence {
            key: 127,
            final_byte: b'u',
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1B[127u");

        // Release event.
        let mut out = Vec::new();
        let seq = KittySequence {
            key: 127,
            final_byte: b'u',
            event: KittyEventType::Release,
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1B[127;1:3u");

        // Shift.
        let mut out = Vec::new();
        let seq = KittySequence {
            key: 127,
            final_byte: b'u',
            mods: KittyMods {
                shift: true,
                ..KittyMods::default()
            },
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1B[127;2u");
    }

    // Port of test "KittySequence: text".
    #[test]
    fn kitty_sequence_text() {
        let mut out = Vec::new();
        let seq = KittySequence {
            key: 127,
            final_byte: b'u',
            text: "A".to_string(),
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1B[127;;65u");

        let mut out = Vec::new();
        let seq = KittySequence {
            key: 127,
            final_byte: b'u',
            event: KittyEventType::Release,
            text: "A".to_string(),
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1B[127;1:3;65u");

        let mut out = Vec::new();
        let seq = KittySequence {
            key: 127,
            final_byte: b'u',
            mods: KittyMods {
                shift: true,
                ..KittyMods::default()
            },
            text: "A".to_string(),
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1B[127;2;65u");
    }

    // Port of test "KittySequence: text with control characters".
    #[test]
    fn kitty_sequence_text_with_control_characters() {
        let mut out = Vec::new();
        let seq = KittySequence {
            key: 127,
            final_byte: b'u',
            text: "\n".to_string(),
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1b[127u");

        let mut out = Vec::new();
        let seq = KittySequence {
            key: 127,
            final_byte: b'u',
            text: "A\n".to_string(),
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1b[127;;65u");
    }

    // Port of test "KittySequence: special no mods".
    #[test]
    fn kitty_sequence_special_no_mods() {
        let mut out = Vec::new();
        let seq = KittySequence {
            key: 1,
            final_byte: b'A',
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1B[A");
    }

    // Port of test "KittySequence: special mods only".
    #[test]
    fn kitty_sequence_special_mods_only() {
        let mut out = Vec::new();
        let seq = KittySequence {
            key: 1,
            final_byte: b'A',
            mods: KittyMods {
                shift: true,
                ..KittyMods::default()
            },
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1B[1;2A");
    }

    // Port of test "KittySequence: special mods and event".
    #[test]
    fn kitty_sequence_special_mods_and_event() {
        let mut out = Vec::new();
        let seq = KittySequence {
            key: 1,
            final_byte: b'A',
            event: KittyEventType::Release,
            mods: KittyMods {
                shift: true,
                ..KittyMods::default()
            },
            ..KittySequence::default()
        };
        seq.encode(&mut out);
        assert_eq!(out, b"\x1B[1;2:3A");
    }

    // Port of test "kitty: plain text".
    #[test]
    fn kitty_plain_text() {
        let event = KeyEvent {
            key: Key::KeyA,
            utf8: "abcd".to_string(),
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"abcd");
    }

    // Port of test "kitty: repeat with just disambiguate".
    #[test]
    fn kitty_repeat_with_just_disambiguate() {
        let event = KeyEvent {
            key: Key::KeyA,
            action: Action::Repeat,
            utf8: "a".to_string(),
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"a");
    }

    // Port of test "kitty: enter, backspace, tab" (split into sub-cases for
    // readability; all assertions from the Zig test are preserved).
    #[test]
    fn kitty_enter_backspace_tab() {
        let flags = KittyFlags {
            disambiguate: true,
            ..KittyFlags::DISABLED
        };

        assert_eq!(kitty_encode(ev(Key::Enter), flags), b"\r");

        // DECBKM reset.
        assert_eq!(
            kitty_encode(
                KeyEvent {
                    key: Key::Backspace,
                    ..KeyEvent::default()
                },
                flags,
            ),
            b"\x7f"
        );
        // DECBKM set (kitty doesn't support DECBKM, so no change):
        // `kitty()` never reads `backarrow_key_mode`, matching Zig, so this
        // is equivalent to the reset case above and is covered by it.

        assert_eq!(kitty_encode(ev(Key::Tab), flags), b"\t");

        // No release events if "report_all" is not set.
        let flags_events = KittyFlags {
            disambiguate: true,
            report_events: true,
            ..KittyFlags::DISABLED
        };
        for key in [Key::Enter, Key::Backspace, Key::Tab] {
            let event = KeyEvent {
                key,
                action: Action::Release,
                ..KeyEvent::default()
            };
            assert_eq!(kitty_encode(event, flags_events), Vec::<u8>::new());
        }

        // Release events if "report_all" is set.
        let flags_all = KittyFlags {
            disambiguate: true,
            report_events: true,
            report_all: true,
            ..KittyFlags::DISABLED
        };
        let cases = [
            (Key::Enter, "\x1b[13;1:3u"),
            (Key::Backspace, "\x1b[127;1:3u"),
            (Key::Tab, "\x1b[9;1:3u"),
        ];
        for (key, expected) in cases {
            let event = KeyEvent {
                key,
                action: Action::Release,
                ..KeyEvent::default()
            };
            assert_eq!(kitty_encode(event, flags_all), expected.as_bytes());
        }
    }

    // Port of test "kitty: shift+backspace emits CSI u".
    #[test]
    fn kitty_shift_backspace_emits_csi_u() {
        let event = KeyEvent {
            key: Key::Backspace,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[127;2u");
    }

    // Port of test "kitty: shift+enter emits CSI u".
    #[test]
    fn kitty_shift_enter_emits_csi_u() {
        let event = KeyEvent {
            key: Key::Enter,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[13;2u");
    }

    // Port of test "kitty: shift+tab emits CSI u".
    #[test]
    fn kitty_shift_tab_emits_csi_u() {
        let event = KeyEvent {
            key: Key::Tab,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[9;2u");
    }

    // Port of test "kitty: enter with all flags".
    #[test]
    fn kitty_enter_with_all_flags() {
        let out = kitty_encode(ev(Key::Enter), KittyFlags::ALL);
        assert_eq!(&out[1..], b"[13u");
    }

    // Port of test "kitty: ctrl with all flags".
    #[test]
    fn kitty_ctrl_with_all_flags() {
        let event = KeyEvent {
            key: Key::ControlLeft,
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(&out[1..], b"[57442;5u");
    }

    // Port of test "kitty: ctrl release with ctrl mod set".
    #[test]
    fn kitty_ctrl_release_with_ctrl_mod_set() {
        let event = KeyEvent {
            action: Action::Release,
            key: Key::ControlLeft,
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(&out[1..], b"[57442;5:3u");
    }

    // Port of test "kitty: delete".
    #[test]
    fn kitty_delete() {
        let event = KeyEvent {
            key: Key::Delete,
            utf8: "\x7F".to_string(),
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[3~");
    }

    // Port of test "kitty: composing with no modifier".
    #[test]
    fn kitty_composing_with_no_modifier() {
        let event = KeyEvent {
            key: Key::KeyA,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            composing: true,
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, Vec::<u8>::new());
    }

    // Port of test "kitty: composing with modifier".
    #[test]
    fn kitty_composing_with_modifier() {
        let event = KeyEvent {
            key: Key::ShiftLeft,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            composing: true,
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                report_all: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[57441;2u");
    }

    // Port of test "kitty: composed text with report all".
    #[test]
    fn kitty_composed_text_with_report_all() {
        let event = KeyEvent {
            key: Key::Unidentified,
            utf8: "\u{fb}".to_string(), // "û"
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(out, "\u{fb}".as_bytes());
    }

    // Port of test "kitty: shift+a on US keyboard".
    #[test]
    fn kitty_shift_a_on_us_keyboard() {
        let event = KeyEvent {
            key: Key::KeyA,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            utf8: "A".to_string(),
            unshifted_codepoint: 97,
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                report_alternates: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[97:65;2u");
    }

    // Port of test "kitty: matching unshifted codepoint".
    #[test]
    fn kitty_matching_unshifted_codepoint() {
        let event = KeyEvent {
            key: Key::KeyA,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            utf8: "A".to_string(),
            unshifted_codepoint: 65,
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                report_alternates: true,
                ..KittyFlags::DISABLED
            },
        );
        // Not a valid real-world encoding (hypothetical unshifted_codepoint);
        // exercises the "unshifted codepoint doesn't match base key" branch.
        assert_eq!(out, b"\x1b[65::97;2u");
    }

    // Port of test "kitty: report alternates with caps".
    #[test]
    fn kitty_report_alternates_with_caps() {
        let event = KeyEvent {
            key: Key::KeyJ,
            mods: Mods {
                caps_lock: true,
                ..Mods::default()
            },
            utf8: "J".to_string(),
            unshifted_codepoint: 106,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(out, b"\x1b[106;65;74u");
    }

    // Port of test "kitty: report alternates colon (shift+';')".
    #[test]
    fn kitty_report_alternates_colon_shift_semicolon() {
        let event = KeyEvent {
            key: Key::Semicolon,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            utf8: ":".to_string(),
            unshifted_codepoint: ';' as u32,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(out, b"\x1b[59:58;2;58u");
    }

    // Port of test "kitty: report alternates with ru layout".
    #[test]
    fn kitty_report_alternates_with_ru_layout() {
        let event = KeyEvent {
            key: Key::Semicolon,
            utf8: "\u{447}".to_string(), // "ч"
            unshifted_codepoint: 1095,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(out, "\x1b[1095::59;;1095u".as_bytes());
    }

    // Port of test "kitty: report alternates with ru layout shifted".
    #[test]
    fn kitty_report_alternates_with_ru_layout_shifted() {
        let event = KeyEvent {
            key: Key::Semicolon,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            utf8: "\u{427}".to_string(), // "Ч"
            unshifted_codepoint: 1095,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(out, "\x1b[1095:1063:59;2;1063u".as_bytes());
    }

    // Port of test "kitty: report alternates with ru layout caps lock".
    #[test]
    fn kitty_report_alternates_with_ru_layout_caps_lock() {
        let event = KeyEvent {
            key: Key::Semicolon,
            mods: Mods {
                caps_lock: true,
                ..Mods::default()
            },
            utf8: "\u{427}".to_string(), // "Ч"
            unshifted_codepoint: 1095,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(out, "\x1b[1095::59;65;1063u".as_bytes());
    }

    // Port of test "kitty: report alternates with hu layout release".
    #[test]
    fn kitty_report_alternates_with_hu_layout_release() {
        let event = KeyEvent {
            action: Action::Release,
            key: Key::BracketLeft,
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            unshifted_codepoint: 337,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(&out[1..], "[337::91;5:3u".as_bytes());
    }

    // Port of test "kitty: up arrow with utf8" (macOS generates utf8 text for
    // arrow keys).
    #[test]
    fn kitty_up_arrow_with_utf8() {
        let event = KeyEvent {
            key: Key::ArrowUp,
            utf8: "\u{1e}".to_string(),
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[A");
    }

    // Port of test "kitty: shift+tab".
    #[test]
    fn kitty_shift_tab() {
        let event = KeyEvent {
            key: Key::Tab,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                report_alternates: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[9;2u");
    }

    // Port of test "kitty: left shift".
    #[test]
    fn kitty_left_shift() {
        let event = ev(Key::ShiftLeft);
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                report_alternates: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, Vec::<u8>::new());
    }

    // Port of test "kitty: left shift with report all".
    #[test]
    fn kitty_left_shift_with_report_all() {
        let event = ev(Key::ShiftLeft);
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                report_all: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[57441u");
    }

    // Port of test "kitty: report associated with alt text on macOS with
    // option". Zig `SkipZigTest`s this off-Darwin; ported to just return
    // early on non-macOS hosts, matching that skip.
    #[test]
    fn kitty_report_associated_with_alt_text_on_macos_with_option() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let event = KeyEvent {
            key: Key::KeyW,
            mods: Mods {
                alt: true,
                ..Mods::default()
            },
            utf8: "\u{2211}".to_string(), // "∑"
            unshifted_codepoint: 119,
            ..KeyEvent::default()
        };
        let opts = Options {
            kitty_flags: KittyFlags::ALL,
            macos_option_as_alt: OptionAsAlt::False,
            ..Options::default()
        };
        let out = encode(&event, &opts);
        assert_eq!(out, "\x1b[119;3;8721u".as_bytes());
    }

    // Port of test "kitty: report associated with alt text on macOS with
    // alt". Gated like the Zig test (Darwin-only).
    #[test]
    fn kitty_report_associated_with_alt_text_on_macos_with_alt() {
        if !cfg!(target_os = "macos") {
            return;
        }

        // With Alt modifier.
        let event = KeyEvent {
            key: Key::KeyW,
            mods: Mods {
                alt: true,
                ..Mods::default()
            },
            utf8: "\u{2211}".to_string(),
            unshifted_codepoint: 119,
            ..KeyEvent::default()
        };
        let opts = Options {
            kitty_flags: KittyFlags::ALL,
            macos_option_as_alt: OptionAsAlt::True,
            ..Options::default()
        };
        let out = encode(&event, &opts);
        assert_eq!(out, "\x1b[119;3u".as_bytes());

        // Without Alt modifier.
        let event = KeyEvent {
            key: Key::KeyW,
            utf8: "\u{2211}".to_string(),
            unshifted_codepoint: 119,
            ..KeyEvent::default()
        };
        let out = encode(&event, &opts);
        assert_eq!(out, "\x1b[119;;8721u".as_bytes());
    }

    // Port of test "kitty: report associated with modifiers".
    #[test]
    fn kitty_report_associated_with_modifiers() {
        let event = KeyEvent {
            key: Key::KeyJ,
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            utf8: "j".to_string(),
            unshifted_codepoint: 106,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(out, b"\x1b[106;5u");
    }

    // Port of test "kitty: report associated".
    #[test]
    fn kitty_report_associated() {
        let event = KeyEvent {
            key: Key::KeyJ,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            utf8: "J".to_string(),
            unshifted_codepoint: 106,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(out, b"\x1b[106:74;2;74u");
    }

    // Port of test "kitty: report associated on release".
    #[test]
    fn kitty_report_associated_on_release() {
        let event = KeyEvent {
            action: Action::Release,
            key: Key::KeyJ,
            mods: Mods {
                shift: true,
                ..Mods::default()
            },
            utf8: "J".to_string(),
            unshifted_codepoint: 106,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(&out[1..], b"[106:74;2:3u");
    }

    // Port of test "kitty: alternates omit control characters".
    #[test]
    fn kitty_alternates_omit_control_characters() {
        let event = KeyEvent {
            key: Key::Delete,
            utf8: "\x7F".to_string(),
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                report_alternates: true,
                report_all: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"\x1b[3~");
    }

    // Port of test "kitty: enter with utf8 (dead key state)".
    #[test]
    fn kitty_enter_with_utf8_dead_key_state() {
        let event = KeyEvent {
            key: Key::Enter,
            utf8: "A".to_string(),
            unshifted_codepoint: 0x0D,
            ..KeyEvent::default()
        };
        let out = kitty_encode(
            event,
            KittyFlags {
                disambiguate: true,
                report_alternates: true,
                report_all: true,
                ..KittyFlags::DISABLED
            },
        );
        assert_eq!(out, b"A");
    }

    // Port of test "kitty: keypad number".
    #[test]
    fn kitty_keypad_number() {
        let event = KeyEvent {
            key: Key::Numpad1,
            utf8: "1".to_string(),
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(&out[1..], b"[57400;;49u");
    }

    // Port of test "kitty: backspace with utf8 (dead key state)".
    #[test]
    fn kitty_backspace_with_utf8_dead_key_state() {
        let event = KeyEvent {
            key: Key::Backspace,
            utf8: "A".to_string(),
            unshifted_codepoint: 0x0D,
            ..KeyEvent::default()
        };
        let out = kitty_encode(event, KittyFlags::ALL);
        assert_eq!(out, Vec::<u8>::new());
    }

    // Port of test "kitty: backspace (DECBKM reset) (report_all: true)" and
    // "kitty: backspace (DECBKM set) (report_all: true)". Kitty does not
    // support DECBKM, so `backarrow_key_mode` has no effect on the kitty
    // path — both Zig tests expect the identical output; ported as one test
    // with both `Options` values to keep that "no effect" property explicit.
    #[test]
    fn kitty_backspace_decbkm_has_no_effect_with_report_all() {
        let event = KeyEvent {
            key: Key::Backspace,
            ..KeyEvent::default()
        };
        for backarrow_key_mode in [false, true] {
            let opts = Options {
                kitty_flags: KittyFlags::ALL,
                backarrow_key_mode,
                ..Options::default()
            };
            let out = encode(&event, &opts);
            assert_eq!(out, b"\x1b[127u");
        }
    }

    // Port of test "modifier sequence values" (KittyMods variant, embedded in
    // the Zig `KittyMods` struct).
    #[test]
    fn kitty_mods_modifier_sequence_values() {
        assert_eq!(KittyMods::default().seq_int(), 1);
        assert_eq!(
            KittyMods {
                shift: true,
                ..KittyMods::default()
            }
            .seq_int(),
            2
        );
        assert_eq!(
            KittyMods {
                alt: true,
                ..KittyMods::default()
            }
            .seq_int(),
            3
        );
        assert_eq!(
            KittyMods {
                ctrl: true,
                ..KittyMods::default()
            }
            .seq_int(),
            5
        );
        assert_eq!(
            KittyMods {
                alt: true,
                shift: true,
                ..KittyMods::default()
            }
            .seq_int(),
            4
        );
        assert_eq!(
            KittyMods {
                ctrl: true,
                shift: true,
                ..KittyMods::default()
            }
            .seq_int(),
            6
        );
        assert_eq!(
            KittyMods {
                alt: true,
                ctrl: true,
                ..KittyMods::default()
            }
            .seq_int(),
            7
        );
        assert_eq!(
            KittyMods {
                alt: true,
                ctrl: true,
                shift: true,
                ..KittyMods::default()
            }
            .seq_int(),
            8
        );
    }

    // Legacy-seam smoke tests: not a Zig port (the legacy encoder itself is
    // out of scope for this chunk), just confirming the placeholder
    // reproduces the spike window's pre-port behavior for a few keys.
    #[test]
    fn legacy_stub_arrow_key_normal_mode() {
        let event = ev(Key::ArrowUp);
        let opts = Options::default();
        assert_eq!(encode(&event, &opts), b"\x1b[A");
    }

    #[test]
    fn legacy_stub_arrow_key_application_mode() {
        let event = ev(Key::ArrowUp);
        let opts = Options {
            cursor_key_application: true,
            ..Options::default()
        };
        assert_eq!(encode(&event, &opts), b"\x1bOA");
    }

    #[test]
    fn legacy_stub_ctrl_c() {
        let event = KeyEvent {
            key: Key::KeyC,
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            ..KeyEvent::default()
        };
        let opts = Options::default();
        assert_eq!(encode(&event, &opts), vec![0x03]);
    }
}
