//! Binding flags. Port of the `Flags` packed struct in `input/Binding.zig`
//! (upstream `2da015cd6`, lines 31-70).
//!
//! The four prefixes (`unconsumed:`, `all:`, `global:`, `performable:`) each
//! set one field. The C ABI bit layout is consumed=1, all=2, global=4,
//! performable=8 (verified by the `Flags cval` upstream test); we don't expose
//! the packed integer here, but keep the field set and defaults identical.

/// Flags that modify how a binding's action is dispatched.
///
/// `consumed` defaults to `true`; the other three default to `false` — matching
/// `Binding.Flags`'s field defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Flags {
    /// When the action fires, the key input is consumed and NOT encoded to the
    /// pty. `unconsumed:` clears this so the action fires *and* the key is still
    /// forwarded.
    pub consumed: bool,

    /// The binding is forwarded to all active surfaces in the application, not
    /// just the focused one. Set by `all:`.
    pub all: bool,

    /// A system-wide binding that works even when the app is unfocused. Set by
    /// `global:`. "May not work on all platforms."
    pub global: bool,

    /// The binding only triggers if the action *can* be performed; otherwise
    /// the key falls through to normal encoding as if unbound. Set by
    /// `performable:`. Performable bindings are also excluded from the reverse
    /// (action→trigger) map so GUI toolkits don't register them as menu
    /// accelerators.
    pub performable: bool,
}

impl Default for Flags {
    fn default() -> Self {
        Flags {
            consumed: true,
            all: false,
            global: false,
            performable: false,
        }
    }
}

impl Flags {
    /// A fresh flag set with upstream defaults (`consumed = true`).
    pub fn new() -> Self {
        Flags::default()
    }
}
