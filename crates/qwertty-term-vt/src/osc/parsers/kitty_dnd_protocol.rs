//! OSC 72: kitty drag-and-drop protocol. Port of
//! `osc/parsers/kitty_dnd_protocol.zig`.

use crate::osc::support::read_key_value;
use crate::osc::{Command, Terminator};

/// Values for the `t` (event type) metadata key. Port of
/// `kitty_dnd_protocol.zig` `EventType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DndEventType {
    /// ('a') Terminal registers itself as willing to accept drops.
    AcceptDrops,
    /// ('A') Terminal unregisters itself.
    StopAcceptingDrops,
    /// ('m') Pointer is moving over the terminal during a drag.
    DropMove,
    /// ('M') Items were dropped onto the terminal.
    DropDropped,
    /// ('r') Terminal requests data for a MIME type from the drag source.
    RequestData,
    /// ('R') Error response to a `request_data` event.
    RequestError,
    /// ('o') Terminal offers data for an outgoing drag.
    OfferDrag,
    /// ('p') Drag source presents the payload for a requested MIME type.
    PresentData,
    /// ('P') Replace the current drag image with a new one.
    ChangeDragImage,
    /// ('e') Event notification on an outgoing drag offer.
    DragOfferEvent,
    /// ('E') Error on an outgoing drag offer.
    DragOfferError,
    /// ('k') URI list data delivered as part of a drag/clipboard transfer.
    UriListData,
    /// ('q') Query terminal capabilities.
    Query,
}

impl DndEventType {
    fn init(s: &str) -> Option<DndEventType> {
        if s.len() != 1 {
            return None;
        }
        Some(match s.as_bytes()[0] {
            b'a' => DndEventType::AcceptDrops,
            b'A' => DndEventType::StopAcceptingDrops,
            b'm' => DndEventType::DropMove,
            b'M' => DndEventType::DropDropped,
            b'r' => DndEventType::RequestData,
            b'R' => DndEventType::RequestError,
            b'o' => DndEventType::OfferDrag,
            b'p' => DndEventType::PresentData,
            b'P' => DndEventType::ChangeDragImage,
            b'e' => DndEventType::DragOfferEvent,
            b'E' => DndEventType::DragOfferError,
            b'k' => DndEventType::UriListData,
            b'q' => DndEventType::Query,
            _ => return None,
        })
    }
}

/// OSC 72 command payload. Port of `kitty_dnd_protocol.zig` `OSC`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyDndProtocol {
    pub metadata: String,
    pub payload: Option<String>,
    pub terminator: Terminator,
}

impl KittyDndProtocol {
    /// Read the event-type (`t`) metadata key.
    pub fn event_type(&self) -> Option<DndEventType> {
        read_key_value(&self.metadata, ':', "t").and_then(DndEventType::init)
    }

    /// Read an integer metadata key (`m`/`i`/`o`/`x`/`y`/`X`/`Y`).
    pub fn int_option(&self, key: &str) -> Option<i32> {
        read_key_value(&self.metadata, ':', key)?.parse().ok()
    }
}

/// Parse OSC 72. Port of `kitty_dnd_protocol.zig` `parse`.
pub fn parse(rest: &str, terminator: Terminator) -> Option<Command> {
    let data = rest.strip_prefix(';').unwrap_or("");
    let (metadata, payload) = match data.find(';') {
        Some(sep) => (&data[..sep], Some(data[sep + 1..].to_string())),
        None => (data, None),
    };
    Some(Command::KittyDndProtocol(KittyDndProtocol {
        metadata: metadata.to_string(),
        payload,
        terminator,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    fn run(body: &str, term: u8) -> KittyDndProtocol {
        let mut p = osc::Parser::with_allocator();
        for c in body.bytes() {
            p.next(c);
        }
        match p.end(Some(term)) {
            Some(Command::KittyDndProtocol(d)) => d,
            other => panic!("expected KittyDndProtocol, got {other:?}"),
        }
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: metadata only, no payload".
    #[test]
    fn osc_72_metadata_only_no_payload() {
        let d = run("72;t=a", 0x1b);
        assert_eq!(d.metadata, "t=a");
        assert_eq!(d.payload, None);
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: metadata and empty payload".
    #[test]
    fn osc_72_metadata_and_empty_payload() {
        let d = run("72;t=a;", 0x1b);
        assert_eq!(d.metadata, "t=a");
        assert_eq!(d.payload, Some(String::new()));
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: metadata and non-empty payload".
    #[test]
    fn osc_72_metadata_and_non_empty_payload() {
        let d = run("72;t=a:i=5;text/plain text/uri-list", 0x1b);
        assert_eq!(d.metadata, "t=a:i=5");
        assert_eq!(d.payload, Some("text/plain text/uri-list".to_string()));
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: readOption .t valid event types".
    #[test]
    fn osc_72_read_option_t_valid_event_types() {
        let cases = [
            ("72;t=a", DndEventType::AcceptDrops),
            ("72;t=A", DndEventType::StopAcceptingDrops),
            ("72;t=m", DndEventType::DropMove),
            ("72;t=M", DndEventType::DropDropped),
            ("72;t=r", DndEventType::RequestData),
            ("72;t=R", DndEventType::RequestError),
            ("72;t=o", DndEventType::OfferDrag),
            ("72;t=p", DndEventType::PresentData),
            ("72;t=P", DndEventType::ChangeDragImage),
            ("72;t=e", DndEventType::DragOfferEvent),
            ("72;t=E", DndEventType::DragOfferError),
            ("72;t=k", DndEventType::UriListData),
            ("72;t=q", DndEventType::Query),
        ];
        for (body, expected) in cases {
            let d = run(body, 0x1b);
            assert_eq!(d.event_type(), Some(expected));
        }
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: readOption .t unknown value returns null".
    #[test]
    fn osc_72_read_option_t_unknown_value() {
        let d = run("72;t=z", 0x1b);
        assert_eq!(d.event_type(), None);
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: readOption integer keys".
    #[test]
    fn osc_72_read_option_integer_keys() {
        let d = run("72;t=m:i=3:x=10:y=5:X=320:Y=200:o=1:m=0", 0x1b);
        assert_eq!(d.int_option("i"), Some(3));
        assert_eq!(d.int_option("x"), Some(10));
        assert_eq!(d.int_option("y"), Some(5));
        assert_eq!(d.int_option("X"), Some(320));
        assert_eq!(d.int_option("Y"), Some(200));
        assert_eq!(d.int_option("o"), Some(1));
        assert_eq!(d.int_option("m"), Some(0));
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: readOption negative sentinel".
    #[test]
    fn osc_72_read_option_negative_sentinel() {
        let d = run("72;t=m:x=-1:y=-1", 0x1b);
        assert_eq!(d.int_option("x"), Some(-1));
        assert_eq!(d.int_option("y"), Some(-1));
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: readOption case-sensitive key matching".
    #[test]
    fn osc_72_read_option_case_sensitive() {
        let d = run("72;x=10:Y=200", 0x1b);
        assert_eq!(d.int_option("x"), Some(10));
        assert_eq!(d.int_option("X"), None);
        assert_eq!(d.int_option("Y"), Some(200));
        assert_eq!(d.int_option("y"), None);
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: readOption absent key returns null".
    #[test]
    fn osc_72_read_option_absent_key() {
        let d = run("72;t=a", 0x1b);
        assert_eq!(d.int_option("i"), None);
        assert_eq!(d.int_option("x"), None);
        assert_eq!(d.int_option("X"), None);
        assert_eq!(d.int_option("m"), None);
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: readOption malformed integer returns null".
    #[test]
    fn osc_72_read_option_malformed_integer() {
        let d = run("72;x=notanumber", 0x1b);
        assert_eq!(d.int_option("x"), None);
    }

    // Zig: kitty_dnd_protocol.zig "OSC 72: BEL terminator recorded".
    #[test]
    fn osc_72_bel_terminator_recorded() {
        let d = run("72;t=q", 0x07);
        assert_eq!(d.terminator, Terminator::Bel);
    }
}
