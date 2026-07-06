use crate::{parser::param_or, terminal::Terminal};

impl Terminal {
    pub(super) fn device_status_report(&mut self, private: bool, params: &[Option<usize>]) {
        match (private, param_or(params, 0, 0)) {
            (false, 5) => self.output.extend_from_slice(b"\x1b[0n"),
            (false, 6) => {
                let cursor = self.cursor();
                let response = format!("\x1b[{};{}R", cursor.row + 1, cursor.col + 1);
                self.output.extend_from_slice(response.as_bytes());
            }
            (true, 996) => {
                // TODO(port): wire this to host appearance once there is a
                // real app/window layer. Report dark for now, matching the
                // most common terminal background.
                self.output.extend_from_slice(b"\x1b[?997;1n");
            }
            _ => {}
        }
    }

    pub(super) fn device_attributes(&mut self, raw_csi: &str) {
        if raw_csi.starts_with('>') {
            self.output.extend_from_slice(b"\x1b[>1;0;0c");
        } else {
            self.output.extend_from_slice(b"\x1b[?62;22c");
        }
    }
}
