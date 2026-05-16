use std::str;

use base64::{Engine, engine::general_purpose};

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum OscAction {
    Title(String),
    Clipboard(String),
}

pub(crate) fn parse_osc(bytes: &[u8]) -> Option<OscAction> {
    let payload = str::from_utf8(bytes).ok()?;
    let (command, value) = payload.split_once(';')?;
    match command {
        "0" | "2" => Some(OscAction::Title(value.to_string())),
        "52" => parse_clipboard(value).map(OscAction::Clipboard),
        _ => None,
    }
}

fn parse_clipboard(value: &str) -> Option<String> {
    let (_selection, payload) = value.split_once(';')?;
    if payload == "?" {
        return None;
    }

    let bytes = general_purpose::STANDARD.decode(payload).ok()?;
    String::from_utf8(bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_title_actions() {
        assert_eq!(
            parse_osc(b"0;hello"),
            Some(OscAction::Title("hello".into()))
        );
        assert_eq!(
            parse_osc(b"2;world"),
            Some(OscAction::Title("world".into()))
        );
    }

    #[test]
    fn parses_clipboard_write() {
        assert_eq!(
            parse_osc(b"52;c;aGVsbG8="),
            Some(OscAction::Clipboard("hello".into()))
        );
    }

    #[test]
    fn ignores_clipboard_readback_and_invalid_payloads() {
        assert_eq!(parse_osc(b"52;c;?"), None);
        assert_eq!(parse_osc(b"52;c;not base64"), None);
    }
}
