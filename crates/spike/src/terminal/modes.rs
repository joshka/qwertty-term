use crate::{parser::param_or, terminal::Terminal};

impl Terminal {
    pub(super) fn set_private_modes(&mut self, params: &[Option<usize>], enabled: bool) {
        for mode in params.iter().flatten().copied() {
            if self.modes.set_private_mode(mode, enabled) {
                continue;
            }
            match mode {
                1049 => self.switch_alternate_screen(enabled),
                _ => {
                    // TODO(port): most DEC private modes affect cursor,
                    // keyboard, mouse, and rendering state outside this PoC.
                }
            }
        }
    }

    pub(super) fn set_cursor_shape(&mut self, params: &[Option<usize>]) {
        self.modes.set_cursor_shape(param_or(params, 0, 0));
    }
}
