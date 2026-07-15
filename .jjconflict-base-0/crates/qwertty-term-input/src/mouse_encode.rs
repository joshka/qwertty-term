//! X10/UTF8/SGR/urxvt/SGR-pixels mouse report encoding (port of
//! `input/mouse_encode.zig`).
//!
//! Freestanding adaptation notes:
//!
//! - The Zig `Options.size` is `renderer_size.Size`, which does pixel<->cell
//!   conversion via machinery that lives in `qwertty-term-vt`/the renderer. Since
//!   this crate cannot depend on `qwertty-term-vt`, [`Size`] is a minimal
//!   freestanding re-implementation. Every Zig test's `testSize()` sets
//!   `.padding = .{}` (always zero), so padding is omitted entirely here —
//!   there is no test coverage that would tell us the right freestanding
//!   shape for it, so it is left for a future port to add if ever needed.
//! - `terminal.MouseEvent` and `terminal.MouseFormat` (the mode-bit enums
//!   normally owned by `qwertty-term-vt`) are redefined locally as [`MouseEvent`]
//!   and [`MouseFormat`]. Callers (future window/engine code) map their own
//!   terminal-mode state into these enums.
//! - The Zig `Options` embeds `last_cell: ?*?point.Coordinate` (an optional
//!   pointer to an optional coordinate) directly in the options struct.
//!   Threading that shape through Rust's aliasing rules is awkward, so
//!   [`encode`] instead takes `last_cell: &mut Option<(i64, i64)>` as a
//!   separate third parameter that the caller owns.
//! - The Zig X10-format 223-cell overflow case logs via
//!   `log.info(...)` before returning. This crate has no logging
//!   dependency (kept dependency-free), so that case just silently
//!   returns no bytes.

use crate::key_mods::Mods;
use crate::mouse::{Action, Button};

/// Minimal freestanding replacement for `renderer_size.Size`. Padding is
/// omitted; see the module doc comment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Size {
    pub screen_width: f64,
    pub screen_height: f64,
    pub cell_width: f64,
    pub cell_height: f64,
}

/// Terminal mouse reporting mode. Port of `terminal.MouseEvent` (a.k.a.
/// `terminal.mouse.Event`), redefined locally since this crate cannot depend
/// on `qwertty-term-vt`. Variant order/names match the Zig enum (xterm mouse
/// tracking mode numbers noted for reference: `x10` = 9, `normal` = 1000,
/// `button` = 1002, `any` = 1003).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseEvent {
    #[default]
    None,
    X10,
    Normal,
    Button,
    Any,
}

impl MouseEvent {
    /// Returns true if this event mode sends motion events. Port of
    /// `terminal.mouse.eventSendsMotion`.
    pub fn sends_motion(self) -> bool {
        matches!(self, MouseEvent::Button | MouseEvent::Any)
    }
}

/// Terminal mouse reporting format. Port of `terminal.MouseFormat` (a.k.a.
/// `terminal.mouse.Format`), redefined locally since this crate cannot
/// depend on `qwertty-term-vt`. Variant order/names match the Zig enum (protocol
/// numbers noted for reference: `utf8` = 1005, `sgr` = 1006, `urxvt` = 1015,
/// `sgr_pixels` = 1016).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MouseFormat {
    #[default]
    X10,
    Utf8,
    Sgr,
    Urxvt,
    SgrPixels,
}

/// Options that affect mouse encoding behavior and provide runtime context.
/// Port of `mouse_encode.Options`. The Zig `fromTerminal` constructor and
/// the `last_cell` field are skipped/moved; see the module doc comment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Options {
    /// Terminal mouse reporting mode (X10, normal, button, any).
    pub event: MouseEvent,

    /// Terminal mouse reporting format.
    pub format: MouseFormat,

    /// Renderer size used to convert surface-space pixel positions into
    /// grid cell coordinates (for most formats) and terminal-space pixel
    /// coordinates (for SGR-Pixels), as well as to determine whether a
    /// position falls outside the visible viewport.
    pub size: Size,

    /// Whether any mouse button is currently pressed. When a motion event
    /// occurs outside the viewport, it is only reported if a button is held
    /// down and the event mode supports motion tracking. Without this,
    /// out-of-viewport motions are silently dropped.
    ///
    /// This should reflect the state of the current event as well, so if
    /// the encoded event is a button press, this should be true.
    pub any_button_pressed: bool,
}

/// Mouse position in surface-space pixels, with (0, 0) at the top-left of
/// the terminal. Negative values are allowed and indicate positions above
/// or to the left of the terminal. Values larger than the terminal size are
/// also allowed and indicate right or below the terminal. Port of
/// `mouse_encode.Event.Pos`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Pos {
    pub x: f32,
    pub y: f32,
}

/// A normalized mouse event for protocol encoding. Port of
/// `mouse_encode.Event`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Event {
    /// The action of this mouse event.
    pub action: Action,

    /// The button involved in this event. This can be `None` in the case of
    /// a motion action with no pressed buttons.
    pub button: Option<Button>,

    /// Keyboard modifiers held during this event.
    pub mods: Mods,

    /// Mouse position in surface-space pixels.
    pub pos: Pos,
}

/// Terminal-space pixel position for SGR pixel reporting. Port of
/// `mouse_encode.PixelPoint`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PixelPoint {
    x: i32,
    y: i32,
}

/// Encode the mouse event to bytes according to the options. Returns an
/// empty `Vec` if the event should not be reported.
///
/// `last_cell` tracks the last reported viewport cell for motion
/// deduplication; the caller owns it and should pass the same value back in
/// across calls for a given surface/session. Port of `mouse_encode.encode`.
pub fn encode(event: Event, opts: &Options, last_cell: &mut Option<(i64, i64)>) -> Vec<u8> {
    if !should_report(&event, opts) {
        return Vec::new();
    }

    // Handle scenarios where the mouse position is outside the viewport. We
    // always report release events no matter where they happen.
    if event.action != Action::Release && pos_out_of_viewport(event.pos, opts.size) {
        // If we don't have a motion-tracking event mode, do nothing, because
        // events outside the viewport are never reported in such cases.
        if !opts.event.sends_motion() {
            return Vec::new();
        }

        // For motion modes, we only report if a button is currently
        // pressed. This lets a TUI detect a click over the surface + drag
        // out of the surface.
        if !opts.any_button_pressed {
            return Vec::new();
        }
    }

    let cell = pos_to_cell(event.pos, opts.size);

    // We only send motion events when the cell changed unless we're
    // tracking raw pixels.
    if event.action == Action::Motion
        && opts.format != MouseFormat::SgrPixels
        && *last_cell == Some(cell)
    {
        return Vec::new();
    }

    // Update the last reported cell if we are tracking it.
    *last_cell = Some(cell);

    let button_code = match button_code(&event, opts) {
        Some(c) => c,
        None => return Vec::new(),
    };

    let mut out = Vec::new();
    match opts.format {
        MouseFormat::X10 => {
            if cell.0 > 222 || cell.1 > 222 {
                // X10 mouse format can only encode X/Y up to 223. The Zig
                // original logs this at info level; we have no logging
                // dependency, so we just silently drop the report.
                return Vec::new();
            }

            // + 1 because our x/y are zero-indexed and the protocol uses
            // 1-indexing.
            out.extend_from_slice(b"\x1B[M");
            out.push(32u8.wrapping_add(button_code));
            out.push(32u8.wrapping_add(cell.0 as u8).wrapping_add(1));
            out.push(32u8.wrapping_add(cell.1 as u8).wrapping_add(1));
        }

        MouseFormat::Utf8 => {
            out.extend_from_slice(b"\x1B[M");

            // The button code always fits in a single byte.
            out.push(32u8.wrapping_add(button_code));

            let x_cp = cell.0 as u32 + 33;
            let y_cp = cell.1 as u32 + 33;

            let mut buf = [0u8; 4];
            let ch = char::from_u32(x_cp).expect("x codepoint is a valid char");
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            let ch = char::from_u32(y_cp).expect("y codepoint is a valid char");
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }

        MouseFormat::Sgr => {
            let suffix = if event.action == Action::Release {
                'm'
            } else {
                'M'
            };
            out.extend_from_slice(
                format!(
                    "\x1B[<{};{};{}{}",
                    button_code,
                    cell.0 + 1,
                    cell.1 + 1,
                    suffix
                )
                .as_bytes(),
            );
        }

        MouseFormat::Urxvt => {
            out.extend_from_slice(
                format!(
                    "\x1B[{};{};{}M",
                    32 + button_code as i64,
                    cell.0 + 1,
                    cell.1 + 1
                )
                .as_bytes(),
            );
        }

        MouseFormat::SgrPixels => {
            let pixels = pos_to_pixels(event.pos, opts.size);
            let suffix = if event.action == Action::Release {
                'm'
            } else {
                'M'
            };
            out.extend_from_slice(
                format!("\x1B[<{};{};{}{}", button_code, pixels.x, pixels.y, suffix).as_bytes(),
            );
        }
    }

    out
}

/// Returns true if this event should be reported for the given mouse event
/// mode. Port of `mouse_encode.shouldReport`.
fn should_report(event: &Event, opts: &Options) -> bool {
    match opts.event {
        MouseEvent::None => false,

        // X10 only reports button presses of left, middle, and right.
        MouseEvent::X10 => {
            event.action == Action::Press
                && matches!(
                    event.button,
                    Some(Button::Left | Button::Middle | Button::Right)
                )
        }

        // Normal mode does not report motion.
        MouseEvent::Normal => event.action != Action::Motion,

        // Button mode requires an active button for motion events.
        MouseEvent::Button => event.button.is_some(),

        // Any mode reports everything.
        MouseEvent::Any => true,
    }
}

/// Port of `mouse_encode.buttonCode`.
fn button_code(event: &Event, opts: &Options) -> Option<u8> {
    let mut acc: u8 = match event.button {
        None => {
            // Null button means motion with no pressed button.
            3
        }
        Some(button) => {
            if event.action == Action::Release
                && opts.format != MouseFormat::Sgr
                && opts.format != MouseFormat::SgrPixels
            {
                // Legacy releases are always encoded as button 3.
                3
            } else {
                match button {
                    Button::Left => 0,
                    Button::Middle => 1,
                    Button::Right => 2,
                    Button::Four => 64,
                    Button::Five => 65,
                    Button::Six => 66,
                    Button::Seven => 67,
                    Button::Eight => 128,
                    Button::Nine => 129,
                    _ => return None,
                }
            }
        }
    };

    // X10 does not include modifiers. Note this checks `opts.event` (the
    // MouseEvent mode), not `opts.format`.
    if opts.event != MouseEvent::X10 {
        if event.mods.shift {
            acc += 4;
        }
        if event.mods.alt {
            acc += 8;
        }
        if event.mods.ctrl {
            acc += 16;
        }
    }

    // Motion adds another bit.
    if event.action == Action::Motion {
        acc += 32;
    }

    Some(acc)
}

/// Returns true if the surface-space pixel position is outside the visible
/// viewport bounds (negative or beyond screen dimensions). Port of
/// `mouse_encode.posOutOfViewport`.
fn pos_out_of_viewport(pos: Pos, size: Size) -> bool {
    let max_x = size.screen_width as f32;
    let max_y = size.screen_height as f32;
    pos.x < 0.0 || pos.y < 0.0 || pos.x > max_x || pos.y > max_y
}

/// Converts a surface-space pixel position to a zero-based grid cell
/// coordinate (column, row) within the terminal viewport. Out-of-bounds
/// values are clamped to the valid grid range (0 to columns/rows - 1). Port
/// of `mouse_encode.posToCell` (with the `renderer_size.Coordinate`
/// grid-conversion logic inlined, since padding is always zero here).
fn pos_to_cell(pos: Pos, size: Size) -> (i64, i64) {
    // Grid size: screen size (no padding to subtract) divided by cell size,
    // floored, minimum of 1 (matching `GridSize.update`'s `@max(1, ...)`).
    let columns = ((size.screen_width / size.cell_width) as i64).max(1);
    let rows = ((size.screen_height / size.cell_height) as i64).max(1);

    let term_x = pos.x as f64;
    let term_y = pos.y as f64;
    let clamped_x = term_x.max(0.0);
    let clamped_y = term_y.max(0.0);

    let col = (clamped_x / size.cell_width) as i64;
    let row = (clamped_y / size.cell_height) as i64;

    let clamped_col = col.min(columns - 1);
    let clamped_row = row.min(rows - 1);

    (clamped_col, clamped_row)
}

/// Converts a surface-space pixel position to terminal-space pixel
/// coordinates (accounting for padding/scaling) used by SGR-Pixels mode.
/// Unlike grid conversion, terminal-space coordinates are not clamped and
/// may be negative or exceed the terminal dimensions. Port of
/// `mouse_encode.posToPixels` (padding is always zero here, so
/// terminal-space == surface-space).
fn pos_to_pixels(pos: Pos, _size: Size) -> PixelPoint {
    PixelPoint {
        x: (pos.x as f64).round() as i32,
        y: (pos.y as f64).round() as i32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_size() -> Size {
        Size {
            screen_width: 1_000.0,
            screen_height: 1_000.0,
            cell_width: 1.0,
            cell_height: 1.0,
        }
    }

    fn opts(event: MouseEvent, format: MouseFormat, size: Size) -> Options {
        Options {
            event,
            format,
            size,
            any_button_pressed: false,
        }
    }

    fn ev(button: Option<Button>, action: Action) -> Event {
        Event {
            action,
            button,
            mods: Mods::default(),
            pos: Pos::default(),
        }
    }

    // Port of `test "shouldReport: none mode never reports"`.
    #[test]
    fn should_report_none_mode_never_reports() {
        let size = test_size();
        for action in [Action::Press, Action::Release, Action::Motion] {
            assert!(!should_report(
                &ev(Some(Button::Left), action),
                &opts(MouseEvent::None, MouseFormat::X10, size)
            ));
        }
    }

    // Port of `test "shouldReport: x10 reports only left/middle/right press"`.
    #[test]
    fn should_report_x10_reports_only_left_middle_right_press() {
        let size = test_size();
        for btn in [Button::Left, Button::Middle, Button::Right] {
            assert!(should_report(
                &ev(Some(btn), Action::Press),
                &opts(MouseEvent::X10, MouseFormat::X10, size)
            ));
        }

        // Release is not reported.
        assert!(!should_report(
            &ev(Some(Button::Left), Action::Release),
            &opts(MouseEvent::X10, MouseFormat::X10, size)
        ));

        // Motion is not reported.
        assert!(!should_report(
            &ev(Some(Button::Left), Action::Motion),
            &opts(MouseEvent::X10, MouseFormat::X10, size)
        ));

        // Other buttons are not reported.
        assert!(!should_report(
            &ev(Some(Button::Four), Action::Press),
            &opts(MouseEvent::X10, MouseFormat::X10, size)
        ));

        // Null button is not reported.
        assert!(!should_report(
            &ev(None, Action::Press),
            &opts(MouseEvent::X10, MouseFormat::X10, size)
        ));
    }

    // Port of `test "shouldReport: normal reports press and release but not motion"`.
    #[test]
    fn should_report_normal_reports_press_and_release_but_not_motion() {
        let size = test_size();
        assert!(should_report(
            &ev(Some(Button::Left), Action::Press),
            &opts(MouseEvent::Normal, MouseFormat::X10, size)
        ));
        assert!(should_report(
            &ev(Some(Button::Left), Action::Release),
            &opts(MouseEvent::Normal, MouseFormat::X10, size)
        ));
        assert!(!should_report(
            &ev(Some(Button::Left), Action::Motion),
            &opts(MouseEvent::Normal, MouseFormat::X10, size)
        ));
    }

    // Port of `test "shouldReport: button mode requires a button"`.
    #[test]
    fn should_report_button_mode_requires_a_button() {
        let size = test_size();
        for action in [Action::Press, Action::Release, Action::Motion] {
            assert!(should_report(
                &ev(Some(Button::Left), action),
                &opts(MouseEvent::Button, MouseFormat::X10, size)
            ));
        }
        for action in [Action::Press, Action::Release, Action::Motion] {
            assert!(!should_report(
                &ev(None, action),
                &opts(MouseEvent::Button, MouseFormat::X10, size)
            ));
        }
    }

    // Port of `test "shouldReport: any mode reports everything"`.
    #[test]
    fn should_report_any_mode_reports_everything() {
        let size = test_size();
        for action in [Action::Press, Action::Release, Action::Motion] {
            assert!(should_report(
                &ev(Some(Button::Left), action),
                &opts(MouseEvent::Any, MouseFormat::X10, size)
            ));
        }

        // Even null button + motion reports.
        assert!(should_report(
            &ev(None, Action::Motion),
            &opts(MouseEvent::Any, MouseFormat::X10, size)
        ));
    }

    // Port of `test "x10 press left"`.
    #[test]
    fn x10_press_left() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Press,
                button: Some(Button::Left),
                mods: Mods {
                    shift: true,
                    alt: true,
                    ctrl: true,
                    ..Default::default()
                },
                pos: Pos { x: 0.0, y: 0.0 },
            },
            &opts(MouseEvent::X10, MouseFormat::X10, test_size()),
            &mut last,
        );

        assert_eq!(result, vec![0x1B, b'[', b'M', 32, 33, 33]);
    }

    // Port of `test "x10 ignores release"`.
    #[test]
    fn x10_ignores_release() {
        let mut last = None;
        let result = encode(
            ev(Some(Button::Left), Action::Release),
            &opts(MouseEvent::X10, MouseFormat::X10, test_size()),
            &mut last,
        );
        assert_eq!(result.len(), 0);
    }

    // Port of `test "normal ignores motion"`.
    #[test]
    fn normal_ignores_motion() {
        let mut last = None;
        let result = encode(
            ev(Some(Button::Left), Action::Motion),
            &opts(MouseEvent::Normal, MouseFormat::Sgr, test_size()),
            &mut last,
        );
        assert_eq!(result.len(), 0);
    }

    // Port of `test "button mode requires button"`.
    #[test]
    fn button_mode_requires_button() {
        let mut last = None;
        let result = encode(
            ev(None, Action::Motion),
            &opts(MouseEvent::Button, MouseFormat::Sgr, test_size()),
            &mut last,
        );
        assert_eq!(result.len(), 0);
    }

    // Port of `test "sgr release keeps button identity"`.
    #[test]
    fn sgr_release_keeps_button_identity() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Release,
                button: Some(Button::Right),
                mods: Mods::default(),
                pos: Pos { x: 4.0, y: 5.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::Sgr, test_size()),
            &mut last,
        );
        assert_eq!(String::from_utf8(result).unwrap(), "\x1B[<2;5;6m");
    }

    // Port of `test "sgr motion with no button"`.
    #[test]
    fn sgr_motion_with_no_button() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Motion,
                button: None,
                mods: Mods::default(),
                pos: Pos { x: 1.0, y: 2.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::Sgr, test_size()),
            &mut last,
        );
        assert_eq!(String::from_utf8(result).unwrap(), "\x1B[<35;2;3M");
    }

    // Port of `test "urxvt with modifiers"`.
    #[test]
    fn urxvt_with_modifiers() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Press,
                button: Some(Button::Left),
                mods: Mods {
                    shift: true,
                    alt: true,
                    ctrl: true,
                    ..Default::default()
                },
                pos: Pos { x: 2.0, y: 3.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::Urxvt, test_size()),
            &mut last,
        );
        assert_eq!(String::from_utf8(result).unwrap(), "\x1B[60;3;4M");
    }

    // Port of `test "utf8 encodes large coordinates"`.
    #[test]
    fn utf8_encodes_large_coordinates() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Press,
                button: Some(Button::Left),
                mods: Mods::default(),
                pos: Pos { x: 300.0, y: 400.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::Utf8, test_size()),
            &mut last,
        );

        assert_eq!(&result[0..4], &[0x1B, b'[', b'M', 32]);
        let s = std::str::from_utf8(&result[4..]).unwrap();
        let mut it = s.chars();
        assert_eq!(it.next(), Some(char::from_u32(333).unwrap()));
        assert_eq!(it.next(), Some(char::from_u32(433).unwrap()));
        assert_eq!(it.next(), None);
    }

    // Port of `test "x10 coordinate limit"`.
    #[test]
    fn x10_coordinate_limit() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Press,
                button: Some(Button::Left),
                mods: Mods::default(),
                pos: Pos { x: 223.0, y: 0.0 },
            },
            &opts(MouseEvent::X10, MouseFormat::X10, test_size()),
            &mut last,
        );
        assert_eq!(result.len(), 0);
    }

    // Port of `test "sgr wheel button mappings"`.
    #[test]
    fn sgr_wheel_button_mappings() {
        for (button, code) in [
            (Button::Four, 64),
            (Button::Five, 65),
            (Button::Six, 66),
            (Button::Seven, 67),
        ] {
            let mut last = None;
            let result = encode(
                Event {
                    action: Action::Press,
                    button: Some(button),
                    mods: Mods::default(),
                    pos: Pos { x: 0.0, y: 0.0 },
                },
                &opts(MouseEvent::Any, MouseFormat::Sgr, test_size()),
                &mut last,
            );
            let want = format!("\x1B[<{};1;1M", code);
            assert_eq!(String::from_utf8(result).unwrap(), want);
        }
    }

    // Port of `test "urxvt release uses legacy button 3 encoding"`.
    #[test]
    fn urxvt_release_uses_legacy_button_3_encoding() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Release,
                button: Some(Button::Right),
                mods: Mods::default(),
                pos: Pos { x: 2.0, y: 3.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::Urxvt, test_size()),
            &mut last,
        );
        assert_eq!(String::from_utf8(result).unwrap(), "\x1B[35;3;4M");
    }

    // Port of `test "unsupported button is ignored"`.
    #[test]
    fn unsupported_button_is_ignored() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Press,
                button: Some(Button::Ten),
                mods: Mods::default(),
                pos: Pos { x: 1.0, y: 1.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::Sgr, test_size()),
            &mut last,
        );
        assert_eq!(result.len(), 0);
    }

    // Port of `test "sgr pixels uses terminal-space cursor coordinates"`.
    #[test]
    fn sgr_pixels_uses_terminal_space_cursor_coordinates() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Press,
                button: Some(Button::Left),
                mods: Mods::default(),
                pos: Pos { x: 10.0, y: 20.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::SgrPixels, test_size()),
            &mut last,
        );
        assert_eq!(String::from_utf8(result).unwrap(), "\x1B[<0;10;20M");
    }

    // Port of `test "sgr pixels release keeps button identity"`.
    #[test]
    fn sgr_pixels_release_keeps_button_identity() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Release,
                button: Some(Button::Right),
                mods: Mods::default(),
                pos: Pos { x: 10.0, y: 20.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::SgrPixels, test_size()),
            &mut last,
        );
        assert_eq!(String::from_utf8(result).unwrap(), "\x1B[<2;10;20m");
    }

    // Port of `test "position exactly at viewport boundary is encoded in final cell"`.
    #[test]
    fn position_exactly_at_viewport_boundary_is_encoded_in_final_cell() {
        let size = Size {
            screen_width: 10.0,
            screen_height: 10.0,
            cell_width: 2.0,
            cell_height: 2.0,
        };

        let mut last = None;
        let result = encode(
            Event {
                action: Action::Press,
                button: Some(Button::Left),
                mods: Mods::default(),
                pos: Pos { x: 10.0, y: 10.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::Sgr, size),
            &mut last,
        );
        assert_eq!(String::from_utf8(result).unwrap(), "\x1B[<0;5;5M");
    }

    // Port of `test "outside viewport motion with no pressed button is ignored"`.
    #[test]
    fn outside_viewport_motion_with_no_pressed_button_is_ignored() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Motion,
                button: Some(Button::Left),
                mods: Mods::default(),
                pos: Pos { x: -1.0, y: -1.0 },
            },
            &Options {
                event: MouseEvent::Any,
                format: MouseFormat::Sgr,
                size: test_size(),
                any_button_pressed: false,
            },
            &mut last,
        );
        assert_eq!(result.len(), 0);
    }

    // Port of `test "outside viewport motion with pressed button is reported"`.
    #[test]
    fn outside_viewport_motion_with_pressed_button_is_reported() {
        let mut last = None;
        let result = encode(
            Event {
                action: Action::Motion,
                button: Some(Button::Left),
                mods: Mods::default(),
                pos: Pos { x: -1.0, y: -1.0 },
            },
            &Options {
                event: MouseEvent::Any,
                format: MouseFormat::Sgr,
                size: test_size(),
                any_button_pressed: true,
            },
            &mut last,
        );
        assert_eq!(String::from_utf8(result).unwrap(), "\x1B[<32;1;1M");
    }

    // Port of `test "motion is deduped by last cell except sgr pixels"`.
    #[test]
    fn motion_is_deduped_by_last_cell_except_sgr_pixels() {
        let mut last = None;

        let result = encode(
            Event {
                action: Action::Motion,
                button: Some(Button::Left),
                mods: Mods::default(),
                pos: Pos { x: 5.0, y: 6.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::Sgr, test_size()),
            &mut last,
        );
        assert!(!result.is_empty());

        let result = encode(
            Event {
                action: Action::Motion,
                button: Some(Button::Left),
                mods: Mods::default(),
                pos: Pos { x: 5.0, y: 6.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::Sgr, test_size()),
            &mut last,
        );
        assert_eq!(result.len(), 0);

        let result = encode(
            Event {
                action: Action::Motion,
                button: Some(Button::Left),
                mods: Mods::default(),
                pos: Pos { x: 5.0, y: 6.0 },
            },
            &opts(MouseEvent::Any, MouseFormat::SgrPixels, test_size()),
            &mut last,
        );
        assert!(!result.is_empty());
    }
}
