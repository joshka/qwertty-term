//! Kitty keyboard protocol flag stack (port of `src/terminal/kitty/key.zig`,
//! commit `2da015cd6`).
//!
//! Implements the push/pop/set behavior of the CSI `> u` / `< u` / `= u`
//! sequences. The stack is fixed-size (8 deep) to avoid heap allocation and to
//! bound a DoS vector (a malicious client spamming pop). Owned by `Screen`.

/// The possible flags for the Kitty keyboard protocol. Port of `key.Flags`
/// (`packed struct(u5)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Flags {
    pub disambiguate: bool,
    pub report_events: bool,
    pub report_alternates: bool,
    pub report_all: bool,
    pub report_associated: bool,
}

impl Flags {
    /// All flags off. Port of `Flags.disabled`.
    pub const DISABLED: Flags = Flags {
        disambiguate: false,
        report_events: false,
        report_alternates: false,
        report_all: false,
        report_associated: false,
    };

    /// All flags on. Port of `Flags."true"`.
    pub const ALL: Flags = Flags {
        disambiguate: true,
        report_events: true,
        report_alternates: true,
        report_all: true,
        report_associated: true,
    };

    /// The u5 bit representation. Port of `Flags.int`. LSB-first order matching
    /// the Zig `packed struct(u5)` field order.
    pub fn int(self) -> u8 {
        (self.disambiguate as u8)
            | ((self.report_events as u8) << 1)
            | ((self.report_alternates as u8) << 2)
            | ((self.report_all as u8) << 3)
            | ((self.report_associated as u8) << 4)
    }

    /// Reconstruct from the u5 bit representation. Inverse of [`int`](Self::int).
    pub fn from_int(v: u8) -> Flags {
        Flags {
            disambiguate: v & 0b0_0001 != 0,
            report_events: v & 0b0_0010 != 0,
            report_alternates: v & 0b0_0100 != 0,
            report_all: v & 0b0_1000 != 0,
            report_associated: v & 0b1_0000 != 0,
        }
    }
}

/// The modes for setting the key flags (CSI `= u`). Port of `key.SetMode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetMode {
    Set,
    Or,
    Not,
}

/// Fixed-size stack for the key flags. Port of `key.FlagStack`.
#[derive(Debug, Clone, Copy)]
pub struct FlagStack {
    flags: [Flags; Self::LEN],
    /// The Zig field is a `u3` (0..=7) with wrapping arithmetic.
    idx: u8,
}

impl Default for FlagStack {
    fn default() -> Self {
        FlagStack {
            flags: [Flags::DISABLED; Self::LEN],
            idx: 0,
        }
    }
}

impl FlagStack {
    const LEN: usize = 8;
    /// Mask for the `u3` index wrapping (`idx +%= 1` / `idx -%= 1`).
    const IDX_MASK: u8 = 0b111;

    /// Return the current stack value. Port of `current`.
    pub fn current(self) -> Flags {
        self.flags[self.idx as usize]
    }

    /// Perform the "set" operation for the CSI `= u` sequence. Port of `set`.
    pub fn set(&mut self, mode: SetMode, v: Flags) {
        let i = self.idx as usize;
        self.flags[i] = match mode {
            SetMode::Set => v,
            SetMode::Or => Flags::from_int(self.flags[i].int() | v.int()),
            SetMode::Not => Flags::from_int(self.flags[i].int() & !v.int()),
        };
    }

    /// Push a new set of flags. If the stack is full the oldest entry is
    /// evicted (the `u3` index wraps). Port of `push`.
    pub fn push(&mut self, flags: Flags) {
        self.idx = self.idx.wrapping_add(1) & Self::IDX_MASK;
        self.flags[self.idx as usize] = flags;
    }

    /// Pop `n` entries. Wraps around if `n` exceeds the stack; resets entirely
    /// if `n >= len` (DoS defense). Port of `pop`.
    pub fn pop(&mut self, n: usize) {
        if n >= Self::LEN {
            self.idx = 0;
            self.flags = [Flags::DISABLED; Self::LEN];
            return;
        }

        for _ in 0..n {
            self.flags[self.idx as usize] = Flags::DISABLED;
            self.idx = self.idx.wrapping_sub(1) & Self::IDX_MASK;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of `key.zig` anonymous `test {}` (overflow wrap check).
    #[test]
    fn flagstack_overflow_wraps() {
        let mut stack = FlagStack::default();
        stack.idx = (FlagStack::LEN - 1) as u8;
        stack.idx = stack.idx.wrapping_add(1) & FlagStack::IDX_MASK;
        assert_eq!(stack.idx, 0);

        stack.idx = 0;
        stack.idx = stack.idx.wrapping_sub(1) & FlagStack::IDX_MASK;
        assert_eq!(stack.idx as usize, FlagStack::LEN - 1);
    }

    // Port of `Flags` anonymous `test {}` (packed struct ordering).
    #[test]
    fn flags_bit_ordering() {
        assert_eq!(
            Flags {
                disambiguate: true,
                ..Flags::DISABLED
            }
            .int(),
            0b1
        );
        assert_eq!(
            Flags {
                report_events: true,
                ..Flags::DISABLED
            }
            .int(),
            0b10
        );
    }

    // Port of `test "FlagStack: push pop"`.
    #[test]
    fn flagstack_push_pop() {
        let mut stack = FlagStack::default();
        stack.push(Flags {
            disambiguate: true,
            ..Flags::DISABLED
        });
        assert_eq!(
            stack.current(),
            Flags {
                disambiguate: true,
                ..Flags::DISABLED
            }
        );

        stack.pop(1);
        assert_eq!(stack.current(), Flags::default());
    }

    // Port of `test "FlagStack: pop big number"`.
    #[test]
    fn flagstack_pop_big_number() {
        let mut stack = FlagStack::default();
        stack.pop(100);
        assert_eq!(stack.current(), Flags::default());
    }

    // Port of `test "FlagStack: set"`.
    #[test]
    fn flagstack_set() {
        let mut stack = FlagStack::default();
        stack.set(
            SetMode::Set,
            Flags {
                disambiguate: true,
                ..Flags::DISABLED
            },
        );
        assert_eq!(
            stack.current(),
            Flags {
                disambiguate: true,
                ..Flags::DISABLED
            }
        );

        stack.set(
            SetMode::Or,
            Flags {
                report_events: true,
                ..Flags::DISABLED
            },
        );
        assert_eq!(
            stack.current(),
            Flags {
                disambiguate: true,
                report_events: true,
                ..Flags::DISABLED
            }
        );

        stack.set(
            SetMode::Not,
            Flags {
                report_events: true,
                ..Flags::DISABLED
            },
        );
        assert_eq!(
            stack.current(),
            Flags {
                disambiguate: true,
                ..Flags::DISABLED
            }
        );
    }
}
