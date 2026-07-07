//! Menu actions: a platform-independent action model + dispatch, and (macOS)
//! the `NSMenu` construction that maps menu items and Cmd-key equivalents onto
//! it.
//!
//! The *actions* and the mapping from a key-equivalent (Cmd + a character) to an
//! action are pure data, unit-tested here without AppKit. The macOS
//! `build_menu` / selector wiring lives in [`crate::app`]; it routes every menu
//! item and `performKeyEquivalent` hit through [`MenuAction`] so there is a
//! single, testable definition of what each command does.

/// A menu command. Every menu item and every Cmd-key equivalent resolves to one
/// of these; the app's controller executes it against the active window/tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// New window (Cmd-N).
    NewWindow,
    /// New tab in the frontmost window (Cmd-T). Inherits the active tab's pwd.
    NewTab,
    /// Close the frontmost tab (Cmd-W). Closes the window when it's the last
    /// tab.
    CloseTab,
    /// Copy the current selection to the clipboard (Cmd-C). Selection is
    /// deferred for R5, so this is a no-op placeholder that still occupies the
    /// binding.
    Copy,
    /// Paste the clipboard into the active tab's PTY (Cmd-V), bracketed if the
    /// program enabled bracketed paste.
    Paste,
    /// Increase font size (Cmd-+/Cmd-=).
    FontSizeUp,
    /// Decrease font size (Cmd--).
    FontSizeDown,
    /// Reset font size to the configured default (Cmd-0).
    FontSizeReset,
    /// Quit the application (Cmd-Q).
    Quit,
}

impl MenuAction {
    /// All actions, in menu-construction order. Drives both the NSMenu build and
    /// the completeness test that every action has a key equivalent.
    pub const ALL: [MenuAction; 9] = [
        MenuAction::NewWindow,
        MenuAction::NewTab,
        MenuAction::CloseTab,
        MenuAction::Copy,
        MenuAction::Paste,
        MenuAction::FontSizeUp,
        MenuAction::FontSizeDown,
        MenuAction::FontSizeReset,
        MenuAction::Quit,
    ];

    /// The human-readable menu-item title.
    pub fn title(self) -> &'static str {
        match self {
            MenuAction::NewWindow => "New Window",
            MenuAction::NewTab => "New Tab",
            MenuAction::CloseTab => "Close Tab",
            MenuAction::Copy => "Copy",
            MenuAction::Paste => "Paste",
            MenuAction::FontSizeUp => "Increase Font Size",
            MenuAction::FontSizeDown => "Decrease Font Size",
            MenuAction::FontSizeReset => "Reset Font Size",
            MenuAction::Quit => "Quit ghostty-rs",
        }
    }

    /// The Cmd-key equivalent character for this action (the menu uses Command
    /// as the modifier for all of them). Lowercase; AppKit adds Shift display as
    /// needed. `+` and `=` both map to FontSizeUp via [`MenuAction::for_key`].
    pub fn key_equivalent(self) -> char {
        match self {
            MenuAction::NewWindow => 'n',
            MenuAction::NewTab => 't',
            MenuAction::CloseTab => 'w',
            MenuAction::Copy => 'c',
            MenuAction::Paste => 'v',
            MenuAction::FontSizeUp => '=', // Cmd-= is Cmd-+ without Shift
            MenuAction::FontSizeDown => '-',
            MenuAction::FontSizeReset => '0',
            MenuAction::Quit => 'q',
        }
    }

    /// Which top-level menu this action belongs under (App / Shell / Edit /
    /// View). Drives the NSMenu grouping.
    pub fn menu(self) -> TopMenu {
        match self {
            MenuAction::Quit => TopMenu::App,
            MenuAction::NewWindow | MenuAction::NewTab | MenuAction::CloseTab => TopMenu::Shell,
            MenuAction::Copy | MenuAction::Paste => TopMenu::Edit,
            MenuAction::FontSizeUp | MenuAction::FontSizeDown | MenuAction::FontSizeReset => {
                TopMenu::View
            }
        }
    }

    /// Resolve a Cmd-key press (the character AppKit reports, lowercased) to an
    /// action, or `None` if unbound. Handles the `+`/`=` synonym for
    /// FontSizeUp.
    pub fn for_key(ch: char) -> Option<MenuAction> {
        let ch = ch.to_ascii_lowercase();
        match ch {
            'n' => Some(MenuAction::NewWindow),
            't' => Some(MenuAction::NewTab),
            'w' => Some(MenuAction::CloseTab),
            'c' => Some(MenuAction::Copy),
            'v' => Some(MenuAction::Paste),
            '=' | '+' => Some(MenuAction::FontSizeUp),
            '-' => Some(MenuAction::FontSizeDown),
            '0' => Some(MenuAction::FontSizeReset),
            'q' => Some(MenuAction::Quit),
            _ => None,
        }
    }

    /// A stable integer tag, used to round-trip an action through an NSMenuItem's
    /// `tag` so a single Objective-C action selector can recover which command
    /// fired.
    pub fn tag(self) -> isize {
        match self {
            MenuAction::NewWindow => 1,
            MenuAction::NewTab => 2,
            MenuAction::CloseTab => 3,
            MenuAction::Copy => 4,
            MenuAction::Paste => 5,
            MenuAction::FontSizeUp => 6,
            MenuAction::FontSizeDown => 7,
            MenuAction::FontSizeReset => 8,
            MenuAction::Quit => 9,
        }
    }

    /// Recover an action from its [`MenuAction::tag`].
    pub fn from_tag(tag: isize) -> Option<MenuAction> {
        MenuAction::ALL.into_iter().find(|a| a.tag() == tag)
    }
}

/// A top-level menu bar entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopMenu {
    App,
    Shell,
    Edit,
    View,
}

impl TopMenu {
    /// The four top-level menus in bar order.
    pub const ALL: [TopMenu; 4] = [TopMenu::App, TopMenu::Shell, TopMenu::Edit, TopMenu::View];

    pub fn title(self) -> &'static str {
        match self {
            TopMenu::App => "ghostty-rs",
            TopMenu::Shell => "Shell",
            TopMenu::Edit => "Edit",
            TopMenu::View => "View",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_has_a_key_equivalent_that_round_trips() {
        for action in MenuAction::ALL {
            let ch = action.key_equivalent();
            // for_key must resolve the equivalent back to the same action
            // (FontSizeUp's '=' is its canonical equivalent).
            assert_eq!(
                MenuAction::for_key(ch),
                Some(action),
                "{action:?} key '{ch}' did not round-trip"
            );
        }
    }

    #[test]
    fn tags_round_trip_and_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for action in MenuAction::ALL {
            assert!(seen.insert(action.tag()), "duplicate tag for {action:?}");
            assert_eq!(MenuAction::from_tag(action.tag()), Some(action));
        }
        assert_eq!(MenuAction::from_tag(999), None);
    }

    #[test]
    fn plus_and_equals_both_increase_font_size() {
        assert_eq!(MenuAction::for_key('+'), Some(MenuAction::FontSizeUp));
        assert_eq!(MenuAction::for_key('='), Some(MenuAction::FontSizeUp));
    }

    #[test]
    fn key_matching_is_case_insensitive() {
        assert_eq!(MenuAction::for_key('N'), Some(MenuAction::NewWindow));
        assert_eq!(MenuAction::for_key('Q'), Some(MenuAction::Quit));
    }

    #[test]
    fn unbound_key_is_none() {
        assert_eq!(MenuAction::for_key('z'), None);
    }

    #[test]
    fn actions_are_grouped_under_all_four_menus() {
        for menu in TopMenu::ALL {
            assert!(
                MenuAction::ALL.iter().any(|a| a.menu() == menu),
                "{menu:?} has no actions"
            );
        }
    }
}
