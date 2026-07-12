//! The default keybind set. Port of `Config.Keybinds.init`
//! (upstream `2da015cd6`, `config/Config.zig:6389-7158`).
//!
//! This is a mechanical, per-entry port of the Zig builder: every `.put` /
//! `.putFlags` call there becomes one [`Set::put`] here, with the same trigger
//! (physical vs unicode exactly as the source spells it), the same modifiers,
//! the same action and parameters, and the same `performable` flag. Each
//! binding cites its `Config.zig` line in a trailing comment.
//!
//! ## Platform split
//!
//! The Zig builder resolves three OS-dependent shapes at `comptime`:
//!
//! - `inputpkg.ctrlOrSuper(mods)` adds `super` on Darwin, `ctrl` elsewhere
//!   (`input/key.zig:862`).
//! - `if (isDarwin) .{ .super = true } else …` blocks pick a whole modifier
//!   set (the clipboard c/v mods and the goto-tab mods).
//! - `if (isDarwin)` / `if (!isDarwin)` blocks gate entire binding groups (the
//!   non-Darwin "Windowing" block and the Darwin "Mac-specific" block).
//!
//! We reproduce that split with `#[cfg(target_os = "macos")]`: [`build_macos`]
//! installs the Darwin-effective keymap (used by this macOS-first project),
//! [`build_other`] installs the non-Darwin keymap. Because the Darwin/non-Darwin
//! choice is made at compile time in Zig too, no runtime branch is needed.
//!
//! ## `text:` action values
//!
//! The natural-text-editing binds store *raw* Zig string-literal syntax, e.g.
//! the source's `.{ .text = "\\x05" }` is the four bytes `\x05` (backslash, x,
//! 0, 5), decoded to the control byte only at execution time. [`Action::Text`]
//! is documented as holding unparsed Zig string-literal syntax, so we store the
//! same four bytes verbatim (`"\\x05"` in Rust source) rather than a decoded
//! `U+0005`.

use crate::key::Key;
use crate::key_mods::Mods;

use super::action::{
    Action, CloseTabMode, CopyToClipboard, ResizeSplit, SplitDirection, SplitFocusDirection,
    SplitResizeDirection, WriteScreen, WriteScreenAction, WriteScreenFormat,
};
use super::flags::Flags;
use super::parser::Binding;
use super::set::Set;
use super::trigger::{Trigger, TriggerKey};

/// A `Mods` literal from the set of flags that are `true`, e.g.
/// `mods!(super_, shift)` mirrors the Zig `.{ .super = true, .shift = true }`.
/// `mods!()` is the empty modifier set.
macro_rules! mods {
    ($($field:ident),* $(,)?) => {
        Mods { $($field: true,)* ..Mods::default() }
    };
}

/// A physical-key trigger (Zig `.{ .physical = … }`).
fn phys(key: Key, mods: Mods) -> Trigger {
    Trigger {
        key: TriggerKey::Physical(key),
        mods,
    }
}

/// A unicode-codepoint trigger (Zig `.{ .unicode = … }`).
fn uni(cp: char, mods: Mods) -> Trigger {
    Trigger {
        key: TriggerKey::Unicode(cp as u32),
        mods,
    }
}

/// `Set.put`: a plain (consumed, non-performable) binding.
fn put(set: &mut Set, trigger: Trigger, action: Action) {
    set.put(Binding {
        trigger,
        action,
        flags: Flags::default(),
    });
}

/// `Set.putFlags` with `.{ .performable = true }`.
fn put_perf(set: &mut Set, trigger: Trigger, action: Action) {
    set.put(Binding {
        trigger,
        action,
        flags: Flags {
            performable: true,
            ..Flags::default()
        },
    });
}

/// `write_screen_file` with the given action and the default `plain` format,
/// matching the Zig `WriteScreen.copy`/`.paste`/`.open` constants
/// (Binding.zig:1118-1120).
fn write_screen(action: WriteScreenAction) -> Action {
    Action::WriteScreenFile(WriteScreen {
        action,
        format: WriteScreenFormat::Plain,
    })
}

/// The eight physical digit keys used by the goto-tab loop (Config.zig:6791).
const DIGIT_KEYS: [Key; 8] = [
    Key::Digit1,
    Key::Digit2,
    Key::Digit3,
    Key::Digit4,
    Key::Digit5,
    Key::Digit6,
    Key::Digit7,
    Key::Digit8,
];

/// Build the default keybind set for the current platform. Port of
/// `Config.Keybinds.init` (Config.zig:6389-7158).
pub fn default_set() -> Set {
    let mut set = Set::new();
    #[cfg(target_os = "macos")]
    build_macos(&mut set);
    #[cfg(not(target_os = "macos"))]
    build_other(&mut set);
    set
}

/// Install the Darwin-effective keymap: the all-platform binds with the Darwin
/// spelling of `ctrlOrSuper` (`super`), the Darwin clipboard/goto-tab modifier
/// choices, and the Mac-specific block (Config.zig:6871-7157). The non-Darwin
/// "Windowing" block (6569-6779) is not installed.
#[cfg(target_os = "macos")]
fn build_macos(set: &mut Set) {
    // Opening and reloading config (Config.zig:6398-6408).
    put(set, uni(',', mods!(super_, shift)), Action::ReloadConfig); // 6399
    put(set, uni(',', mods!(super_)), Action::OpenConfig); // 6404

    // Clipboard (Config.zig:6410-6460).
    put(
        set,
        phys(Key::Copy, mods!()),
        Action::CopyToClipboard(CopyToClipboard::Mixed),
    ); // 6411
    put(set, phys(Key::Paste, mods!()), Action::PasteFromClipboard); // 6416
    // (ctrl+insert / shift+insert are non-Darwin only: 6428-6439, skipped.)
    // macOS defaults clipboard to super (ctrl+c would kill the process).
    put_perf(
        set,
        uni('c', mods!(super_)),
        Action::CopyToClipboard(CopyToClipboard::Mixed),
    ); // 6448
    put_perf(set, uni('v', mods!(super_)), Action::PasteFromClipboard); // 6454

    // Font size (Config.zig:6462-6486). ctrlOrSuper -> super.
    put(set, uni('=', mods!(super_)), Action::IncreaseFontSize(1.0)); // 6466
    put(set, uni('+', mods!(super_)), Action::IncreaseFontSize(1.0)); // 6471
    put(set, uni('-', mods!(super_)), Action::DecreaseFontSize(1.0)); // 6477
    put(set, uni('0', mods!(super_)), Action::ResetFontSize); // 6482

    // Write screen to file (Config.zig:6488-6504).
    // 6488 hardcodes shift+ctrl+super (not ctrlOrSuper).
    put(
        set,
        uni('j', mods!(shift, ctrl, super_)),
        write_screen(WriteScreenAction::Copy),
    ); // 6488
    put(
        set,
        uni('j', mods!(super_, shift)),
        write_screen(WriteScreenAction::Paste),
    ); // 6494
    put(
        set,
        uni('j', mods!(super_, shift, alt)),
        write_screen(WriteScreenAction::Open),
    ); // 6500

    // Expand selection (Config.zig:6506-6554), all performable.
    put_perf(
        set,
        phys(Key::ArrowLeft, mods!(shift)),
        Action::AdjustSelection(super::action::AdjustSelection::Left),
    ); // 6507
    put_perf(
        set,
        phys(Key::ArrowRight, mods!(shift)),
        Action::AdjustSelection(super::action::AdjustSelection::Right),
    ); // 6513
    put_perf(
        set,
        phys(Key::ArrowUp, mods!(shift)),
        Action::AdjustSelection(super::action::AdjustSelection::Up),
    ); // 6519
    put_perf(
        set,
        phys(Key::ArrowDown, mods!(shift)),
        Action::AdjustSelection(super::action::AdjustSelection::Down),
    ); // 6525
    put_perf(
        set,
        phys(Key::PageUp, mods!(shift)),
        Action::AdjustSelection(super::action::AdjustSelection::PageUp),
    ); // 6531
    put_perf(
        set,
        phys(Key::PageDown, mods!(shift)),
        Action::AdjustSelection(super::action::AdjustSelection::PageDown),
    ); // 6537
    put_perf(
        set,
        phys(Key::Home, mods!(shift)),
        Action::AdjustSelection(super::action::AdjustSelection::Home),
    ); // 6543
    put_perf(
        set,
        phys(Key::End, mods!(shift)),
        Action::AdjustSelection(super::action::AdjustSelection::End),
    ); // 6549

    // Tabs common to all platforms (Config.zig:6556-6566).
    put(set, phys(Key::Tab, mods!(ctrl, shift)), Action::PreviousTab); // 6557
    put(set, phys(Key::Tab, mods!(ctrl)), Action::NextTab); // 6562

    // (The non-Darwin "Windowing" block, 6569-6779, is not installed on macOS.)

    // Goto tab (Config.zig:6780-6847). macOS: super, performable = !isDarwin =
    // false, so these register as plain (consumed) binds. We register BOTH the
    // physical digit key and the unicode digit for layouts like AZERTY.
    for (idx, &key) in DIGIT_KEYS.iter().enumerate() {
        let n = idx + 1; // goto_tab index (1-based)
        let digit = char::from_digit(n as u32, 10).expect("1..=8 is a digit");
        put(set, phys(key, mods!(super_)), Action::GotoTab(n)); // 6799
        put(set, uni(digit, mods!(super_)), Action::GotoTab(n)); // 6823
    }
    put(set, uni('9', mods!(super_)), Action::LastTab); // 6835

    // Toggle fullscreen / split zoom / command palette (Config.zig:6849-6868).
    put(
        set,
        phys(Key::Enter, mods!(super_)),
        Action::ToggleFullscreen,
    ); // 6850
    put(
        set,
        phys(Key::Enter, mods!(super_, shift)),
        Action::ToggleSplitZoom,
    ); // 6857
    put(
        set,
        uni('p', mods!(super_, shift)),
        Action::ToggleCommandPalette,
    ); // 6864

    // --- Mac-specific keyboard bindings (Config.zig:6871-7157). ---
    put(set, uni('q', mods!(super_)), Action::Quit); // 6872
    put_perf(set, uni('k', mods!(super_)), Action::ClearScreen); // 6877
    put(set, uni('a', mods!(super_)), Action::SelectAll); // 6883

    // Undo / redo (Config.zig:6889-6907).
    put_perf(set, uni('t', mods!(super_, shift)), Action::Undo); // 6890
    put_perf(set, uni('z', mods!(super_)), Action::Undo); // 6896
    put_perf(set, uni('z', mods!(super_, shift)), Action::Redo); // 6902

    // Viewport scrolling (Config.zig:6909-6935).
    put(set, phys(Key::Home, mods!(super_)), Action::ScrollToTop); // 6910
    put(set, phys(Key::End, mods!(super_)), Action::ScrollToBottom); // 6915
    put(set, phys(Key::PageUp, mods!(super_)), Action::ScrollPageUp); // 6920
    put(
        set,
        phys(Key::PageDown, mods!(super_)),
        Action::ScrollPageDown,
    ); // 6925
    put_perf(set, uni('j', mods!(super_)), Action::ScrollToSelection); // 6930

    // Semantic prompts (Config.zig:6937-6947).
    put(
        set,
        phys(Key::ArrowUp, mods!(super_, shift)),
        Action::JumpToPrompt(-1),
    ); // 6938
    put(
        set,
        phys(Key::ArrowDown, mods!(super_, shift)),
        Action::JumpToPrompt(1),
    ); // 6943

    // Mac windowing (Config.zig:6949-6999).
    put(set, uni('n', mods!(super_)), Action::NewWindow); // 6950
    put(set, uni('w', mods!(super_)), Action::CloseSurface); // 6955
    put(
        set,
        uni('w', mods!(super_, alt)),
        Action::CloseTab(CloseTabMode::This),
    ); // 6960
    put(set, uni('w', mods!(super_, shift)), Action::CloseWindow); // 6965
    put(
        set,
        uni('w', mods!(super_, shift, alt)),
        Action::CloseAllWindows,
    ); // 6970
    put(set, uni('t', mods!(super_)), Action::NewTab); // 6975
    put(set, uni('[', mods!(super_, shift)), Action::PreviousTab); // 6980
    put(set, uni(']', mods!(super_, shift)), Action::NextTab); // 6985
    put(
        set,
        uni('d', mods!(super_)),
        Action::NewSplit(SplitDirection::Right),
    ); // 6990
    put(
        set,
        uni('d', mods!(super_, shift)),
        Action::NewSplit(SplitDirection::Down),
    ); // 6995

    // Goto split (Config.zig:7000-7029).
    put(
        set,
        uni('[', mods!(super_)),
        Action::GotoSplit(SplitFocusDirection::Previous),
    ); // 7000
    put(
        set,
        uni(']', mods!(super_)),
        Action::GotoSplit(SplitFocusDirection::Next),
    ); // 7005
    put(
        set,
        phys(Key::ArrowUp, mods!(super_, alt)),
        Action::GotoSplit(SplitFocusDirection::Up),
    ); // 7010
    put(
        set,
        phys(Key::ArrowDown, mods!(super_, alt)),
        Action::GotoSplit(SplitFocusDirection::Down),
    ); // 7015
    put(
        set,
        phys(Key::ArrowLeft, mods!(super_, alt)),
        Action::GotoSplit(SplitFocusDirection::Left),
    ); // 7020
    put(
        set,
        phys(Key::ArrowRight, mods!(super_, alt)),
        Action::GotoSplit(SplitFocusDirection::Right),
    ); // 7025

    // Resize split (Config.zig:7030-7049).
    put(
        set,
        phys(Key::ArrowUp, mods!(super_, ctrl)),
        Action::ResizeSplit(ResizeSplit {
            direction: SplitResizeDirection::Up,
            amount: 10,
        }),
    ); // 7030
    put(
        set,
        phys(Key::ArrowDown, mods!(super_, ctrl)),
        Action::ResizeSplit(ResizeSplit {
            direction: SplitResizeDirection::Down,
            amount: 10,
        }),
    ); // 7035
    put(
        set,
        phys(Key::ArrowLeft, mods!(super_, ctrl)),
        Action::ResizeSplit(ResizeSplit {
            direction: SplitResizeDirection::Left,
            amount: 10,
        }),
    ); // 7040
    put(
        set,
        phys(Key::ArrowRight, mods!(super_, ctrl)),
        Action::ResizeSplit(ResizeSplit {
            direction: SplitResizeDirection::Right,
            amount: 10,
        }),
    ); // 7045
    put(set, uni('=', mods!(super_, ctrl)), Action::EqualizeSplits); // 7050

    // Jump to prompt, matches Terminal.app (Config.zig:7056-7066).
    put(
        set,
        phys(Key::ArrowUp, mods!(super_)),
        Action::JumpToPrompt(-1),
    ); // 7057
    put(
        set,
        phys(Key::ArrowDown, mods!(super_)),
        Action::JumpToPrompt(1),
    ); // 7062

    // Search (Config.zig:7068-7104).
    put_perf(set, uni('f', mods!(super_)), Action::StartSearch); // 7069
    put_perf(set, uni('e', mods!(super_)), Action::SearchSelection); // 7075
    put_perf(set, uni('f', mods!(super_, shift)), Action::EndSearch); // 7081
    put_perf(set, phys(Key::Escape, mods!()), Action::EndSearch); // 7087
    put_perf(
        set,
        uni('g', mods!(super_)),
        Action::NavigateSearch(super::action::NavigateSearch::Next),
    ); // 7093
    put_perf(
        set,
        uni('g', mods!(super_, shift)),
        Action::NavigateSearch(super::action::NavigateSearch::Previous),
    ); // 7099

    // Inspector, matching Chromium (Config.zig:7106-7111).
    put(
        set,
        uni('i', mods!(alt, super_)),
        Action::Inspector(super::action::InspectorMode::Toggle),
    ); // 7107

    // Alternate fullscreen, common to Mac programs (Config.zig:7113-7118).
    put(set, uni('f', mods!(super_, ctrl)), Action::ToggleFullscreen); // 7114

    // Selection clipboard paste, matches Terminal.app (Config.zig:7120-7125).
    put(
        set,
        uni('v', mods!(super_, shift)),
        Action::PasteFromSelection,
    ); // 7121

    // "Natural text editing" keybinds (Config.zig:7127-7156). Text values are
    // stored as raw Zig string-literal syntax (see the module docs).
    put(
        set,
        phys(Key::ArrowRight, mods!(super_)),
        Action::Text("\\x05".into()),
    ); // 7132
    put(
        set,
        phys(Key::ArrowLeft, mods!(super_)),
        Action::Text("\\x01".into()),
    ); // 7137
    put(
        set,
        phys(Key::Backspace, mods!(super_)),
        Action::Text("\\x15".into()),
    ); // 7142
    put(
        set,
        phys(Key::ArrowLeft, mods!(alt)),
        Action::Esc("b".into()),
    ); // 7147
    put(
        set,
        phys(Key::ArrowRight, mods!(alt)),
        Action::Esc("f".into()),
    ); // 7152
}

/// Install the non-Darwin keymap: the all-platform binds with the non-Darwin
/// spelling of `ctrlOrSuper` (`ctrl`), the non-Darwin clipboard/goto-tab
/// choices, and the "Windowing" block (Config.zig:6569-6779). The Mac-specific
/// block (6871-7157) is not installed.
#[cfg(not(target_os = "macos"))]
fn build_other(set: &mut Set) {
    use super::action::AdjustSelection;
    use super::action::InspectorMode;

    // Opening and reloading config (Config.zig:6398-6408). ctrlOrSuper -> ctrl.
    put(set, uni(',', mods!(ctrl, shift)), Action::ReloadConfig); // 6399
    put(set, uni(',', mods!(ctrl)), Action::OpenConfig); // 6404

    // Clipboard (Config.zig:6410-6460).
    put(
        set,
        phys(Key::Copy, mods!()),
        Action::CopyToClipboard(CopyToClipboard::Mixed),
    ); // 6411
    put(set, phys(Key::Paste, mods!()), Action::PasteFromClipboard); // 6416
    // Non-Darwin alt clipboard binds (6428-6439).
    put(
        set,
        phys(Key::Insert, mods!(ctrl)),
        Action::CopyToClipboard(CopyToClipboard::Mixed),
    ); // 6429
    put(
        set,
        phys(Key::Insert, mods!(shift)),
        Action::PasteFromClipboard,
    ); // 6434
    // Non-Darwin defaults clipboard to ctrl+shift.
    put_perf(
        set,
        uni('c', mods!(ctrl, shift)),
        Action::CopyToClipboard(CopyToClipboard::Mixed),
    ); // 6448
    put_perf(
        set,
        uni('v', mods!(ctrl, shift)),
        Action::PasteFromClipboard,
    ); // 6454

    // Font size (Config.zig:6462-6486). ctrlOrSuper -> ctrl.
    put(set, uni('=', mods!(ctrl)), Action::IncreaseFontSize(1.0)); // 6466
    put(set, uni('+', mods!(ctrl)), Action::IncreaseFontSize(1.0)); // 6471
    put(set, uni('-', mods!(ctrl)), Action::DecreaseFontSize(1.0)); // 6477
    put(set, uni('0', mods!(ctrl)), Action::ResetFontSize); // 6482

    // Write screen to file (Config.zig:6488-6504).
    put(
        set,
        uni('j', mods!(shift, ctrl, super_)),
        write_screen(WriteScreenAction::Copy),
    ); // 6488
    put(
        set,
        uni('j', mods!(ctrl, shift)),
        write_screen(WriteScreenAction::Paste),
    ); // 6494
    put(
        set,
        uni('j', mods!(ctrl, shift, alt)),
        write_screen(WriteScreenAction::Open),
    ); // 6500

    // Expand selection (Config.zig:6506-6554), all performable.
    put_perf(
        set,
        phys(Key::ArrowLeft, mods!(shift)),
        Action::AdjustSelection(AdjustSelection::Left),
    ); // 6507
    put_perf(
        set,
        phys(Key::ArrowRight, mods!(shift)),
        Action::AdjustSelection(AdjustSelection::Right),
    ); // 6513
    put_perf(
        set,
        phys(Key::ArrowUp, mods!(shift)),
        Action::AdjustSelection(AdjustSelection::Up),
    ); // 6519
    put_perf(
        set,
        phys(Key::ArrowDown, mods!(shift)),
        Action::AdjustSelection(AdjustSelection::Down),
    ); // 6525
    put_perf(
        set,
        phys(Key::PageUp, mods!(shift)),
        Action::AdjustSelection(AdjustSelection::PageUp),
    ); // 6531
    put_perf(
        set,
        phys(Key::PageDown, mods!(shift)),
        Action::AdjustSelection(AdjustSelection::PageDown),
    ); // 6537
    put_perf(
        set,
        phys(Key::Home, mods!(shift)),
        Action::AdjustSelection(AdjustSelection::Home),
    ); // 6543
    put_perf(
        set,
        phys(Key::End, mods!(shift)),
        Action::AdjustSelection(AdjustSelection::End),
    ); // 6549

    // Tabs common to all platforms (Config.zig:6556-6566).
    put(set, phys(Key::Tab, mods!(ctrl, shift)), Action::PreviousTab); // 6557
    put(set, phys(Key::Tab, mods!(ctrl)), Action::NextTab); // 6562

    // --- Non-Darwin "Windowing" block (Config.zig:6569-6779). ---
    put(set, uni('n', mods!(ctrl, shift)), Action::NewWindow); // 6570
    // Quirk: ctrl+shift+w is registered twice (close_surface then close_tab);
    // Set::put is last-wins, so the effective binding is close_tab (6597).
    put(set, uni('w', mods!(ctrl, shift)), Action::CloseSurface); // 6575
    put(set, uni('q', mods!(ctrl, shift)), Action::Quit); // 6580
    put(set, phys(Key::F4, mods!(alt)), Action::CloseWindow); // 6585
    put(set, uni('t', mods!(ctrl, shift)), Action::NewTab); // 6590
    put(
        set,
        uni('w', mods!(ctrl, shift)),
        Action::CloseTab(CloseTabMode::This),
    ); // 6595 (overrides 6575)
    put_perf(
        set,
        phys(Key::ArrowLeft, mods!(ctrl, shift)),
        Action::PreviousTab,
    ); // 6600
    put_perf(
        set,
        phys(Key::ArrowRight, mods!(ctrl, shift)),
        Action::NextTab,
    ); // 6606
    put_perf(set, phys(Key::PageUp, mods!(ctrl)), Action::PreviousTab); // 6612
    put_perf(set, phys(Key::PageDown, mods!(ctrl)), Action::NextTab); // 6618
    put(
        set,
        uni('o', mods!(ctrl, shift)),
        Action::NewSplit(SplitDirection::Right),
    ); // 6624
    put(
        set,
        uni('e', mods!(ctrl, shift)),
        Action::NewSplit(SplitDirection::Down),
    ); // 6629
    put_perf(
        set,
        uni('[', mods!(ctrl, super_)),
        Action::GotoSplit(SplitFocusDirection::Previous),
    ); // 6634
    put_perf(
        set,
        uni(']', mods!(ctrl, super_)),
        Action::GotoSplit(SplitFocusDirection::Next),
    ); // 6640
    put_perf(
        set,
        phys(Key::ArrowUp, mods!(ctrl, alt)),
        Action::GotoSplit(SplitFocusDirection::Up),
    ); // 6646
    put_perf(
        set,
        phys(Key::ArrowDown, mods!(ctrl, alt)),
        Action::GotoSplit(SplitFocusDirection::Down),
    ); // 6652
    put_perf(
        set,
        phys(Key::ArrowLeft, mods!(ctrl, alt)),
        Action::GotoSplit(SplitFocusDirection::Left),
    ); // 6658
    put_perf(
        set,
        phys(Key::ArrowRight, mods!(ctrl, alt)),
        Action::GotoSplit(SplitFocusDirection::Right),
    ); // 6664

    // Resizing splits (Config.zig:6671-6695).
    put_perf(
        set,
        phys(Key::ArrowUp, mods!(super_, ctrl, shift)),
        Action::ResizeSplit(ResizeSplit {
            direction: SplitResizeDirection::Up,
            amount: 10,
        }),
    ); // 6672
    put_perf(
        set,
        phys(Key::ArrowDown, mods!(super_, ctrl, shift)),
        Action::ResizeSplit(ResizeSplit {
            direction: SplitResizeDirection::Down,
            amount: 10,
        }),
    ); // 6678
    put_perf(
        set,
        phys(Key::ArrowLeft, mods!(super_, ctrl, shift)),
        Action::ResizeSplit(ResizeSplit {
            direction: SplitResizeDirection::Left,
            amount: 10,
        }),
    ); // 6684
    put_perf(
        set,
        phys(Key::ArrowRight, mods!(super_, ctrl, shift)),
        Action::ResizeSplit(ResizeSplit {
            direction: SplitResizeDirection::Right,
            amount: 10,
        }),
    ); // 6690

    // Viewport scrolling (Config.zig:6697-6717). These override the shift+home
    // / shift+end / shift+page_up / shift+page_down adjust_selection binds above
    // (last-wins) on non-Darwin.
    put(set, phys(Key::Home, mods!(shift)), Action::ScrollToTop); // 6698
    put(set, phys(Key::End, mods!(shift)), Action::ScrollToBottom); // 6703
    put(set, phys(Key::PageUp, mods!(shift)), Action::ScrollPageUp); // 6708
    put(
        set,
        phys(Key::PageDown, mods!(shift)),
        Action::ScrollPageDown,
    ); // 6713

    // Semantic prompts (Config.zig:6719-6729).
    put(
        set,
        phys(Key::ArrowUp, mods!(shift, ctrl)),
        Action::JumpToPrompt(-1),
    ); // 6720
    put(
        set,
        phys(Key::ArrowDown, mods!(shift, ctrl)),
        Action::JumpToPrompt(1),
    ); // 6725

    // Move tab (Config.zig:6731-6743).
    put_perf(
        set,
        phys(Key::PageUp, mods!(shift, ctrl)),
        Action::MoveTab(-1),
    ); // 6732
    put_perf(
        set,
        phys(Key::PageDown, mods!(shift, ctrl)),
        Action::MoveTab(1),
    ); // 6738

    // Search (Config.zig:6745-6757).
    put_perf(set, uni('f', mods!(ctrl, shift)), Action::StartSearch); // 6746
    put_perf(set, phys(Key::Escape, mods!()), Action::EndSearch); // 6752

    // Inspector, matching Chromium (Config.zig:6759-6764).
    put(
        set,
        uni('i', mods!(shift, ctrl)),
        Action::Inspector(InspectorMode::Toggle),
    ); // 6760

    // Terminal (Config.zig:6766-6771).
    put(set, uni('a', mods!(shift, ctrl)), Action::SelectAll); // 6767

    // Selection clipboard paste (Config.zig:6773-6778).
    put(
        set,
        phys(Key::Insert, mods!(shift)),
        Action::PasteFromSelection,
    ); // 6774

    // Goto tab (Config.zig:6780-6847). Non-Darwin: alt, performable = true. We
    // register BOTH the physical digit key and the unicode digit.
    for (idx, &key) in DIGIT_KEYS.iter().enumerate() {
        let n = idx + 1;
        let digit = char::from_digit(n as u32, 10).expect("1..=8 is a digit");
        put_perf(set, phys(key, mods!(alt)), Action::GotoTab(n)); // 6799
        put_perf(set, uni(digit, mods!(alt)), Action::GotoTab(n)); // 6823
    }
    put_perf(set, uni('9', mods!(alt)), Action::LastTab); // 6835

    // Toggle fullscreen / split zoom / command palette (Config.zig:6849-6868).
    put(set, phys(Key::Enter, mods!(ctrl)), Action::ToggleFullscreen); // 6850
    put(
        set,
        phys(Key::Enter, mods!(ctrl, shift)),
        Action::ToggleSplitZoom,
    ); // 6857
    put(
        set,
        uni('p', mods!(ctrl, shift)),
        Action::ToggleCommandPalette,
    ); // 6864

    // (The Mac-specific block, 6871-7157, is not installed off macOS.)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::action::{AdjustSelection, CopyToClipboard};

    /// The set is non-trivially large (sanity check on the port).
    #[test]
    fn len_is_reasonable() {
        assert!(default_set().len() > 80, "len = {}", default_set().len());
    }

    /// On macOS the port installs exactly 93 bindings.
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_len_is_exact() {
        assert_eq!(default_set().len(), 93);
    }

    /// `cmd+c` resolves to a performable copy-to-clipboard (mixed).
    #[cfg(target_os = "macos")]
    #[test]
    fn cmd_c_copies() {
        let set = default_set();
        let bound = set.get(uni('c', mods!(super_))).expect("cmd+c is bound");
        assert_eq!(
            bound.action,
            Action::CopyToClipboard(CopyToClipboard::Mixed)
        );
        assert!(bound.flags.performable);
    }

    /// `cmd+1` resolves to goto_tab(1) (both the unicode and physical triggers).
    #[cfg(target_os = "macos")]
    #[test]
    fn cmd_1_goto_tab() {
        let set = default_set();
        assert_eq!(
            set.get(uni('1', mods!(super_))).unwrap().action,
            Action::GotoTab(1)
        );
        assert_eq!(
            set.get(phys(Key::Digit1, mods!(super_))).unwrap().action,
            Action::GotoTab(1)
        );
        // On macOS these are plain (not performable).
        assert!(!set.get(uni('1', mods!(super_))).unwrap().flags.performable);
    }

    /// `cmd+q` resolves to quit.
    #[cfg(target_os = "macos")]
    #[test]
    fn cmd_q_quits() {
        let set = default_set();
        assert_eq!(
            set.get(uni('q', mods!(super_))).unwrap().action,
            Action::Quit
        );
    }

    /// `shift+ArrowLeft` (physical) resolves to a performable adjust_selection.
    #[test]
    fn shift_left_adjusts_selection() {
        let set = default_set();
        let bound = set
            .get(phys(Key::ArrowLeft, mods!(shift)))
            .expect("shift+left is bound");
        assert_eq!(bound.action, Action::AdjustSelection(AdjustSelection::Left));
        assert!(bound.flags.performable);
    }

    /// The physical clipboard keys are bound with no modifiers.
    #[test]
    fn clipboard_keys_are_physical() {
        let set = default_set();
        assert_eq!(
            set.get(phys(Key::Copy, mods!())).unwrap().action,
            Action::CopyToClipboard(CopyToClipboard::Mixed)
        );
        assert_eq!(
            set.get(phys(Key::Paste, mods!())).unwrap().action,
            Action::PasteFromClipboard
        );
    }
}
