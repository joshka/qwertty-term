//! Modifier-key bitmask, remapping, and related config types.
//!
//! Port of `input/key_mods.zig` (914 lines, 23 tests), `input/keyboard.zig` (58
//! lines, 0 tests), and `input/config.zig` (8 lines, 0 tests) from Ghostty's
//! Zig source. These three tiny/medium files are folded into this single
//! module (as anticipated by this crate's `lib.rs` module docs) rather than
//! split into `config.rs`/`keyboard.rs`, since `config.zig` is an 8-line enum
//! and `keyboard.zig` a 58-line enum with two methods — neither warrants its
//! own file, and both exist only to support `Mods`/`translation`.
//!
//! ## Deviations from the Zig source
//!
//! - `RemapSet.map` is `std.AutoArrayHashMapUnmanaged(Mods, Mods)` in Zig — an
//!   *ordered* hash map. `RemapSet::finalize` sorts it, and `RemapSet::apply`
//!   relies on that order (first match wins). Rust's `std::collections::HashMap`
//!   has no stable iteration order, so this port uses `Vec<(Mods, Mods)>`
//!   instead: a small ordered association list. This is a behavior-affecting
//!   choice (not cosmetic) — it's what makes "sorted, first-match-wins" work
//!   at all in Rust. See `RemapSet::finalize` / `RemapSet::apply`.
//! - `RemapSet::formatEntry` in Zig writes a full `"key-remap = ...\n"` config
//!   line via Ghostty's `Formatter` abstraction. That abstraction doesn't
//!   exist in this freestanding crate, so it isn't ported. Instead
//!   `format_mod` (the per-`Mods` byte-formatting helper, e.g. `"left_ctrl"`)
//!   is ported standalone, plus `RemapSet::format_entries` which returns the
//!   raw `"from=to"` strings (no `"key-remap = "` prefix, no trailing
//!   newline). The 3 Zig `formatEntry` tests are adapted to check
//!   `format_entries`/`format_mod` directly instead of the config-line
//!   format.

/// Aliases for modifier names. Port of `key_mods.zig`'s `alias`.
pub const ALIAS: &[(&str, Mod)] = &[
    ("cmd", Mod::Super),
    ("command", Mod::Super),
    ("opt", Mod::Alt),
    ("option", Mod::Alt),
    ("control", Mod::Ctrl),
];

/// Single modifier. Port of `key_mods.zig`'s `Mod`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mod {
    Shift,
    Ctrl,
    Alt,
    Super,
}

/// Which side of the keyboard a modifier is on. Port of `Mod.Side`
/// (`enum(u1) { left, right }`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ModSide {
    #[default]
    Left,
    Right,
}

/// A bitmask for all key modifiers. Port of `key_mods.zig`'s
/// `Mods` (`packed struct(u16)`).
///
/// The explicit `_padding: u6` field from the Zig source is dropped (it's a
/// Zig packed-struct implementation detail with no behavior), but [`Mods::int`]
/// still hand-packs the exact same bit layout the Zig struct has, since some
/// formatting/hashing logic depends on specific bit positions matching the
/// Zig source:
///
/// - bit 0: `shift`
/// - bit 1: `ctrl`
/// - bit 2: `alt`
/// - bit 3: `super`
/// - bit 4: `caps_lock`
/// - bit 5: `num_lock`
/// - bits 6-9: `sides` (a packed `Side`, itself: bit0=shift side, bit1=ctrl
///   side, bit2=alt side, bit3=super side)
/// - bits 10-15: padding, always 0
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
    pub sides: Side,
}

impl Mods {
    /// The mask that has all the side bits set (all sides = Right).
    /// Port of `Mods.side_mask`.
    pub const SIDE_MASK: Mods = Mods {
        shift: false,
        ctrl: false,
        alt: false,
        super_: false,
        caps_lock: false,
        num_lock: false,
        sides: Side {
            shift: ModSide::Right,
            ctrl: ModSide::Right,
            alt: ModSide::Right,
            super_: ModSide::Right,
        },
    };

    /// Integer value of this struct. Hand-packed to match the Zig
    /// `packed struct(u16)` bit layout exactly (see the struct doc comment).
    pub fn int(self) -> u16 {
        (self.shift as u16)
            | (self.ctrl as u16) << 1
            | (self.alt as u16) << 2
            | (self.super_ as u16) << 3
            | (self.caps_lock as u16) << 4
            | (self.num_lock as u16) << 5
            | (self.sides.int() as u16) << 6
    }

    /// Reconstruct a `Mods` from its `.int()` bit representation. Inverse of
    /// [`Mods::int`]. Not present by this name in the Zig source (which uses
    /// `@bitCast` freely both ways) but needed in Rust wherever the Zig code
    /// does `@bitCast(some_u16)` back into a `Mods`.
    pub fn from_int(v: u16) -> Mods {
        Mods {
            shift: v & 0b1 != 0,
            ctrl: v & 0b10 != 0,
            alt: v & 0b100 != 0,
            super_: v & 0b1000 != 0,
            caps_lock: v & 0b1_0000 != 0,
            num_lock: v & 0b10_0000 != 0,
            sides: Side::from_int(((v >> 6) & 0b1111) as u8),
        }
    }

    /// Returns true if no modifiers are set.
    pub fn empty(self) -> bool {
        self.int() == 0
    }

    /// Returns true if two mods are equal.
    ///
    /// Note: the Zig source compares via `.int()`; in this port the Rust
    /// struct has no `_padding` field, so `#[derive(PartialEq)]` is exactly
    /// equivalent (every remaining field is compared, same as comparing the
    /// packed integers with padding always zero). `equal` is kept as an
    /// explicit method (delegating to `.int()`, matching the Zig source
    /// literally) both for parity with the Zig API and so callers ported
    /// from Zig (`a.equal(b)`) read the same.
    pub fn equal(self, other: Mods) -> bool {
        self.int() == other.int()
    }

    /// Returns only the keys.
    pub fn keys(self) -> Keys {
        Keys {
            shift: self.shift,
            ctrl: self.ctrl,
            alt: self.alt,
            super_: self.super_,
        }
    }

    /// Return mods that are only relevant for bindings: shift/ctrl/alt/super,
    /// no locks, no sides.
    pub fn binding(self) -> Mods {
        Mods {
            shift: self.shift,
            ctrl: self.ctrl,
            alt: self.alt,
            super_: self.super_,
            ..Mods::default()
        }
    }

    /// Perform `self &~ other` to remove the other mods from self.
    pub fn unset(self, other: Mods) -> Mods {
        Mods::from_int(self.int() & !other.int())
    }

    /// Returns the mods without locks set.
    pub fn without_locks(self) -> Mods {
        let mut copy = self;
        copy.caps_lock = false;
        copy.num_lock = false;
        copy
    }

    /// Return the mods to use for key translation. This handles settings
    /// like `macos-option-as-alt`. The translation mods should be used for
    /// translation but never sent back in for the key callback.
    ///
    /// This uses `cfg!(target_os = "macos")` (a runtime-visible constant)
    /// rather than `#[cfg(target_os = "macos")]` so the logic (and its test)
    /// are exercised on every platform. On non-macOS the `if` is always
    /// false, so this is a no-op passthrough there, matching the Zig source's
    /// `if (comptime builtin.target.os.tag.isDarwin())` gate.
    pub fn translation(self, option_as_alt: OptionAsAlt) -> Mods {
        let mut result = self;

        if cfg!(target_os = "macos") {
            let unset_alt = match option_as_alt {
                OptionAsAlt::False => false,
                OptionAsAlt::True => true,
                OptionAsAlt::Left => self.sides.alt != ModSide::Right,
                OptionAsAlt::Right => self.sides.alt != ModSide::Left,
            };
            if unset_alt {
                result.alt = false;
            }
        }

        result
    }

    /// Checks to see if super is on (macOS) or ctrl.
    pub fn ctrl_or_super(self) -> bool {
        if cfg!(target_os = "macos") {
            self.super_
        } else {
            self.ctrl
        }
    }
}

/// The standard modifier keys only (no locks, no sides). Port of
/// `Mods.Keys` (`packed struct(u4)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Keys {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub super_: bool,
}

impl Keys {
    /// bit0=shift, bit1=ctrl, bit2=alt, bit3=super.
    pub fn int(self) -> u8 {
        (self.shift as u8)
            | (self.ctrl as u8) << 1
            | (self.alt as u8) << 2
            | (self.super_ as u8) << 3
    }

    fn from_int(v: u8) -> Keys {
        Keys {
            shift: v & 0b0001 != 0,
            ctrl: v & 0b0010 != 0,
            alt: v & 0b0100 != 0,
            super_: v & 0b1000 != 0,
        }
    }
}

/// Tracks the side that is active for any given modifier. Note that this
/// doesn't confirm a modifier is pressed; you must check the bool for that
/// in addition to this.
///
/// Not all platforms support this, check apprt for more info.
///
/// Port of `Mods.Side` (`packed struct(u4)`): bit0=shift, bit1=ctrl,
/// bit2=alt, bit3=super. Note this shares bit positions with [`Keys`],
/// which `RemapSet`'s `Mask` relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub struct Side {
    pub shift: ModSide,
    pub ctrl: ModSide,
    pub alt: ModSide,
    pub super_: ModSide,
}

impl Side {
    pub fn int(self) -> u8 {
        (self.shift as u8)
            | (self.ctrl as u8) << 1
            | (self.alt as u8) << 2
            | (self.super_ as u8) << 3
    }

    pub fn from_int(v: u8) -> Side {
        let side = |bit: u8| {
            if v & bit != 0 {
                ModSide::Right
            } else {
                ModSide::Left
            }
        };
        Side {
            shift: side(0b0001),
            ctrl: side(0b0010),
            alt: side(0b0100),
            super_: side(0b1000),
        }
    }
}

/// Determines the macOS option key behavior. See the config
/// `macos-option-as-alt` for a lot more details. Port of `config.zig`'s
/// `OptionAsAlt`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OptionAsAlt {
    #[default]
    False,
    True,
    Left,
    Right,
}

/// Keyboard layouts. Port of `keyboard.zig`'s `Layout`.
///
/// These aren't heavily used in Ghostty and having a fully comprehensive list
/// is not important. We only need to distinguish between a few different
/// layouts for some nice-to-have features, such as setting a default value
/// for "macos-option-as-alt".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Layout {
    /// Unknown, unmapped layout. Ghostty should not make any assumptions
    /// about the layout of the keyboard.
    #[default]
    Unknown,
    UsStandard,
    UsInternational,
}

impl Layout {
    /// Map an Apple keyboard layout ID to a value in this enum. The layout ID
    /// can be retrieved using Carbon's `TIKeyboardLayoutGetInputSourceProperty`
    /// function.
    ///
    /// Even though our layout supports "unknown", we return `None` if we
    /// don't recognize the layout ID so callers can detect this scenario.
    pub fn map_apple_id(id: &str) -> Option<Layout> {
        match id {
            "com.apple.keylayout.US" => Some(Layout::UsStandard),
            "com.apple.keylayout.USInternational" => Some(Layout::UsInternational),
            _ => None,
        }
    }

    /// Returns the default `macos-option-as-alt` value for this layout.
    ///
    /// We apply some heuristics to change the default based on the keyboard
    /// layout if `macos-option-as-alt` is unset. We do this because on some
    /// keyboard layouts such as US standard layouts, users generally expect
    /// an input such as option-b to map to alt-b but macOS by default will
    /// convert it to the codepoint "∫".
    ///
    /// This behavior however is desired on international layout where the
    /// option key is used for important, regularly used inputs.
    pub fn detect_option_as_alt(self) -> OptionAsAlt {
        match self {
            // On US standard, the option key is typically used as alt and
            // not as a modifier for other codepoints. For example,
            // option-B = ∫ but usually the user wants alt-B.
            Layout::UsStandard | Layout::UsInternational => OptionAsAlt::True,
            Layout::Unknown => OptionAsAlt::False,
        }
    }
}

/// Error returned by [`RemapSet::parse`]. Port of `RemapSet.ParseError`
/// (minus `Allocator.Error`, which has no Rust equivalent).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    MissingAssignment,
    InvalidMod,
}

/// Error returned by [`RemapSet::parse_cli`]. Port of the `error.InvalidValue`
/// case Zig's `parseCLI` maps `ParseError` onto (simplified: Rust has no
/// `Allocator.Error::OutOfMemory` case to also handle).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseCliError {
    InvalidValue,
}

/// Tracks which modifier keys and sides have remappings registered. Used as a
/// fast pre-check before doing expensive map lookups. Port of
/// `RemapSet.Mask` (`packed struct(u12)`).
///
/// The mask uses separate tracking for left and right sides because
/// remappings can be side-specific (e.g., only remap left_ctrl).
///
/// Note: `left_sides` uses inverted logic where 1 means "left is remapped"
/// even though `Mod.Side.left = 0`. This allows efficient bitwise matching
/// since we can AND directly with the side bits.
#[derive(Debug, Clone, Copy, Default)]
pub struct Mask {
    /// Which modifier keys (shift/ctrl/alt/super) have any remapping.
    keys: Keys,
    /// Which modifiers have left-side remappings (inverted: 1 = left remapped).
    left_sides: Side,
    /// Which modifiers have right-side remappings (1 = right remapped).
    right_sides: Side,
}

impl Mask {
    /// Adds a modifier to the mask, marking it as having a remapping.
    pub fn update(&mut self, mods: Mods) {
        let keys_int = mods.keys().int();

        // OR the new keys into our existing keys mask.
        // Example: keys=0b0000, new ctrl -> keys=0b0010
        self.keys = Keys::from_int(self.keys.int() | keys_int);

        // Both Keys and Side are u4 with matching bit positions. This lets us
        // use keys_int to select which side bits to update.
        let sides = mods.sides.int();
        let left_int = self.left_sides.int();
        let right_int = self.right_sides.int();

        // Update left_sides: set bit if this key is active AND side is left.
        // Since Side.left=0, we invert sides (~sides) so left becomes 1.
        // keys_int masks to only affect the modifier being added.
        // Example: left_ctrl -> keys_int=0b0010, ~sides=0b1111 (left=0 inverted)
        //          result: left_int | (0b0010 & 0b1111) = left_int | 0b0010
        self.left_sides = Side::from_int(left_int | (keys_int & !sides));

        // Update right_sides: set bit if this key is active AND side is right.
        // Since Side.right=1, we use sides directly.
        // Example: right_ctrl -> keys_int=0b0010, sides=0b0010 (right=1)
        //          result: right_int | (0b0010 & 0b0010) = right_int | 0b0010
        self.right_sides = Side::from_int(right_int | (keys_int & sides));
    }

    /// Returns true if the given mods match any remapping in this mask. This
    /// is a fast check to avoid expensive map lookups when no remapping
    /// could possibly apply.
    ///
    /// Checks both that the modifier key is remapped AND that the specific
    /// side (left/right) being pressed has a remapping.
    pub fn matches(&self, mods: Mods) -> bool {
        // Find which pressed keys have remappings registered.
        // Example: pressed={ctrl,alt}, mask={ctrl} -> active=0b0010 (just ctrl)
        let active = mods.keys().int() & self.keys.int();
        if active == 0 {
            return false;
        }

        // Check if the pressed side matches a remapped side.
        // For left (sides bit = 0): check against left_int (where 1 = left remapped)
        //   ~sides inverts so left becomes 1, then AND with left_int
        // For right (sides bit = 1): check against right_int directly
        //
        // Example: pressing left_ctrl (sides.ctrl=0, left_int.ctrl=1)
        //   ~sides = 0b1111, left_int = 0b0010
        //   (~sides & left_int) = 0b0010 (matches)
        //
        // Example: pressing right_ctrl but only left_ctrl is remapped
        //   sides = 0b0010, left_int = 0b0010, right_int = 0b0000
        //   (~0b0010 & 0b0010) | (0b0010 & 0b0000) = 0b0000 (no match)
        let sides = mods.sides.int();
        let left_int = self.left_sides.int();
        let right_int = self.right_sides.int();
        let side_match = (!sides & left_int) | (sides & right_int);

        // Final check: is any active (pressed + remapped) key also side-matched?
        (active & side_match) != 0
    }
}

/// Modifier remapping. See `key-remap` in Ghostty's `Config.zig` for detailed
/// docs. Port of `key_mods.zig`'s `RemapSet`.
///
/// ## Ordering deviation from Zig
///
/// The Zig source stores `map` as `std.AutoArrayHashMapUnmanaged(Mods, Mods)`,
/// an *ordered* hash map, and `finalize()` sorts its entries (right-sided
/// `from` entries first) because `apply()` walks the map in order and uses
/// the first match. Rust's `std::collections::HashMap` provides no iteration
/// order guarantee, so this port uses `Vec<(Mods, Mods)>` — a small ordered
/// association list — to faithfully preserve "sorted, first-match-wins"
/// semantics. This is a real, behavior-affecting substitution, not a cosmetic
/// one: with a true hash map the sort in `finalize` would be pointless.
#[derive(Debug, Clone, Default)]
pub struct RemapSet {
    /// Available mappings, in match-priority order (see `finalize`).
    map: Vec<(Mods, Mods)>,

    /// The mask of remapped modifiers that can be used to quickly check if
    /// some input mods need remapping.
    mask: Mask,
}

impl RemapSet {
    /// An empty `RemapSet`. Port of `RemapSet.empty`.
    pub fn empty() -> RemapSet {
        RemapSet::default()
    }

    /// Parse from CLI input. Port of `RemapSet.parseCLI`.
    pub fn parse_cli(&mut self, input: Option<&str>) -> Result<(), ParseCliError> {
        let value = input.unwrap_or("");

        // Empty value resets the set.
        if value.is_empty() {
            self.map.clear();
            self.mask = Mask::default();
            return Ok(());
        }

        self.parse(value).map_err(|err| match err {
            ParseError::MissingAssignment | ParseError::InvalidMod => ParseCliError::InvalidValue,
        })
    }

    /// Parse a modifier remap and add it to the set. Port of `RemapSet.parse`.
    pub fn parse(&mut self, input: &str) -> Result<(), ParseError> {
        // Find the assignment point ('=').
        let eq_idx = input.find('=').ok_or(ParseError::MissingAssignment)?;

        // The to side defaults to "left" if no explicit side is given. This
        // is because this is the default unsided value provided by the
        // apprts in the current Mods layout.
        let to = {
            let (to_mod, to_side) = parse_mod(&input[eq_idx + 1..])?;
            init_mods(to_mod, to_side.unwrap_or(ModSide::Left))
        };

        // The from side, if sided, is easy and we put it directly into the
        // map.
        let (from_mod, from_side) = parse_mod(&input[..eq_idx])?;
        if let Some(from_side) = from_side {
            let from = init_mods(from_mod, from_side);
            self.map.push((from, to));
            self.mask.update(from);
            return Ok(());
        }

        // We need to do some combinatorial explosion here for unsided from
        // in order to assign all possible sides.
        let from_left = init_mods(from_mod, ModSide::Left);
        let from_right = init_mods(from_mod, ModSide::Right);
        self.map.push((from_left, to));
        self.map.push((from_right, to));

        self.mask.update(from_left);
        self.mask.update(from_right);

        Ok(())
    }

    /// Must be called prior to any remappings so that the mapping is sorted
    /// properly. Otherwise, you will get invalid results.
    ///
    /// Port of `RemapSet.finalize`. Sorts so that entries whose `from` side
    /// has any right-side bit set come first, matching the Zig comparator
    /// (`a.int() & side_mask != 0`). This uses a stable sort (Rust's
    /// `sort_by_key` is stable) so relative order among entries with equal
    /// "rightness" matches insertion order — our `Vec`-based `map` depends on
    /// order for correctness, so stability here is deliberate.
    pub fn finalize(&mut self) {
        let side_mask = Mods::SIDE_MASK.int();
        self.map
            .sort_by_key(|(from, _to)| std::cmp::Reverse(from.int() & side_mask != 0));
    }

    /// Compare if two RemapSets are equal. Port of `RemapSet.equal`.
    pub fn equal(&self, other: &RemapSet) -> bool {
        if self.map.len() != other.map.len() {
            return false;
        }

        for (key, value) in &self.map {
            let Some((_, other_value)) = other.map.iter().find(|(k, _)| k.equal(*key)) else {
                return false;
            };
            if !value.equal(*other_value) {
                return false;
            }
        }

        true
    }

    /// Returns true if the given mods need remapping. Port of
    /// `RemapSet.isRemapped`.
    pub fn is_remapped(&self, mods: Mods) -> bool {
        self.mask.matches(mods)
    }

    /// Apply a remap to the given mods. Port of `RemapSet.apply`.
    pub fn apply(&self, mods: Mods) -> Mods {
        if !self.is_remapped(mods) {
            return mods;
        }

        let mods_binding = (mods.int() & 0b1111) as u8;
        let mods_sides = mods.sides.int();

        for (from, to) in &self.map {
            let from_binding = (from.int() & 0b1111) as u8;
            if mods_binding & from_binding != from_binding {
                continue;
            }
            let from_sides = from.sides.int();
            if (mods_sides ^ from_sides) & from_binding != 0 {
                continue;
            }

            let mut mods_int = mods.int();
            mods_int &= !from.int();
            mods_int |= to.int();
            return Mods::from_int(mods_int);
        }

        unreachable!(
            "RemapSet::apply: is_remapped(mods) was true but no map entry matched; \
             this indicates the mask and map are out of sync"
        );
    }

    /// Returns the `"from=to"` strings for each entry in the set (e.g.
    /// `"left_ctrl=left_super"`), in map order. See the module doc comment
    /// for why this replaces the Zig `formatEntry` (which formats a full
    /// `"key-remap = ...\n"` config line via a `Formatter` abstraction that
    /// doesn't exist in this crate).
    pub fn format_entries(&self) -> Vec<String> {
        self.map
            .iter()
            .map(|(from, to)| format!("{}={}", format_mod(*from), format_mod(*to)))
            .collect()
    }
}

/// Formats a single [`Mods`] value as e.g. `"left_ctrl"` or `"right_alt"`.
/// Port of `RemapSet.formatMod`. Checks which mod is set and formats it with
/// its side prefix; only the first set mod is written (matching the Zig
/// source, which `return`s after the first `inline for` match — remap
/// entries only ever have exactly one mod set per side, by construction of
/// `init_mods`).
fn format_mod(mods: Mods) -> String {
    let candidates: [(bool, ModSide, &str); 4] = [
        (mods.shift, mods.sides.shift, "shift"),
        (mods.ctrl, mods.sides.ctrl, "ctrl"),
        (mods.alt, mods.sides.alt, "alt"),
        (mods.super_, mods.sides.super_, "super"),
    ];

    for (set, side, name) in candidates {
        if set {
            let prefix = if side == ModSide::Right {
                "right_"
            } else {
                "left_"
            };
            return format!("{prefix}{name}");
        }
    }

    String::new()
}

/// Parses a single mod in a single remapping string, e.g. `"ctrl"` or
/// `"left_shift"`. Port of `RemapSet.parseMod`.
fn parse_mod(input: &str) -> Result<(Mod, Option<ModSide>), ParseError> {
    let (side_str, mod_str) = match input.find('_') {
        Some(idx) => (&input[..idx], &input[idx + 1..]),
        None => ("", input),
    };

    let m = match mod_str {
        "shift" => Mod::Shift,
        "ctrl" => Mod::Ctrl,
        "alt" => Mod::Alt,
        "super" => Mod::Super,
        _ => ALIAS
            .iter()
            .find(|(name, _)| *name == mod_str)
            .map(|(_, m)| *m)
            .ok_or(ParseError::InvalidMod)?,
    };

    let side = if !side_str.is_empty() {
        Some(match side_str {
            "left" => ModSide::Left,
            "right" => ModSide::Right,
            _ => return Err(ParseError::InvalidMod),
        })
    } else {
        None
    };

    Ok((m, side))
}

/// Builds a `Mods` with exactly one modifier set, on the given side. Port of
/// `RemapSet.initMods`.
fn init_mods(m: Mod, side: ModSide) -> Mods {
    let mut mods = Mods::default();
    match m {
        Mod::Shift => {
            mods.shift = true;
            mods.sides.shift = side;
        }
        Mod::Ctrl => {
            mods.ctrl = true;
            mods.sides.ctrl = side;
        }
        Mod::Alt => {
            mods.alt = true;
            mods.sides.alt = side;
        }
        Mod::Super => {
            mods.super_ = true;
            mods.sides.super_ = side;
        }
    }
    mods
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of the anonymous `test {}` block in `Mods` (bit-layout check).
    #[test]
    fn mods_bit_layout() {
        assert_eq!(Mods::default().int(), 0b0);
        assert_eq!(
            Mods {
                shift: true,
                ..Mods::default()
            }
            .int(),
            0b0000_0001
        );
    }

    // Port of test "translation macos-option-as-alt". The Zig test is
    // Darwin-gated (`if (!isDarwin()) return error.SkipZigTest;`); this port
    // uses `cfg!(target_os = "macos")` inside `Mods::translation` itself
    // (rather than `#[cfg(...)]` on the function), so on non-macOS hosts
    // `translation` is a no-op passthrough and every assertion below reduces
    // to `result == mods` trivially (re-checked against the Zig logic: with
    // the Darwin branch skipped, `result` is initialized to `self` and never
    // mutated) — so this test passes harmlessly on every platform while
    // still exercising the real logic on macOS.
    #[test]
    fn translation_macos_option_as_alt() {
        // Unset.
        {
            let mods = Mods::default();
            let result = mods.translation(OptionAsAlt::True);
            assert_eq!(result, mods);
        }

        // Set.
        {
            let mods = Mods {
                alt: true,
                ..Mods::default()
            };
            let result = mods.translation(OptionAsAlt::True);
            if cfg!(target_os = "macos") {
                assert_eq!(result, Mods::default());
            } else {
                assert_eq!(result, mods);
            }
        }

        // Set but disabled.
        {
            let mods = Mods {
                alt: true,
                ..Mods::default()
            };
            let result = mods.translation(OptionAsAlt::False);
            assert_eq!(result, mods);
        }

        // Set wrong side.
        {
            let mods = Mods {
                alt: true,
                sides: Side {
                    alt: ModSide::Right,
                    ..Side::default()
                },
                ..Mods::default()
            };
            let result = mods.translation(OptionAsAlt::Left);
            assert_eq!(result, mods);
        }
        {
            let mods = Mods {
                alt: true,
                sides: Side {
                    alt: ModSide::Left,
                    ..Side::default()
                },
                ..Mods::default()
            };
            let result = mods.translation(OptionAsAlt::Right);
            assert_eq!(result, mods);
        }

        // Set with other mods.
        {
            let mods = Mods {
                alt: true,
                shift: true,
                ..Mods::default()
            };
            let result = mods.translation(OptionAsAlt::True);
            if cfg!(target_os = "macos") {
                assert_eq!(
                    result,
                    Mods {
                        shift: true,
                        ..Mods::default()
                    }
                );
            } else {
                assert_eq!(result, mods);
            }
        }
    }

    // Port of test "RemapSet: unsided remap creates both left and right mappings".
    #[test]
    fn remapset_unsided_remap_creates_both_left_and_right_mappings() {
        let mut set = RemapSet::empty();
        set.parse("ctrl=super").unwrap();
        set.finalize();

        assert_eq!(
            Mods {
                super_: true,
                sides: Side {
                    super_: ModSide::Left,
                    ..Side::default()
                },
                ..Mods::default()
            },
            set.apply(Mods {
                ctrl: true,
                sides: Side {
                    ctrl: ModSide::Left,
                    ..Side::default()
                },
                ..Mods::default()
            })
        );
        assert_eq!(
            Mods {
                super_: true,
                sides: Side {
                    super_: ModSide::Left,
                    ..Side::default()
                },
                ..Mods::default()
            },
            set.apply(Mods {
                ctrl: true,
                sides: Side {
                    ctrl: ModSide::Right,
                    ..Side::default()
                },
                ..Mods::default()
            })
        );
    }

    // Port of test "RemapSet: sided from only maps that side".
    #[test]
    fn remapset_sided_from_only_maps_that_side() {
        let mut set = RemapSet::empty();
        set.parse("left_alt=ctrl").unwrap();
        set.finalize();

        let left_alt = Mods {
            alt: true,
            sides: Side {
                alt: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let left_ctrl = Mods {
            ctrl: true,
            sides: Side {
                ctrl: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(left_ctrl, set.apply(left_alt));

        let right_alt = Mods {
            alt: true,
            sides: Side {
                alt: ModSide::Right,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(right_alt, set.apply(right_alt));
    }

    // Port of test "RemapSet: sided to".
    #[test]
    fn remapset_sided_to() {
        let mut set = RemapSet::empty();
        set.parse("ctrl=right_super").unwrap();
        set.finalize();

        let left_ctrl = Mods {
            ctrl: true,
            sides: Side {
                ctrl: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let right_super = Mods {
            super_: true,
            sides: Side {
                super_: ModSide::Right,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(right_super, set.apply(left_ctrl));
    }

    // Port of test "RemapSet: both sides specified".
    #[test]
    fn remapset_both_sides_specified() {
        let mut set = RemapSet::empty();
        set.parse("left_shift=right_ctrl").unwrap();
        set.finalize();

        let left_shift = Mods {
            shift: true,
            sides: Side {
                shift: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let right_ctrl = Mods {
            ctrl: true,
            sides: Side {
                ctrl: ModSide::Right,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(right_ctrl, set.apply(left_shift));
    }

    // Port of test "RemapSet: multiple parses accumulate".
    #[test]
    fn remapset_multiple_parses_accumulate() {
        let mut set = RemapSet::empty();
        set.parse("left_ctrl=super").unwrap();
        set.parse("left_alt=ctrl").unwrap();
        set.finalize();

        let left_ctrl = Mods {
            ctrl: true,
            sides: Side {
                ctrl: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let left_super = Mods {
            super_: true,
            sides: Side {
                super_: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(left_super, set.apply(left_ctrl));

        let left_alt = Mods {
            alt: true,
            sides: Side {
                alt: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let left_ctrl_result = Mods {
            ctrl: true,
            sides: Side {
                ctrl: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(left_ctrl_result, set.apply(left_alt));
    }

    // Port of test "RemapSet: error on missing assignment".
    #[test]
    fn remapset_error_on_missing_assignment() {
        let mut set = RemapSet::empty();
        assert_eq!(set.parse("ctrl"), Err(ParseError::MissingAssignment));
        assert_eq!(set.parse(""), Err(ParseError::MissingAssignment));
    }

    // Port of test "RemapSet: error on invalid modifier".
    #[test]
    fn remapset_error_on_invalid_modifier() {
        let mut set = RemapSet::empty();
        assert_eq!(set.parse("invalid=ctrl"), Err(ParseError::InvalidMod));
        assert_eq!(set.parse("ctrl=invalid"), Err(ParseError::InvalidMod));
        assert_eq!(set.parse("middle_ctrl=super"), Err(ParseError::InvalidMod));
    }

    // Port of test "RemapSet: isRemapped checks mask".
    #[test]
    fn remapset_is_remapped_checks_mask() {
        let mut set = RemapSet::empty();
        set.parse("ctrl=super").unwrap();
        set.finalize();

        assert!(set.is_remapped(Mods {
            ctrl: true,
            ..Mods::default()
        }));
        assert!(!set.is_remapped(Mods {
            alt: true,
            ..Mods::default()
        }));
        assert!(!set.is_remapped(Mods {
            shift: true,
            ..Mods::default()
        }));
    }

    // Port of test "RemapSet: clone creates independent copy". Rust's
    // `RemapSet` derives `Clone` directly (no explicit allocator to thread
    // through), so this test uses `.clone()` in place of Zig's
    // `set.clone(alloc)`.
    #[test]
    fn remapset_clone_creates_independent_copy() {
        let mut set = RemapSet::empty();
        set.parse("ctrl=super").unwrap();
        set.finalize();

        let cloned = set.clone();

        assert!(set.equal(&cloned));
        assert!(cloned.is_remapped(Mods {
            ctrl: true,
            ..Mods::default()
        }));
    }

    // Port of test "RemapSet: equal compares correctly".
    #[test]
    fn remapset_equal_compares_correctly() {
        let mut set1 = RemapSet::empty();
        let mut set2 = RemapSet::empty();

        assert!(set1.equal(&set2));

        set1.parse("ctrl=super").unwrap();
        assert!(!set1.equal(&set2));

        set2.parse("ctrl=super").unwrap();
        assert!(set1.equal(&set2));

        set1.parse("alt=shift").unwrap();
        assert!(!set1.equal(&set2));
    }

    // Port of test "RemapSet: parseCLI basic".
    #[test]
    fn remapset_parse_cli_basic() {
        let mut set = RemapSet::empty();
        set.parse_cli(Some("ctrl=super")).unwrap();
        assert_eq!(set.map.len(), 2);
    }

    // Port of test "RemapSet: parseCLI empty clears".
    #[test]
    fn remapset_parse_cli_empty_clears() {
        let mut set = RemapSet::empty();
        set.parse_cli(Some("ctrl=super")).unwrap();
        assert_eq!(set.map.len(), 2);

        set.parse_cli(Some("")).unwrap();
        assert_eq!(set.map.len(), 0);
    }

    // Port of test "RemapSet: parseCLI invalid".
    #[test]
    fn remapset_parse_cli_invalid() {
        let mut set = RemapSet::empty();
        assert_eq!(
            set.parse_cli(Some("foo=bar")),
            Err(ParseCliError::InvalidValue)
        );
        assert_eq!(
            set.parse_cli(Some("ctrl")),
            Err(ParseCliError::InvalidValue)
        );
    }

    // Port of test "RemapSet: parse aliased modifiers".
    #[test]
    fn remapset_parse_aliased_modifiers() {
        let mut set = RemapSet::empty();
        set.parse("cmd=ctrl").unwrap();
        set.finalize();

        let left_super = Mods {
            super_: true,
            sides: Side {
                super_: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let left_ctrl = Mods {
            ctrl: true,
            sides: Side {
                ctrl: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(left_ctrl, set.apply(left_super));
    }

    // Port of test "RemapSet: parse aliased modifiers command".
    #[test]
    fn remapset_parse_aliased_modifiers_command() {
        let mut set = RemapSet::empty();
        set.parse("command=alt").unwrap();
        set.finalize();

        let left_super = Mods {
            super_: true,
            sides: Side {
                super_: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let left_alt = Mods {
            alt: true,
            sides: Side {
                alt: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(left_alt, set.apply(left_super));
    }

    // Port of test "RemapSet: parse aliased modifiers opt and option".
    #[test]
    fn remapset_parse_aliased_modifiers_opt_and_option() {
        let mut set = RemapSet::empty();
        set.parse("opt=super").unwrap();
        set.finalize();

        let left_alt = Mods {
            alt: true,
            sides: Side {
                alt: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let left_super = Mods {
            super_: true,
            sides: Side {
                super_: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(left_super, set.apply(left_alt));

        let mut set = RemapSet::empty();
        set.parse("option=shift").unwrap();
        set.finalize();

        let left_shift = Mods {
            shift: true,
            sides: Side {
                shift: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(left_shift, set.apply(left_alt));
    }

    // Port of test "RemapSet: parse aliased modifiers control".
    #[test]
    fn remapset_parse_aliased_modifiers_control() {
        let mut set = RemapSet::empty();
        set.parse("control=super").unwrap();
        set.finalize();

        let left_ctrl = Mods {
            ctrl: true,
            sides: Side {
                ctrl: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let left_super = Mods {
            super_: true,
            sides: Side {
                super_: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(left_super, set.apply(left_ctrl));
    }

    // Port of test "RemapSet: parse aliased modifiers on target side".
    #[test]
    fn remapset_parse_aliased_modifiers_on_target_side() {
        let mut set = RemapSet::empty();
        set.parse("alt=cmd").unwrap();
        set.finalize();

        let left_alt = Mods {
            alt: true,
            sides: Side {
                alt: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        let left_super = Mods {
            super_: true,
            sides: Side {
                super_: ModSide::Left,
                ..Side::default()
            },
            ..Mods::default()
        };
        assert_eq!(left_super, set.apply(left_alt));
    }

    // Adapted from test "RemapSet: formatEntry empty". The Zig test checks
    // the full `"key-remap = \n"` config line via a `Formatter` abstraction
    // this crate doesn't have (see module doc comment); this checks the
    // underlying `format_entries` returns nothing instead.
    #[test]
    fn remapset_format_entries_empty() {
        let set = RemapSet::empty();
        assert!(set.format_entries().is_empty());
    }

    // Adapted from test "RemapSet: formatEntry single sided".
    #[test]
    fn remapset_format_entries_single_sided() {
        let mut set = RemapSet::empty();
        set.parse("left_ctrl=super").unwrap();
        set.finalize();

        assert_eq!(set.format_entries(), vec!["left_ctrl=left_super"]);
    }

    // Adapted from test "RemapSet: formatEntry unsided creates two entries".
    #[test]
    fn remapset_format_entries_unsided_creates_two_entries() {
        let mut set = RemapSet::empty();
        set.parse("ctrl=super").unwrap();
        set.finalize();

        let entries = set.format_entries();
        assert!(entries.iter().any(|e| e == "left_ctrl=left_super"));
        assert!(entries.iter().any(|e| e == "right_ctrl=left_super"));
    }

    // Adapted from test "RemapSet: formatEntry right sided".
    #[test]
    fn remapset_format_entries_right_sided() {
        let mut set = RemapSet::empty();
        set.parse("left_alt=right_ctrl").unwrap();
        set.finalize();

        assert_eq!(set.format_entries(), vec!["left_alt=right_ctrl"]);
    }
}
