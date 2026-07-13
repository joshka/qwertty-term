//! Terminfo capability responses for XTGETTCAP (`DCS + q <names> ST`).
//!
//! A program can ask the terminal for a terminfo capability's value with
//! XTGETTCAP; we reply `ESC P 1 + r <hexname>=<hexvalue> ESC \` for a known
//! capability, or nothing for one we don't answer (matching upstream, which
//! simply skips unknown keys).
//!
//! This ports the commonly-queried subset of `terminfo.ghostty.xtgettcapMap()`
//! rather than the full 271-entry terminfo Source. XTGETTCAP is answered only
//! by the app/termio layer upstream (the lib core ignores DCS), so it has no
//! differential-oracle coverage and is verified by unit tests. Add a capability
//! by extending [`xtgettcap_value`] — the name is the uppercase hex encoding of
//! the terminfo cap name (e.g. `TN` → `b"544E"`).

/// Build the XTGETTCAP reply for a requested (uppercase) hex-encoded capability
/// name, or `None` if we don't answer that capability. `hex_name` is the raw
/// requested key from the DCS parser (already uppercased). The reply is
/// `ESC P 1 + r <hexname>[=<hexvalue>] ESC \`; a boolean capability has no
/// `=value`.
pub fn xtgettcap_response(hex_name: &[u8]) -> Option<Vec<u8>> {
    let value = xtgettcap_value(hex_name)?;
    let mut resp = Vec::with_capacity(6 + hex_name.len() + 1 + value.len() * 2 + 2);
    resp.extend_from_slice(b"\x1bP1+r");
    resp.extend_from_slice(hex_name);
    if !value.is_empty() {
        resp.push(b'=');
        for &b in value {
            resp.push(hex_digit(b >> 4));
            resp.push(hex_digit(b & 0x0f));
        }
    }
    resp.extend_from_slice(b"\x1b\\");
    Some(resp)
}

/// The raw terminfo value for a hex-encoded capability name, or `None` if we
/// don't answer it. A boolean capability returns `Some(b"")` (it is present but
/// carries no value). Port of the relevant `xtgettcapMap` entries.
fn xtgettcap_value(hex_name: &[u8]) -> Option<&'static [u8]> {
    Some(match hex_name {
        b"544E" => b"qwertty-term", // TN  — terminal name (product identity; never "ghostty")
        b"436F" => b"256",          // Co  — maximum number of colors
        b"524742" => b"8",          // RGB — bits per color channel (direct color)
        b"5463" => b"",             // Tc  — 24-bit "true color" support (boolean)
        b"626365" => b"",           // bce — background-color erase (boolean)
        b"616D" => b"",             // am  — automatic right margin (boolean)
        _ => return None,
    })
}

/// A single nibble (0-15) to its uppercase-hex ASCII byte.
fn hex_digit(n: u8) -> u8 {
    match n {
        0..=9 => b'0' + n,
        _ => b'A' + (n - 10),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Uppercase-hex-encode a cap name the way a client sends it.
    fn hex(name: &str) -> Vec<u8> {
        let mut out = Vec::new();
        for &b in name.as_bytes() {
            out.push(hex_digit(b >> 4));
            out.push(hex_digit(b & 0x0f));
        }
        out
    }

    #[test]
    fn string_capability_reply() {
        // TN = "qwertty-term". Reply: ESC P 1 + r <TN-hex>=<value-hex> ESC \.
        let resp = xtgettcap_response(&hex("TN")).unwrap();
        let mut expected = Vec::new();
        expected.extend_from_slice(b"\x1bP1+r544E=");
        for &b in b"qwertty-term" {
            expected.push(hex_digit(b >> 4));
            expected.push(hex_digit(b & 0x0f));
        }
        expected.extend_from_slice(b"\x1b\\");
        assert_eq!(resp, expected);
    }

    #[test]
    fn numeric_capability_reply() {
        // Co = "256".
        let resp = xtgettcap_response(&hex("Co")).unwrap();
        assert_eq!(resp, b"\x1bP1+r436F=323536\x1b\\"); // "256" -> 32 35 36
    }

    #[test]
    fn boolean_capability_reply_has_no_value() {
        // Tc is a boolean: present, no `=value`.
        let resp = xtgettcap_response(&hex("Tc")).unwrap();
        assert_eq!(resp, b"\x1bP1+r5463\x1b\\");
    }

    #[test]
    fn product_name_is_not_ghostty() {
        let resp = xtgettcap_response(&hex("TN")).unwrap();
        // The decoded value must be our product, never "ghostty".
        assert!(!resp.windows(2).any(|w| w == b"67")); // 'g' = 0x67 would start "ghostty"-hex
    }

    #[test]
    fn unknown_capability_is_none() {
        assert_eq!(xtgettcap_response(&hex("ZZ")), None);
    }
}
