//! OSC 7: report the current working directory. Port of
//! `osc/parsers/report_pwd.zig`.

use crate::osc::Command;

/// Parse OSC 7. Port of `report_pwd.zig` `parse`.
pub fn parse(rest: &str) -> Option<Command> {
    let value = rest.strip_prefix(';')?;
    Some(Command::ReportPwd {
        value: value.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    // Zig: report_pwd.zig "OSC 7: report pwd".
    #[test]
    fn osc_7_report_pwd() {
        let mut p = osc::Parser::new();
        for c in "7;file:///tmp/example".bytes() {
            p.next(c);
        }
        assert_eq!(
            p.end(None),
            Some(Command::ReportPwd {
                value: "file:///tmp/example".to_string()
            })
        );
    }

    // Zig: report_pwd.zig "OSC 7: report pwd empty".
    #[test]
    fn osc_7_report_pwd_empty() {
        let mut p = osc::Parser::new();
        for c in "7;".bytes() {
            p.next(c);
        }
        assert_eq!(
            p.end(None),
            Some(Command::ReportPwd {
                value: String::new()
            })
        );
    }
}
