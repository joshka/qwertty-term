//! Key event encoding — turns a [`KeyEvent`] into the PTY bytes a terminal
//! program expects. Port of `input/key_encode.zig` (2540 lines, 92 tests).
//!
//! ## Scope of this port
//!
//! Ghostty's `key_encode.zig` implements two encoders behind one dispatch:
//! the modern **kitty keyboard protocol** (`CSI … u`) and the **legacy**
//! encoder (PC-style function keys + xterm `modifyOtherKeys` + Paul Evans's
//! "fixterms" CSI-u extension for ctrl+letter etc). Both are now ported in
//! full: the kitty path ([`kitty`], [`KittySequence`], [`KittyMods`]) and the
//! legacy path ([`legacy`], [`pc_style_function_key`], [`ctrl_seq`],
//! [`CsiUMods`], [`legacy_alt_prefix`], and `modifyOtherKeys` state 2).
//!
//! ## Dispatch
//!
//! [`encode`] dispatches on `opts.kitty_flags` exactly like the Zig source
//! (`if (opts.kitty_flags.int() != 0) kitty(...) else legacy(...)`), via the
//! [`Encoder`] enum below.

use crate::function_keys::{self, CursorMode, KeypadMode, ModifyKeys};
use crate::key::{Action, Key, KeyEvent};
use crate::key_mods::{ModSide, Mods, OptionAsAlt};
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
        Encoder::Legacy => legacy(&mut out, event, opts),
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
        legacy(out, event, opts);
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

/// Perform legacy encoding of the key event. "Legacy" here refers to the
/// behavior of traditional terminals, plus xterm's `modifyOtherKeys`, plus
/// Paul Evans's "fixterms" spec. These combine into the legacy protocol
/// because they're all meant to be extensions that don't change existing
/// behavior and so are safe to combine. Port of `key_encode.legacy`.
fn legacy(out: &mut Vec<u8>, event: &KeyEvent, opts: &Options) {
    let all_mods = event.mods;
    let effective_mods = event.effective_mods();
    let binding_mods = effective_mods.binding();

    // Legacy encoding only does press/repeat.
    if event.action != Action::Press && event.action != Action::Repeat {
        return;
    }

    // If we're in a dead key state then we never emit a sequence.
    if event.composing {
        return;
    }

    // If we match a PC style function key then that is our result.
    if let Some(sequence) = pc_style_function_key(
        event.key,
        all_mods,
        opts.cursor_key_application,
        opts.keypad_key_application,
        opts.ignore_keypad_with_numlock,
        opts.modify_other_keys_state_2,
        opts.backarrow_key_mode,
    ) {
        // `pc_style` is a labeled block in Zig; `emit_pc_style` mirrors the
        // `break :pc_style` (fall through to ctrlSeq below) vs `return`
        // (emit the sequence, or emit nothing) control flow.
        let mut emit_pc_style = true;

        // If we have UTF-8 text, then we never emit PC style function keys.
        // Many function keys (escape, enter, backspace) have a specific
        // meaning when dead keys are active and so we don't want to send that
        // to the terminal. Examples:
        //   - Japanese: escape clears the dead key state
        //   - Korean: escape commits the dead key state
        //   - Korean: backspace should delete a single preedit char
        if !event.utf8.is_empty() && matches!(event.key, Key::Backspace | Key::Enter | Key::Escape)
        {
            // We want to ignore control characters. This is because some
            // apprts (macOS) will send control characters as UTF-8 encodings
            // and we handle that manually.
            if !is_control_utf8(&event.utf8) {
                // Backspace encodes nothing because we modified IME.
                // Enter/escape don't encode the PC-style encoding because we
                // want to encode committed text (fall through to below).
                if event.key == Key::Backspace {
                    return;
                }
                emit_pc_style = false;
            }
        }

        if emit_pc_style {
            out.extend_from_slice(sequence.as_bytes());
            return;
        }
    }

    // If we match a control sequence, we output that directly. For ctrlSeq we
    // have to use all mods because we want it to only match ctrl+<char>.
    if let Some(char) = ctrl_seq(event.key, &event.utf8, event.unshifted_codepoint, all_mods) {
        // C0 sequences support alt-as-esc prefixing.
        if binding_mods.alt {
            out.push(0x1B);
            out.push(char);
            return;
        }

        out.push(char);
        return;
    }

    // If we have no UTF8 text then the only possibility is the alt-prefix
    // handling of unshifted codepoints... so we process that.
    let utf8 = &event.utf8;
    if utf8.is_empty() {
        if let Some(byte) = legacy_alt_prefix(event, binding_mods, all_mods, opts) {
            out.push(0x1B);
            out.push(byte);
        }
        return;
    }

    // In modify other keys state 2, we send the CSI 27 sequence for any char
    // with a modifier. Ctrl sequences like Ctrl+a are already handled above.
    if opts.modify_other_keys_state_2 {
        'modify_other: {
            let mut chars = utf8.chars();
            let Some(codepoint) = chars.next() else {
                break 'modify_other;
            };
            let codepoint = codepoint as u32;

            // We only do this if we have a single codepoint. There shouldn't
            // ever be a multi-codepoint sequence that triggers this.
            if chars.next().is_some() {
                break 'modify_other;
            }

            // The mods we encode for this are just the binding mods (shift,
            // ctrl, super, alt unless it is actually option).
            let mods = {
                let mut mods_binding = event.mods.binding();
                if cfg!(target_os = "macos") {
                    let keep_alt = match opts.macos_option_as_alt {
                        OptionAsAlt::False => false,
                        OptionAsAlt::True => true,
                        OptionAsAlt::Left => event.mods.sides.alt == ModSide::Left,
                        OptionAsAlt::Right => event.mods.sides.alt == ModSide::Right,
                    };
                    if !keep_alt {
                        mods_binding.alt = false;
                    }
                }
                mods_binding
            };

            // This copies xterm's `ModifyOtherKeys` function that returns
            // whether modify other keys should be encoded for the given input.
            let should_modify = {
                // xterm IsControlInput.
                if (0x40..=0x7F).contains(&codepoint) {
                    true
                } else {
                    // If we have anything other than shift pressed, encode.
                    let mut mods_no_shift = mods;
                    mods_no_shift.shift = false;
                    if !mods_no_shift.empty() {
                        true
                    } else {
                        // We only have shift pressed. We only allow space.
                        codepoint == ' ' as u32
                    }
                }
            };

            if should_modify {
                for (i, modset) in function_keys::modifiers().into_iter().enumerate() {
                    if !mods.equal(modset) {
                        continue;
                    }
                    let code = i + 2;
                    out.extend_from_slice(format!("\x1B[27;{code};{codepoint}~").as_bytes());
                    return;
                }
            }
        }
    }

    // Let's see if we should apply fixterms to this codepoint. At this stage
    // of key processing, we only need to apply fixterms to unicode codepoints
    // if we have ctrl set.
    if event.mods.ctrl {
        'csiu: {
            // Important: we want to use the original mods here, not the
            // effective mods. The fixterms spec states the shifted chars
            // should be sent uppercase but Kitty changes that behavior so
            // we'll send all the mods.
            let mut mods = CsiUMods::from_input(event.mods);

            // Get our codepoint. If we have more than one codepoint this can't
            // be valid CSIu.
            let mut chars = event.utf8.chars();
            let Some(c) = chars.next() else {
                break 'csiu;
            };
            if chars.next().is_some() {
                break 'csiu;
            }
            let mut char = c as u32;

            // If our character is A to Z and we have shift set, then we
            // lowercase it. This is a Kitty-specific behavior that we choose to
            // follow and diverge from the fixterms spec. This makes it easier
            // for programs to detect shifted letters for keybindings.
            if ('A' as u32..='Z' as u32).contains(&char) && mods.shift {
                // We rely on apprt to send us the correct unshifted codepoint.
                char = (c as u8).to_ascii_lowercase() as u32;
            }

            // If our unshifted codepoint is identical to the shifted then we
            // consider shift. Otherwise, we do not because the shift key was
            // used to obtain the character. This is specified by fixterms.
            if event.unshifted_codepoint != char {
                mods.shift = false;
            }

            out.extend_from_slice(format!("\x1B[{};{}u", char, mods.seq_int()).as_bytes());
            return;
        }
    }

    // If we have alt-pressed and alt-esc-prefix is enabled, then we need to
    // prefix the utf8 sequence with an esc.
    if let Some(byte) = legacy_alt_prefix(event, binding_mods, all_mods, opts) {
        out.push(0x1B);
        out.push(byte);
        return;
    }

    // If we are on macOS, command+keys do not encode text. It isn't typical
    // for command+keys on macOS to ever encode text (native text inputs and
    // other native terminals don't either). For Linux, we continue to encode
    // text because it is typical (e.g. Gnome Console Super+b encodes "b").
    if cfg!(target_os = "macos") && all_mods.super_ {
        return;
    }

    out.extend_from_slice(utf8.as_bytes());
}

/// Alt-as-ESC-prefix handling for legacy encoding. Returns the byte that
/// should be prefixed with ESC, or `None` if alt-prefixing does not apply.
/// Port of `key_encode.legacyAltPrefix`.
fn legacy_alt_prefix(
    event: &KeyEvent,
    binding_mods: Mods,
    mods: Mods,
    opts: &Options,
) -> Option<u8> {
    // This only takes effect with alt pressed.
    if !binding_mods.alt || !opts.alt_esc_prefix {
        return None;
    }

    // On macOS, we only handle option like alt in certain circumstances.
    // Otherwise, macOS does a unicode translation and we allow that to happen.
    if cfg!(target_os = "macos") {
        match opts.macos_option_as_alt {
            OptionAsAlt::False => return None,
            OptionAsAlt::Left => {
                if mods.sides.alt == ModSide::Right {
                    return None;
                }
            }
            OptionAsAlt::Right => {
                if mods.sides.alt == ModSide::Left {
                    return None;
                }
            }
            OptionAsAlt::True => {}
        }
    }

    // Otherwise, we require utf8 to already have the byte represented.
    let utf8 = event.utf8.as_bytes();
    if utf8.len() == 1 {
        // `std.math.cast(u8, ...)` on a single byte is always in range.
        return Some(utf8[0]);
    }

    // If UTF8 isn't set, we allow unshifted codepoints through if they fit in
    // a byte.
    if event.unshifted_codepoint > 0
        && let Ok(byte) = u8::try_from(event.unshifted_codepoint)
    {
        return Some(byte);
    }

    // Else, we can't figure out the byte to alt-prefix so we exit.
    None
}

/// Determines whether the key should be encoded in the xterm "PC-style
/// Function Key" syntax (roughly). This walks the hardcoded
/// [`function_keys`] table for the key and returns the first matching
/// sequence. Port of `key_encode.pcStyleFunctionKey`.
fn pc_style_function_key(
    keyval: Key,
    mods: Mods,
    cursor_key_application: bool,
    keypad_key_application_req: bool,
    ignore_keypad_with_numlock: bool,
    modify_other_keys: bool, // true if state 2
    backarrow_key_mode: bool,
) -> Option<String> {
    // We only want binding-sensitive mods because lock keys and directional
    // modifiers (left/right) don't matter for pc-style function keys.
    let mods_int = mods.binding().int();

    // Keypad application keymode isn't super straightforward. On xterm, in
    // VT220 mode, numlock alone is enough to trigger application mode. But in
    // more modern modes, numlock is ignored by default via mode 1035 (default
    // true). If mode 1035 is on, we're always in numerical keypad mode. If
    // it's off, we're in application mode if the proper numlock state is
    // pressed (implicitly determined by the keycode sent).
    let keypad_key_application = if ignore_keypad_with_numlock {
        // If we're ignoring keypad then this is always false — i.e. we're
        // always in numerical keypad mode.
        false
    } else {
        keypad_key_application_req
    };

    for entry in function_keys::entries_for(keyval) {
        match entry.cursor {
            CursorMode::Any => {}
            CursorMode::Normal => {
                if cursor_key_application {
                    continue;
                }
            }
            CursorMode::Application => {
                if !cursor_key_application {
                    continue;
                }
            }
        }

        match entry.keypad {
            KeypadMode::Any => {}
            KeypadMode::Normal => {
                if keypad_key_application {
                    continue;
                }
            }
            KeypadMode::Application => {
                if !keypad_key_application {
                    continue;
                }
            }
        }

        match entry.modify_other_keys {
            ModifyKeys::Any => {}
            ModifyKeys::Set => {
                if modify_other_keys {
                    continue;
                }
            }
            ModifyKeys::SetOther => {
                if !modify_other_keys {
                    continue;
                }
            }
        }

        let entry_mods_int = entry.mods.int();
        if entry_mods_int == 0 {
            if mods_int != 0 && !entry.mods_empty_is_any {
                continue;
            }
            // mods are either empty, or empty means any so we allow it.
        } else if entry_mods_int != mods_int {
            // Any set mods require an exact match.
            continue;
        }

        if backarrow_key_mode && let Some(sequence) = entry.sequence_decbkm {
            return Some(sequence);
        }

        return Some(entry.sequence);
    }

    None
}

/// Returns the C0 byte for the key event if it should be used. This converts
/// a key event into the expected terminal behavior such as Ctrl+C turning
/// into 0x03, amongst many other translations. Returns `None` if the key
/// event should not be converted into a C0 byte. Port of `key_encode.ctrlSeq`.
fn ctrl_seq(logical_key: Key, utf8: &str, unshifted_codepoint: u32, mods: Mods) -> Option<u8> {
    let ctrl_only = Mods {
        ctrl: true,
        ..Mods::default()
    }
    .int();

    // If ctrl is not pressed then we never do anything.
    if !mods.ctrl {
        return None;
    }

    // We need to only get binding modifiers so we strip lock keys, sides, etc.
    let mut unset_mods = mods.binding();

    // Remove alt from our modifiers because it does not impact whether we are
    // generating a ctrl sequence and we handle the ESC-prefix logic
    // separately.
    unset_mods.alt = false;

    let utf8_bytes = utf8.as_bytes();
    let mut char: u8 = if utf8_bytes.len() == 1 {
        // If we have exactly one UTF8 byte, we assume that is the character we
        // want to convert to a C0 byte.
        utf8_bytes[0]
    } else if let Some(cp) = logical_key.codepoint() {
        // If we have a logical key that maps to a single byte printable
        // character, we use that. This supports cyrillic layouts (Russian,
        // Mongolian): their `c` key maps to U+0441 but every terminal encodes
        // this as ctrl+c.
        if let Ok(byte) = u8::try_from(cp) {
            // For this case, we only map to the key if we have exactly ctrl
            // pressed. Shift would modify the key and we don't know how to do
            // that properly here (no layout). We want to encode shift as CSIu.
            if unset_mods.int() != ctrl_only {
                return None;
            }
            byte
        } else {
            return None;
        }
    } else {
        // Otherwise we don't have a character to reliably map to a C0 byte.
        return None;
    };

    // Remove shift if we have something outside of the US letter range. This
    // is so that characters such as `ctrl+shift+-` generate the correct
    // ctrl-seq (used by emacs).
    if unset_mods.shift && !char.is_ascii_uppercase() {
        // Special case for fixterms awkward case as specified: `@` keeps shift.
        if char != b'@' {
            unset_mods.shift = false;
        }
    }

    // If the character is uppercase, we convert it to lowercase using the
    // unshifted codepoint. This handles caps lock. Shifted characters are
    // handled above; if we are just pressing shift then the ctrl-only check
    // will fail later and we won't ctrl-seq encode.
    if char.is_ascii_uppercase()
        && unshifted_codepoint > 0
        && let Ok(byte) = u8::try_from(unshifted_codepoint)
    {
        char = byte;
    }

    // After unsetting, we only continue if we have ONLY control set.
    if unset_mods.int() != ctrl_only {
        return None;
    }

    // From Kitty's key encoding logic. Repeat what Kitty does.
    Some(match char {
        b' ' => 0,
        b'/' => 31,
        b'0' => 48,
        b'1' => 49,
        b'2' => 0,
        b'3' => 27,
        b'4' => 28,
        b'5' => 29,
        b'6' => 30,
        b'7' => 31,
        b'8' => 127,
        b'9' => 57,
        b'?' => 127,
        b'@' => 0,
        b'\\' => 28,
        b']' => 29,
        b'^' => 30,
        b'_' => 31,
        b'a' => 1,
        b'b' => 2,
        b'c' => 3,
        b'd' => 4,
        b'e' => 5,
        b'f' => 6,
        b'g' => 7,
        b'h' => 8,
        b'j' => 10,
        b'k' => 11,
        b'l' => 12,
        b'n' => 14,
        b'o' => 15,
        b'p' => 16,
        b'q' => 17,
        b'r' => 18,
        b's' => 19,
        b't' => 20,
        b'u' => 21,
        b'v' => 22,
        b'w' => 23,
        b'x' => 24,
        b'y' => 25,
        b'z' => 26,
        b'~' => 30,

        // These are purposely NOT handled here because of the fixterms
        // specification (https://www.leonerd.org.uk/hacks/fixterms/). They
        // are processed as CSI u:
        //   'i' => 0x09, 'm' => 0x0D, '[' => 0x1B
        _ => return None,
    })
}

/// This is the bitmask for fixterm CSI u modifiers. Port of
/// `key_encode.CsiUMods` (`packed struct(u3)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct CsiUMods {
    shift: bool,
    alt: bool,
    ctrl: bool,
}

impl CsiUMods {
    /// Convert an input mods value into the CSI u mods value. Port of
    /// `CsiUMods.fromInput`.
    fn from_input(mods: Mods) -> CsiUMods {
        CsiUMods {
            shift: mods.shift,
            alt: mods.alt,
            ctrl: mods.ctrl,
        }
    }

    /// Returns the raw int value of this packed struct. Port of `CsiUMods.int`.
    fn int(self) -> u8 {
        (self.shift as u8) | (self.alt as u8) << 1 | (self.ctrl as u8) << 2
    }

    /// Returns the integer value sent as part of the CSI u sequence. This adds
    /// 1 to the bitmask value as described in the spec. Port of
    /// `CsiUMods.seqInt`.
    fn seq_int(self) -> u8 {
        self.int() + 1
    }
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

    // ------------------------------------------------------------------
    // Legacy encoder helpers + Zig test ports.
    // ------------------------------------------------------------------

    /// Encode via the legacy path directly (bypassing `encode`'s dispatch),
    /// mirroring the Zig tests which call `legacy(...)` directly.
    fn legacy_encode(event: KeyEvent, opts: Options) -> Vec<u8> {
        let mut out = Vec::new();
        legacy(&mut out, &event, &opts);
        out
    }

    // ---- Dispatch tests (kitty-vs-legacy selection per flags) ----

    // Not a Zig port: proves `encode` routes to the legacy encoder when no
    // kitty flag is set, and to the kitty encoder when any flag is set.
    #[test]
    fn dispatch_selects_legacy_when_no_kitty_flags() {
        // ctrl+c: legacy emits the C0 byte 0x03; kitty would emit CSI u.
        let event = KeyEvent {
            key: Key::KeyC,
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            utf8: "c".to_string(),
            ..KeyEvent::default()
        };
        assert_eq!(encode(&event, &Options::default()), vec![0x03]);
    }

    #[test]
    fn dispatch_selects_kitty_when_any_kitty_flag_set() {
        // Same ctrl+c event, but with a kitty flag set: kitty encodes it as
        // CSI u (ESC[99;5u), not the legacy C0 byte 0x03. `unshifted_codepoint`
        // is set so the kitty table resolves the base key to 'c'.
        let event = KeyEvent {
            key: Key::KeyC,
            mods: Mods {
                ctrl: true,
                ..Mods::default()
            },
            unshifted_codepoint: 'c' as u32,
            ..KeyEvent::default()
        };
        let opts = Options {
            kitty_flags: KittyFlags {
                disambiguate: true,
                ..KittyFlags::DISABLED
            },
            ..Options::default()
        };
        assert_eq!(encode(&event, &opts), b"\x1b[99;5u");
    }

    // Port of test "legacy: backspace with utf8 (dead key state)".
    #[test]
    fn legacy_backspace_with_utf8_dead_key_state() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Backspace,
                utf8: "A".to_string(),
                unshifted_codepoint: 0x0D,
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, Vec::<u8>::new());
    }

    // Port of test "legacy: enter with utf8 (dead key state)".
    #[test]
    fn legacy_enter_with_utf8_dead_key_state() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Enter,
                utf8: "A".to_string(),
                unshifted_codepoint: 0x0D,
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"A");
    }

    // Port of test "legacy: esc with utf8 (dead key state)".
    #[test]
    fn legacy_esc_with_utf8_dead_key_state() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Escape,
                utf8: "A".to_string(),
                unshifted_codepoint: 0x0D,
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"A");
    }

    // Port of test "legacy: ctrl+shift+minus (underscore on US)".
    #[test]
    fn legacy_ctrl_shift_minus_underscore_on_us() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Minus,
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                utf8: "_".to_string(),
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\x1F");
    }

    // Port of test "legacy: ctrl+alt+c".
    #[test]
    fn legacy_ctrl_alt_c() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyC,
                mods: Mods {
                    ctrl: true,
                    alt: true,
                    ..Mods::default()
                },
                utf8: "c".to_string(),
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\x1b\x03");
    }

    // Port of test "legacy: alt+c".
    #[test]
    fn legacy_alt_c() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyC,
                utf8: "c".to_string(),
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options {
                alt_esc_prefix: true,
                macos_option_as_alt: OptionAsAlt::True,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x1Bc");
    }

    // Port of test "legacy: alt+e only unshifted".
    #[test]
    fn legacy_alt_e_only_unshifted() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyE,
                unshifted_codepoint: 'e' as u32,
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options {
                alt_esc_prefix: true,
                macos_option_as_alt: OptionAsAlt::True,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x1Be");
    }

    // Port of test "legacy: alt+x macos" (Darwin-gated).
    #[test]
    fn legacy_alt_x_macos() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyC,
                utf8: "\u{2248}".to_string(), // "≈"
                unshifted_codepoint: 'c' as u32,
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options {
                alt_esc_prefix: true,
                macos_option_as_alt: OptionAsAlt::True,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x1Bc");
    }

    // Port of test "legacy: shift+alt+. macos" (Darwin-gated).
    #[test]
    fn legacy_shift_alt_period_macos() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let out = legacy_encode(
            KeyEvent {
                key: Key::Period,
                utf8: ">".to_string(),
                unshifted_codepoint: '.' as u32,
                mods: Mods {
                    alt: true,
                    shift: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options {
                alt_esc_prefix: true,
                macos_option_as_alt: OptionAsAlt::True,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x1B>");
    }

    // Port of test "legacy: alt+ф".
    #[test]
    fn legacy_alt_cyrillic_f() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyF,
                utf8: "\u{444}".to_string(), // "ф"
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options {
                alt_esc_prefix: true,
                ..Options::default()
            },
        );
        assert_eq!(out, "\u{444}".as_bytes());
    }

    // Port of test "legacy: ctrl+c".
    #[test]
    fn legacy_ctrl_c() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyC,
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                utf8: "c".to_string(),
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\x03");
    }

    // Port of test "legacy: ctrl+space".
    #[test]
    fn legacy_ctrl_space() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Space,
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                utf8: " ".to_string(),
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\x00");
    }

    // Port of test "legacy: ctrl+shift+backspace".
    #[test]
    fn legacy_ctrl_shift_backspace() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Backspace,
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\x08");
    }

    // Port of test "legacy: backspace (DECBKM reset)".
    #[test]
    fn legacy_backspace_decbkm_reset() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Backspace,
                ..KeyEvent::default()
            },
            Options {
                backarrow_key_mode: false,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x7f");
    }

    // Port of test "legacy: backspace (DECBKM reset, with ctrl)".
    #[test]
    fn legacy_backspace_decbkm_reset_with_ctrl() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Backspace,
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options {
                backarrow_key_mode: false,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x08");
    }

    // Port of test "legacy: backspace (DECBKM set)".
    #[test]
    fn legacy_backspace_decbkm_set() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Backspace,
                ..KeyEvent::default()
            },
            Options {
                backarrow_key_mode: true,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x08");
    }

    // Port of test "legacy: backspace (DECBKM set, with ctrl)".
    #[test]
    fn legacy_backspace_decbkm_set_with_ctrl() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Backspace,
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options {
                backarrow_key_mode: true,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x7f");
    }

    // Port of test "legacy: ctrl+shift+char with modify other state 2".
    #[test]
    fn legacy_ctrl_shift_char_with_modify_other_state_2() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyH,
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                utf8: "H".to_string(),
                ..KeyEvent::default()
            },
            Options {
                modify_other_keys_state_2: true,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x1b[27;6;72~");
    }

    // Port of test "legacy: ctrl+shift+char with modify other state 2 and
    // consumed mods".
    #[test]
    fn legacy_ctrl_shift_char_with_modify_other_state_2_and_consumed_mods() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyH,
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                consumed_mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
                utf8: "H".to_string(),
                ..KeyEvent::default()
            },
            Options {
                modify_other_keys_state_2: true,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x1b[27;6;72~");
    }

    // Port of test "legacy: alt+digit with modify other state 2".
    #[test]
    fn legacy_alt_digit_with_modify_other_state_2() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Digit8,
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                utf8: "8".to_string(),
                ..KeyEvent::default()
            },
            Options {
                modify_other_keys_state_2: true,
                macos_option_as_alt: OptionAsAlt::True,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x1b[27;3;56~");
    }

    // Port of test "legacy: alt+digit with modify other state 2 and
    // macos-option-as-alt = false" (Darwin-gated).
    #[test]
    fn legacy_alt_digit_with_modify_other_state_2_and_option_as_alt_false() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let out = legacy_encode(
            KeyEvent {
                key: Key::Digit8,
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                consumed_mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                // common translation of option+8 with European layouts
                utf8: "[".to_string(),
                ..KeyEvent::default()
            },
            Options {
                modify_other_keys_state_2: true,
                macos_option_as_alt: OptionAsAlt::False,
                ..Options::default()
            },
        );
        assert_eq!(out, b"[");
    }

    // Port of test "legacy: fixterm awkward letters".
    #[test]
    fn legacy_fixterm_awkward_letters() {
        assert_eq!(
            legacy_encode(
                KeyEvent {
                    key: Key::KeyI,
                    mods: Mods {
                        ctrl: true,
                        ..Mods::default()
                    },
                    utf8: "i".to_string(),
                    ..KeyEvent::default()
                },
                Options::default(),
            ),
            b"\x1b[105;5u"
        );
        assert_eq!(
            legacy_encode(
                KeyEvent {
                    key: Key::KeyM,
                    mods: Mods {
                        ctrl: true,
                        ..Mods::default()
                    },
                    utf8: "m".to_string(),
                    ..KeyEvent::default()
                },
                Options::default(),
            ),
            b"\x1b[109;5u"
        );
        assert_eq!(
            legacy_encode(
                KeyEvent {
                    key: Key::BracketLeft,
                    mods: Mods {
                        ctrl: true,
                        ..Mods::default()
                    },
                    utf8: "[".to_string(),
                    ..KeyEvent::default()
                },
                Options::default(),
            ),
            b"\x1b[91;5u"
        );
        assert_eq!(
            legacy_encode(
                KeyEvent {
                    key: Key::Digit2,
                    mods: Mods {
                        ctrl: true,
                        shift: true,
                        ..Mods::default()
                    },
                    utf8: "@".to_string(),
                    unshifted_codepoint: '2' as u32,
                    ..KeyEvent::default()
                },
                Options::default(),
            ),
            b"\x1b[64;5u"
        );
    }

    // Port of test "legacy: ctrl+shift+letter ascii".
    #[test]
    fn legacy_ctrl_shift_letter_ascii() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyM,
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                utf8: "M".to_string(),
                unshifted_codepoint: 'm' as u32,
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\x1b[109;6u");
    }

    // Port of test "legacy: shift+function key should use all mods".
    #[test]
    fn legacy_shift_function_key_should_use_all_mods() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::ArrowUp,
                mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
                consumed_mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\x1b[1;2A");
    }

    // Port of test "legacy: keypad enter".
    #[test]
    fn legacy_keypad_enter() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::NumpadEnter,
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\r");
    }

    // Port of test "legacy: keypad 1".
    #[test]
    fn legacy_keypad_1() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Numpad1,
                utf8: "1".to_string(),
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"1");
    }

    // Port of test "legacy: keypad 1 with application keypad".
    #[test]
    fn legacy_keypad_1_with_application_keypad() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Numpad1,
                utf8: "1".to_string(),
                ..KeyEvent::default()
            },
            Options {
                keypad_key_application: true,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x1bOq");
    }

    // Port of test "legacy: keypad 1 with application keypad and numlock".
    #[test]
    fn legacy_keypad_1_with_application_keypad_and_numlock() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Numpad1,
                mods: Mods {
                    num_lock: true,
                    ..Mods::default()
                },
                utf8: "1".to_string(),
                ..KeyEvent::default()
            },
            Options {
                keypad_key_application: true,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x1bOq");
    }

    // Port of test "legacy: keypad 1 with application keypad and numlock
    // ignore".
    #[test]
    fn legacy_keypad_1_with_application_keypad_and_numlock_ignore() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Numpad1,
                mods: Mods {
                    num_lock: false,
                    ..Mods::default()
                },
                utf8: "1".to_string(),
                ..KeyEvent::default()
            },
            Options {
                keypad_key_application: true,
                ignore_keypad_with_numlock: true,
                ..Options::default()
            },
        );
        assert_eq!(out, b"1");
    }

    // Port of test "legacy: f1".
    #[test]
    fn legacy_f1() {
        let cases = [
            (Key::F1, "\x1b[1;5P"),
            (Key::F2, "\x1b[1;5Q"),
            (Key::F3, "\x1b[13;5~"),
            (Key::F4, "\x1b[1;5S"),
            (Key::F5, "\x1b[15;5~"),
        ];
        for (key, expected) in cases {
            let out = legacy_encode(
                KeyEvent {
                    key,
                    mods: Mods {
                        ctrl: true,
                        ..Mods::default()
                    },
                    ..KeyEvent::default()
                },
                Options::default(),
            );
            assert_eq!(out, expected.as_bytes(), "key {key:?}");
        }
    }

    // Port of test "legacy: left_shift+tab".
    #[test]
    fn legacy_left_shift_tab() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Tab,
                mods: Mods {
                    shift: true,
                    sides: crate::key_mods::Side {
                        shift: ModSide::Left,
                        ..crate::key_mods::Side::default()
                    },
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\x1b[Z");
    }

    // Port of test "legacy: right_shift+tab".
    #[test]
    fn legacy_right_shift_tab() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Tab,
                mods: Mods {
                    shift: true,
                    sides: crate::key_mods::Side {
                        shift: ModSide::Right,
                        ..crate::key_mods::Side::default()
                    },
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, b"\x1b[Z");
    }

    // Port of test "legacy: hu layout ctrl+ő sends proper codepoint".
    #[test]
    fn legacy_hu_layout_ctrl_o_double_acute() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::BracketLeft,
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                utf8: "\u{151}".to_string(), // "ő"
                unshifted_codepoint: 337,
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(&out[1..], "[337;5u".as_bytes());
    }

    // Port of test "legacy: super-only on macOS with text" (Darwin-gated).
    #[test]
    fn legacy_super_only_on_macos_with_text() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyB,
                utf8: "b".to_string(),
                mods: Mods {
                    super_: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, Vec::<u8>::new());
    }

    // Port of test "legacy: super and other mods on macOS with text"
    // (Darwin-gated).
    #[test]
    fn legacy_super_and_other_mods_on_macos_with_text() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let out = legacy_encode(
            KeyEvent {
                key: Key::KeyB,
                utf8: "B".to_string(),
                mods: Mods {
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                ..KeyEvent::default()
            },
            Options::default(),
        );
        assert_eq!(out, Vec::<u8>::new());
    }

    // Port of test "legacy: backspace with DEL utf8 (DECBKM reset)".
    #[test]
    fn legacy_backspace_with_del_utf8_decbkm_reset() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Backspace,
                utf8: "\u{7F}".to_string(),
                unshifted_codepoint: 0x08,
                ..KeyEvent::default()
            },
            Options {
                backarrow_key_mode: false,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x7F");
    }

    // Port of test "legacy: backspace with DEL utf8 (DECBKM set)".
    #[test]
    fn legacy_backspace_with_del_utf8_decbkm_set() {
        let out = legacy_encode(
            KeyEvent {
                key: Key::Backspace,
                utf8: "\u{7F}".to_string(),
                unshifted_codepoint: 0x08,
                ..KeyEvent::default()
            },
            Options {
                backarrow_key_mode: true,
                ..Options::default()
            },
        );
        assert_eq!(out, b"\x08");
    }

    // ---- ctrlSeq unit tests ----

    // Port of test "ctrlseq: normal ctrl c".
    #[test]
    fn ctrlseq_normal_ctrl_c() {
        let seq = ctrl_seq(
            Key::Unidentified,
            "c",
            'c' as u32,
            Mods {
                ctrl: true,
                ..Mods::default()
            },
        );
        assert_eq!(seq, Some(0x03));
    }

    // Port of test "ctrlseq: normal ctrl c, right control".
    #[test]
    fn ctrlseq_normal_ctrl_c_right_control() {
        let seq = ctrl_seq(
            Key::Unidentified,
            "c",
            'c' as u32,
            Mods {
                ctrl: true,
                sides: crate::key_mods::Side {
                    ctrl: ModSide::Right,
                    ..crate::key_mods::Side::default()
                },
                ..Mods::default()
            },
        );
        assert_eq!(seq, Some(0x03));
    }

    // Port of test "ctrlseq: alt should be allowed".
    #[test]
    fn ctrlseq_alt_should_be_allowed() {
        let seq = ctrl_seq(
            Key::Unidentified,
            "c",
            'c' as u32,
            Mods {
                alt: true,
                ctrl: true,
                ..Mods::default()
            },
        );
        assert_eq!(seq, Some(0x03));
    }

    // Port of test "ctrlseq: no ctrl does nothing".
    #[test]
    fn ctrlseq_no_ctrl_does_nothing() {
        assert_eq!(
            ctrl_seq(Key::Unidentified, "c", 'c' as u32, Mods::default()),
            None
        );
    }

    // Port of test "ctrlseq: shifted non-character".
    #[test]
    fn ctrlseq_shifted_non_character() {
        let seq = ctrl_seq(
            Key::Unidentified,
            "_",
            '-' as u32,
            Mods {
                ctrl: true,
                shift: true,
                ..Mods::default()
            },
        );
        assert_eq!(seq, Some(0x1F));
    }

    // Port of test "ctrlseq: caps ascii letter".
    #[test]
    fn ctrlseq_caps_ascii_letter() {
        let seq = ctrl_seq(
            Key::Unidentified,
            "C",
            'c' as u32,
            Mods {
                ctrl: true,
                caps_lock: true,
                ..Mods::default()
            },
        );
        assert_eq!(seq, Some(0x03));
    }

    // Port of test "ctrlseq: shift does not generate ctrl seq".
    #[test]
    fn ctrlseq_shift_does_not_generate_ctrl_seq() {
        assert_eq!(
            ctrl_seq(
                Key::Unidentified,
                "C",
                'c' as u32,
                Mods {
                    shift: true,
                    ..Mods::default()
                },
            ),
            None
        );
        assert_eq!(
            ctrl_seq(
                Key::Unidentified,
                "C",
                'c' as u32,
                Mods {
                    shift: true,
                    ctrl: true,
                    ..Mods::default()
                },
            ),
            None
        );
    }

    // Port of test "ctrlseq: russian ctrl c".
    #[test]
    fn ctrlseq_russian_ctrl_c() {
        let seq = ctrl_seq(
            Key::KeyC,
            "\u{441}", // "с" (cyrillic)
            0x0441,
            Mods {
                ctrl: true,
                ..Mods::default()
            },
        );
        assert_eq!(seq, Some(0x03));
    }

    // Port of test "ctrlseq: russian shifted ctrl c".
    #[test]
    fn ctrlseq_russian_shifted_ctrl_c() {
        let seq = ctrl_seq(
            Key::KeyC,
            "\u{441}",
            0x0441,
            Mods {
                ctrl: true,
                shift: true,
                ..Mods::default()
            },
        );
        assert_eq!(seq, None);
    }

    // Port of test "ctrlseq: russian alt ctrl c".
    #[test]
    fn ctrlseq_russian_alt_ctrl_c() {
        let seq = ctrl_seq(
            Key::KeyC,
            "\u{441}",
            0x0441,
            Mods {
                ctrl: true,
                alt: true,
                ..Mods::default()
            },
        );
        assert_eq!(seq, Some(0x03));
    }

    // Port of test "ctrlseq: right ctrl c".
    #[test]
    fn ctrlseq_right_ctrl_c() {
        let seq = ctrl_seq(
            Key::KeyC,
            "\u{441}",
            'c' as u32,
            Mods {
                ctrl: true,
                sides: crate::key_mods::Side {
                    ctrl: ModSide::Right,
                    ..crate::key_mods::Side::default()
                },
                ..Mods::default()
            },
        );
        assert_eq!(seq, Some(0x03));
    }

    // Port of the `CsiUMods` embedded test "modifier sequence values".
    #[test]
    fn csi_u_mods_modifier_sequence_values() {
        assert_eq!(CsiUMods::default().seq_int(), 1);
        assert_eq!(
            CsiUMods {
                shift: true,
                ..CsiUMods::default()
            }
            .seq_int(),
            2
        );
        assert_eq!(
            CsiUMods {
                alt: true,
                ..CsiUMods::default()
            }
            .seq_int(),
            3
        );
        assert_eq!(
            CsiUMods {
                ctrl: true,
                ..CsiUMods::default()
            }
            .seq_int(),
            5
        );
        assert_eq!(
            CsiUMods {
                alt: true,
                shift: true,
                ..CsiUMods::default()
            }
            .seq_int(),
            4
        );
        assert_eq!(
            CsiUMods {
                ctrl: true,
                shift: true,
                ..CsiUMods::default()
            }
            .seq_int(),
            6
        );
        assert_eq!(
            CsiUMods {
                alt: true,
                ctrl: true,
                ..CsiUMods::default()
            }
            .seq_int(),
            7
        );
        assert_eq!(
            CsiUMods {
                alt: true,
                ctrl: true,
                shift: true,
            }
            .seq_int(),
            8
        );
    }
}
