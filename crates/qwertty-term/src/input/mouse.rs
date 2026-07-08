//! Mouse-event → PTY-byte encoding glue over `qwertty_term_input::mouse_encode`.
//!
//! The AppKit view reports pointer events in view-space points; this maps them
//! to the freestanding [`mouse_encode::Event`] (surface-space pixels + button +
//! action + mods) and encodes them against the terminal's live mouse-tracking
//! mode/format. Selection is deferred for R5, but mouse *reporting* (so
//! full-screen apps like vim/htop receive clicks and scroll) is wired.
//!
//! Pure and AppKit-free so the coordinate + event mapping is unit-testable; the
//! view supplies the raw fields.

use qwertty_term_input::key_mods::Mods;
use qwertty_term_input::mouse::{Action, Button};
use qwertty_term_input::mouse_encode::{self, Event, MouseEvent, MouseFormat, Options, Pos, Size};

/// The geometry + live mouse modes needed to encode one event.
#[derive(Debug, Clone, Copy)]
pub struct MouseContext {
    pub event_mode: MouseEvent,
    pub format: MouseFormat,
    pub screen_width: f64,
    pub screen_height: f64,
    pub cell_width: f64,
    pub cell_height: f64,
    /// Whether any button is currently held (drives out-of-viewport motion).
    pub any_button_pressed: bool,
}

/// Encode a mouse event to PTY bytes, or empty if reporting is off / the event
/// shouldn't be reported. `last_cell` is per-surface motion-dedup state the
/// caller owns across calls.
pub fn encode(
    action: Action,
    button: Option<Button>,
    mods: Mods,
    x: f32,
    y: f32,
    ctx: &MouseContext,
    last_cell: &mut Option<(i64, i64)>,
) -> Vec<u8> {
    if ctx.event_mode == MouseEvent::None {
        return Vec::new();
    }
    let event = Event {
        action,
        button,
        mods,
        pos: Pos { x, y },
    };
    let opts = Options {
        event: ctx.event_mode,
        format: ctx.format,
        size: Size {
            screen_width: ctx.screen_width,
            screen_height: ctx.screen_height,
            cell_width: ctx.cell_width,
            cell_height: ctx.cell_height,
        },
        any_button_pressed: ctx.any_button_pressed,
    };
    mouse_encode::encode(event, &opts, last_cell)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(mode: MouseEvent) -> MouseContext {
        MouseContext {
            event_mode: mode,
            format: MouseFormat::Sgr,
            screen_width: 800.0,
            screen_height: 480.0,
            cell_width: 8.0,
            cell_height: 16.0,
            any_button_pressed: true,
        }
    }

    #[test]
    fn reporting_off_produces_nothing() {
        let mut last = None;
        let bytes = encode(
            Action::Press,
            Some(Button::Left),
            Mods::default(),
            10.0,
            20.0,
            &ctx(MouseEvent::None),
            &mut last,
        );
        assert!(bytes.is_empty());
    }

    #[test]
    fn sgr_left_press_reports_button_zero() {
        let mut last = None;
        let bytes = encode(
            Action::Press,
            Some(Button::Left),
            Mods::default(),
            0.0,
            0.0,
            &ctx(MouseEvent::Normal),
            &mut last,
        );
        // SGR press: ESC [ < 0 ; col ; row M  (cell 1;1 at pixel 0,0).
        assert_eq!(bytes, b"\x1b[<0;1;1M");
    }
}
