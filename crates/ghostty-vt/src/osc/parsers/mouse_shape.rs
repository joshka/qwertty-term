//! OSC 22: set the mouse cursor shape. Port of
//! `osc/parsers/mouse_shape.zig`.

use crate::osc::Command;

/// Parse OSC 22. Port of `mouse_shape.zig` `parse`.
pub fn parse(rest: &str) -> Option<Command> {
    let value = rest.strip_prefix(';')?;
    Some(Command::MouseShape {
        value: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    // Zig: mouse_shape.zig "OSC 22: pointer cursor".
    #[test]
    fn osc_22_pointer_cursor() {
        let mut p = osc::Parser::new();
        for c in "22;pointer".bytes() {
            p.next(c);
        }
        assert_eq!(
            p.end(None),
            Some(Command::MouseShape {
                value: "pointer".to_string()
            })
        );
    }
}
