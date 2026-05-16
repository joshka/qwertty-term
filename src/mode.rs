#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MouseTracking {
    Button,
    Drag,
    Any,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorShape {
    Block,
    Underline,
    Bar,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TerminalModes {
    pub(crate) wraparound: bool,
    pub(crate) cursor_visible: bool,
    pub(crate) cursor_shape: CursorShape,
    pub(crate) application_cursor_keys: bool,
    pub(crate) bracketed_paste: bool,
    pub(crate) focus_reporting: bool,
    pub(crate) mouse_tracking: Option<MouseTracking>,
    pub(crate) sgr_mouse: bool,
}

impl Default for TerminalModes {
    fn default() -> Self {
        Self {
            wraparound: true,
            cursor_visible: true,
            cursor_shape: CursorShape::Block,
            application_cursor_keys: false,
            bracketed_paste: false,
            focus_reporting: false,
            mouse_tracking: None,
            sgr_mouse: false,
        }
    }
}

impl TerminalModes {
    pub(crate) fn set_private_mode(&mut self, mode: usize, enabled: bool) -> bool {
        match mode {
            1 => self.application_cursor_keys = enabled,
            7 => self.wraparound = enabled,
            25 => self.cursor_visible = enabled,
            1000 => self.set_mouse_tracking(MouseTracking::Button, enabled),
            1002 => self.set_mouse_tracking(MouseTracking::Drag, enabled),
            1003 => self.set_mouse_tracking(MouseTracking::Any, enabled),
            1004 => self.focus_reporting = enabled,
            1006 => self.sgr_mouse = enabled,
            2004 => self.bracketed_paste = enabled,
            _ => return false,
        }
        true
    }

    pub(crate) fn set_cursor_shape(&mut self, param: usize) {
        self.cursor_shape = match param {
            3 | 4 => CursorShape::Underline,
            5 | 6 => CursorShape::Bar,
            _ => CursorShape::Block,
        };
    }

    fn set_mouse_tracking(&mut self, tracking: MouseTracking, enabled: bool) {
        if enabled {
            self.mouse_tracking = Some(tracking);
        } else if self.mouse_tracking == Some(tracking) {
            self.mouse_tracking = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_modes_toggle_terminal_mode_state() {
        let mut modes = TerminalModes::default();

        assert!(modes.set_private_mode(1002, true));
        assert_eq!(modes.mouse_tracking, Some(MouseTracking::Drag));
        assert!(modes.set_private_mode(1004, true));
        assert!(modes.focus_reporting);
        assert!(modes.set_private_mode(2004, true));
        assert!(modes.bracketed_paste);

        assert!(modes.set_private_mode(1002, false));
        assert_eq!(modes.mouse_tracking, None);
        assert!(!modes.set_private_mode(1049, true));
    }

    #[test]
    fn cursor_shape_params_map_to_shapes() {
        let mut modes = TerminalModes::default();

        modes.set_cursor_shape(4);
        assert_eq!(modes.cursor_shape, CursorShape::Underline);

        modes.set_cursor_shape(6);
        assert_eq!(modes.cursor_shape, CursorShape::Bar);

        modes.set_cursor_shape(0);
        assert_eq!(modes.cursor_shape, CursorShape::Block);
    }
}
