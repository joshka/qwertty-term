//! OSC 133: semantic prompt markers. Port of
//! `osc/parsers/semantic_prompt.zig`.

use crate::osc::Command;
use crate::osc::string_encoding::{printf_q_decode, url_percent_decode};
use crate::osc::support::read_semicolon_field;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticPromptAction {
    /// `'L'`
    FreshLine,
    /// `'A'`
    FreshLineNewPrompt,
    /// `'N'`
    NewCommand,
    /// `'P'`
    PromptStart,
    /// `'B'`
    EndPromptStartInput,
    /// `'I'`
    EndPromptStartInputTerminateEol,
    /// `'C'`
    EndInputStartOutput,
    /// `'D'`
    EndCommand,
}

/// An OSC 133 semantic prompt command. Port of `semantic_prompt.zig`
/// `Command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticPrompt {
    pub action: SemanticPromptAction,
    pub options_unvalidated: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Click {
    Line,
    Multiple,
    ConservativeVertical,
    SmartVertical,
}

impl Click {
    fn init(value: &str) -> Option<Click> {
        if value.len() == 1 {
            match value.as_bytes()[0] {
                b'm' => return Some(Click::Multiple),
                b'v' => return Some(Click::ConservativeVertical),
                b'w' => return Some(Click::SmartVertical),
                _ => return None,
            }
        }
        if value == "line" {
            return Some(Click::Line);
        }
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    Initial,
    Right,
    Continuation,
    Secondary,
}

impl PromptKind {
    fn init(c: u8) -> Option<PromptKind> {
        Some(match c {
            b'i' => PromptKind::Initial,
            b'r' => PromptKind::Right,
            b'c' => PromptKind::Continuation,
            b's' => PromptKind::Secondary,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Redraw {
    True,
    False,
    Last,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClickEvents {
    Absolute,
    Relative,
}

impl SemanticPrompt {
    pub fn aid(&self) -> Option<&str> {
        read_semicolon_field(&self.options_unvalidated, "aid")
    }
    pub fn cl(&self) -> Option<Click> {
        read_semicolon_field(&self.options_unvalidated, "cl").and_then(Click::init)
    }
    pub fn prompt_kind(&self) -> Option<PromptKind> {
        let v = read_semicolon_field(&self.options_unvalidated, "k")?;
        if v.len() == 1 {
            PromptKind::init(v.as_bytes()[0])
        } else {
            None
        }
    }
    pub fn err(&self) -> Option<&str> {
        read_semicolon_field(&self.options_unvalidated, "err")
    }
    pub fn cmdline(&self) -> Option<&str> {
        read_semicolon_field(&self.options_unvalidated, "cmdline")
    }
    pub fn cmdline_url(&self) -> Option<&str> {
        read_semicolon_field(&self.options_unvalidated, "cmdline_url")
    }
    pub fn redraw(&self) -> Option<Redraw> {
        let v = read_semicolon_field(&self.options_unvalidated, "redraw")?;
        match v {
            "0" => Some(Redraw::False),
            "1" => Some(Redraw::True),
            "last" => Some(Redraw::Last),
            _ => None,
        }
    }
    pub fn special_key(&self) -> Option<bool> {
        let v = read_semicolon_field(&self.options_unvalidated, "special_key")?;
        if v.len() == 1 {
            match v.as_bytes()[0] {
                b'0' => Some(false),
                b'1' => Some(true),
                _ => None,
            }
        } else {
            None
        }
    }
    pub fn click_events(&self) -> Option<ClickEvents> {
        let v = read_semicolon_field(&self.options_unvalidated, "click_events")?;
        if v.len() == 1 {
            match v.as_bytes()[0] {
                b'1' => Some(ClickEvents::Absolute),
                b'2' => Some(ClickEvents::Relative),
                _ => None,
            }
        } else {
            None
        }
    }
    /// The `D` action's positional exit-code field (the first
    /// `;`-delimited field, not a `key=value` pair).
    pub fn exit_code(&self) -> Option<i32> {
        let field = self.options_unvalidated.split(';').next()?;
        field.parse().ok()
    }

    /// Decode the effective command line (`cmdline` or `cmdline_url`) into
    /// `out`. Port of `Command.writeCommandLine`.
    pub fn write_command_line(
        &self,
        out: &mut String,
    ) -> Result<(), super::super::string_encoding::DecodeError> {
        if let Some(cmdline) = self.cmdline() {
            return printf_q_decode(out, cmdline);
        }
        if let Some(cmdline_url) = self.cmdline_url() {
            let mut bytes = Vec::new();
            url_percent_decode(&mut bytes, cmdline_url)?;
            // Ghostty's cmdline_url is percent-decoded bytes reinterpreted
            // as UTF-8; lossy fallback mirrors ghostty's writer-based
            // approach closely enough for this chunk's test corpus (all
            // ASCII).
            out.push_str(&String::from_utf8_lossy(&bytes));
            return Ok(());
        }
        Ok(())
    }
}

/// Construct a bare `fresh_line_new_prompt` command (used by OSC 9;12's
/// ConEmu alias). Port of `semantic_prompt.zig` `Command.init`.
pub(super) fn fresh_line_new_prompt() -> SemanticPrompt {
    SemanticPrompt {
        action: SemanticPromptAction::FreshLineNewPrompt,
        options_unvalidated: String::new(),
    }
}

/// Parse OSC 133. Port of `semantic_prompt.zig` `parse`.
pub fn parse(rest: &str) -> Option<Command> {
    let data = rest.strip_prefix(';')?;
    if data.is_empty() {
        return None;
    }
    let bytes = data.as_bytes();

    macro_rules! with_options {
        ($action:expr) => {{
            if data.len() == 1 {
                SemanticPrompt {
                    action: $action,
                    options_unvalidated: String::new(),
                }
            } else if bytes[1] != b';' {
                return None;
            } else {
                SemanticPrompt {
                    action: $action,
                    options_unvalidated: data[2..].to_string(),
                }
            }
        }};
    }

    let cmd = match bytes[0] {
        b'A' => with_options!(SemanticPromptAction::FreshLineNewPrompt),
        b'B' => with_options!(SemanticPromptAction::EndPromptStartInput),
        b'I' => with_options!(SemanticPromptAction::EndPromptStartInputTerminateEol),
        b'C' => with_options!(SemanticPromptAction::EndInputStartOutput),
        b'D' => with_options!(SemanticPromptAction::EndCommand),
        b'L' => {
            if data.len() > 1 {
                return None;
            }
            SemanticPrompt {
                action: SemanticPromptAction::FreshLine,
                options_unvalidated: String::new(),
            }
        }
        b'N' => with_options!(SemanticPromptAction::NewCommand),
        b'P' => with_options!(SemanticPromptAction::PromptStart),
        _ => return None,
    };

    Some(Command::SemanticPrompt(cmd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    fn run(body: &str) -> Option<Command> {
        let mut p = osc::Parser::new();
        for c in body.bytes() {
            p.next(c);
        }
        p.end(None)
    }

    fn prompt(body: &str) -> SemanticPrompt {
        match run(body) {
            Some(Command::SemanticPrompt(p)) => p,
            other => panic!("expected SemanticPrompt, got {other:?}"),
        }
    }

    // --- C: end_input_start_output ---

    // Zig: "OSC 133: end_input_start_output".
    #[test]
    fn end_input_start_output() {
        let p = prompt("133;C");
        assert_eq!(p.action, SemanticPromptAction::EndInputStartOutput);
        assert_eq!(p.aid(), None);
        assert_eq!(p.cl(), None);
    }

    // Zig: "OSC 133: end_input_start_output extra contents".
    #[test]
    fn end_input_start_output_extra_contents() {
        assert_eq!(run("133;Cextra"), None);
    }

    // Zig: "OSC 133: end_input_start_output with options".
    #[test]
    fn end_input_start_output_with_options() {
        let p = prompt("133;C;aid=foo");
        assert_eq!(p.aid(), Some("foo"));
    }

    // Zig: "OSC 133: end_input_start_output with cmdline".
    #[test]
    fn end_input_start_output_with_cmdline() {
        let p = prompt("133;C;cmdline=echo bobr kurwa");
        let mut out = String::new();
        p.write_command_line(&mut out).unwrap();
        assert_eq!(out, "echo bobr kurwa");
    }

    // Zig: "OSC 133: end_input_start_output with cmdline 3".
    #[test]
    fn end_input_start_output_with_cmdline_3() {
        let p = prompt("133;C;cmdline=echo bobr\\nkurwa");
        let mut out = String::new();
        p.write_command_line(&mut out).unwrap();
        assert_eq!(out, "echo bobr\nkurwa");
    }

    // Zig: "OSC 133: end_input_start_output with cmdline 4".
    #[test]
    fn end_input_start_output_with_cmdline_4() {
        let p = prompt("133;C;cmdline=$'echo bobr kurwa'");
        let mut out = String::new();
        p.write_command_line(&mut out).unwrap();
        assert_eq!(out, "echo bobr kurwa");
    }

    // Zig: "OSC 133: end_input_start_output with cmdline 5".
    #[test]
    fn end_input_start_output_with_cmdline_5() {
        let p = prompt("133;C;cmdline='echo bobr kurwa'");
        let mut out = String::new();
        p.write_command_line(&mut out).unwrap();
        assert_eq!(out, "echo bobr kurwa");
    }

    // Zig: "OSC 133: end_input_start_output with cmdline 6".
    #[test]
    fn end_input_start_output_with_cmdline_6() {
        let p = prompt("133;C;cmdline='echo bobr kurwa");
        let mut out = String::new();
        assert!(p.write_command_line(&mut out).is_err());
    }

    // Zig: "OSC 133: end_input_start_output with cmdline 7".
    #[test]
    fn end_input_start_output_with_cmdline_7() {
        let p = prompt("133;C;cmdline=$'echo bobr kurwa");
        let mut out = String::new();
        assert!(p.write_command_line(&mut out).is_err());
    }

    // Zig: "OSC 133: end_input_start_output with cmdline 8".
    #[test]
    fn end_input_start_output_with_cmdline_8() {
        let p = prompt("133;C;cmdline=$'");
        let mut out = String::new();
        assert!(p.write_command_line(&mut out).is_err());
    }

    // Zig: "OSC 133: end_input_start_output with cmdline 9".
    #[test]
    fn end_input_start_output_with_cmdline_9() {
        let p = prompt("133;C;cmdline=");
        let mut out = String::new();
        p.write_command_line(&mut out).unwrap();
        assert_eq!(out, "");
    }

    // Zig: "OSC 133: end_input_start_output with cmdline_url 1".
    #[test]
    fn end_input_start_output_with_cmdline_url_1() {
        let p = prompt("133;C;cmdline_url=echo bobr kurwa");
        let mut out = String::new();
        p.write_command_line(&mut out).unwrap();
        assert_eq!(out, "echo bobr kurwa");
    }

    // Zig: "OSC 133: end_input_start_output with cmdline_url 2".
    #[test]
    fn end_input_start_output_with_cmdline_url_2() {
        let p = prompt("133;C;cmdline_url=echo bobr%20kurwa");
        let mut out = String::new();
        p.write_command_line(&mut out).unwrap();
        assert_eq!(out, "echo bobr kurwa");
    }

    // Zig: "OSC 133: end_input_start_output with cmdline_url 3".
    #[test]
    fn end_input_start_output_with_cmdline_url_3() {
        let p = prompt("133;C;cmdline_url=echo bobr%3bkurwa");
        let mut out = String::new();
        p.write_command_line(&mut out).unwrap();
        assert_eq!(out, "echo bobr;kurwa");
    }

    // Zig: "OSC 133: end_input_start_output with cmdline_url 4".
    #[test]
    fn end_input_start_output_with_cmdline_url_4() {
        let p = prompt("133;C;cmdline_url=echo bobr%3kurwa");
        let mut out = String::new();
        assert!(p.write_command_line(&mut out).is_err());
    }

    // Zig: "OSC 133: end_input_start_output with cmdline_url 5".
    #[test]
    fn end_input_start_output_with_cmdline_url_5() {
        let p = prompt("133;C;cmdline_url=echo bobr%kurwa");
        let mut out = String::new();
        assert!(p.write_command_line(&mut out).is_err());
    }

    // Zig: "OSC 133: end_input_start_output with cmdline_url 6".
    #[test]
    fn end_input_start_output_with_cmdline_url_6() {
        let p = prompt("133;C;cmdline_url=echo bobr kurwa%20");
        let mut out = String::new();
        p.write_command_line(&mut out).unwrap();
        assert_eq!(out, "echo bobr kurwa ");
    }

    // Zig: "OSC 133: end_input_start_output with cmdline_url 7".
    #[test]
    fn end_input_start_output_with_cmdline_url_7() {
        let p = prompt("133;C;cmdline_url=echo bobr kurwa%2");
        let mut out = String::new();
        assert!(p.write_command_line(&mut out).is_err());
    }

    // Zig: "OSC 133: end_input_start_output with cmdline_url 8".
    #[test]
    fn end_input_start_output_with_cmdline_url_8() {
        let p = prompt("133;C;cmdline_url=echo bobr kurwa%");
        let mut out = String::new();
        assert!(p.write_command_line(&mut out).is_err());
    }

    // --- L: fresh_line ---

    // Zig: "OSC 133: fresh_line".
    #[test]
    fn fresh_line() {
        let p = prompt("133;L");
        assert_eq!(p.action, SemanticPromptAction::FreshLine);
    }

    // Zig: "OSC 133: fresh_line extra contents".
    #[test]
    fn fresh_line_extra_contents() {
        assert_eq!(run("133;Lol"), None);
        assert_eq!(run("133;L;aid=foo"), None);
    }

    // --- A: fresh_line_new_prompt ---

    // Zig: "OSC 133: fresh_line_new_prompt".
    #[test]
    fn fresh_line_new_prompt_bare() {
        let p = prompt("133;A");
        assert_eq!(p.action, SemanticPromptAction::FreshLineNewPrompt);
        assert_eq!(p.aid(), None);
        assert_eq!(p.cl(), None);
    }

    // Zig: "OSC 133: fresh_line_new_prompt with aid".
    #[test]
    fn fresh_line_new_prompt_with_aid() {
        let p = prompt("133;A;aid=14");
        assert_eq!(p.aid(), Some("14"));
    }

    // Zig: "OSC 133: fresh_line_new_prompt with '=' in aid".
    #[test]
    fn fresh_line_new_prompt_with_equals_in_aid() {
        let p = prompt("133;A;aid=a=b");
        assert_eq!(p.aid(), Some("a=b"));
    }

    // Zig: "OSC 133: fresh_line_new_prompt with cl=line".
    #[test]
    fn fresh_line_new_prompt_with_cl_line() {
        let p = prompt("133;A;cl=line");
        assert_eq!(p.cl(), Some(Click::Line));
    }

    // Zig: "OSC 133: fresh_line_new_prompt with cl=m".
    #[test]
    fn fresh_line_new_prompt_with_cl_m() {
        let p = prompt("133;A;cl=m");
        assert_eq!(p.cl(), Some(Click::Multiple));
    }

    // Zig: "OSC 133: fresh_line_new_prompt with invalid cl".
    #[test]
    fn fresh_line_new_prompt_with_invalid_cl() {
        let p = prompt("133;A;cl=invalid");
        assert_eq!(p.cl(), None);
    }

    // Zig: "OSC 133: fresh_line_new_prompt with trailing ;".
    #[test]
    fn fresh_line_new_prompt_with_trailing_semicolon() {
        let p = prompt("133;A;");
        assert_eq!(p.action, SemanticPromptAction::FreshLineNewPrompt);
    }

    // Zig: "OSC 133: fresh_line_new_prompt with bare key".
    #[test]
    fn fresh_line_new_prompt_with_bare_key() {
        let p = prompt("133;A;barekey");
        assert_eq!(p.aid(), None);
        assert_eq!(p.cl(), None);
    }

    // Zig: "OSC 133: fresh_line_new_prompt with multiple options".
    #[test]
    fn fresh_line_new_prompt_with_multiple_options() {
        let p = prompt("133;A;aid=foo;cl=line");
        assert_eq!(p.aid(), Some("foo"));
        assert_eq!(p.cl(), Some(Click::Line));
    }

    // Zig: "OSC 133: fresh_line_new_prompt default redraw".
    #[test]
    fn fresh_line_new_prompt_default_redraw() {
        let p = prompt("133;A");
        assert_eq!(p.redraw(), None);
    }

    // Zig: "OSC 133: fresh_line_new_prompt with redraw=0".
    #[test]
    fn fresh_line_new_prompt_with_redraw_0() {
        let p = prompt("133;A;redraw=0");
        assert_eq!(p.redraw(), Some(Redraw::False));
    }

    // Zig: "OSC 133: fresh_line_new_prompt with redraw=1".
    #[test]
    fn fresh_line_new_prompt_with_redraw_1() {
        let p = prompt("133;A;redraw=1");
        assert_eq!(p.redraw(), Some(Redraw::True));
    }

    // Zig: "OSC 133: fresh_line_new_prompt with invalid redraw".
    #[test]
    fn fresh_line_new_prompt_with_invalid_redraw() {
        let p = prompt("133;A;redraw=x");
        assert_eq!(p.redraw(), None);
    }

    // --- P: prompt_start ---

    // Zig: "OSC 133: prompt_start".
    #[test]
    fn prompt_start_bare() {
        let p = prompt("133;P");
        assert_eq!(p.action, SemanticPromptAction::PromptStart);
        assert_eq!(p.prompt_kind(), None);
    }

    // Zig: "OSC 133: prompt_start with k=i".
    #[test]
    fn prompt_start_with_k_i() {
        assert_eq!(prompt("133;P;k=i").prompt_kind(), Some(PromptKind::Initial));
    }

    // Zig: "OSC 133: prompt_start with k=r".
    #[test]
    fn prompt_start_with_k_r() {
        assert_eq!(prompt("133;P;k=r").prompt_kind(), Some(PromptKind::Right));
    }

    // Zig: "OSC 133: prompt_start with k=c".
    #[test]
    fn prompt_start_with_k_c() {
        assert_eq!(
            prompt("133;P;k=c").prompt_kind(),
            Some(PromptKind::Continuation)
        );
    }

    // Zig: "OSC 133: prompt_start with k=s".
    #[test]
    fn prompt_start_with_k_s() {
        assert_eq!(
            prompt("133;P;k=s").prompt_kind(),
            Some(PromptKind::Secondary)
        );
    }

    // Zig: "OSC 133: prompt_start with invalid k".
    #[test]
    fn prompt_start_with_invalid_k() {
        assert_eq!(prompt("133;P;k=x").prompt_kind(), None);
    }

    // Zig: "OSC 133: prompt_start extra contents".
    #[test]
    fn prompt_start_extra_contents() {
        assert_eq!(run("133;Pextra"), None);
    }

    // --- N: new_command ---

    // Zig: "OSC 133: new_command".
    #[test]
    fn new_command_bare() {
        let p = prompt("133;N");
        assert_eq!(p.action, SemanticPromptAction::NewCommand);
        assert_eq!(p.aid(), None);
        assert_eq!(p.cl(), None);
    }

    // Zig: "OSC 133: new_command with aid".
    #[test]
    fn new_command_with_aid() {
        assert_eq!(prompt("133;N;aid=foo").aid(), Some("foo"));
    }

    // Zig: "OSC 133: new_command with cl=line".
    #[test]
    fn new_command_with_cl_line() {
        assert_eq!(prompt("133;N;cl=line").cl(), Some(Click::Line));
    }

    // Zig: "OSC 133: new_command with multiple options".
    #[test]
    fn new_command_with_multiple_options() {
        let p = prompt("133;N;aid=foo;cl=line");
        assert_eq!(p.aid(), Some("foo"));
        assert_eq!(p.cl(), Some(Click::Line));
    }

    // Zig: "OSC 133: new_command extra contents".
    #[test]
    fn new_command_extra_contents() {
        assert_eq!(run("133;Nextra"), None);
    }

    // --- B: end_prompt_start_input ---

    // Zig: "OSC 133: end_prompt_start_input".
    #[test]
    fn end_prompt_start_input() {
        assert_eq!(
            prompt("133;B").action,
            SemanticPromptAction::EndPromptStartInput
        );
    }

    // Zig: "OSC 133: end_prompt_start_input extra contents".
    #[test]
    fn end_prompt_start_input_extra_contents() {
        assert_eq!(run("133;Bextra"), None);
    }

    // Zig: "OSC 133: end_prompt_start_input with options".
    #[test]
    fn end_prompt_start_input_with_options() {
        assert_eq!(prompt("133;B;aid=foo").aid(), Some("foo"));
    }

    // --- I: end_prompt_start_input_terminate_eol ---

    // Zig: "OSC 133: end_prompt_start_input_terminate_eol".
    #[test]
    fn end_prompt_start_input_terminate_eol() {
        assert_eq!(
            prompt("133;I").action,
            SemanticPromptAction::EndPromptStartInputTerminateEol
        );
    }

    // Zig: "OSC 133: end_prompt_start_input_terminate_eol extra contents".
    #[test]
    fn end_prompt_start_input_terminate_eol_extra_contents() {
        assert_eq!(run("133;Iextra"), None);
    }

    // Zig: "OSC 133: end_prompt_start_input_terminate_eol with options".
    #[test]
    fn end_prompt_start_input_terminate_eol_with_options() {
        assert_eq!(prompt("133;I;aid=foo").aid(), Some("foo"));
    }

    // --- D: end_command ---

    // Zig: "OSC 133: end_command".
    #[test]
    fn end_command_bare() {
        let p = prompt("133;D");
        assert_eq!(p.action, SemanticPromptAction::EndCommand);
        assert_eq!(p.exit_code(), None);
        assert_eq!(p.aid(), None);
        assert_eq!(p.err(), None);
    }

    // Zig: "OSC 133: end_command extra contents".
    #[test]
    fn end_command_extra_contents() {
        assert_eq!(run("133;Dextra"), None);
    }

    // Zig: "OSC 133: end_command with exit code 0".
    #[test]
    fn end_command_with_exit_code_0() {
        assert_eq!(prompt("133;D;0").exit_code(), Some(0));
    }

    // Zig: "OSC 133: end_command with exit code and aid".
    #[test]
    fn end_command_with_exit_code_and_aid() {
        let p = prompt("133;D;12;aid=foo");
        assert_eq!(p.aid(), Some("foo"));
        assert_eq!(p.exit_code(), Some(12));
    }

    // --- Option.read unit tests (direct field-scan exercise) ---

    // Zig: "Option.read aid".
    #[test]
    fn option_read_aid() {
        assert_eq!(read_semicolon_field("aid=test123", "aid"), Some("test123"));
        assert_eq!(
            read_semicolon_field("cl=line;aid=myaid;k=i", "aid"),
            Some("myaid")
        );
        assert_eq!(read_semicolon_field("cl=line;k=i", "aid"), None);
        assert_eq!(read_semicolon_field("aid=", "aid"), Some(""));
        assert_eq!(read_semicolon_field("k=i;aid=last", "aid"), Some("last"));
        assert_eq!(read_semicolon_field("aid=first;k=i", "aid"), Some("first"));
        assert_eq!(read_semicolon_field("", "aid"), None);
        assert_eq!(read_semicolon_field("aid", "aid"), None);
        assert_eq!(read_semicolon_field(";;aid=value;;", "aid"), Some("value"));
    }

    // Zig: "Option.read cl".
    #[test]
    fn option_read_cl() {
        assert_eq!(Click::init("line"), Some(Click::Line));
        assert_eq!(Click::init("m"), Some(Click::Multiple));
        assert_eq!(Click::init("v"), Some(Click::ConservativeVertical));
        assert_eq!(Click::init("w"), Some(Click::SmartVertical));
        assert_eq!(Click::init("invalid"), None);
        assert_eq!(read_semicolon_field("aid=foo", "cl"), None);
    }

    // Zig: "Option.read prompt_kind".
    #[test]
    fn option_read_prompt_kind() {
        assert_eq!(PromptKind::init(b'i'), Some(PromptKind::Initial));
        assert_eq!(PromptKind::init(b'r'), Some(PromptKind::Right));
        assert_eq!(PromptKind::init(b'c'), Some(PromptKind::Continuation));
        assert_eq!(PromptKind::init(b's'), Some(PromptKind::Secondary));
        assert_eq!(PromptKind::init(b'x'), None);
        assert_eq!(prompt("133;P;k=ii").prompt_kind(), None);
        assert_eq!(prompt("133;P;k=").prompt_kind(), None);
    }

    // Zig: "Option.read err".
    #[test]
    fn option_read_err() {
        assert_eq!(prompt("133;C;err=some_error").err(), Some("some_error"));
        assert_eq!(read_semicolon_field("aid=foo", "err"), None);
    }

    // Zig: "Option.read redraw".
    #[test]
    fn option_read_redraw() {
        assert_eq!(prompt("133;A;redraw=1").redraw(), Some(Redraw::True));
        assert_eq!(prompt("133;A;redraw=0").redraw(), Some(Redraw::False));
        assert_eq!(prompt("133;A;redraw=last").redraw(), Some(Redraw::Last));
        assert_eq!(prompt("133;A;redraw=2").redraw(), None);
        assert_eq!(prompt("133;A;redraw=10").redraw(), None);
        assert_eq!(prompt("133;A;redraw=").redraw(), None);
    }

    // Zig: "Option.read special_key".
    #[test]
    fn option_read_special_key() {
        assert_eq!(prompt("133;A;special_key=1").special_key(), Some(true));
        assert_eq!(prompt("133;A;special_key=0").special_key(), Some(false));
        assert_eq!(prompt("133;A;special_key=x").special_key(), None);
    }

    // Zig: "Option.read click_events".
    #[test]
    fn option_read_click_events() {
        assert_eq!(prompt("133;A;click_events=yes").click_events(), None);
        assert_eq!(prompt("133;A;click_events=0").click_events(), None);
        assert_eq!(
            prompt("133;A;click_events=1").click_events(),
            Some(ClickEvents::Absolute)
        );
        assert_eq!(
            prompt("133;A;click_events=2").click_events(),
            Some(ClickEvents::Relative)
        );
    }

    // Zig: "Option.read exit_code".
    #[test]
    fn option_read_exit_code() {
        assert_eq!(prompt("133;D;42").exit_code(), Some(42));
        assert_eq!(prompt("133;D;0").exit_code(), Some(0));
        assert_eq!(prompt("133;D;-1").exit_code(), Some(-1));
        assert_eq!(prompt("133;D;abc").exit_code(), None);
        assert_eq!(prompt("133;D;127;aid=foo").exit_code(), Some(127));
    }
}
