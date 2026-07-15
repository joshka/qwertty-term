//! Keybind actions — port of the `Action` union from `input/Binding.zig`
//! (upstream `2da015cd6`, lines 303-1432).
//!
//! [`Action`] is the thing a [`Trigger`](super::Trigger) is bound to. The
//! config spelling of each action is the Zig `snake_case` union tag (e.g.
//! `copy_to_clipboard`); the Rust variant is its `UpperCamelCase` equivalent
//! (`CopyToClipboard`). Parameters follow the Zig field types: `void` becomes
//! a unit variant, `[]const u8` becomes [`String`], numeric fields keep their
//! width, and each Zig parameter enum/struct gets a dedicated Rust type here.
//!
//! The parser ([`Action::parse`]) ports `Action.parse` (Binding.zig:1253-1317)
//! exactly, including its quirks: only the *first* colon splits the name from
//! the raw parameter, string parameters are taken verbatim (no unescaping, no
//! trimming), and `cursor_key` is deliberately not settable from config.

use super::BindError;

/// A keybind action. Port of the `Action` tagged union (Binding.zig:303-971).
///
/// Variants are listed in upstream source order. `f32` payloads mean [`Action`]
/// cannot derive `Eq`/`Hash`; upstream hashes floats by bit-cast instead
/// (Binding.zig:1613-1624), which we do not need here.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Ignore this key combination. Not processed nor forwarded to the child
    /// process, though the OS may still see it. (Binding.zig:309)
    Ignore,

    /// Unbind a previously bound key binding. This is a pseudo-action handled
    /// by the binding set and is never stored. (Binding.zig:315)
    Unbind,

    /// Send a CSI sequence (the value is the sequence without the `ESC [`
    /// header). (Binding.zig:323)
    Csi(String),

    /// Send an `ESC` sequence. (Binding.zig:326)
    Esc(String),

    /// Send the specified text (Zig string-literal syntax, currently
    /// unvalidated). (Binding.zig:333)
    Text(String),

    /// Send data to the pty depending on whether cursor-key mode is enabled.
    /// This action is **not settable from config**: [`Action::parse`] returns
    /// [`BindError::InvalidAction`] for it (Binding.zig:337, 1280).
    CursorKey(CursorKey),

    /// Reset the terminal (equivalent to the `reset` command). (Binding.zig:347)
    Reset,

    /// Copy the selected text to the clipboard. (Binding.zig:373)
    CopyToClipboard(CopyToClipboard),

    /// Paste the contents of the default clipboard. (Binding.zig:376)
    PasteFromClipboard,

    /// Paste the contents of the selection clipboard. (Binding.zig:379)
    PasteFromSelection,

    /// If there is a URL under the cursor, copy it to the default clipboard.
    /// (Binding.zig:382)
    CopyUrlToClipboard,

    /// Copy the terminal title to the clipboard. (Binding.zig:386)
    CopyTitleToClipboard,

    /// Increase the font size by the specified amount in points. (Binding.zig:392)
    IncreaseFontSize(f32),

    /// Decrease the font size by the specified amount in points. (Binding.zig:398)
    DecreaseFontSize(f32),

    /// Reset the font size to the original configured size. (Binding.zig:401)
    ResetFontSize,

    /// Set the font size to the specified size in points. (Binding.zig:407)
    SetFontSize(f32),

    /// Start a search for the given text (empty text cancels the search).
    /// (Binding.zig:415)
    Search(String),

    /// Start a search for the current text selection. (Binding.zig:420)
    SearchSelection,

    /// Navigate the search results. (Binding.zig:426)
    NavigateSearch(NavigateSearch),

    /// Start a search if not already started, without setting terms.
    /// (Binding.zig:430)
    StartSearch,

    /// End the current search and hide any GUI elements. (Binding.zig:433)
    EndSearch,

    /// Clear the screen and all scrollback. (Binding.zig:436)
    ClearScreen,

    /// Select all text on the screen. (Binding.zig:439)
    SelectAll,

    /// Scroll to the top of the screen. (Binding.zig:442)
    ScrollToTop,

    /// Scroll to the bottom of the screen. (Binding.zig:445)
    ScrollToBottom,

    /// Scroll to the selected text. (Binding.zig:448)
    ScrollToSelection,

    /// Scroll to the given absolute row (0 is the first row). (Binding.zig:452)
    ScrollToRow(usize),

    /// Scroll the screen up by one page. (Binding.zig:455)
    ScrollPageUp,

    /// Scroll the screen down by one page. (Binding.zig:458)
    ScrollPageDown,

    /// Scroll the screen by the specified fraction of a page (positive scrolls
    /// down, negative up). (Binding.zig:467)
    ScrollPageFractional(f32),

    /// Scroll the screen by the specified number of lines (positive scrolls
    /// down, negative up). (Binding.zig:476)
    ScrollPageLines(i16),

    /// Adjust the current selection in the given direction. (Binding.zig:508)
    AdjustSelection(AdjustSelection),

    /// Jump the viewport forward or back by the given number of prompts.
    /// (Binding.zig:515)
    JumpToPrompt(i16),

    /// Write the entire scrollback into a temporary file. (Binding.zig:537)
    WriteScrollbackFile(WriteScreen),

    /// Write the contents of the screen into a temporary file. (Binding.zig:543)
    WriteScreenFile(WriteScreen),

    /// Write the currently selected text into a temporary file. (Binding.zig:551)
    WriteSelectionFile(WriteScreen),

    /// Open a new window. (Binding.zig:557)
    NewWindow,

    /// Open a new tab. (Binding.zig:560)
    NewTab,

    /// Go to the previous tab. (Binding.zig:563)
    PreviousTab,

    /// Go to the next tab. (Binding.zig:566)
    NextTab,

    /// Go to the last tab. (Binding.zig:569)
    LastTab,

    /// Go to the tab with the specific index, starting from 1. (Binding.zig:575)
    GotoTab(usize),

    /// Move a tab by a relative offset (wraps cyclically). (Binding.zig:588)
    MoveTab(isize),

    /// Toggle the tab overview (Linux, libadwaita >= 1.4). (Binding.zig:595)
    ToggleTabOverview,

    /// Change the title of the current focused surface via a pop-up prompt.
    /// (Binding.zig:598)
    PromptSurfaceTitle,

    /// Change the title of the current tab via a pop-up prompt. (Binding.zig:603)
    PromptTabTitle,

    /// Set the title for the current focused surface (empty resets it).
    /// (Binding.zig:608)
    SetSurfaceTitle(String),

    /// Set the title for the current focused tab (empty clears the override).
    /// (Binding.zig:613)
    SetTabTitle(String),

    /// Create a new split in the specified direction. (Binding.zig:629)
    NewSplit(SplitDirection),

    /// Focus a split by direction or creation order. (Binding.zig:634)
    GotoSplit(SplitFocusDirection),

    /// Focus the previous or next window. (Binding.zig:637)
    GotoWindow(GotoWindow),

    /// Zoom in or out of the current split. (Binding.zig:644)
    ToggleSplitZoom,

    /// Toggle read-only mode for the current surface. (Binding.zig:654)
    ToggleReadonly,

    /// Resize the current split in the specified direction and pixel amount.
    /// (Binding.zig:659)
    ResizeSplit(ResizeSplit),

    /// Equalize the size of all splits in the current window. (Binding.zig:662)
    EqualizeSplits,

    /// Reset the window to the default size (macOS only). (Binding.zig:669)
    ResetWindowSize,

    /// Control the visibility of the terminal inspector. (Binding.zig:674)
    Inspector(InspectorMode),

    /// Show the GTK inspector (no-op on macOS). (Binding.zig:679)
    ShowGtkInspector,

    /// Show the on-screen keyboard if present (Linux/GTK). (Binding.zig:687)
    ShowOnScreenKeyboard,

    /// Open the configuration file in the default OS editor. (Binding.zig:694)
    OpenConfig,

    /// Reload the configuration. (Binding.zig:701)
    ReloadConfig,

    /// Close the current surface (window, tab, split, etc.). (Binding.zig:707)
    CloseSurface,

    /// Close the specified tabs and all splits therein. (Binding.zig:727)
    CloseTab(CloseTabMode),

    /// Close the current window and all tabs and splits therein. (Binding.zig:733)
    CloseWindow,

    /// Deprecated no-op; use `all:close_window` instead. (Binding.zig:740)
    CloseAllWindows,

    /// Maximize or unmaximize the current window (no-op on macOS). (Binding.zig:746)
    ToggleMaximize,

    /// Fullscreen or unfullscreen the current window. (Binding.zig:749)
    ToggleFullscreen,

    /// Toggle window decorations (Linux only). (Binding.zig:754)
    ToggleWindowDecorations,

    /// Toggle always-float-on-top (macOS only). (Binding.zig:762)
    ToggleWindowFloatOnTop,

    /// Toggle secure input mode (macOS only, application-wide). (Binding.zig:773)
    ToggleSecureInput,

    /// Toggle mouse reporting on or off. (Binding.zig:784)
    ToggleMouseReporting,

    /// Toggle the command palette (Linux: libadwaita >= 1.5). (Binding.zig:794)
    ToggleCommandPalette,

    /// Toggle the quick (Quake-style drop-down) terminal. (Binding.zig:846)
    ToggleQuickTerminal,

    /// Show or hide all windows (macOS only). (Binding.zig:855)
    ToggleVisibility,

    /// Toggle the window background opacity (macOS only). (Binding.zig:865)
    ToggleBackgroundOpacity,

    /// Check for updates (macOS only). (Binding.zig:870)
    CheckForUpdates,

    /// Undo the last undoable action for the focused surface. (Binding.zig:894)
    Undo,

    /// Redo the last undone action. (Binding.zig:899)
    Redo,

    /// End the currently active key sequence, flushing prior keys (excluding
    /// the triggering key) to the terminal. (Binding.zig:913)
    EndKeySequence,

    /// Activate a named key table until it is deactivated. (Binding.zig:925)
    ActivateKeyTable(String),

    /// Activate a named key table until the first valid keybinding is used.
    /// (Binding.zig:935)
    ActivateKeyTableOnce(String),

    /// Deactivate the currently active key table. (Binding.zig:940)
    DeactivateKeyTable,

    /// Deactivate all active key tables. (Binding.zig:944)
    DeactivateAllKeyTables,

    /// Quit Ghostty. (Binding.zig:947)
    Quit,

    /// Deliberately crash (panic) in the chosen thread, for crash-report
    /// testing. (Binding.zig:971)
    Crash(CrashThread),
}

/// Payload for [`Action::CursorKey`]. Port of `Action.CursorKey`
/// (Binding.zig:991-1013). Provided for completeness; this action cannot be
/// constructed from config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorKey {
    /// Data sent when cursor-key mode is disabled (`normal`).
    pub normal: String,
    /// Data sent when cursor-key mode is enabled (`application`).
    pub application: String,
}

/// Parameter for [`Action::CopyToClipboard`]. Port of `Action.CopyToClipboard`
/// (Binding.zig:1101-1112).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CopyToClipboard {
    /// Copy the selection as plain text only.
    Plain,
    /// Copy as plain text preserving terminal escape sequences.
    Vt,
    /// Copy as HTML, preserving colors and styles as markup.
    Html,
    /// Place multiple tagged representations on the clipboard at once.
    Mixed,
}

impl CopyToClipboard {
    /// The default when no parameter is given (`mixed`, Binding.zig:1111).
    pub const DEFAULT: CopyToClipboard = CopyToClipboard::Mixed;

    fn parse(value: &str) -> Result<CopyToClipboard, BindError> {
        match value {
            "plain" => Ok(CopyToClipboard::Plain),
            "vt" => Ok(CopyToClipboard::Vt),
            "html" => Ok(CopyToClipboard::Html),
            "mixed" => Ok(CopyToClipboard::Mixed),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Parameter for [`Action::NavigateSearch`]. Port of `Action.NavigateSearch`
/// (Binding.zig:1015-1018).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NavigateSearch {
    /// Navigate to the previous result.
    Previous,
    /// Navigate to the next result.
    Next,
}

impl NavigateSearch {
    fn parse(value: &str) -> Result<NavigateSearch, BindError> {
        match value {
            "previous" => Ok(NavigateSearch::Previous),
            "next" => Ok(NavigateSearch::Next),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Parameter for [`Action::AdjustSelection`]. Port of `Action.AdjustSelection`
/// (Binding.zig:1020-1031).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdjustSelection {
    /// Adjust one cell to the left.
    Left,
    /// Adjust one cell to the right.
    Right,
    /// Adjust one line upwards.
    Up,
    /// Adjust one line downwards.
    Down,
    /// Adjust one page upwards.
    PageUp,
    /// Adjust one page downwards.
    PageDown,
    /// Adjust to the top-left corner of the screen.
    Home,
    /// Adjust to the bottom-right corner of the screen.
    End,
    /// Adjust to the beginning of the line.
    BeginningOfLine,
    /// Adjust to the end of the line.
    EndOfLine,
}

impl AdjustSelection {
    fn parse(value: &str) -> Result<AdjustSelection, BindError> {
        match value {
            "left" => Ok(AdjustSelection::Left),
            "right" => Ok(AdjustSelection::Right),
            "up" => Ok(AdjustSelection::Up),
            "down" => Ok(AdjustSelection::Down),
            "page_up" => Ok(AdjustSelection::PageUp),
            "page_down" => Ok(AdjustSelection::PageDown),
            "home" => Ok(AdjustSelection::Home),
            "end" => Ok(AdjustSelection::End),
            "beginning_of_line" => Ok(AdjustSelection::BeginningOfLine),
            "end_of_line" => Ok(AdjustSelection::EndOfLine),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Parameter for [`Action::NewSplit`]. Port of `Action.SplitDirection`
/// (Binding.zig:1033-1041).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// Split to the right.
    Right,
    /// Split downwards.
    Down,
    /// Split to the left.
    Left,
    /// Split upwards.
    Up,
    /// Split along the larger direction.
    Auto,
}

impl SplitDirection {
    /// The default when no parameter is given (`auto`, Binding.zig:1040).
    pub const DEFAULT: SplitDirection = SplitDirection::Auto;

    fn parse(value: &str) -> Result<SplitDirection, BindError> {
        match value {
            "right" => Ok(SplitDirection::Right),
            "down" => Ok(SplitDirection::Down),
            "left" => Ok(SplitDirection::Left),
            "up" => Ok(SplitDirection::Up),
            "auto" => Ok(SplitDirection::Auto),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Parameter for [`Action::GotoSplit`]. Port of `Action.SplitFocusDirection`
/// (Binding.zig:1043-1082).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitFocusDirection {
    /// Focus the previously created split.
    Previous,
    /// Focus the next created split.
    Next,
    /// Focus the split above.
    Up,
    /// Focus the split to the left.
    Left,
    /// Focus the split below.
    Down,
    /// Focus the split to the right.
    Right,
}

impl SplitFocusDirection {
    /// Custom parse (Binding.zig:1051-1063). In addition to the enum names, the
    /// legacy aliases `top` -> [`SplitFocusDirection::Up`] and `bottom` ->
    /// [`SplitFocusDirection::Down`] are accepted for backwards compatibility.
    fn parse(value: &str) -> Result<SplitFocusDirection, BindError> {
        match value {
            "previous" => Ok(SplitFocusDirection::Previous),
            "next" => Ok(SplitFocusDirection::Next),
            "up" => Ok(SplitFocusDirection::Up),
            "left" => Ok(SplitFocusDirection::Left),
            "down" => Ok(SplitFocusDirection::Down),
            "right" => Ok(SplitFocusDirection::Right),
            // Backwards compatibility: map "top"/"bottom" onto up/down.
            "top" => Ok(SplitFocusDirection::Up),
            "bottom" => Ok(SplitFocusDirection::Down),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Direction component of [`ResizeSplit`]. Port of `Action.SplitResizeDirection`
/// (Binding.zig:1084-1089).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitResizeDirection {
    /// Resize upwards.
    Up,
    /// Resize downwards.
    Down,
    /// Resize to the left.
    Left,
    /// Resize to the right.
    Right,
}

impl SplitResizeDirection {
    fn parse(value: &str) -> Result<SplitResizeDirection, BindError> {
        match value {
            "up" => Ok(SplitResizeDirection::Up),
            "down" => Ok(SplitResizeDirection::Down),
            "left" => Ok(SplitResizeDirection::Left),
            "right" => Ok(SplitResizeDirection::Right),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Parameter for [`Action::GotoWindow`]. Port of `Action.GotoWindow`
/// (Binding.zig:1091-1094).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GotoWindow {
    /// Focus the previous window.
    Previous,
    /// Focus the next window.
    Next,
}

impl GotoWindow {
    fn parse(value: &str) -> Result<GotoWindow, BindError> {
        match value {
            "previous" => Ok(GotoWindow::Previous),
            "next" => Ok(GotoWindow::Next),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Parameter for [`Action::ResizeSplit`]. Port of `Action.SplitResizeParameter`,
/// the tuple `(SplitResizeDirection, u16)` (Binding.zig:1096-1099). Parsed from
/// `dir,amount`, e.g. `resize_split:up,10`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResizeSplit {
    /// The direction in which to resize the split.
    pub direction: SplitResizeDirection,
    /// The amount to resize by, in pixels.
    pub amount: u16,
}

impl ResizeSplit {
    /// Parse the comma-joined `dir,amount` form. Mirrors the tuple-struct arm of
    /// `parseParameter` (Binding.zig:1223-1243): the value is split on `,` and
    /// both a missing element and an extra element are [`BindError::InvalidFormat`].
    fn parse(param: &str) -> Result<ResizeSplit, BindError> {
        let mut it = param.split(',');
        // `str::split` always yields at least one element for the first `next`.
        let direction = SplitResizeDirection::parse(it.next().ok_or(BindError::InvalidFormat)?)?;
        let amount = parse_int_u16(it.next().ok_or(BindError::InvalidFormat)?)?;
        // Any extra element is an error.
        if it.next().is_some() {
            return Err(BindError::InvalidFormat);
        }
        Ok(ResizeSplit { direction, amount })
    }
}

/// Parameter for [`Action::Inspector`]. Port of `Action.InspectorMode`
/// (Binding.zig:1175-1179).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InspectorMode {
    /// Toggle the inspector's visibility.
    Toggle,
    /// Show the inspector.
    Show,
    /// Hide the inspector.
    Hide,
}

impl InspectorMode {
    fn parse(value: &str) -> Result<InspectorMode, BindError> {
        match value {
            "toggle" => Ok(InspectorMode::Toggle),
            "show" => Ok(InspectorMode::Show),
            "hide" => Ok(InspectorMode::Hide),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Parameter for [`Action::CloseTab`]. Port of `Action.CloseTabMode`
/// (Binding.zig:1181-1187).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseTabMode {
    /// Close the current tab and all splits within it.
    This,
    /// Close every tab in the current window except the current tab.
    Other,
    /// Close every tab to the right of the current tab.
    Right,
}

impl CloseTabMode {
    /// The default when no parameter is given (`this`, Binding.zig:1186).
    pub const DEFAULT: CloseTabMode = CloseTabMode::This;

    fn parse(value: &str) -> Result<CloseTabMode, BindError> {
        match value {
            "this" => Ok(CloseTabMode::This),
            "other" => Ok(CloseTabMode::Other),
            "right" => Ok(CloseTabMode::Right),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Parameter for [`Action::CrashThread`]-style crashes. Port of
/// `Action.CrashThread` (Binding.zig:985-989).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashThread {
    /// Crash on the main (GUI) thread.
    Main,
    /// Crash on the IO thread for the focused surface.
    Io,
    /// Crash on the render thread for the focused surface.
    Render,
}

impl CrashThread {
    fn parse(value: &str) -> Result<CrashThread, BindError> {
        match value {
            "main" => Ok(CrashThread::Main),
            "io" => Ok(CrashThread::Io),
            "render" => Ok(CrashThread::Render),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// The "what to do with the temp file" component of [`WriteScreen`]. Port of
/// `Action.WriteScreen.Action` (Binding.zig:1122-1126).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteScreenAction {
    /// Copy the file path into the clipboard.
    Copy,
    /// Paste the file path into the terminal.
    Paste,
    /// Open the file in the default OS editor for text files.
    Open,
}

impl WriteScreenAction {
    fn parse(value: &str) -> Result<WriteScreenAction, BindError> {
        match value {
            "copy" => Ok(WriteScreenAction::Copy),
            "paste" => Ok(WriteScreenAction::Paste),
            "open" => Ok(WriteScreenAction::Open),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// The output-format component of [`WriteScreen`]. Port of
/// `Action.WriteScreen.Format` (Binding.zig:1128-1132).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteScreenFormat {
    /// Plain text.
    Plain,
    /// Plain text preserving terminal escape sequences.
    Vt,
    /// HTML markup.
    Html,
}

impl WriteScreenFormat {
    fn parse(value: &str) -> Result<WriteScreenFormat, BindError> {
        match value {
            "plain" => Ok(WriteScreenFormat::Plain),
            "vt" => Ok(WriteScreenFormat::Vt),
            "html" => Ok(WriteScreenFormat::Html),
            _ => Err(BindError::InvalidFormat),
        }
    }
}

/// Parameter for the `write_*_file` actions. Port of `Action.WriteScreen`
/// (Binding.zig:1114-1172).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteScreen {
    /// What to do with the temp-file path once written.
    pub action: WriteScreenAction,
    /// The format to write the file contents in.
    pub format: WriteScreenFormat,
}

impl WriteScreen {
    /// Custom parse (Binding.zig:1134-1156). The parameter is `action` with an
    /// optional `,format` suffix. When the comma (and format) is absent the
    /// format defaults to [`WriteScreenFormat::Plain`] — this is important for
    /// backwards compatibility with configs written before Ghostty 1.3, which
    /// had no output formats.
    fn parse(param: &str) -> Result<WriteScreen, BindError> {
        match param.find(',') {
            None => Ok(WriteScreen {
                action: WriteScreenAction::parse(param)?,
                format: WriteScreenFormat::Plain,
            }),
            Some(idx) => Ok(WriteScreen {
                action: WriteScreenAction::parse(&param[..idx])?,
                format: WriteScreenFormat::parse(&param[idx + 1..])?,
            }),
        }
    }
}

/// The scope of an action — the context in which it must be executed. Port of
/// `Action.Scope` (Binding.zig:1321-1324).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The action is executed against the whole application.
    App,
    /// The action is executed against a specific surface.
    Surface,
}

/// Parse a base-10 `i16`, mapping any failure to [`BindError::InvalidFormat`].
/// Port of `parseInt` (Binding.zig:1193-1195).
fn parse_int_i16(value: &str) -> Result<i16, BindError> {
    value.parse::<i16>().map_err(|_| BindError::InvalidFormat)
}

/// Parse a base-10 `u16`, mapping any failure to [`BindError::InvalidFormat`].
fn parse_int_u16(value: &str) -> Result<u16, BindError> {
    value.parse::<u16>().map_err(|_| BindError::InvalidFormat)
}

/// Parse a base-10 `usize`, mapping any failure to [`BindError::InvalidFormat`].
fn parse_int_usize(value: &str) -> Result<usize, BindError> {
    value.parse::<usize>().map_err(|_| BindError::InvalidFormat)
}

/// Parse a base-10 `isize`, mapping any failure to [`BindError::InvalidFormat`].
fn parse_int_isize(value: &str) -> Result<isize, BindError> {
    value.parse::<isize>().map_err(|_| BindError::InvalidFormat)
}

/// Parse an `f32`, mapping any failure to [`BindError::InvalidFormat`]. Port of
/// `parseFloat` (Binding.zig:1197-1199). Rust's float parser, like Zig's,
/// accepts a leading `+` (e.g. `+0.5`).
fn parse_float_f32(value: &str) -> Result<f32, BindError> {
    value.parse::<f32>().map_err(|_| BindError::InvalidFormat)
}

impl Action {
    /// Parse an action from its config spelling `name` or `name:param`. Port of
    /// `Action.parse` (Binding.zig:1253-1317).
    ///
    /// The input is split on the **first** `:`; the part before it is the action
    /// name (an empty name is [`BindError::InvalidFormat`]). The remainder — the
    /// raw parameter — is taken verbatim, with no unescaping or trimming, so it
    /// may itself contain further colons (e.g. `text:a:b`).
    ///
    /// Parameter rules by kind:
    /// - `void` actions reject any present parameter ([`BindError::InvalidFormat`]).
    /// - string actions require the colon; the value may be empty.
    /// - `cursor_key` is always [`BindError::InvalidAction`].
    /// - enum/int/float/struct actions parse the parameter if present, otherwise
    ///   fall back to a `default` if the type has one, else
    ///   [`BindError::InvalidFormat`].
    /// - An unknown action name is [`BindError::InvalidAction`].
    pub fn parse(input: &str) -> Result<Action, BindError> {
        // Split on the first colon; everything before is the action name and the
        // (optional) remainder is the raw parameter.
        let colon_idx = input.find(':');
        let name = &input[..colon_idx.unwrap_or(input.len())];
        let param: Option<&str> = colon_idx.map(|i| &input[i + 1..]);

        // An action name is always required.
        if name.is_empty() {
            return Err(BindError::InvalidFormat);
        }

        match name {
            // --- void actions: a present parameter is an error. ---
            "ignore" => void(param, Action::Ignore),
            "unbind" => void(param, Action::Unbind),
            "reset" => void(param, Action::Reset),
            "paste_from_clipboard" => void(param, Action::PasteFromClipboard),
            "paste_from_selection" => void(param, Action::PasteFromSelection),
            "copy_url_to_clipboard" => void(param, Action::CopyUrlToClipboard),
            "copy_title_to_clipboard" => void(param, Action::CopyTitleToClipboard),
            "reset_font_size" => void(param, Action::ResetFontSize),
            "search_selection" => void(param, Action::SearchSelection),
            "start_search" => void(param, Action::StartSearch),
            "end_search" => void(param, Action::EndSearch),
            "clear_screen" => void(param, Action::ClearScreen),
            "select_all" => void(param, Action::SelectAll),
            "scroll_to_top" => void(param, Action::ScrollToTop),
            "scroll_to_bottom" => void(param, Action::ScrollToBottom),
            "scroll_to_selection" => void(param, Action::ScrollToSelection),
            "scroll_page_up" => void(param, Action::ScrollPageUp),
            "scroll_page_down" => void(param, Action::ScrollPageDown),
            "new_window" => void(param, Action::NewWindow),
            "new_tab" => void(param, Action::NewTab),
            "previous_tab" => void(param, Action::PreviousTab),
            "next_tab" => void(param, Action::NextTab),
            "last_tab" => void(param, Action::LastTab),
            "toggle_tab_overview" => void(param, Action::ToggleTabOverview),
            "prompt_surface_title" => void(param, Action::PromptSurfaceTitle),
            "prompt_tab_title" => void(param, Action::PromptTabTitle),
            "toggle_split_zoom" => void(param, Action::ToggleSplitZoom),
            "toggle_readonly" => void(param, Action::ToggleReadonly),
            "equalize_splits" => void(param, Action::EqualizeSplits),
            "reset_window_size" => void(param, Action::ResetWindowSize),
            "show_gtk_inspector" => void(param, Action::ShowGtkInspector),
            "show_on_screen_keyboard" => void(param, Action::ShowOnScreenKeyboard),
            "open_config" => void(param, Action::OpenConfig),
            "reload_config" => void(param, Action::ReloadConfig),
            "close_surface" => void(param, Action::CloseSurface),
            "close_window" => void(param, Action::CloseWindow),
            "close_all_windows" => void(param, Action::CloseAllWindows),
            "toggle_maximize" => void(param, Action::ToggleMaximize),
            "toggle_fullscreen" => void(param, Action::ToggleFullscreen),
            "toggle_window_decorations" => void(param, Action::ToggleWindowDecorations),
            "toggle_window_float_on_top" => void(param, Action::ToggleWindowFloatOnTop),
            "toggle_secure_input" => void(param, Action::ToggleSecureInput),
            "toggle_mouse_reporting" => void(param, Action::ToggleMouseReporting),
            "toggle_command_palette" => void(param, Action::ToggleCommandPalette),
            "toggle_quick_terminal" => void(param, Action::ToggleQuickTerminal),
            "toggle_visibility" => void(param, Action::ToggleVisibility),
            "toggle_background_opacity" => void(param, Action::ToggleBackgroundOpacity),
            "check_for_updates" => void(param, Action::CheckForUpdates),
            "undo" => void(param, Action::Undo),
            "redo" => void(param, Action::Redo),
            "end_key_sequence" => void(param, Action::EndKeySequence),
            "deactivate_key_table" => void(param, Action::DeactivateKeyTable),
            "deactivate_all_key_tables" => void(param, Action::DeactivateAllKeyTables),
            "quit" => void(param, Action::Quit),

            // --- string actions: the colon is required, value taken verbatim. ---
            "csi" => Ok(Action::Csi(string(param)?)),
            "esc" => Ok(Action::Esc(string(param)?)),
            "text" => Ok(Action::Text(string(param)?)),
            "search" => Ok(Action::Search(string(param)?)),
            "set_surface_title" => Ok(Action::SetSurfaceTitle(string(param)?)),
            "set_tab_title" => Ok(Action::SetTabTitle(string(param)?)),
            "activate_key_table" => Ok(Action::ActivateKeyTable(string(param)?)),
            "activate_key_table_once" => Ok(Action::ActivateKeyTableOnce(string(param)?)),

            // --- cursor_key: never settable from config. ---
            "cursor_key" => Err(BindError::InvalidAction),

            // --- float actions (no default). ---
            "increase_font_size" => Ok(Action::IncreaseFontSize(required(param, parse_float_f32)?)),
            "decrease_font_size" => Ok(Action::DecreaseFontSize(required(param, parse_float_f32)?)),
            "set_font_size" => Ok(Action::SetFontSize(required(param, parse_float_f32)?)),
            "scroll_page_fractional" => Ok(Action::ScrollPageFractional(required(
                param,
                parse_float_f32,
            )?)),

            // --- int actions (no default). ---
            "scroll_to_row" => Ok(Action::ScrollToRow(required(param, parse_int_usize)?)),
            "scroll_page_lines" => Ok(Action::ScrollPageLines(required(param, parse_int_i16)?)),
            "jump_to_prompt" => Ok(Action::JumpToPrompt(required(param, parse_int_i16)?)),
            "goto_tab" => Ok(Action::GotoTab(required(param, parse_int_usize)?)),
            "move_tab" => Ok(Action::MoveTab(required(param, parse_int_isize)?)),

            // --- enum/struct actions (no default). ---
            "navigate_search" => Ok(Action::NavigateSearch(required(
                param,
                NavigateSearch::parse,
            )?)),
            "adjust_selection" => Ok(Action::AdjustSelection(required(
                param,
                AdjustSelection::parse,
            )?)),
            "write_scrollback_file" => Ok(Action::WriteScrollbackFile(required(
                param,
                WriteScreen::parse,
            )?)),
            "write_screen_file" => Ok(Action::WriteScreenFile(required(
                param,
                WriteScreen::parse,
            )?)),
            "write_selection_file" => Ok(Action::WriteSelectionFile(required(
                param,
                WriteScreen::parse,
            )?)),
            "goto_split" => Ok(Action::GotoSplit(required(
                param,
                SplitFocusDirection::parse,
            )?)),
            "goto_window" => Ok(Action::GotoWindow(required(param, GotoWindow::parse)?)),
            "resize_split" => Ok(Action::ResizeSplit(required(param, ResizeSplit::parse)?)),
            "inspector" => Ok(Action::Inspector(required(param, InspectorMode::parse)?)),
            "crash" => Ok(Action::Crash(required(param, CrashThread::parse)?)),

            // --- enum actions with a default (missing colon uses the default). ---
            "copy_to_clipboard" => Ok(Action::CopyToClipboard(optional(
                param,
                CopyToClipboard::DEFAULT,
                CopyToClipboard::parse,
            )?)),
            "new_split" => Ok(Action::NewSplit(optional(
                param,
                SplitDirection::DEFAULT,
                SplitDirection::parse,
            )?)),
            "close_tab" => Ok(Action::CloseTab(optional(
                param,
                CloseTabMode::DEFAULT,
                CloseTabMode::parse,
            )?)),

            // --- unknown action name. ---
            _ => Err(BindError::InvalidAction),
        }
    }

    /// Returns the scope of this action. Port of `Action.scope`
    /// (Binding.zig:1327-1432).
    ///
    /// The app-scoped set is: `ignore`, `unbind`, `open_config`, `reload_config`,
    /// `close_all_windows`, `quit`, `toggle_quick_terminal`, `toggle_visibility`,
    /// `check_for_updates`, `show_gtk_inspector`, `new_window`, `undo`, `redo`.
    /// Everything else (including all tab/split/window management actions such as
    /// `new_tab`, `goto_split`, and `close_window`) is surface-scoped.
    pub fn scope(&self) -> Scope {
        match self {
            Action::Ignore
            | Action::Unbind
            | Action::OpenConfig
            | Action::ReloadConfig
            | Action::CloseAllWindows
            | Action::Quit
            | Action::ToggleQuickTerminal
            | Action::ToggleVisibility
            | Action::CheckForUpdates
            | Action::ShowGtkInspector
            | Action::NewWindow
            | Action::Undo
            | Action::Redo => Scope::App,
            _ => Scope::Surface,
        }
    }
}

/// Build a `void` action, rejecting a present parameter. Port of the `void` arm
/// of `Action.parse` (Binding.zig:1268-1271).
fn void(param: Option<&str>, action: Action) -> Result<Action, BindError> {
    match param {
        Some(_) => Err(BindError::InvalidFormat),
        None => Ok(action),
    }
}

/// Extract a required string parameter (a missing colon is an error). Port of
/// the `[]const u8` arm of `Action.parse` (Binding.zig:1273-1277).
fn string(param: Option<&str>) -> Result<String, BindError> {
    param.map(str::to_string).ok_or(BindError::InvalidFormat)
}

/// Parse a required parameter with `f`; a missing colon is
/// [`BindError::InvalidFormat`]. Mirrors the "no default" path of `Action.parse`
/// (Binding.zig:1286-1310).
fn required<T>(
    param: Option<&str>,
    f: impl Fn(&str) -> Result<T, BindError>,
) -> Result<T, BindError> {
    f(param.ok_or(BindError::InvalidFormat)?)
}

/// Parse an optional parameter with `f`, falling back to `default` when the
/// colon is absent. Mirrors the `default`-decl path of `Action.parse`
/// (Binding.zig:1286-1303).
fn optional<T>(
    param: Option<&str>,
    default: T,
    f: impl Fn(&str) -> Result<T, BindError>,
) -> Result<T, BindError> {
    match param {
        Some(p) => f(p),
        None => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unknown action name -> `InvalidAction` (Binding.zig:3306-3311).
    #[test]
    fn unknown_action() {
        assert_eq!(Action::parse("nopenopenope"), Err(BindError::InvalidAction));
    }

    /// A void action with no parameter parses; with a parameter it is an
    /// `InvalidFormat` (Binding.zig:3313-3325).
    #[test]
    fn void_action() {
        assert_eq!(Action::parse("ignore"), Ok(Action::Ignore));
        assert_eq!(Action::parse("ignore:A"), Err(BindError::InvalidFormat));
    }

    /// String parameters are taken verbatim, including further colons
    /// (Binding.zig:3327-3352).
    #[test]
    fn string_action() {
        assert_eq!(
            Action::parse("text:hello"),
            Ok(Action::Text("hello".into()))
        );
        assert_eq!(Action::parse("text:a:b"), Ok(Action::Text("a:b".into())));
        assert_eq!(Action::parse("csi:A"), Ok(Action::Csi("A".into())));
        assert_eq!(Action::parse("esc:A"), Ok(Action::Esc("A".into())));
        assert_eq!(
            Action::parse("set_surface_title:surface"),
            Ok(Action::SetSurfaceTitle("surface".into()))
        );
        // The colon is required for string actions.
        assert_eq!(Action::parse("text"), Err(BindError::InvalidFormat));
        // An empty value is permitted.
        assert_eq!(Action::parse("text:"), Ok(Action::Text(String::new())));
    }

    /// Enum parameter (Binding.zig:3354-3363).
    #[test]
    fn enum_action() {
        assert_eq!(
            Action::parse("new_split:right"),
            Ok(Action::NewSplit(SplitDirection::Right))
        );
    }

    /// Enum parameter with a default: missing colon uses the default
    /// (Binding.zig:3365-3374).
    #[test]
    fn enum_action_default() {
        assert_eq!(
            Action::parse("new_split"),
            Ok(Action::NewSplit(SplitDirection::Auto))
        );
        assert_eq!(
            Action::parse("copy_to_clipboard"),
            Ok(Action::CopyToClipboard(CopyToClipboard::Mixed))
        );
        assert_eq!(
            Action::parse("close_tab"),
            Ok(Action::CloseTab(CloseTabMode::This))
        );
    }

    /// Int parameter, with sign (Binding.zig:3376-3390).
    #[test]
    fn int_action() {
        assert_eq!(
            Action::parse("jump_to_prompt:-1"),
            Ok(Action::JumpToPrompt(-1))
        );
        assert_eq!(
            Action::parse("jump_to_prompt:10"),
            Ok(Action::JumpToPrompt(10))
        );
    }

    /// Float parameter, accepting a leading `+` (Binding.zig:3392-3406).
    #[test]
    fn float_action() {
        assert_eq!(
            Action::parse("scroll_page_fractional:-0.5"),
            Ok(Action::ScrollPageFractional(-0.5))
        );
        assert_eq!(
            Action::parse("scroll_page_fractional:+0.5"),
            Ok(Action::ScrollPageFractional(0.5))
        );
    }

    /// Tuple parameter and its arity/type errors (Binding.zig:3408-3427).
    #[test]
    fn tuple_action() {
        assert_eq!(
            Action::parse("resize_split:up,10"),
            Ok(Action::ResizeSplit(ResizeSplit {
                direction: SplitResizeDirection::Up,
                amount: 10,
            }))
        );
        // Missing element.
        assert_eq!(
            Action::parse("resize_split:up"),
            Err(BindError::InvalidFormat)
        );
        // Too many elements.
        assert_eq!(
            Action::parse("resize_split:up,10,12"),
            Err(BindError::InvalidFormat)
        );
        // Invalid element type.
        assert_eq!(
            Action::parse("resize_split:up,four"),
            Err(BindError::InvalidFormat)
        );
    }

    /// `cursor_key` is never settable from config (Binding.zig:1280).
    #[test]
    fn cursor_key_unsettable() {
        assert_eq!(Action::parse("cursor_key"), Err(BindError::InvalidAction));
        assert_eq!(
            Action::parse("cursor_key:foo"),
            Err(BindError::InvalidAction)
        );
    }

    /// `goto_split` accepts the legacy `top`/`bottom` aliases
    /// (Binding.zig:1051-1063).
    #[test]
    fn goto_split_legacy_alias() {
        assert_eq!(
            Action::parse("goto_split:top"),
            Ok(Action::GotoSplit(SplitFocusDirection::Up))
        );
        assert_eq!(
            Action::parse("goto_split:bottom"),
            Ok(Action::GotoSplit(SplitFocusDirection::Down))
        );
        assert_eq!(
            Action::parse("goto_split:left"),
            Ok(Action::GotoSplit(SplitFocusDirection::Left))
        );
    }

    /// `write_scrollback_file` defaults the format to `plain` when no comma is
    /// present, and parses the `action,format` form otherwise
    /// (Binding.zig:1134-1156).
    #[test]
    fn write_screen_action() {
        assert_eq!(
            Action::parse("write_scrollback_file:copy"),
            Ok(Action::WriteScrollbackFile(WriteScreen {
                action: WriteScreenAction::Copy,
                format: WriteScreenFormat::Plain,
            }))
        );
        assert_eq!(
            Action::parse("write_screen_file:paste,html"),
            Ok(Action::WriteScreenFile(WriteScreen {
                action: WriteScreenAction::Paste,
                format: WriteScreenFormat::Html,
            }))
        );
    }

    /// Empty action name -> `InvalidFormat` (Binding.zig:1261).
    #[test]
    fn empty_name() {
        assert_eq!(Action::parse(":foo"), Err(BindError::InvalidFormat));
    }

    /// Scope partition spot-checks (Binding.zig:1327-1432).
    #[test]
    fn scopes() {
        assert_eq!(Action::Quit.scope(), Scope::App);
        assert_eq!(Action::NewWindow.scope(), Scope::App);
        assert_eq!(Action::Undo.scope(), Scope::App);
        assert_eq!(Action::Ignore.scope(), Scope::App);
        // Tab/split management is surface-scoped despite forwarding to the app.
        assert_eq!(Action::NewTab.scope(), Scope::Surface);
        assert_eq!(Action::CloseWindow.scope(), Scope::Surface);
        assert_eq!(
            Action::GotoSplit(SplitFocusDirection::Next).scope(),
            Scope::Surface
        );
        assert_eq!(Action::Text("x".into()).scope(), Scope::Surface);
    }
}
