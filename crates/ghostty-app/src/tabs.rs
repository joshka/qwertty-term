//! Tab registry: which tabs exist, which is active, and the working-directory
//! a new tab inherits.
//!
//! Platform-independent bookkeeping, unit-tested without AppKit. Each terminal
//! tab (its own `Engine` + PTY + render state) is identified by a [`TabId`]; the
//! registry tracks insertion order and the active tab so the controller can
//! answer "close the frontmost tab", "which tab receives this keystroke", and
//! "what pwd should the new tab start in". The AppKit `NSWindow`-per-tab objects
//! are held elsewhere (`crate::app`), keyed by the same [`TabId`].
//!
//! macOS native tabbing groups these windows visually (each tab is a real
//! `NSWindow` with `tabbingMode = .preferred`), but the *ownership* model — one
//! engine+PTY per tab — is exactly this registry.

use std::path::PathBuf;

/// A stable per-tab identifier. Monotonic; never reused within a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TabId(pub u64);

/// The registry of live tabs and the active one.
#[derive(Debug, Default)]
pub struct TabRegistry {
    /// Tabs in insertion order.
    order: Vec<TabId>,
    /// Index into `order` of the active tab, if any.
    active: Option<usize>,
    /// Next id to hand out.
    next_id: u64,
}

impl TabRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Mint a fresh tab id, append it, and make it active. Returns the new id.
    pub fn add(&mut self) -> TabId {
        let id = TabId(self.next_id);
        self.next_id += 1;
        self.order.push(id);
        self.active = Some(self.order.len() - 1);
        id
    }

    /// Remove `id`. If it was active, activate a neighbor (the previous tab, or
    /// the new last tab). Returns whether the tab existed.
    pub fn remove(&mut self, id: TabId) -> bool {
        let Some(pos) = self.order.iter().position(|t| *t == id) else {
            return false;
        };
        self.order.remove(pos);

        self.active = if self.order.is_empty() {
            None
        } else {
            // Keep the active pointer on a sensible neighbor: if we removed a
            // tab at or before the active one, step back (clamped); otherwise
            // the active index is unchanged but may now be out of range.
            let prev = self.active.unwrap_or(0);
            let new = if pos < prev {
                prev - 1
            } else {
                prev.min(self.order.len() - 1)
            };
            Some(new)
        };
        true
    }

    /// The active tab id, if any.
    pub fn active(&self) -> Option<TabId> {
        self.active.map(|i| self.order[i])
    }

    /// Make `id` the active tab. No-op (returns false) if it isn't registered.
    pub fn activate(&mut self, id: TabId) -> bool {
        if let Some(pos) = self.order.iter().position(|t| *t == id) {
            self.active = Some(pos);
            true
        } else {
            false
        }
    }

    /// The number of live tabs.
    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// All tab ids in order.
    pub fn ids(&self) -> &[TabId] {
        &self.order
    }
}

/// The working directory a *new* tab should start in, given the active tab's
/// reported OSC 7 pwd. Returns `Some(dir)` when the active tab reported a
/// still-existing directory; `None` (inherit the process cwd) otherwise.
///
/// Split out as pure logic so the new-tab inheritance rule is testable without a
/// live PTY: the caller passes the active engine's [`pwd`](crate::engine::Engine::pwd)
/// and this decides whether to honor it.
pub fn inherit_pwd(active_pwd: Option<&str>) -> Option<PathBuf> {
    let dir = PathBuf::from(active_pwd?);
    if dir.is_dir() { Some(dir) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_makes_new_tab_active() {
        let mut reg = TabRegistry::new();
        let a = reg.add();
        assert_eq!(reg.active(), Some(a));
        let b = reg.add();
        assert_eq!(reg.active(), Some(b));
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn ids_are_unique_and_monotonic() {
        let mut reg = TabRegistry::new();
        let a = reg.add();
        let b = reg.add();
        assert_ne!(a, b);
        assert!(b.0 > a.0);
    }

    #[test]
    fn remove_active_falls_back_to_neighbor() {
        let mut reg = TabRegistry::new();
        let a = reg.add();
        let b = reg.add();
        let c = reg.add();
        assert_eq!(reg.active(), Some(c));
        // Remove the active (last) tab → previous becomes active.
        reg.remove(c);
        assert_eq!(reg.active(), Some(b));
        // Remove a middle-of-order tab that's active → step back to `a`.
        reg.remove(b);
        assert_eq!(reg.active(), Some(a));
    }

    #[test]
    fn remove_non_active_keeps_active() {
        let mut reg = TabRegistry::new();
        let a = reg.add();
        let b = reg.add();
        reg.activate(a);
        reg.remove(b);
        assert_eq!(reg.active(), Some(a));
    }

    #[test]
    fn remove_last_tab_clears_active() {
        let mut reg = TabRegistry::new();
        let a = reg.add();
        reg.remove(a);
        assert_eq!(reg.active(), None);
        assert!(reg.is_empty());
    }

    #[test]
    fn remove_unknown_is_false() {
        let mut reg = TabRegistry::new();
        reg.add();
        assert!(!reg.remove(TabId(999)));
    }

    #[test]
    fn activate_unknown_is_false() {
        let mut reg = TabRegistry::new();
        assert!(!reg.activate(TabId(0)));
    }

    #[test]
    fn inherit_pwd_honors_existing_dir() {
        // The temp dir always exists.
        let tmp = std::env::temp_dir();
        let tmp_str = tmp.to_str().unwrap();
        assert_eq!(inherit_pwd(Some(tmp_str)), Some(tmp));
    }

    #[test]
    fn inherit_pwd_rejects_missing_dir_and_none() {
        assert_eq!(inherit_pwd(Some("/no/such/dir/anywhere/xyz")), None);
        assert_eq!(inherit_pwd(None), None);
    }
}
