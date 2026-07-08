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
    /// Copy the active tab's current selection to the clipboard (Cmd-C).
    /// No-op if there is no selection.
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
    /// Show the next tab (Window menu; standard macOS Cmd-Shift-]). Wraps.
    ShowNextTab,
    /// Show the previous tab (Window menu; standard macOS Cmd-Shift-[). Wraps.
    ShowPreviousTab,
    /// Quit the application (Cmd-Q).
    Quit,
}

impl MenuAction {
    /// All actions, in menu-construction order. Drives both the NSMenu build and
    /// the completeness test that every action has a key equivalent.
    pub const ALL: [MenuAction; 11] = [
        MenuAction::NewWindow,
        MenuAction::NewTab,
        MenuAction::CloseTab,
        MenuAction::Copy,
        MenuAction::Paste,
        MenuAction::FontSizeUp,
        MenuAction::FontSizeDown,
        MenuAction::FontSizeReset,
        MenuAction::ShowNextTab,
        MenuAction::ShowPreviousTab,
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
            MenuAction::ShowNextTab => "Show Next Tab",
            MenuAction::ShowPreviousTab => "Show Previous Tab",
            MenuAction::Quit => "Quit qwertty-term",
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
            // Standard macOS tab-cycling equivalents (Cmd-Shift-] / Cmd-Shift-[).
            MenuAction::ShowNextTab => ']',
            MenuAction::ShowPreviousTab => '[',
            MenuAction::Quit => 'q',
        }
    }

    /// Whether this action's key equivalent includes Shift (in addition to the
    /// Command modifier every menu item carries). The tab-cycling items use the
    /// standard macOS Cmd-Shift-]/[ equivalents; everything else is plain Cmd.
    pub fn key_equivalent_shift(self) -> bool {
        matches!(self, MenuAction::ShowNextTab | MenuAction::ShowPreviousTab)
    }

    /// Which top-level menu this action belongs under (App / Shell / Edit /
    /// View / Window). Drives the NSMenu grouping.
    pub fn menu(self) -> TopMenu {
        match self {
            MenuAction::Quit => TopMenu::App,
            MenuAction::NewWindow | MenuAction::NewTab | MenuAction::CloseTab => TopMenu::Shell,
            MenuAction::Copy | MenuAction::Paste => TopMenu::Edit,
            MenuAction::FontSizeUp | MenuAction::FontSizeDown | MenuAction::FontSizeReset => {
                TopMenu::View
            }
            MenuAction::ShowNextTab | MenuAction::ShowPreviousTab => TopMenu::Window,
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
            // ShowNextTab / ShowPreviousTab are Cmd-Shift chords, resolved via
            // the view's performKeyEquivalent tab path, not this Cmd-only map.
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
            MenuAction::ShowNextTab => 10,
            MenuAction::ShowPreviousTab => 11,
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
    /// The macOS Window menu — hosts the tab-cycling items (Show Next/Previous
    /// Tab), matching where real Ghostty's menu places them.
    Window,
}

impl TopMenu {
    /// The five top-level menus in bar order.
    pub const ALL: [TopMenu; 5] = [
        TopMenu::App,
        TopMenu::Shell,
        TopMenu::Edit,
        TopMenu::View,
        TopMenu::Window,
    ];

    pub fn title(self) -> &'static str {
        match self {
            TopMenu::App => "qwertty-term",
            TopMenu::Shell => "Shell",
            TopMenu::Edit => "Edit",
            TopMenu::View => "View",
            TopMenu::Window => "Window",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cmd_action_has_a_key_equivalent_that_round_trips() {
        for action in MenuAction::ALL {
            // The tab-cycling items are Cmd-Shift chords resolved via the view's
            // tab path, not the Cmd-only `for_key` map; skip them here.
            if action.key_equivalent_shift() {
                continue;
            }
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
    fn tab_cycling_items_live_under_window_menu_with_bracket_equivalents() {
        assert_eq!(MenuAction::ShowNextTab.menu(), TopMenu::Window);
        assert_eq!(MenuAction::ShowPreviousTab.menu(), TopMenu::Window);
        assert_eq!(MenuAction::ShowNextTab.key_equivalent(), ']');
        assert_eq!(MenuAction::ShowPreviousTab.key_equivalent(), '[');
        assert!(MenuAction::ShowNextTab.key_equivalent_shift());
        assert!(MenuAction::ShowPreviousTab.key_equivalent_shift());
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
