//! Terminal modes: the enum of supported ANSI/DEC modes, packed state
//! storage, save/restore semantics, and DECRPM report encoding. Port of
//! `src/terminal/modes.zig` (386 lines, 12 inline tests).
//!
//! Ghostty generates `Mode` and its packed bool-storage struct at comptime
//! from a single table of `(name, value, ansi, default)` entries, so the
//! enum, the storage layout, and `modeFromInt` can never drift out of sync.
//! Rust has no equivalent to comptime struct-field generation, so this port
//! hand-writes [`Mode`] as a fieldless enum and [`ModeValues`] as a plain
//! struct of `bool` fields with the same names, both expanded once from a
//! single macro invocation ([`define_modes!`]) over the entry list — the
//! practical Rust analogue of the Zig comptime table: the entry list is
//! still the single source of truth, just expanded by `macro_rules!` instead
//! of `@Type()`.
//!
//! Representational difference from Zig worth flagging: Zig packs the mode's
//! "ansi" bit into the *enum tag* itself (`Mode`'s backing `u16` is bit-cast
//! to/from `ModeTag{ value: u15, ansi: bool }`), so `@intFromEnum(mode)`
//! round-trips through `ModeTag`. Rust enums don't support arbitrary
//! bit-cast tag layouts like that, so [`Mode`] is a plain fieldless enum and
//! [`ModeTag`] is instead constructed explicitly from a `Mode` via
//! [`ModeTag::from_mode`] (a lookup, not a bit-cast) — same external
//! behavior, different mechanism.

/// A single entry in the mode table: mirrors one `ModeEntry` in
/// `modes.zig`'s `entries` array. Declared as a macro so [`Mode`] and
/// [`ModeValues`] expand from exactly the same list (see module docs).
///
/// UpperCamel variant, snake_case field, snake_case name string, value, ansi, default
macro_rules! define_modes {
    ($( ($variant:ident, $field:ident, $name:literal, $value:expr, $ansi:expr, $default:expr) ),* $(,)?) => {
        /// An enum of the available modes. Port of `modes.zig` `Mode`.
        ///
        /// Variant names are UpperCamelCase renderings of the Zig
        /// snake_case field names (e.g. `cursor_keys` -> `CursorKeys`);
        /// [`Mode::name`] returns the original snake_case string for
        /// diagnostics/parity with `@tagName`.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum Mode {
            $($variant),*
        }

        impl Mode {
            /// The snake_case name, matching Zig's `@tagName(mode)`.
            pub const fn name(self) -> &'static str {
                match self {
                    $(Mode::$variant => $name),*
                }
            }

            /// All modes, in table order. Mirrors iterating `entries` in Zig;
            /// used by the formatter's `modes` extra and by tests.
            pub const ALL: &'static [Mode] = &[$(Mode::$variant),*];
        }

        /// A packed struct of all the settable modes. Port of `modes.zig`
        /// `ModePacked`. This shouldn't be used directly but rather through
        /// [`ModeState`].
        ///
        /// Zig packs these into a `packed struct` of exactly one bit per
        /// mode (asserted to be 8 bytes = 64 bits total, see
        /// `ModeState`'s inline `test {}` block, ported as
        /// [`tests::mode_packed_bit_budget`]). We use plain `bool` fields
        /// instead: correctness of set/get is unaffected by the storage
        /// width, only the byte size differs (this struct is larger than
        /// 8 bytes), which is why that Zig test is ported as a "budget"
        /// check (documenting a size divergence) rather than an assertion
        /// on `size_of::<ModeValues>()`.
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
        pub struct ModeValues {
            $(pub $field: bool),*
        }

        impl ModeValues {
            /// Build the default values, from each entry's `default`.
            const fn defaults() -> Self {
                Self {
                    $($field: $default),*
                }
            }

            fn get(&self, mode: Mode) -> bool {
                match mode {
                    $(Mode::$variant => self.$field),*
                }
            }

            fn set(&mut self, mode: Mode, value: bool) {
                match mode {
                    $(Mode::$variant => self.$field = value),*
                }
            }
        }

        /// Look up a [`Mode`] by its `(value, ansi)` pair. Port of
        /// `modes.zig` `modeFromInt`.
        pub fn mode_from_int(value: u16, ansi: bool) -> Option<Mode> {
            $(
                if $value == value && $ansi == ansi {
                    return Some(Mode::$variant);
                }
            )*
            None
        }

        /// The `(value, ansi)` pair for a mode. Used by [`ModeTag::from_mode`].
        fn mode_value_ansi(mode: Mode) -> (u16, bool) {
            match mode {
                $(Mode::$variant => ($value, $ansi)),*
            }
        }
    };
}

define_modes! {
    // ANSI
    (DisableKeyboard, disable_keyboard, "disable_keyboard", 2, true, false), // KAM
    (Insert, insert, "insert", 4, true, false),
    (SendReceiveMode, send_receive_mode, "send_receive_mode", 12, true, true), // SRM
    (Linefeed, linefeed, "linefeed", 20, true, false),

    // DEC
    (CursorKeys, cursor_keys, "cursor_keys", 1, false, false), // DECCKM
    // Zig field/tag name is the identifier `132_column`, which isn't a legal
    // Rust identifier (leading digit); named `column_132` here instead. The
    // wire-format name string is kept as the original "132_column" since
    // that's what `Mode::name()`/diagnostics should say.
    (Column132, column_132, "132_column", 3, false, false),
    (SlowScroll, slow_scroll, "slow_scroll", 4, false, false),
    (ReverseColors, reverse_colors, "reverse_colors", 5, false, false),
    (Origin, origin, "origin", 6, false, false),
    (Wraparound, wraparound, "wraparound", 7, false, true),
    (Autorepeat, autorepeat, "autorepeat", 8, false, false),
    (MouseEventX10, mouse_event_x10, "mouse_event_x10", 9, false, false),
    (CursorBlinking, cursor_blinking, "cursor_blinking", 12, false, false),
    (CursorVisible, cursor_visible, "cursor_visible", 25, false, true),
    (EnableMode3, enable_mode_3, "enable_mode_3", 40, false, false),
    (ReverseWrap, reverse_wrap, "reverse_wrap", 45, false, false),
    (AltScreenLegacy, alt_screen_legacy, "alt_screen_legacy", 47, false, false),
    (KeypadKeys, keypad_keys, "keypad_keys", 66, false, false),
    // DEC Backarrow Key Mode (DECBKM)
    // See https://vt100.net/dec/ek-vt3xx-tp-002.pdf page 170
    // If `false` (the default), `backspace` emits 0x7f
    // If `true`, `backspace` emits 0x08
    (BackarrowKeyMode, backarrow_key_mode, "backarrow_key_mode", 67, false, false),
    (EnableLeftAndRightMargin, enable_left_and_right_margin, "enable_left_and_right_margin", 69, false, false),
    (MouseEventNormal, mouse_event_normal, "mouse_event_normal", 1000, false, false),
    (MouseEventButton, mouse_event_button, "mouse_event_button", 1002, false, false),
    (MouseEventAny, mouse_event_any, "mouse_event_any", 1003, false, false),
    (FocusEvent, focus_event, "focus_event", 1004, false, false),
    (MouseFormatUtf8, mouse_format_utf8, "mouse_format_utf8", 1005, false, false),
    (MouseFormatSgr, mouse_format_sgr, "mouse_format_sgr", 1006, false, false),
    (MouseAlternateScroll, mouse_alternate_scroll, "mouse_alternate_scroll", 1007, false, true),
    (MouseFormatUrxvt, mouse_format_urxvt, "mouse_format_urxvt", 1015, false, false),
    (MouseFormatSgrPixels, mouse_format_sgr_pixels, "mouse_format_sgr_pixels", 1016, false, false),
    (IgnoreKeypadWithNumlock, ignore_keypad_with_numlock, "ignore_keypad_with_numlock", 1035, false, true),
    (AltEscPrefix, alt_esc_prefix, "alt_esc_prefix", 1036, false, true),
    (AltSendsEscape, alt_sends_escape, "alt_sends_escape", 1039, false, false),
    (ReverseWrapExtended, reverse_wrap_extended, "reverse_wrap_extended", 1045, false, false),
    (AltScreen, alt_screen, "alt_screen", 1047, false, false),
    (SaveCursor, save_cursor, "save_cursor", 1048, false, false),
    (AltScreenSaveCursorClearEnter, alt_screen_save_cursor_clear_enter, "alt_screen_save_cursor_clear_enter", 1049, false, false),
    (BracketedPaste, bracketed_paste, "bracketed_paste", 2004, false, false),
    (SynchronizedOutput, synchronized_output, "synchronized_output", 2026, false, false),
    (GraphemeCluster, grapheme_cluster, "grapheme_cluster", 2027, false, false),
    (ReportColorScheme, report_color_scheme, "report_color_scheme", 2031, false, false),
    (InBandSizeReports, in_band_size_reports, "in_band_size_reports", 2048, false, false),
}

/// The tag identifying a mode by its raw `(value, ansi)` pair, independent of
/// whether the value is a recognized [`Mode`]. Port of `modes.zig`
/// `ModeTag`.
///
/// Zig bit-casts this to/from `Mode`'s backing `u16`; see the module docs
/// for why this port keeps `ModeTag` as a plain struct built by lookup
/// instead ([`ModeTag::from_mode`]) rather than a transmute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModeTag {
    pub value: u16,
    pub ansi: bool,
}

impl ModeTag {
    /// Build a tag from a known mode. Port of `modes.zig` `ModeTag.fromMode`.
    pub fn from_mode(mode: Mode) -> Self {
        let (value, ansi) = mode_value_ansi(mode);
        Self { value, ansi }
    }
}

/// A struct that maintains the state of all the settable modes. Port of
/// `modes.zig` `ModeState`.
#[derive(Debug, Clone, Copy, Default)]
pub struct ModeState {
    /// The values of the current modes.
    values: ModeValues,
    /// The saved values. We only allow saving each mode once, in line with
    /// other terminals that implement XTSAVE/XTRESTORE.
    saved: ModeValues,
    /// The default values, used to reset the modes.
    default: ModeValues,
}

impl ModeState {
    /// Construct with all modes at their documented defaults. There is no
    /// direct Zig equivalent (Zig's `ModeState{}` already default-initializes
    /// each packed field via `default_value_ptr`); this is the constructor
    /// callers should use instead of `ModeState::default()`, whose `values`
    /// would otherwise be all-false rather than the documented per-mode
    /// defaults.
    pub fn new() -> Self {
        let defaults = ModeValues::defaults();
        Self {
            values: defaults,
            // Upstream inits `values`, `saved`, and `default` all to the same
            // `ModePacked = .{}` (per-mode defaults) — NOT an all-false struct.
            // So restoring a mode that was never saved yields its default, e.g.
            // `CSI ? 25 r` leaves cursor_visible = true. Using the derived
            // (all-false) `Default` here would wrongly restore such modes to
            // false. (Found via the ghostty AFL corpus differential replay.)
            saved: defaults,
            default: defaults,
        }
    }

    /// Reset the modes to their default values. This also clears the saved
    /// state. Port of `ModeState.reset`.
    pub fn reset(&mut self) {
        self.values = self.default;
        // Match upstream `self.saved = .{}`: reset saved to the per-mode
        // defaults, not an all-false struct (see `new`).
        self.saved = self.default;
    }

    /// Set a mode to a value. Port of `ModeState.set`.
    pub fn set(&mut self, mode: Mode, value: bool) {
        self.values.set(mode, value);
    }

    /// Get the value of a mode. Port of `ModeState.get`.
    pub fn get(&self, mode: Mode) -> bool {
        self.values.get(mode)
    }

    /// Get a mode's default value. Mirrors Zig's access of
    /// `self.terminal.modes.default` in the formatter's `modes` extra, which
    /// emits only modes that differ from their default.
    pub fn default_value(&self, mode: Mode) -> bool {
        self.default.get(mode)
    }

    /// Save the state of the given mode; see [`ModeState::restore`]. Port of
    /// `ModeState.save`.
    pub fn save(&mut self, mode: Mode) {
        let v = self.values.get(mode);
        self.saved.set(mode, v);
    }

    /// Restore the previously-saved state of a mode, returning the restored
    /// value. Port of `ModeState.restore`.
    pub fn restore(&mut self, mode: Mode) -> bool {
        let v = self.saved.get(mode);
        self.values.set(mode, v);
        v
    }

    /// Return a DECRPM report for the given mode tag. If the tag does not
    /// correspond to a known mode, the report state is
    /// [`ReportState::NotRecognized`]. Port of `ModeState.getReport`.
    pub fn get_report(&self, tag: ModeTag) -> Report {
        match mode_from_int(tag.value, tag.ansi) {
            None => Report {
                tag,
                state: ReportState::NotRecognized,
            },
            Some(mode) => Report {
                tag,
                state: if self.get(mode) {
                    ReportState::Set
                } else {
                    ReportState::Reset
                },
            },
        }
    }
}

/// A DECRPM mode report response. Port of `modes.zig` `Report`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Report {
    pub tag: ModeTag,
    pub state: ReportState,
}

/// The state of a mode as reported in a DECRPM response. Port of
/// `modes.zig` `Report.State`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ReportState {
    NotRecognized = 0,
    Set = 1,
    Reset = 2,
    PermanentlySet = 3,
    PermanentlyReset = 4,
}

impl Report {
    /// Encode the DECRPM report sequence: `ESC [ ? Ps ; Pm $ y` (ANSI modes
    /// omit the `?`). Port of `modes.zig` `Report.encode`.
    pub fn encode(&self, out: &mut String) {
        use std::fmt::Write as _;
        let _ = write!(
            out,
            "\x1B[{}{};{}$y",
            if self.tag.ansi { "" } else { "?" },
            self.tag.value,
            self.state as u8
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of modes.zig's inline `ModeState.test {}` block asserting
    // `@sizeOf(ModePacked) == 8`. Our `ModeValues` uses one `bool` (1 byte,
    // not 1 bit) per mode, so the byte size necessarily differs from Zig's
    // tightly packed bitset; we assert the field count instead (mode table
    // has 41 entries, matching `grep -c '.{ .name' modes.zig` exactly) so a
    // future accidental extra/removed field is caught, documenting the size
    // divergence rather than pretending to match it.
    #[test]
    fn mode_packed_bit_budget() {
        assert_eq!(Mode::ALL.len(), 41);
        assert_eq!(size_of::<ModeValues>(), Mode::ALL.len());
    }

    #[test]
    fn mode_from_int_known_and_unknown() {
        assert_eq!(mode_from_int(4, true), Some(Mode::Insert));
        assert_eq!(mode_from_int(9, true), None);
        assert_eq!(mode_from_int(9, false), Some(Mode::MouseEventX10));
        assert_eq!(mode_from_int(14, true), None);
    }

    #[test]
    fn mode_state_basic() {
        let mut state = ModeState::new();

        // Normal set/get
        assert!(!state.get(Mode::CursorKeys));
        state.set(Mode::CursorKeys, true);
        assert!(state.get(Mode::CursorKeys));

        // Save/restore
        state.save(Mode::CursorKeys);
        state.set(Mode::CursorKeys, false);
        assert!(!state.get(Mode::CursorKeys));
        assert!(state.restore(Mode::CursorKeys));
        assert!(state.get(Mode::CursorKeys));
    }

    #[test]
    fn get_report_known_dec_mode() {
        let state = ModeState::new();
        let report = state.get_report(ModeTag {
            value: 1,
            ansi: false,
        });
        assert_eq!(report.state, ReportState::Reset);
        assert!(!report.tag.ansi);
        assert_eq!(report.tag.value, 1);

        let mut state = ModeState::new();
        state.set(Mode::CursorKeys, true);
        let report2 = state.get_report(ModeTag {
            value: 1,
            ansi: false,
        });
        assert_eq!(report2.state, ReportState::Set);
    }

    #[test]
    fn get_report_known_ansi_mode() {
        let mut state = ModeState::new();
        state.set(Mode::Insert, true);
        let report = state.get_report(ModeTag {
            value: 4,
            ansi: true,
        });
        assert_eq!(report.state, ReportState::Set);
        assert!(report.tag.ansi);
    }

    #[test]
    fn get_report_unknown_mode() {
        let state = ModeState::new();
        let report = state.get_report(ModeTag {
            value: 9999,
            ansi: false,
        });
        assert_eq!(report.state, ReportState::NotRecognized);
    }

    #[test]
    fn report_encode_dec_mode_set() {
        let mut buf = String::new();
        let report = Report {
            tag: ModeTag {
                value: 1,
                ansi: false,
            },
            state: ReportState::Set,
        };
        report.encode(&mut buf);
        assert_eq!(buf, "\x1B[?1;1$y");
    }

    #[test]
    fn report_encode_dec_mode_reset() {
        let mut buf = String::new();
        let report = Report {
            tag: ModeTag {
                value: 1,
                ansi: false,
            },
            state: ReportState::Reset,
        };
        report.encode(&mut buf);
        assert_eq!(buf, "\x1B[?1;2$y");
    }

    #[test]
    fn report_encode_ansi_mode() {
        let mut buf = String::new();
        let report = Report {
            tag: ModeTag {
                value: 4,
                ansi: true,
            },
            state: ReportState::Set,
        };
        report.encode(&mut buf);
        assert_eq!(buf, "\x1B[4;1$y");
    }

    #[test]
    fn report_encode_not_recognized() {
        let mut buf = String::new();
        let report = Report {
            tag: ModeTag {
                value: 9999,
                ansi: false,
            },
            state: ReportState::NotRecognized,
        };
        report.encode(&mut buf);
        assert_eq!(buf, "\x1B[?9999;0$y");
    }

    // Not a direct port (ModeTag.test "order" relies on Zig's bit-cast,
    // which this port doesn't use — see module docs) but pins the
    // equivalent external behavior: from_mode round-trips value/ansi.
    #[test]
    fn mode_tag_from_mode_round_trips() {
        let tag = ModeTag::from_mode(Mode::CursorKeys);
        assert_eq!(tag.value, 1);
        assert!(!tag.ansi);

        let tag2 = ModeTag::from_mode(Mode::Insert);
        assert_eq!(tag2.value, 4);
        assert!(tag2.ansi);
    }

    // Sanity beyond the ported set: every table entry is reachable via
    // mode_from_int and Mode::ALL is non-empty (guards the macro expansion).
    #[test]
    fn all_modes_round_trip_through_mode_from_int() {
        for &mode in Mode::ALL {
            let (value, ansi) = mode_value_ansi(mode);
            assert_eq!(mode_from_int(value, ansi), Some(mode));
        }
    }

    // Restoring a mode that was never saved must yield its DEFAULT, not `false`.
    // Upstream inits `saved` to the per-mode defaults (`ModePacked = .{}`), so
    // e.g. `CSI ? 25 r` (XTRESTORE of cursor_visible, default true) with no
    // prior save leaves the mode at true. Found via the ghostty AFL corpus
    // differential replay.
    #[test]
    fn restore_unsaved_mode_yields_its_default() {
        let mut m = ModeState::new();
        // CursorVisible defaults to true; nothing saved yet.
        assert!(m.get(Mode::CursorVisible));
        m.set(Mode::CursorVisible, false);
        assert!(!m.get(Mode::CursorVisible));
        // Restore with no matching save -> back to the default (true), not false.
        assert!(m.restore(Mode::CursorVisible));
        assert!(m.get(Mode::CursorVisible));

        // reset() must also restore `saved` to the defaults, not all-false.
        m.set(Mode::CursorVisible, false);
        m.reset();
        assert!(m.restore(Mode::CursorVisible));
    }
}
