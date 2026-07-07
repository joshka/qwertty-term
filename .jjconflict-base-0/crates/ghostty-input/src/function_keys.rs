//! PC-style function key table (port of `input/function_keys.zig`).
//!
//! This is the list of "PC style function keys" that xterm supports for the
//! legacy keyboard protocols. These always take priority since even the most
//! modern keyboard protocols still are backwards compatible with regard to
//! these sequences.
//!
//! Based on a variety of sources cross-referenced but mostly based on foot's
//! keymap.h: <https://codeberg.org/dnkl/foot/src/branch/master/keymap.h>
//!
//! ## Scope note
//!
//! This module ports only the function-key *table* (the data model). The
//! legacy (non-kitty) key encoder that consumes this table is out of scope
//! for this chunk and will be built later; the table itself is a standalone
//! data model and is ported completely and faithfully here so it's ready to
//! use when that encoder is written.
//!
//! ## Design: `entries_for` instead of a static `EnumArray`
//!
//! The Zig source builds a `std.EnumArray(Key, []const Entry)` at comptime,
//! using helper functions (`pcStyle`, `kpKeys`, `kpDefault`, `cursorKey`)
//! that call `std.fmt.comptimePrint` to bake ~15 modifier-coded escape
//! sequences (codes 2..=16) into `'static` strings for each of ~40 keys, all
//! resolved at compile time.
//!
//! Rust has no equivalent comptime string formatting: `format!` is a runtime
//! operation, and there's no ergonomic way to build ~40 arrays of `'static
//! str`s out of it without leaking memory or hand-writing a huge static
//! table literal. Since this crate has no `alloc`-free requirement and this
//! table is queried at most once per keypress (not in a hot loop), we adopt
//! a different, idiomatic-for-Rust design instead:
//!
//! - [`Entry::sequence`] is an owned `String` rather than `&'static str`.
//! - [`entries_for`] computes and returns a `Vec<Entry>` for a given [`Key`]
//!   on demand, rather than indexing into a giant static table built once.
//!
//! This trades a small per-lookup allocation for a much simpler port that
//! doesn't fight Rust's const-eval model. The helper functions `pc_style`,
//! `kp_default`, `kp_keys`, and `cursor_key` are ported as plain functions
//! operating on runtime `Vec<Entry>`/`String`s instead of Zig comptime
//! blocks.

use crate::key::Key;
use crate::key_mods::Mods;

/// The state required for cursor key mode. Port of `function_keys.CursorMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorMode {
    #[default]
    Any,
    Normal,
    Application,
}

/// The state required for keypad mode. Port of `function_keys.KeypadMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeypadMode {
    #[default]
    Any,
    Normal,
    Application,
}

/// A bit confusing so I'll document this one: this is the "modify other
/// keys" setting. We only change behavior for "set_other" which is
/// `ESC [ > 4; 2 m`. So this can be "any" which means we don't care what's
/// going on. Or it can be "set" which means modify keys must be set EXCEPT
/// FOR "other keys" mode, and "set_other" which means modify keys must be
/// set to "other keys" mode.
///
/// See: <https://invisible-island.net/xterm/modified-keys.html>
///
/// Port of `function_keys.ModifyKeys`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ModifyKeys {
    #[default]
    Any,
    Set,
    SetOther,
}

/// A single entry in the table of keys. Port of `function_keys.Entry`.
///
/// The Zig `sequence`/`sequence_decbkm` fields are `[]const u8` (`'static`
/// string literals baked in at comptime); this port uses owned `String`s
/// instead, since [`entries_for`] builds these on demand at runtime rather
/// than as a `'static` table (see the module doc comment for why).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The exact set of modifiers that must be active for this entry to
    /// match. If `mods_empty_is_any` is true then empty mods means any set
    /// of mods can match. Otherwise, empty mods means no mods must be
    /// active.
    pub mods: Mods,
    pub mods_empty_is_any: bool,

    /// The state required for cursor/keypad mode.
    pub cursor: CursorMode,
    pub keypad: KeypadMode,

    /// Whether or not this entry should be used.
    pub modify_other_keys: ModifyKeys,

    /// The sequence to send to the pty if this entry matches.
    pub sequence: String,

    /// Sequence to send to the PTY if DECBKM is set.
    pub sequence_decbkm: Option<String>,
}

impl Default for Entry {
    fn default() -> Self {
        Entry {
            mods: Mods::default(),
            mods_empty_is_any: true,
            cursor: CursorMode::Any,
            keypad: KeypadMode::Any,
            modify_other_keys: ModifyKeys::Any,
            sequence: String::new(),
            sequence_decbkm: None,
        }
    }
}

/// Convenience: an `Entry` with only `sequence` set, everything else default.
fn seq(sequence: &str) -> Entry {
    Entry {
        sequence: sequence.to_string(),
        ..Entry::default()
    }
}

/// The list of modifier combinations for modify-other-key sequences. The
/// mode value is index + 2. Port of `function_keys.modifiers`.
pub fn modifiers() -> [Mods; 15] {
    [
        Mods {
            shift: true,
            ..Mods::default()
        },
        Mods {
            alt: true,
            ..Mods::default()
        },
        Mods {
            shift: true,
            alt: true,
            ..Mods::default()
        },
        Mods {
            ctrl: true,
            ..Mods::default()
        },
        Mods {
            shift: true,
            ctrl: true,
            ..Mods::default()
        },
        Mods {
            alt: true,
            ctrl: true,
            ..Mods::default()
        },
        Mods {
            shift: true,
            alt: true,
            ctrl: true,
            ..Mods::default()
        },
        Mods {
            super_: true,
            ..Mods::default()
        },
        Mods {
            shift: true,
            super_: true,
            ..Mods::default()
        },
        Mods {
            alt: true,
            super_: true,
            ..Mods::default()
        },
        Mods {
            shift: true,
            alt: true,
            super_: true,
            ..Mods::default()
        },
        Mods {
            ctrl: true,
            super_: true,
            ..Mods::default()
        },
        Mods {
            shift: true,
            ctrl: true,
            super_: true,
            ..Mods::default()
        },
        Mods {
            alt: true,
            ctrl: true,
            super_: true,
            ..Mods::default()
        },
        Mods {
            shift: true,
            alt: true,
            ctrl: true,
            super_: true,
            ..Mods::default()
        },
    ]
}

/// Constructs a set of pc-style function key entries using the given
/// formatter. `format_code` is called with each modifier code in `2..=16`
/// (matching `modifiers()[i]`, code = index + 2) and must return the escape
/// sequence for that code, e.g. `|code| format!("\x1b[1;{code}A")` for the
/// arrow-up family.
///
/// Port of `function_keys.pcStyle`, which used
/// `std.fmt.comptimePrint(fmt, .{code})` at comptime; this is the runtime
/// `format!`-based equivalent (see module doc comment for why this can't
/// stay comptime in Rust).
fn pc_style(format_code: impl Fn(u32) -> String) -> Vec<Entry> {
    modifiers()
        .into_iter()
        .enumerate()
        .map(|(i, mods)| Entry {
            mods,
            sequence: format_code((i + 2) as u32),
            ..Entry::default()
        })
        .collect()
}

/// Returns the default keypad application mode entry. Port of
/// `function_keys.kpDefault`.
fn kp_default(suffix: &str) -> Vec<Entry> {
    vec![Entry {
        mods_empty_is_any: false,
        keypad: KeypadMode::Application,
        sequence: format!("\x1bO{suffix}"),
        ..Entry::default()
    }]
}

/// Returns the entries for a keypad key. The suffix is the final character
/// of the sent sequence, such as `"r"` for numpad_2.
///
/// Port of `function_keys.kpKeys`.
fn kp_keys(suffix: &str) -> Vec<Entry> {
    let mut pc = pc_style(|code| format!("\x1bO{code}{suffix}"));
    for entry in &mut pc {
        entry.keypad = KeypadMode::Application;
    }
    let mut result = kp_default(suffix);
    result.extend(pc);
    result
}

/// Returns entries that are dependent on cursor key settings. Port of
/// `function_keys.cursorKey`.
fn cursor_key(normal: &str, application: &str) -> Vec<Entry> {
    vec![
        Entry {
            cursor: CursorMode::Normal,
            sequence: normal.to_string(),
            ..Entry::default()
        },
        Entry {
            cursor: CursorMode::Application,
            sequence: application.to_string(),
            ..Entry::default()
        },
    ]
}

/// Returns the table entries for the given key, in match-priority order (the
/// caller should try entries in order and use the first match). Returns an
/// empty `Vec` for keys with no PC-style function key entries.
///
/// Port of the Zig `function_keys.keys` `EnumArray`, built here as a
/// function computed on demand rather than a static table (see module doc
/// comment for rationale). All ~40 `result.set(...)` calls from the Zig
/// source are ported below, in the same order.
pub fn entries_for(key: Key) -> Vec<Entry> {
    match key {
        Key::ArrowUp => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}A"));
            v.extend(cursor_key("\x1b[A", "\x1bOA"));
            v
        }
        Key::ArrowDown => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}B"));
            v.extend(cursor_key("\x1b[B", "\x1bOB"));
            v
        }
        Key::ArrowRight => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}C"));
            v.extend(cursor_key("\x1b[C", "\x1bOC"));
            v
        }
        Key::ArrowLeft => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}D"));
            v.extend(cursor_key("\x1b[D", "\x1bOD"));
            v
        }
        Key::Home => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}H"));
            v.extend(cursor_key("\x1b[H", "\x1bOH"));
            v
        }
        Key::End => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}F"));
            v.extend(cursor_key("\x1b[F", "\x1bOF"));
            v
        }
        Key::Insert => {
            let mut v = pc_style(|code| format!("\x1b[2;{code}~"));
            v.push(seq("\x1b[2~"));
            v
        }
        Key::Delete => {
            let mut v = pc_style(|code| format!("\x1b[3;{code}~"));
            v.push(seq("\x1b[3~"));
            v
        }
        Key::PageUp => {
            let mut v = pc_style(|code| format!("\x1b[5;{code}~"));
            v.push(seq("\x1b[5~"));
            v
        }
        Key::PageDown => {
            let mut v = pc_style(|code| format!("\x1b[6;{code}~"));
            v.push(seq("\x1b[6~"));
            v
        }

        // Function Keys. todo: f13-f35 but we need to add to input.Key
        Key::F1 => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}P"));
            v.push(seq("\x1bOP"));
            v
        }
        Key::F2 => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}Q"));
            v.push(seq("\x1bOQ"));
            v
        }
        Key::F3 => {
            let mut v = pc_style(|code| format!("\x1b[13;{code}~"));
            v.push(seq("\x1bOR"));
            v
        }
        Key::F4 => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}S"));
            v.push(seq("\x1bOS"));
            v
        }
        Key::F5 => {
            let mut v = pc_style(|code| format!("\x1b[15;{code}~"));
            v.push(seq("\x1b[15~"));
            v
        }
        Key::F6 => {
            let mut v = pc_style(|code| format!("\x1b[17;{code}~"));
            v.push(seq("\x1b[17~"));
            v
        }
        Key::F7 => {
            let mut v = pc_style(|code| format!("\x1b[18;{code}~"));
            v.push(seq("\x1b[18~"));
            v
        }
        Key::F8 => {
            let mut v = pc_style(|code| format!("\x1b[19;{code}~"));
            v.push(seq("\x1b[19~"));
            v
        }
        Key::F9 => {
            let mut v = pc_style(|code| format!("\x1b[20;{code}~"));
            v.push(seq("\x1b[20~"));
            v
        }
        Key::F10 => {
            let mut v = pc_style(|code| format!("\x1b[21;{code}~"));
            v.push(seq("\x1b[21~"));
            v
        }
        Key::F11 => {
            let mut v = pc_style(|code| format!("\x1b[23;{code}~"));
            v.push(seq("\x1b[23~"));
            v
        }
        Key::F12 => {
            let mut v = pc_style(|code| format!("\x1b[24;{code}~"));
            v.push(seq("\x1b[24~"));
            v
        }

        // Keypad keys
        Key::Numpad0 => kp_keys("p"),
        Key::Numpad1 => kp_keys("q"),
        Key::Numpad2 => kp_keys("r"),
        Key::Numpad3 => kp_keys("s"),
        Key::Numpad4 => kp_keys("t"),
        Key::Numpad5 => kp_keys("u"),
        Key::Numpad6 => kp_keys("v"),
        Key::Numpad7 => kp_keys("w"),
        Key::Numpad8 => kp_keys("x"),
        Key::Numpad9 => kp_keys("y"),
        Key::NumpadDecimal => kp_keys("n"),
        Key::NumpadDivide => kp_keys("o"),
        Key::NumpadMultiply => kp_keys("j"),
        Key::NumpadSubtract => kp_keys("m"),
        Key::NumpadAdd => kp_keys("k"),
        Key::NumpadEnter => {
            let mut v = kp_keys("M");
            v.push(seq("\r"));
            v
        }
        Key::NumpadUp => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}A"));
            v.extend(cursor_key("\x1b[A", "\x1bOA"));
            v
        }
        Key::NumpadDown => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}B"));
            v.extend(cursor_key("\x1b[B", "\x1bOB"));
            v
        }
        Key::NumpadRight => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}C"));
            v.extend(cursor_key("\x1b[C", "\x1bOC"));
            v
        }
        Key::NumpadLeft => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}D"));
            v.extend(cursor_key("\x1b[D", "\x1bOD"));
            v
        }
        Key::NumpadBegin => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}E"));
            v.extend(cursor_key("\x1b[E", "\x1bOE"));
            v
        }
        Key::NumpadHome => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}H"));
            v.extend(cursor_key("\x1b[H", "\x1bOH"));
            v
        }
        Key::NumpadEnd => {
            let mut v = pc_style(|code| format!("\x1b[1;{code}F"));
            v.extend(cursor_key("\x1b[F", "\x1bOF"));
            v
        }
        Key::NumpadInsert => {
            let mut v = pc_style(|code| format!("\x1b[2;{code}~"));
            v.push(seq("\x1b[2~"));
            v
        }
        Key::NumpadDelete => {
            let mut v = pc_style(|code| format!("\x1b[3;{code}~"));
            v.push(seq("\x1b[3~"));
            v
        }
        Key::NumpadPageUp => {
            let mut v = pc_style(|code| format!("\x1b[5;{code}~"));
            v.push(seq("\x1b[5~"));
            v
        }
        Key::NumpadPageDown => {
            let mut v = pc_style(|code| format!("\x1b[6;{code}~"));
            v.push(seq("\x1b[6~"));
            v
        }

        Key::Backspace => vec![
            // Modify Keys Normal
            Entry {
                mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x7f".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b\x7f".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b\x7f".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x08".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ctrl: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b\x08".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x7f".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x7f".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b\x7f".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b\x7f".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x08".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x08".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b\x08".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b\x08".to_string(),
                ..Entry::default()
            },
            // Modify Keys Other
            Entry {
                mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;2;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;3;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;4;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;6;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ctrl: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;7;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    shift: true,
                    ctrl: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;8;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;9;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;10;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;11;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;12;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;13;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;14;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;15;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;16;127~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x08".to_string(),
                sequence_decbkm: Some("\x7f".to_string()),
                ..Entry::default()
            },
            Entry {
                sequence: "\x7f".to_string(),
                sequence_decbkm: Some("\x08".to_string()),
                ..Entry::default()
            },
        ],

        Key::Tab => vec![
            // Modify Keys Normal
            Entry {
                mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b[Z".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b\t".to_string(),
                ..Entry::default()
            },
            // Modify Keys Other
            Entry {
                mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;2;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;3;9~".to_string(),
                ..Entry::default()
            },
            // Everything else
            Entry {
                mods: Mods {
                    alt: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;4;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;5;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;6;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;7;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;8;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;9;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;10;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;11;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;12;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;13;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;14;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;15;9~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;16;9~".to_string(),
                ..Entry::default()
            },
            seq("\t"),
        ],

        Key::Enter => vec![
            Entry {
                mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;2;13~".to_string(),
                ..Entry::default()
            },
            // Modify Keys Normal
            Entry {
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::Set,
                sequence: "\x1b\r".to_string(),
                ..Entry::default()
            },
            // Modify Keys Other
            Entry {
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                modify_other_keys: ModifyKeys::SetOther,
                sequence: "\x1b[27;3;13~".to_string(),
                ..Entry::default()
            },
            // Everything else
            Entry {
                mods: Mods {
                    alt: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;4;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;5;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;6;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;7;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;8;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;9;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;10;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;11;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;12;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;13;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;14;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;15;13~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;16;13~".to_string(),
                ..Entry::default()
            },
            seq("\r"),
        ],

        Key::Escape => vec![
            Entry {
                mods: Mods {
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;2;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ..Mods::default()
                },
                sequence: "\x1b\x1b".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;4;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;5;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;6;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;7;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;8;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;9;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;10;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;11;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;12;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;13;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;14;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;15;27~".to_string(),
                ..Entry::default()
            },
            Entry {
                mods: Mods {
                    alt: true,
                    super_: true,
                    ctrl: true,
                    shift: true,
                    ..Mods::default()
                },
                sequence: "\x1b[27;16;27~".to_string(),
                ..Entry::default()
            },
            seq("\x1b"),
        ],

        // Every other key has no PC-style function key entries.
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Adapted from the Zig `test "keys"`. The original test is a no-op in
    /// this crate's context: it early-returns via
    /// `switch (@import("terminal_options").artifact) { .lib => return
    /// error.SkipZigTest, ... }` because it needs `termio.Message.WriteReq
    /// .Small.Max`, a termio-specific buffer size constant that doesn't
    /// exist in this freestanding crate (and this crate has no `termio`
    /// dependency by design — see the crate doc comment). There is no
    /// meaningful 1:1 port of "does every sequence fit in termio's write
    /// buffer" here.
    ///
    /// Instead, these tests assert concrete, known-good sequences for a
    /// representative sample of keys and modifier codes, which is a more
    /// useful correctness check for this table than the original's buffer
    /// size assertion.
    #[test]
    fn arrow_up_shift_and_normal_cursor_mode() {
        let entries = entries_for(Key::ArrowUp);

        // Modifier code 2 = shift (see `modifiers()`, code = index + 2).
        let shift_entry = entries
            .iter()
            .find(|e| {
                e.mods
                    == Mods {
                        shift: true,
                        ..Mods::default()
                    }
            })
            .expect("shift entry present");
        assert_eq!(shift_entry.sequence, "\x1b[1;2A");

        let normal_cursor_entry = entries
            .iter()
            .find(|e| e.cursor == CursorMode::Normal)
            .expect("normal cursor entry present");
        assert_eq!(normal_cursor_entry.sequence, "\x1b[A");

        let application_cursor_entry = entries
            .iter()
            .find(|e| e.cursor == CursorMode::Application)
            .expect("application cursor entry present");
        assert_eq!(application_cursor_entry.sequence, "\x1bOA");
    }

    #[test]
    fn f1_pc_style_and_default() {
        let entries = entries_for(Key::F1);

        let shift_entry = entries
            .iter()
            .find(|e| {
                e.mods
                    == Mods {
                        shift: true,
                        ..Mods::default()
                    }
            })
            .expect("shift entry present");
        assert_eq!(shift_entry.sequence, "\x1b[1;2P");

        let default_entry = entries
            .iter()
            .find(|e| e.mods_empty_is_any && e.mods == Mods::default() && e.sequence == "\x1bOP");
        assert!(default_entry.is_some());
    }

    #[test]
    fn numpad_2_keypad_application_mode() {
        let entries = entries_for(Key::Numpad2);

        // kp_default("r"): mods_empty_is_any = false, keypad = application.
        let default_entry = entries
            .iter()
            .find(|e| !e.mods_empty_is_any && e.keypad == KeypadMode::Application)
            .expect("kp_default entry present");
        assert_eq!(default_entry.sequence, "\x1bOr");

        // pc_style shift entry, keypad forced to application by kp_keys.
        let shift_entry = entries
            .iter()
            .find(|e| {
                e.mods
                    == Mods {
                        shift: true,
                        ..Mods::default()
                    }
            })
            .expect("shift entry present");
        assert_eq!(shift_entry.sequence, "\x1bO2r");
        assert_eq!(shift_entry.keypad, KeypadMode::Application);
    }

    #[test]
    fn numpad_enter_has_kp_keys_and_plain_cr() {
        let entries = entries_for(Key::NumpadEnter);
        assert!(entries.iter().any(|e| e.sequence == "\r"));
        assert!(
            entries
                .iter()
                .any(|e| !e.mods_empty_is_any && e.sequence == "\x1bOM")
        );
    }

    #[test]
    fn backspace_default_and_ctrl_decbkm() {
        let entries = entries_for(Key::Backspace);

        // Final unconditional entry: plain DEL, with DECBKM swap to BS.
        let default_entry = entries.last().expect("backspace has entries");
        assert_eq!(default_entry.sequence, "\x7f");
        assert_eq!(default_entry.sequence_decbkm.as_deref(), Some("\x08"));

        let ctrl_entry = entries
            .iter()
            .find(|e| {
                e.mods
                    == Mods {
                        ctrl: true,
                        ..Mods::default()
                    }
                    && e.modify_other_keys == ModifyKeys::Any
            })
            .expect("ctrl entry present");
        assert_eq!(ctrl_entry.sequence, "\x08");
        assert_eq!(ctrl_entry.sequence_decbkm.as_deref(), Some("\x7f"));
    }

    #[test]
    fn tab_default_and_shift_set_mode() {
        let entries = entries_for(Key::Tab);
        assert!(entries.iter().any(|e| e.sequence == "\t"));

        let shift_set = entries
            .iter()
            .find(|e| {
                e.mods
                    == Mods {
                        shift: true,
                        ..Mods::default()
                    }
                    && e.modify_other_keys == ModifyKeys::Set
            })
            .expect("shift set entry present");
        assert_eq!(shift_set.sequence, "\x1b[Z");
    }

    #[test]
    fn enter_default_and_shift() {
        let entries = entries_for(Key::Enter);
        assert!(entries.iter().any(|e| e.sequence == "\r"));

        let shift_entry = entries
            .iter()
            .find(|e| {
                e.mods
                    == Mods {
                        shift: true,
                        ..Mods::default()
                    }
            })
            .expect("shift entry present");
        assert_eq!(shift_entry.sequence, "\x1b[27;2;13~");
    }

    #[test]
    fn escape_default_and_alt() {
        let entries = entries_for(Key::Escape);
        assert!(entries.iter().any(|e| e.sequence == "\x1b"));

        let alt_entry = entries
            .iter()
            .find(|e| {
                e.mods
                    == Mods {
                        alt: true,
                        ..Mods::default()
                    }
            })
            .expect("alt entry present");
        assert_eq!(alt_entry.sequence, "\x1b\x1b");
    }

    #[test]
    fn keys_without_entries_return_empty() {
        assert!(entries_for(Key::KeyA).is_empty());
        assert!(entries_for(Key::Space).is_empty());
    }

    #[test]
    fn modifiers_table_has_15_entries_in_order() {
        let mods = modifiers();
        assert_eq!(mods.len(), 15);
        assert_eq!(
            mods[0],
            Mods {
                shift: true,
                ..Mods::default()
            }
        );
        assert_eq!(
            mods[14],
            Mods {
                shift: true,
                alt: true,
                ctrl: true,
                super_: true,
                ..Mods::default()
            }
        );
    }
}
