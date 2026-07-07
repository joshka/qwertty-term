//! The set of screens behind a terminal (primary + lazy alternate). Port of
//! `src/terminal/ScreenSet.zig`.
//!
//! Initially just primary vs. alternate. The primary screen is always
//! initialized; the alternate is created lazily on first use. A monotonic
//! generation counter per key lets external handles detect a re-initialized
//! screen after a `remove`.

use crate::screen::{Options, Screen};

/// The key for a screen in the set. Port of `ScreenSet.Key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenKey {
    Primary,
    Alternate,
}

/// A set of terminal screens. Port of `ScreenSet`.
pub struct ScreenSet {
    active_key: ScreenKey,
    primary: Box<Screen>,
    alternate: Option<Box<Screen>>,
    /// Options used to lazily initialize the alternate screen.
    opts: Options,
    gen_primary: usize,
    gen_alternate: usize,
}

impl ScreenSet {
    /// Initialize a new set with the primary screen. Port of `init`.
    pub fn new(opts: Options) -> ScreenSet {
        ScreenSet {
            active_key: ScreenKey::Primary,
            primary: Box::new(Screen::init(opts)),
            alternate: None,
            opts,
            gen_primary: 0,
            gen_alternate: 0,
        }
    }

    /// The active screen key. Port of `active_key`.
    #[inline]
    pub fn active_key(&self) -> ScreenKey {
        self.active_key
    }

    /// The active screen. Port of `active`.
    #[inline]
    pub fn active(&self) -> &Screen {
        match self.active_key {
            ScreenKey::Primary => &self.primary,
            ScreenKey::Alternate => self
                .alternate
                .as_ref()
                .expect("alternate active but not initialized"),
        }
    }

    /// The active screen (mutable).
    #[inline]
    pub fn active_mut(&mut self) -> &mut Screen {
        match self.active_key {
            ScreenKey::Primary => &mut self.primary,
            ScreenKey::Alternate => self
                .alternate
                .as_mut()
                .expect("alternate active but not initialized"),
        }
    }

    /// Get the screen for `key`, if initialized. Port of `get`.
    pub fn get(&self, key: ScreenKey) -> Option<&Screen> {
        match key {
            ScreenKey::Primary => Some(&self.primary),
            ScreenKey::Alternate => self.alternate.as_deref(),
        }
    }

    /// The current generation for `key`. Port of `generation`.
    pub fn generation(&self, key: ScreenKey) -> usize {
        match key {
            ScreenKey::Primary => self.gen_primary,
            ScreenKey::Alternate => self.gen_alternate,
        }
    }

    /// Get the screen for `key`, initializing it if necessary. Port of `getInit`.
    pub fn get_init(&mut self, key: ScreenKey) -> &mut Screen {
        match key {
            ScreenKey::Primary => &mut self.primary,
            ScreenKey::Alternate => {
                if self.alternate.is_none() {
                    self.alternate = Some(Box::new(Screen::init(self.opts)));
                }
                self.alternate.as_mut().unwrap()
            }
        }
    }

    /// Remove a key from the set (primary cannot be removed). Port of `remove`.
    pub fn remove(&mut self, key: ScreenKey) {
        debug_assert_ne!(key, ScreenKey::Primary);
        if key == ScreenKey::Alternate && self.alternate.take().is_some() {
            self.gen_alternate = self.gen_alternate.wrapping_add(1);
        }
    }

    /// Switch the active screen to `key` (must be initialized). Port of `switchTo`.
    pub fn switch_to(&mut self, key: ScreenKey) {
        debug_assert!(self.get(key).is_some(), "switch_to uninitialized screen");
        self.active_key = key;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of `test ScreenSet`.
    #[test]
    fn screen_set_basic() {
        let mut set = ScreenSet::new(Options::default());
        assert_eq!(set.active_key(), ScreenKey::Primary);
        assert_eq!(set.generation(ScreenKey::Primary), 0);
        assert_eq!(set.generation(ScreenKey::Alternate), 0);

        // Initialize a secondary screen.
        let _ = set.get_init(ScreenKey::Alternate);
        assert_eq!(set.generation(ScreenKey::Alternate), 0);

        // Switch to it.
        set.switch_to(ScreenKey::Alternate);
        assert_eq!(set.active_key(), ScreenKey::Alternate);

        // Remove it bumps the generation and switches back requires init.
        set.switch_to(ScreenKey::Primary);
        set.remove(ScreenKey::Alternate);
        assert_eq!(set.generation(ScreenKey::Alternate), 1);
        assert!(set.get(ScreenKey::Alternate).is_none());
    }
}
