//! OSC 5522: kitty clipboard protocol. Port of
//! `osc/parsers/kitty_clipboard_protocol.zig`.

use crate::osc::support::read_key_value;
use crate::osc::{Command, Terminator};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardLocation {
    Primary,
}

impl ClipboardLocation {
    fn init(s: &str) -> Option<ClipboardLocation> {
        match s {
            "primary" => Some(ClipboardLocation::Primary),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardOperation {
    Read,
    Walias,
    Wdata,
    Write,
}

impl ClipboardOperation {
    fn init(s: &str) -> Option<ClipboardOperation> {
        match s {
            "read" => Some(ClipboardOperation::Read),
            "walias" => Some(ClipboardOperation::Walias),
            "wdata" => Some(ClipboardOperation::Wdata),
            "write" => Some(ClipboardOperation::Write),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardStatus {
    Data,
    Done,
    Ebusy,
    Einval,
    Eio,
    Enosys,
    Eperm,
    Ok,
}

impl ClipboardStatus {
    fn init(s: &str) -> Option<ClipboardStatus> {
        match s {
            "DATA" => Some(ClipboardStatus::Data),
            "DONE" => Some(ClipboardStatus::Done),
            "EBUSY" => Some(ClipboardStatus::Ebusy),
            "EINVAL" => Some(ClipboardStatus::Einval),
            "EIO" => Some(ClipboardStatus::Eio),
            "ENOSYS" => Some(ClipboardStatus::Enosys),
            "EPERM" => Some(ClipboardStatus::Eperm),
            "OK" => Some(ClipboardStatus::Ok),
            _ => None,
        }
    }
}

const VALID_IDENTIFIER_CHARS: &str =
    "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_+.";

fn is_valid_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| VALID_IDENTIFIER_CHARS.contains(c))
}

/// OSC 5522 command payload. Port of `kitty_clipboard_protocol.zig` `OSC`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyClipboardProtocol {
    pub metadata: String,
    pub payload: Option<String>,
    pub terminator: Terminator,
}

impl KittyClipboardProtocol {
    pub fn id(&self) -> Option<&str> {
        read_key_value(&self.metadata, ':', "id").filter(|s| is_valid_identifier(s))
    }
    pub fn loc(&self) -> Option<ClipboardLocation> {
        read_key_value(&self.metadata, ':', "loc").and_then(ClipboardLocation::init)
    }
    pub fn mime(&self) -> Option<&str> {
        read_key_value(&self.metadata, ':', "mime")
    }
    pub fn name(&self) -> Option<&str> {
        read_key_value(&self.metadata, ':', "name")
    }
    pub fn password(&self) -> Option<&str> {
        read_key_value(&self.metadata, ':', "password")
    }
    pub fn pw(&self) -> Option<&str> {
        read_key_value(&self.metadata, ':', "pw")
    }
    pub fn status(&self) -> Option<ClipboardStatus> {
        read_key_value(&self.metadata, ':', "status").and_then(ClipboardStatus::init)
    }
    pub fn op_type(&self) -> Option<ClipboardOperation> {
        read_key_value(&self.metadata, ':', "type").and_then(ClipboardOperation::init)
    }
}

/// Parse OSC 5522. Port of `kitty_clipboard_protocol.zig` `parse`.
pub fn parse(rest: &str, terminator: Terminator) -> Option<Command> {
    let data = rest.strip_prefix(';').unwrap_or("");
    let (metadata, payload) = match data.find(';') {
        Some(sep) => (&data[..sep], Some(data[sep + 1..].to_string())),
        None => (data, None),
    };
    Some(Command::KittyClipboardProtocol(KittyClipboardProtocol {
        metadata: metadata.to_string(),
        payload,
        terminator,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    fn run(body: &str) -> KittyClipboardProtocol {
        let mut p = osc::Parser::with_allocator();
        for c in body.bytes() {
            p.next(c);
        }
        match p.end(Some(0x1b)) {
            Some(Command::KittyClipboardProtocol(d)) => d,
            other => panic!("expected KittyClipboardProtocol, got {other:?}"),
        }
    }

    fn assert_all_none(d: &KittyClipboardProtocol) {
        assert_eq!(d.id(), None);
        assert_eq!(d.loc(), None);
        assert_eq!(d.mime(), None);
        assert_eq!(d.name(), None);
        assert_eq!(d.password(), None);
        assert_eq!(d.pw(), None);
        assert_eq!(d.status(), None);
    }

    // Zig: "OSC: 5522: empty metadata and missing payload".
    #[test]
    fn empty_metadata_and_missing_payload() {
        let d = run("5522;");
        assert_eq!(d.metadata, "");
        assert_eq!(d.payload, None);
        assert_all_none(&d);
        assert_eq!(d.op_type(), None);
    }

    // Zig: "OSC: 5522: empty metadata and empty payload".
    #[test]
    fn empty_metadata_and_empty_payload() {
        let d = run("5522;;");
        assert_eq!(d.metadata, "");
        assert_eq!(d.payload, Some(String::new()));
        assert_all_none(&d);
    }

    // Zig: "OSC: 5522: non-empty metadata and payload".
    #[test]
    fn non_empty_metadata_and_payload() {
        let d = run("5522;type=read;dGV4dC9wbGFpbg==");
        assert_eq!(d.metadata, "type=read");
        assert_eq!(d.payload, Some("dGV4dC9wbGFpbg==".to_string()));
        assert_all_none(&d);
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }

    // Zig: "OSC: 5522: empty id".
    #[test]
    fn empty_id() {
        let d = run("5522;id=");
        assert_eq!(d.id(), None);
    }

    // Zig: "OSC: 5522: valid id".
    #[test]
    fn valid_id() {
        let d = run("5522;id=5c076ad9-d36f-4705-847b-d4dbf356cc0d");
        assert_eq!(d.id(), Some("5c076ad9-d36f-4705-847b-d4dbf356cc0d"));
    }

    // Zig: "OSC: 5522: invalid id".
    #[test]
    fn invalid_id() {
        let d = run("5522;id=*42*");
        assert_eq!(d.id(), None);
    }

    // Zig: "OSC: 5522: invalid status".
    #[test]
    fn invalid_status() {
        let d = run("5522;status=BOBR");
        assert_eq!(d.status(), None);
    }

    // Zig: "OSC: 5522: valid status".
    #[test]
    fn valid_status() {
        let d = run("5522;status=DONE");
        assert_eq!(d.status(), Some(ClipboardStatus::Done));
    }

    // Zig: "OSC: 5522: invalid location".
    #[test]
    fn invalid_location() {
        let d = run("5522;loc=bobr");
        assert_eq!(d.loc(), None);
    }

    // Zig: "OSC: 5522: valid location".
    #[test]
    fn valid_location() {
        let d = run("5522;loc=primary");
        assert_eq!(d.loc(), Some(ClipboardLocation::Primary));
    }

    // Zig: "OSC: 5522: password 1".
    #[test]
    fn password_1() {
        let d = run("5522;pw=R2hvc3R0eQ==:name=Qk9CUiBLVVJXQQ==");
        assert_eq!(d.pw(), Some("R2hvc3R0eQ=="));
        assert_eq!(d.name(), Some("Qk9CUiBLVVJXQQ=="));
    }

    // Zig: "OSC: 5522: password 2".
    #[test]
    fn password_2() {
        let d = run("5522;password=R2hvc3R0eQ==");
        assert_eq!(d.password(), Some("R2hvc3R0eQ=="));
    }

    // Zig: "OSC: 5522: example 1".
    #[test]
    fn example_1() {
        let d = run("5522;type=read:status=OK");
        assert_eq!(d.payload, None);
        assert_eq!(d.id(), None);
        assert_eq!(d.loc(), None);
        assert_eq!(d.mime(), None);
        assert_eq!(d.name(), None);
        assert_eq!(d.password(), None);
        assert_eq!(d.pw(), None);
        assert_eq!(d.status(), Some(ClipboardStatus::Ok));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }

    // Zig: "OSC: 5522: example 2".
    #[test]
    fn example_2() {
        let d = run("5522;type=read:mime=dGV4dC9wbGFpbg==;R2hvc3R0eQ==");
        assert_eq!(d.payload, Some("R2hvc3R0eQ==".to_string()));
        assert_eq!(d.id(), None);
        assert_eq!(d.loc(), None);
        assert_eq!(d.mime(), Some("dGV4dC9wbGFpbg=="));
        assert_eq!(d.name(), None);
        assert_eq!(d.password(), None);
        assert_eq!(d.pw(), None);
        assert_eq!(d.status(), None);
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }

    // Zig: "OSC: 5522: example 3" (duplicate of example 1 in the Zig source).
    #[test]
    fn example_3() {
        let d = run("5522;type=read:status=OK");
        assert_eq!(d.payload, None);
        assert_eq!(d.status(), Some(ClipboardStatus::Ok));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }

    // Zig: "OSC: 5522: example 4".
    #[test]
    fn example_4() {
        let d = run("5522;type=write");
        assert_eq!(d.payload, None);
        assert_eq!(d.status(), None);
        assert_eq!(d.op_type(), Some(ClipboardOperation::Write));
    }

    // Zig: "OSC: 5522: example 5".
    #[test]
    fn example_5() {
        let d = run("5522;type=wdata:mime=dGV4dC9wbGFpbg==;R2hvc3R0eQ==");
        assert_eq!(d.payload, Some("R2hvc3R0eQ==".to_string()));
        assert_eq!(d.mime(), Some("dGV4dC9wbGFpbg=="));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Wdata));
    }

    // Zig: "OSC: 5522: example 6".
    #[test]
    fn example_6() {
        let d = run("5522;type=wdata");
        assert_eq!(d.payload, None);
        assert_eq!(d.op_type(), Some(ClipboardOperation::Wdata));
    }

    // Zig: "OSC: 5522: example 7".
    #[test]
    fn example_7() {
        let d = run("5522;type=write:status=DONE");
        assert_eq!(d.payload, None);
        assert_eq!(d.status(), Some(ClipboardStatus::Done));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Write));
    }

    // Zig: "OSC: 5522: example 8".
    #[test]
    fn example_8() {
        let d = run("5522;type=write:status=EPERM");
        assert_eq!(d.payload, None);
        assert_eq!(d.status(), Some(ClipboardStatus::Eperm));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Write));
    }

    // Zig: "OSC: 5522: example 9".
    #[test]
    fn example_9() {
        let d = run("5522;type=walias:mime=dGV4dC9wbGFpbg==;dGV4dC9odG1sIGFwcGxpY2F0aW9uL2pzb24=");
        assert_eq!(
            d.payload,
            Some("dGV4dC9odG1sIGFwcGxpY2F0aW9uL2pzb24=".to_string())
        );
        assert_eq!(d.mime(), Some("dGV4dC9wbGFpbg=="));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Walias));
    }

    // Zig: "OSC: 5522: example 10".
    #[test]
    fn example_10() {
        let d = run("5522;type=read:status=OK:password=Qk9CUiBLVVJXQQ==");
        assert_eq!(d.payload, None);
        assert_eq!(d.password(), Some("Qk9CUiBLVVJXQQ=="));
        assert_eq!(d.status(), Some(ClipboardStatus::Ok));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }

    // Zig: "OSC: 5522: example 11".
    #[test]
    fn example_11() {
        let d = run("5522;type=read:status=DATA:mime=dGV4dC9wbGFpbg==");
        assert_eq!(d.payload, None);
        assert_eq!(d.mime(), Some("dGV4dC9wbGFpbg=="));
        assert_eq!(d.status(), Some(ClipboardStatus::Data));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }

    // Zig: "OSC: 5522: example 12".
    #[test]
    fn example_12() {
        let d = run("5522;type=read:mime=dGV4dC9wbGFpbg==:password=Qk9CUiBLVVJXQQ==");
        assert_eq!(d.payload, None);
        assert_eq!(d.mime(), Some("dGV4dC9wbGFpbg=="));
        assert_eq!(d.password(), Some("Qk9CUiBLVVJXQQ=="));
        assert_eq!(d.status(), None);
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }

    // Zig: "OSC: 5522: example 13" (duplicate of example 1 in the Zig source).
    #[test]
    fn example_13() {
        let d = run("5522;type=read:status=OK");
        assert_eq!(d.status(), Some(ClipboardStatus::Ok));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }

    // Zig: "OSC: 5522: example 14".
    #[test]
    fn example_14() {
        let d = run("5522;type=read:status=DATA:mime=dGV4dC9wbGFpbg==;Qk9CUiBLVVJXQQ==");
        assert_eq!(d.payload, Some("Qk9CUiBLVVJXQQ==".to_string()));
        assert_eq!(d.mime(), Some("dGV4dC9wbGFpbg=="));
        assert_eq!(d.status(), Some(ClipboardStatus::Data));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }

    // Zig: "OSC: 5522: example 15" (duplicate of example 1 in the Zig source).
    #[test]
    fn example_15() {
        let d = run("5522;type=read:status=OK");
        assert_eq!(d.status(), Some(ClipboardStatus::Ok));
        assert_eq!(d.op_type(), Some(ClipboardOperation::Read));
    }
}
