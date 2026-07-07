//! OSC 3008: hierarchical context signalling (UAPI spec). Port of
//! `osc/parsers/context_signal.zig`.

use crate::osc::Command;
use crate::osc::support::read_semicolon_field;

const MAX_CONTEXT_ID_LEN: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextAction {
    Start,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextType {
    Boot,
    Container,
    Vm,
    Elevate,
    Chpriv,
    Subcontext,
    Remote,
    Shell,
    Command,
    App,
    Service,
    Session,
}

impl ContextType {
    pub fn parse(value: &str) -> Option<ContextType> {
        Some(match value {
            "boot" => ContextType::Boot,
            "container" => ContextType::Container,
            "vm" => ContextType::Vm,
            "elevate" => ContextType::Elevate,
            "chpriv" => ContextType::Chpriv,
            "subcontext" => ContextType::Subcontext,
            "remote" => ContextType::Remote,
            "shell" => ContextType::Shell,
            "command" => ContextType::Command,
            "app" => ContextType::App,
            "service" => ContextType::Service,
            "session" => ContextType::Session,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitStatus {
    Success,
    Failure,
    Crash,
    Interrupt,
}

impl ExitStatus {
    pub fn parse(value: &str) -> Option<ExitStatus> {
        Some(match value {
            "success" => ExitStatus::Success,
            "failure" => ExitStatus::Failure,
            "crash" => ExitStatus::Crash,
            "interrupt" => ExitStatus::Interrupt,
            _ => return None,
        })
    }
}

/// An OSC 3008 context signal command. Port of `context_signal.zig`
/// `Command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextSignal {
    pub action: ContextAction,
    pub id: String,
    pub metadata: String,
}

impl ContextSignal {
    pub fn r#type(&self) -> Option<ContextType> {
        read_semicolon_field(&self.metadata, "type").and_then(ContextType::parse)
    }
    pub fn user(&self) -> Option<&str> {
        read_semicolon_field(&self.metadata, "user")
    }
    pub fn hostname(&self) -> Option<&str> {
        read_semicolon_field(&self.metadata, "hostname")
    }
    pub fn machineid(&self) -> Option<&str> {
        read_semicolon_field(&self.metadata, "machineid")
    }
    pub fn bootid(&self) -> Option<&str> {
        read_semicolon_field(&self.metadata, "bootid")
    }
    pub fn pid(&self) -> Option<u64> {
        read_semicolon_field(&self.metadata, "pid")
            .filter(|v| v.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|v| v.parse().ok())
    }
    pub fn pidfdid(&self) -> Option<u64> {
        read_semicolon_field(&self.metadata, "pidfdid")
            .filter(|v| v.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|v| v.parse().ok())
    }
    pub fn comm(&self) -> Option<&str> {
        read_semicolon_field(&self.metadata, "comm")
    }
    pub fn cwd(&self) -> Option<&str> {
        read_semicolon_field(&self.metadata, "cwd")
    }
    pub fn cmdline(&self) -> Option<&str> {
        read_semicolon_field(&self.metadata, "cmdline")
    }
    pub fn container(&self) -> Option<&str> {
        read_semicolon_field(&self.metadata, "container")
    }
    pub fn exit(&self) -> Option<ExitStatus> {
        read_semicolon_field(&self.metadata, "exit").and_then(ExitStatus::parse)
    }
    pub fn status(&self) -> Option<u64> {
        read_semicolon_field(&self.metadata, "status")
            .filter(|v| v.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|v| v.parse().ok())
    }
    pub fn signal(&self) -> Option<&str> {
        read_semicolon_field(&self.metadata, "signal")
    }
}

/// Parse OSC 3008. Port of `context_signal.zig` `parse`.
pub fn parse(rest: &str) -> Option<Command> {
    let data = rest.strip_prefix(';')?;
    if data.is_empty() {
        return None;
    }

    let (action, prefix_len) = if let Some(r) = data.strip_prefix("start=") {
        (ContextAction::Start, data.len() - r.len())
    } else if let Some(r) = data.strip_prefix("end=") {
        (ContextAction::End, data.len() - r.len())
    } else {
        return None;
    };

    let after_prefix = &data[prefix_len..];
    if after_prefix.is_empty() {
        return None;
    }

    let id_end = after_prefix.find(';').unwrap_or(after_prefix.len());
    let id = &after_prefix[..id_end];

    if id.is_empty() || id.len() > MAX_CONTEXT_ID_LEN {
        return None;
    }
    if !id.bytes().all(|c| (0x20..=0x7e).contains(&c)) {
        return None;
    }

    let metadata = if id_end < after_prefix.len() {
        &after_prefix[id_end + 1..]
    } else {
        ""
    };

    Some(Command::ContextSignal(ContextSignal {
        action,
        id: id.to_string(),
        metadata: metadata.to_string(),
    }))
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

    fn signal(body: &str) -> ContextSignal {
        match run(body) {
            Some(Command::ContextSignal(s)) => s,
            other => panic!("expected ContextSignal, got {other:?}"),
        }
    }

    // Zig: "OSC 3008: basic start command".
    #[test]
    fn basic_start_command() {
        let s = signal("3008;start=abc123");
        assert_eq!(s.action, ContextAction::Start);
        assert_eq!(s.id, "abc123");
        assert_eq!(s.metadata, "");
    }

    // Zig: "OSC 3008: basic end command".
    #[test]
    fn basic_end_command() {
        let s = signal("3008;end=abc123");
        assert_eq!(s.action, ContextAction::End);
        assert_eq!(s.id, "abc123");
        assert_eq!(s.metadata, "");
    }

    // Zig: "OSC 3008: start with metadata fields".
    #[test]
    fn start_with_metadata_fields() {
        let s = signal(
            "3008;start=bed86fab93af4328bbed0a1224af6d40;type=container;user=lennart;hostname=zeta",
        );
        assert_eq!(s.action, ContextAction::Start);
        assert_eq!(s.id, "bed86fab93af4328bbed0a1224af6d40");
        assert_eq!(s.r#type(), Some(ContextType::Container));
        assert_eq!(s.user(), Some("lennart"));
        assert_eq!(s.hostname(), Some("zeta"));
    }

    // Zig: "OSC 3008: start with all common fields".
    #[test]
    fn start_with_all_common_fields() {
        let s = signal(
            "3008;start=myctx;type=shell;user=root;hostname=myhost;machineid=3deb5353d3ba43d08201c136a47ead7b;bootid=d4a3d0fdf2e24fdea6d971ce73f4fbf2;pid=1062862;pidfdid=1063162;comm=bash",
        );
        assert_eq!(s.r#type(), Some(ContextType::Shell));
        assert_eq!(s.user(), Some("root"));
        assert_eq!(s.hostname(), Some("myhost"));
        assert_eq!(s.machineid(), Some("3deb5353d3ba43d08201c136a47ead7b"));
        assert_eq!(s.bootid(), Some("d4a3d0fdf2e24fdea6d971ce73f4fbf2"));
        assert_eq!(s.pid(), Some(1062862));
        assert_eq!(s.pidfdid(), Some(1063162));
        assert_eq!(s.comm(), Some("bash"));
    }

    // Zig: "OSC 3008: end with exit metadata".
    #[test]
    fn end_with_exit_metadata() {
        let s = signal("3008;end=myctx;exit=success;status=0");
        assert_eq!(s.action, ContextAction::End);
        assert_eq!(s.id, "myctx");
        assert_eq!(s.exit(), Some(ExitStatus::Success));
        assert_eq!(s.status(), Some(0));
    }

    // Zig: "OSC 3008: end with failure exit".
    #[test]
    fn end_with_failure_exit() {
        let s = signal("3008;end=myctx;exit=failure;status=1;signal=SIGKILL");
        assert_eq!(s.exit(), Some(ExitStatus::Failure));
        assert_eq!(s.status(), Some(1));
        assert_eq!(s.signal(), Some("SIGKILL"));
    }

    // Zig: "OSC 3008: unknown fields are ignored".
    #[test]
    fn unknown_fields_are_ignored() {
        let s = signal("3008;start=myctx;type=shell;unknownfield=value;user=root");
        assert_eq!(s.r#type(), Some(ContextType::Shell));
        assert_eq!(s.user(), Some("root"));
    }

    // Zig: "OSC 3008: missing field returns null".
    #[test]
    fn missing_field_returns_null() {
        let s = signal("3008;start=myctx;user=lennart");
        assert_eq!(s.r#type(), None);
        assert_eq!(s.hostname(), None);
        assert_eq!(s.pid(), None);
    }

    // Zig: "OSC 3008: invalid prefix".
    #[test]
    fn invalid_prefix() {
        assert_eq!(run("3008;bogus=abc123"), None);
    }

    // Zig: "OSC 3008: empty data".
    #[test]
    fn empty_data() {
        assert_eq!(run("3008;start="), None);
    }

    // Zig: "OSC 3008: max length context ID".
    #[test]
    fn max_length_context_id() {
        let id = "a".repeat(64);
        let s = signal(&format!("3008;start={id}"));
        assert_eq!(s.id, id);
    }

    // Zig: "OSC 3008: over-length context ID".
    #[test]
    fn over_length_context_id() {
        let id = "a".repeat(65);
        assert_eq!(run(&format!("3008;start={id}")), None);
    }

    // Zig: "OSC 3008: context type enum coverage".
    #[test]
    fn context_type_enum_coverage() {
        let cases = [
            ("boot", ContextType::Boot),
            ("container", ContextType::Container),
            ("vm", ContextType::Vm),
            ("elevate", ContextType::Elevate),
            ("chpriv", ContextType::Chpriv),
            ("subcontext", ContextType::Subcontext),
            ("remote", ContextType::Remote),
            ("shell", ContextType::Shell),
            ("command", ContextType::Command),
            ("app", ContextType::App),
            ("service", ContextType::Service),
            ("session", ContextType::Session),
        ];
        for (s, expected) in cases {
            assert_eq!(ContextType::parse(s), Some(expected));
        }
        assert_eq!(ContextType::parse("invalid"), None);
    }

    // Zig: "OSC 3008: exit status enum coverage".
    #[test]
    fn exit_status_enum_coverage() {
        assert_eq!(ExitStatus::parse("success"), Some(ExitStatus::Success));
        assert_eq!(ExitStatus::parse("failure"), Some(ExitStatus::Failure));
        assert_eq!(ExitStatus::parse("crash"), Some(ExitStatus::Crash));
        assert_eq!(ExitStatus::parse("interrupt"), Some(ExitStatus::Interrupt));
        assert_eq!(ExitStatus::parse("invalid"), None);
    }

    // Zig: "OSC 3008: spec example - container start".
    #[test]
    fn spec_example_container_start() {
        let s = signal(
            "3008;start=bed86fab93af4328bbed0a1224af6d40;type=container;user=lennart;hostname=zeta;machineid=3deb5353d3ba43d08201c136a47ead7b;bootid=d4a3d0fdf2e24fdea6d971ce73f4fbf2;pid=1062862;pidfdid=1063162;comm=systemd-nspawn;container=foobar",
        );
        assert_eq!(s.action, ContextAction::Start);
        assert_eq!(s.id, "bed86fab93af4328bbed0a1224af6d40");
        assert_eq!(s.r#type(), Some(ContextType::Container));
        assert_eq!(s.user(), Some("lennart"));
        assert_eq!(s.hostname(), Some("zeta"));
        assert_eq!(s.comm(), Some("systemd-nspawn"));
        assert_eq!(s.container(), Some("foobar"));
        assert_eq!(s.pid(), Some(1062862));
    }

    // Zig: "OSC 3008: spec example - context end".
    #[test]
    fn spec_example_context_end() {
        let s = signal("3008;end=bed86fab93af4328bbed0a1224af6d40");
        assert_eq!(s.action, ContextAction::End);
        assert_eq!(s.id, "bed86fab93af4328bbed0a1224af6d40");
    }

    // Zig: "OSC 3008: cwd and cmdline fields".
    #[test]
    fn cwd_and_cmdline_fields() {
        let s = signal("3008;start=myctx;type=command;cwd=/home/user;cmdline=ls -la");
        assert_eq!(s.cwd(), Some("/home/user"));
        assert_eq!(s.cmdline(), Some("ls -la"));
    }

    // Zig: "OSC 3008: start command with no fields".
    #[test]
    fn start_command_with_no_fields() {
        let s = signal("3008;start=simpleid");
        assert_eq!(s.action, ContextAction::Start);
        assert_eq!(s.id, "simpleid");
        assert_eq!(s.r#type(), None);
        assert_eq!(s.user(), None);
        assert_eq!(s.exit(), None);
    }
}
