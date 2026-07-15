//! Right-click context menu: the pure, testable model of what the menu
//! contains and how each item maps to a controller action. [`crate::view`]
//! builds the real `NSMenu` from [`context_items`] and routes clicks back
//! through [`ContextAction::from_tag`]; [`crate::app`] performs them against
//! the right-clicked pane.
//!
//! Port of the right-click menu in upstream `SurfaceView_AppKit.swift`
//! (`menu(for:)`, ~L1577): Copy (only with a selection) / Paste / splits /
//! close. The inspector/read-only/reset/title items upstream also shows are
//! out of scope for this app; the split + copy/paste/close core is what
//! ports cleanly onto our existing controller actions.
//!
//! `right-click-action` (`Config.zig:8591`) selects the behavior: `context-menu`
//! (default) shows this menu; `paste`/`copy`/`copy-or-paste` act directly; and
//! `ignore` does nothing (the event falls through to mouse reporting).

use crate::splits::Direction;

/// What `right-click-action` does. Port of upstream `RightClickAction`
/// (default `context-menu`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RightClickAction {
    /// Show the context menu (upstream default).
    #[default]
    ContextMenu,
    /// Paste the system clipboard.
    Paste,
    /// Copy the selection to the system clipboard.
    Copy,
    /// Copy the selection, or paste when there's no selection.
    CopyOrPaste,
    /// Do nothing (fall through to mouse reporting).
    Ignore,
}

impl RightClickAction {
    /// Parse the config spelling; unknown values fall back to the default.
    pub fn parse(s: &str) -> RightClickAction {
        match s.trim() {
            "context-menu" => RightClickAction::ContextMenu,
            "paste" => RightClickAction::Paste,
            "copy" => RightClickAction::Copy,
            "copy-or-paste" => RightClickAction::CopyOrPaste,
            "ignore" => RightClickAction::Ignore,
            _ => RightClickAction::default(),
        }
    }
}

/// A single actionable context-menu command. Round-trips through an
/// `NSMenuItem.tag` so one Objective-C selector can recover which fired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    Copy,
    Paste,
    SplitRight,
    SplitLeft,
    SplitDown,
    SplitUp,
    ClosePane,
}

impl ContextAction {
    /// The menu-item title.
    pub fn title(self) -> &'static str {
        match self {
            ContextAction::Copy => "Copy",
            ContextAction::Paste => "Paste",
            ContextAction::SplitRight => "Split Right",
            ContextAction::SplitLeft => "Split Left",
            ContextAction::SplitDown => "Split Down",
            ContextAction::SplitUp => "Split Up",
            ContextAction::ClosePane => "Close Pane",
        }
    }

    /// A stable non-zero tag (0 is reserved for "no tag" on NSMenuItem).
    pub fn tag(self) -> isize {
        match self {
            ContextAction::Copy => 1,
            ContextAction::Paste => 2,
            ContextAction::SplitRight => 3,
            ContextAction::SplitLeft => 4,
            ContextAction::SplitDown => 5,
            ContextAction::SplitUp => 6,
            ContextAction::ClosePane => 7,
        }
    }

    /// Recover an action from its [`ContextAction::tag`].
    pub fn from_tag(tag: isize) -> Option<ContextAction> {
        [
            ContextAction::Copy,
            ContextAction::Paste,
            ContextAction::SplitRight,
            ContextAction::SplitLeft,
            ContextAction::SplitDown,
            ContextAction::SplitUp,
            ContextAction::ClosePane,
        ]
        .into_iter()
        .find(|a| a.tag() == tag)
    }

    /// The split direction for the split actions (else `None`).
    pub fn split_direction(self) -> Option<Direction> {
        match self {
            ContextAction::SplitRight => Some(Direction::Right),
            ContextAction::SplitLeft => Some(Direction::Left),
            ContextAction::SplitDown => Some(Direction::Down),
            ContextAction::SplitUp => Some(Direction::Up),
            _ => None,
        }
    }
}

/// One entry in the built menu: an actionable item or a separator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextItem {
    Action(ContextAction),
    Separator,
}

/// The ordered context-menu items for a pane. `has_selection` gates the Copy
/// item (upstream only adds Copy when there's selected text). Mirrors the
/// upstream group order: copy/paste · splits · close.
pub fn context_items(has_selection: bool) -> Vec<ContextItem> {
    let mut items = Vec::new();
    if has_selection {
        items.push(ContextItem::Action(ContextAction::Copy));
    }
    items.push(ContextItem::Action(ContextAction::Paste));
    items.push(ContextItem::Separator);
    items.push(ContextItem::Action(ContextAction::SplitRight));
    items.push(ContextItem::Action(ContextAction::SplitLeft));
    items.push(ContextItem::Action(ContextAction::SplitDown));
    items.push(ContextItem::Action(ContextAction::SplitUp));
    items.push(ContextItem::Separator);
    items.push(ContextItem::Action(ContextAction::ClosePane));
    items
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn right_click_action_parses() {
        assert_eq!(
            RightClickAction::parse("context-menu"),
            RightClickAction::ContextMenu
        );
        assert_eq!(RightClickAction::parse("paste"), RightClickAction::Paste);
        assert_eq!(RightClickAction::parse("ignore"), RightClickAction::Ignore);
        // Unknown → default (context-menu).
        assert_eq!(
            RightClickAction::parse("nonsense"),
            RightClickAction::ContextMenu
        );
        assert_eq!(RightClickAction::default(), RightClickAction::ContextMenu);
    }

    #[test]
    fn tags_round_trip_and_are_unique_and_nonzero() {
        let all = [
            ContextAction::Copy,
            ContextAction::Paste,
            ContextAction::SplitRight,
            ContextAction::SplitLeft,
            ContextAction::SplitDown,
            ContextAction::SplitUp,
            ContextAction::ClosePane,
        ];
        let mut seen = std::collections::HashSet::new();
        for a in all {
            assert_ne!(a.tag(), 0, "{a:?} has the reserved 0 tag");
            assert!(seen.insert(a.tag()), "duplicate tag for {a:?}");
            assert_eq!(ContextAction::from_tag(a.tag()), Some(a));
        }
        assert_eq!(ContextAction::from_tag(999), None);
    }

    #[test]
    fn copy_only_present_with_selection() {
        let with = context_items(true);
        assert_eq!(with[0], ContextItem::Action(ContextAction::Copy));
        assert_eq!(with[1], ContextItem::Action(ContextAction::Paste));

        let without = context_items(false);
        // No Copy → Paste is first.
        assert_eq!(without[0], ContextItem::Action(ContextAction::Paste));
        assert!(!without.contains(&ContextItem::Action(ContextAction::Copy)));
    }

    #[test]
    fn menu_has_splits_and_close_grouped_by_separators() {
        let items = context_items(true);
        // Two separators split the three groups (copy/paste · splits · close).
        let seps = items
            .iter()
            .filter(|i| **i == ContextItem::Separator)
            .count();
        assert_eq!(seps, 2);
        // Close is last.
        assert_eq!(
            *items.last().unwrap(),
            ContextItem::Action(ContextAction::ClosePane)
        );
        // All four split directions are present.
        for a in [
            ContextAction::SplitRight,
            ContextAction::SplitLeft,
            ContextAction::SplitDown,
            ContextAction::SplitUp,
        ] {
            assert!(items.contains(&ContextItem::Action(a)), "missing {a:?}");
            assert!(a.split_direction().is_some());
        }
    }
}
