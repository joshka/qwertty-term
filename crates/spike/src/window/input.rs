use eframe::egui::{Key, Modifiers, PointerButton};

pub(super) fn mouse_button_code(button: PointerButton) -> Option<u8> {
    match button {
        PointerButton::Primary => Some(0),
        PointerButton::Middle => Some(1),
        PointerButton::Secondary => Some(2),
        _ => None,
    }
}

pub(super) fn encode_key(
    key: Key,
    modifiers: Modifiers,
    application_cursor_keys: bool,
) -> Option<Vec<u8>> {
    let bytes = match key {
        Key::Enter => b"\r".to_vec(),
        Key::Backspace => vec![0x7f],
        Key::Tab => b"\t".to_vec(),
        Key::Escape => vec![0x1b],
        Key::ArrowLeft => cursor_key(b'D', application_cursor_keys),
        Key::ArrowRight => cursor_key(b'C', application_cursor_keys),
        Key::ArrowUp => cursor_key(b'A', application_cursor_keys),
        Key::ArrowDown => cursor_key(b'B', application_cursor_keys),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        _ => {
            if modifiers.ctrl {
                control_key(key)?
            } else {
                return None;
            }
        }
    };
    Some(bytes)
}

fn cursor_key(final_byte: u8, application_cursor_keys: bool) -> Vec<u8> {
    if application_cursor_keys {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn control_key(key: Key) -> Option<Vec<u8>> {
    let ch = match key {
        Key::A => b'A',
        Key::B => b'B',
        Key::C => b'C',
        Key::D => b'D',
        Key::E => b'E',
        Key::F => b'F',
        Key::G => b'G',
        Key::H => b'H',
        Key::I => b'I',
        Key::J => b'J',
        Key::K => b'K',
        Key::L => b'L',
        Key::M => b'M',
        Key::N => b'N',
        Key::O => b'O',
        Key::P => b'P',
        Key::Q => b'Q',
        Key::R => b'R',
        Key::S => b'S',
        Key::T => b'T',
        Key::U => b'U',
        Key::V => b'V',
        Key::W => b'W',
        Key::X => b'X',
        Key::Y => b'Y',
        Key::Z => b'Z',
        _ => return None,
    };
    Some(vec![ch - b'@'])
}
