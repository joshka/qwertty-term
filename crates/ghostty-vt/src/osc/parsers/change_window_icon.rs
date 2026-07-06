//! OSC 1: change window icon. Port of
//! `osc/parsers/change_window_icon.zig`.

use crate::osc::Command;

/// Parse OSC 1. Port of `change_window_icon.zig` `parse`.
pub fn parse(rest: &str) -> Option<Command> {
    let name = rest.strip_prefix(';')?;
    Some(Command::ChangeWindowIcon(name.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    // Zig: change_window_icon.zig "OSC 1: change_window_icon".
    #[test]
    fn osc_1_change_window_icon() {
        let mut p = osc::Parser::new();
        for c in "1;ab".bytes() {
            p.next(c);
        }
        assert_eq!(
            p.end(None),
            Some(Command::ChangeWindowIcon("ab".to_string()))
        );
    }
}
