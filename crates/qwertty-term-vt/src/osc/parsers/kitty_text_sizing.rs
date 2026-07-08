//! OSC 66: kitty text sizing protocol. Port of
//! `osc/parsers/kitty_text_sizing.zig`.

use crate::osc::Command;

pub const MAX_PAYLOAD_LENGTH: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VAlign {
    #[default]
    Top,
    Bottom,
    Center,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HAlign {
    #[default]
    Left,
    Right,
    Center,
}

/// OSC 66 command payload. Port of `kitty_text_sizing.zig` `OSC`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KittyTextSizing {
    pub scale: u8,
    pub width: u8,
    pub numerator: u8,
    pub denominator: u8,
    pub valign: VAlign,
    pub halign: HAlign,
    pub text: String,
}

impl Default for KittyTextSizing {
    fn default() -> Self {
        KittyTextSizing {
            scale: 1,
            width: 0,
            numerator: 0,
            denominator: 0,
            valign: VAlign::default(),
            halign: HAlign::default(),
            text: String::new(),
        }
    }
}

impl KittyTextSizing {
    fn update(&mut self, key: u8, value: &str) -> Result<(), ()> {
        let v: u8 = value.parse().map_err(|_| ())?;
        if v > 15 {
            return Err(());
        }
        match key {
            b's' => {
                if v == 0 {
                    return Err(());
                }
                if v > 7 {
                    return Err(());
                }
                self.scale = v;
            }
            b'w' => {
                if v > 7 {
                    return Err(());
                }
                self.width = v;
            }
            b'n' => self.numerator = v,
            b'd' => self.denominator = v,
            b'v' => {
                self.valign = match v {
                    0 => VAlign::Top,
                    1 => VAlign::Bottom,
                    2 => VAlign::Center,
                    _ => return Err(()),
                }
            }
            b'h' => {
                self.halign = match v {
                    0 => HAlign::Left,
                    1 => HAlign::Right,
                    2 => HAlign::Center,
                    _ => return Err(()),
                }
            }
            _ => return Err(()),
        }
        Ok(())
    }
}

/// Parse OSC 66. Port of `kitty_text_sizing.zig` `parse`. Requires
/// unbounded capture (checked by the caller).
pub fn parse(rest: &str) -> Option<Command> {
    let data = rest.strip_prefix(';').unwrap_or("");

    let payload_start = data.find(';')?;
    let payload = &data[payload_start + 1..];

    if payload.len() > MAX_PAYLOAD_LENGTH {
        return None;
    }
    if !crate::osc::parsers::is_safe_utf8(payload) {
        return None;
    }

    let mut cmd = KittyTextSizing {
        text: payload.to_string(),
        ..KittyTextSizing::default()
    };

    if payload_start > 0 {
        for kv in data[..payload_start].split(':') {
            let mut it = kv.splitn(2, '=');
            let Some(k) = it.next() else { continue };
            if k.len() != 1 {
                continue;
            }
            let Some(value) = it.next() else { continue };
            let _ = cmd.update(k.as_bytes()[0], value);
        }
    }

    Some(Command::KittyTextSizing(cmd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    fn run(body: &str) -> Option<Command> {
        let mut p = osc::Parser::with_allocator();
        for c in body.bytes() {
            p.next(c);
        }
        p.end(Some(0x1b))
    }

    // Zig: kitty_text_sizing.zig "OSC 66: empty parameters".
    #[test]
    fn osc_66_empty_parameters() {
        let cmd = run("66;;bobr").unwrap();
        let Command::KittyTextSizing(t) = cmd else {
            panic!()
        };
        assert_eq!(t.scale, 1);
        assert_eq!(t.text, "bobr");
    }

    // Zig: kitty_text_sizing.zig "OSC 66: single parameter".
    #[test]
    fn osc_66_single_parameter() {
        let cmd = run("66;s=2;kurwa").unwrap();
        let Command::KittyTextSizing(t) = cmd else {
            panic!()
        };
        assert_eq!(t.scale, 2);
        assert_eq!(t.text, "kurwa");
    }

    // Zig: kitty_text_sizing.zig "OSC 66: multiple parameters".
    #[test]
    fn osc_66_multiple_parameters() {
        let cmd = run("66;s=2:w=7:n=13:d=15:v=1:h=2;long").unwrap();
        let Command::KittyTextSizing(t) = cmd else {
            panic!()
        };
        assert_eq!(t.scale, 2);
        assert_eq!(t.width, 7);
        assert_eq!(t.numerator, 13);
        assert_eq!(t.denominator, 15);
        assert_eq!(t.valign, VAlign::Bottom);
        assert_eq!(t.halign, HAlign::Center);
        assert_eq!(t.text, "long");
    }

    // Zig: kitty_text_sizing.zig "OSC 66: scale is zero".
    #[test]
    fn osc_66_scale_is_zero() {
        let cmd = run("66;s=0;nope").unwrap();
        let Command::KittyTextSizing(t) = cmd else {
            panic!()
        };
        assert_eq!(t.scale, 1);
    }

    // Zig: kitty_text_sizing.zig "OSC 66: invalid parameters".
    #[test]
    fn osc_66_invalid_parameters() {
        let cmd = run("66;w=8:v=3:n=16;").unwrap();
        let Command::KittyTextSizing(t) = cmd else {
            panic!()
        };
        assert_eq!(t.width, 0);
        assert_eq!(t.valign, VAlign::Top);
        assert_eq!(t.numerator, 0);
    }

    // Zig: kitty_text_sizing.zig "OSC 66: UTF-8".
    #[test]
    fn osc_66_utf8() {
        let cmd = run("66;;👻魑魅魍魉ゴースッティ").unwrap();
        let Command::KittyTextSizing(t) = cmd else {
            panic!()
        };
        assert_eq!(t.text, "👻魑魅魍魉ゴースッティ");
    }

    // Zig: kitty_text_sizing.zig "OSC 66: unsafe UTF-8".
    #[test]
    fn osc_66_unsafe_utf8() {
        assert_eq!(run("66;;\n"), None);
    }

    // Zig: kitty_text_sizing.zig "OSC 66: overlong UTF-8".
    #[test]
    fn osc_66_overlong_utf8() {
        let long = "bobr".repeat(1025);
        let body = format!("66;;{long}");
        assert_eq!(run(&body), None);
    }
}
