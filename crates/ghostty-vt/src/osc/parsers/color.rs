//! OSC 4,5,10-19,104,105,110-119: color get/set/reset operations. Port of
//! `osc/parsers/color.zig`.

use crate::osc::rgb::{Dynamic, InvalidFormat, Rgb, Special};
use crate::osc::{Command, Terminator};

/// The OSC number a color request came in on. Port of `color.zig`
/// `Operation`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Osc4,
    Osc5,
    Osc10,
    Osc11,
    Osc12,
    Osc13,
    Osc14,
    Osc15,
    Osc16,
    Osc17,
    Osc18,
    Osc19,
    Osc104,
    Osc105,
    Osc110,
    Osc111,
    Osc112,
    Osc113,
    Osc114,
    Osc115,
    Osc116,
    Osc117,
    Osc118,
    Osc119,
}

impl Op {
    pub fn from_osc_number(n: u32) -> Option<Op> {
        Some(match n {
            4 => Op::Osc4,
            5 => Op::Osc5,
            10 => Op::Osc10,
            11 => Op::Osc11,
            12 => Op::Osc12,
            13 => Op::Osc13,
            14 => Op::Osc14,
            15 => Op::Osc15,
            16 => Op::Osc16,
            17 => Op::Osc17,
            18 => Op::Osc18,
            19 => Op::Osc19,
            104 => Op::Osc104,
            105 => Op::Osc105,
            110 => Op::Osc110,
            111 => Op::Osc111,
            112 => Op::Osc112,
            113 => Op::Osc113,
            114 => Op::Osc114,
            115 => Op::Osc115,
            116 => Op::Osc116,
            117 => Op::Osc117,
            118 => Op::Osc118,
            119 => Op::Osc119,
            _ => return None,
        })
    }
}

/// A target for a color operation. Port of `color.zig` `Target`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTarget {
    Palette(u8),
    Special(Special),
    Dynamic(Dynamic),
}

/// A single color operation. Port of `color.zig` `Request`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColorRequest {
    Set { target: ColorTarget, color: Rgb },
    Query(ColorTarget),
    Reset(ColorTarget),
    ResetPalette,
    ResetSpecial,
}

/// A batch of color operations from one OSC. Port of `color.zig` `List`
/// (a `std.SegmentedList`; the segmented-list allocation optimization
/// doesn't apply in Rust, so this is a plain `Vec`).
pub type ColorList = Vec<ColorRequest>;

/// Parse OSCs 4, 5, 10-19, 104, 105, 110-119. Port of `color.zig` `parse`
/// + `parseColor`.
///
/// `rest` is the body after the OSC number prefix (e.g. for `4;0;red` this
/// is called with `";0;red"`).
pub fn parse(op: Op, rest: &str, terminator: Terminator) -> Option<Command> {
    let data = rest.strip_prefix(';').unwrap_or("");
    let requests = parse_color(op, data);
    Some(Command::ColorOperation {
        requests,
        terminator,
    })
}

fn parse_color(op: Op, buf: &str) -> ColorList {
    // Zig tokenizes on ';', skipping empty tokens (tokenizeScalar), unlike
    // splitScalar which keeps them. Match that here.
    let mut it = buf.split(';').filter(|s| !s.is_empty());
    match op {
        Op::Osc4 => parse_get_set_ansi_color(op, &mut it),
        Op::Osc5 => parse_get_set_ansi_color(op, &mut it),
        Op::Osc104 => parse_reset_ansi_color(op, buf),
        Op::Osc105 => parse_reset_ansi_color(op, buf),
        Op::Osc10 => parse_get_set_dynamic_color(Dynamic::Foreground, &mut it),
        Op::Osc11 => parse_get_set_dynamic_color(Dynamic::Background, &mut it),
        Op::Osc12 => parse_get_set_dynamic_color(Dynamic::Cursor, &mut it),
        Op::Osc13 => parse_get_set_dynamic_color(Dynamic::PointerForeground, &mut it),
        Op::Osc14 => parse_get_set_dynamic_color(Dynamic::PointerBackground, &mut it),
        Op::Osc15 => parse_get_set_dynamic_color(Dynamic::TektronixForeground, &mut it),
        Op::Osc16 => parse_get_set_dynamic_color(Dynamic::TektronixBackground, &mut it),
        Op::Osc17 => parse_get_set_dynamic_color(Dynamic::HighlightBackground, &mut it),
        Op::Osc18 => parse_get_set_dynamic_color(Dynamic::TektronixCursor, &mut it),
        Op::Osc19 => parse_get_set_dynamic_color(Dynamic::HighlightForeground, &mut it),
        Op::Osc110 => parse_reset_dynamic_color(Dynamic::Foreground, &mut it),
        Op::Osc111 => parse_reset_dynamic_color(Dynamic::Background, &mut it),
        Op::Osc112 => parse_reset_dynamic_color(Dynamic::Cursor, &mut it),
        Op::Osc113 => parse_reset_dynamic_color(Dynamic::PointerForeground, &mut it),
        Op::Osc114 => parse_reset_dynamic_color(Dynamic::PointerBackground, &mut it),
        Op::Osc115 => parse_reset_dynamic_color(Dynamic::TektronixForeground, &mut it),
        Op::Osc116 => parse_reset_dynamic_color(Dynamic::TektronixBackground, &mut it),
        Op::Osc117 => parse_reset_dynamic_color(Dynamic::HighlightBackground, &mut it),
        Op::Osc118 => parse_reset_dynamic_color(Dynamic::TektronixCursor, &mut it),
        Op::Osc119 => parse_reset_dynamic_color(Dynamic::HighlightForeground, &mut it),
    }
}

fn ansi_target(op: Op, color: u32) -> Result<ColorTarget, InvalidFormat> {
    match op {
        Op::Osc5 => {
            let idx: u8 = color.try_into().map_err(|_| InvalidFormat)?;
            Special::from_u8(idx)
                .map(ColorTarget::Special)
                .ok_or(InvalidFormat)
        }
        Op::Osc4 => {
            if let Ok(idx) = u8::try_from(color) {
                Ok(ColorTarget::Palette(idx))
            } else {
                let idx: u8 = (color - 256).try_into().map_err(|_| InvalidFormat)?;
                Special::from_u8(idx)
                    .map(ColorTarget::Special)
                    .ok_or(InvalidFormat)
            }
        }
        _ => unreachable!(),
    }
}

/// OSC 4/5: get/set ANSI colors. Port of `color.zig`
/// `parseGetSetAnsiColor`.
fn parse_get_set_ansi_color(op: Op, it: &mut dyn Iterator<Item = &str>) -> ColorList {
    let mut result = ColorList::new();
    loop {
        let Some(color_str) = it.next() else {
            return result;
        };
        let Some(spec_str) = it.next() else {
            return result;
        };

        let Ok(color) = color_str.parse::<u32>() else {
            return result;
        };
        if color > 0x1FF {
            return result;
        }
        let Ok(target) = ansi_target(op, color) else {
            return result;
        };

        if spec_str == "?" {
            result.push(ColorRequest::Query(target));
            continue;
        }

        let Ok(rgb) = Rgb::parse(spec_str) else {
            return result;
        };
        result.push(ColorRequest::Set { target, color: rgb });
    }
}

/// OSC 104/105: reset ANSI colors. Port of `color.zig`
/// `parseResetAnsiColor`.
fn parse_reset_ansi_color(op: Op, buf: &str) -> ColorList {
    let mut result = ColorList::new();
    // Zig uses splitScalar (keeps empty tokens) here, not tokenizeScalar,
    // and explicitly skips empty color strings as "not an error" rather
    // than tokenizing them away -- same observable result, ported with
    // splitScalar semantics to match the "trailing semicolon allowed"
    // xterm-compat test.
    if buf.is_empty() {
        result.push(match op {
            Op::Osc104 => ColorRequest::ResetPalette,
            Op::Osc105 => ColorRequest::ResetSpecial,
            _ => unreachable!(),
        });
        return result;
    }
    let mut any = false;
    for color_str in buf.split(';') {
        if color_str.is_empty() {
            continue;
        }
        any = true;
        let Ok(color) = color_str.parse::<u32>() else {
            continue;
        };
        if color > 0x1FF {
            continue;
        }
        let target = match op {
            Op::Osc105 => {
                let Ok(idx) = u8::try_from(color) else {
                    continue;
                };
                let Some(special) = Special::from_u8(idx) else {
                    continue;
                };
                ColorTarget::Special(special)
            }
            Op::Osc104 => {
                if let Ok(idx) = u8::try_from(color) {
                    ColorTarget::Palette(idx)
                } else {
                    let Ok(idx) = u8::try_from(color - 256) else {
                        continue;
                    };
                    let Some(special) = Special::from_u8(idx) else {
                        continue;
                    };
                    ColorTarget::Special(special)
                }
            }
            _ => unreachable!(),
        };
        result.push(ColorRequest::Reset(target));
    }
    if !any && result.is_empty() {
        result.push(match op {
            Op::Osc104 => ColorRequest::ResetPalette,
            Op::Osc105 => ColorRequest::ResetSpecial,
            _ => unreachable!(),
        });
    }
    result
}

/// OSC 10-19: get/set dynamic colors. Port of `color.zig`
/// `parseGetSetDynamicColor`.
fn parse_get_set_dynamic_color(start: Dynamic, it: &mut dyn Iterator<Item = &str>) -> ColorList {
    let mut result = ColorList::new();
    let mut color = start;
    loop {
        let Some(spec_str) = it.next() else {
            return result;
        };
        if spec_str == "?" {
            result.push(ColorRequest::Query(ColorTarget::Dynamic(color)));
        } else {
            let Ok(rgb) = Rgb::parse(spec_str) else {
                return result;
            };
            result.push(ColorRequest::Set {
                target: ColorTarget::Dynamic(color),
                color: rgb,
            });
        }
        let Some(next) = color.next() else {
            return result;
        };
        color = next;
    }
}

/// OSC 110-119: reset dynamic colors. Port of `color.zig`
/// `parseResetDynamicColor`.
fn parse_reset_dynamic_color(color: Dynamic, it: &mut dyn Iterator<Item = &str>) -> ColorList {
    let mut result = ColorList::new();
    if it.next().is_some() {
        return result;
    }
    result.push(ColorRequest::Reset(ColorTarget::Dynamic(color)));
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::osc;

    fn run(op: &str, body: &str) -> ColorList {
        let full = format!("{op};{body}");
        let mut p = osc::Parser::with_allocator();
        for c in full.bytes() {
            p.next(c);
        }
        match p.end(Some(0x1b)) {
            Some(Command::ColorOperation { requests, .. }) => requests,
            other => panic!("expected ColorOperation, got {other:?}"),
        }
    }

    // Zig: color.zig "OSC 4: empty param". The Zig test uses
    // `Parser.init(null)` (no allocator): OSC 4's trie state requires an
    // allocator the moment it sees the body's leading `;`
    // (`ensureAllocator`, osc.zig:449-454) -- with none, the whole parse
    // is invalidated regardless of what follows. This is really a "no
    // allocator" test, not an empty-param test; ported as such here.
    #[test]
    fn osc_4_empty_param_without_allocator() {
        let mut p = osc::Parser::new();
        for c in "4;;".bytes() {
            p.next(c);
        }
        assert_eq!(p.end(Some(0x1b)), None);
    }

    // Zig: color.zig "OSC 4:" (full sweep over every palette index +
    // every special color; ported as a representative sweep, matching
    // Zig's own loop-based test rather than expanding to 256+5 literal
    // cases).
    #[test]
    fn osc_4_full_sweep() {
        for idx in 0u32..255 {
            // Zig uses "red" (X11 name); this port doesn't have X11 names
            // (docs/analysis/osc.md divergence #1), so "#ff0000" is used
            // as the literal equivalent.
            let list = run("4", &format!("{idx};#ff0000"));
            assert_eq!(
                list,
                vec![ColorRequest::Set {
                    target: ColorTarget::Palette(idx as u8),
                    color: Rgb::new(255, 0, 0),
                }]
            );

            let list = run("4", &format!("{idx};?"));
            assert_eq!(
                list,
                vec![ColorRequest::Query(ColorTarget::Palette(idx as u8))]
            );

            // Trailing invalid data produces results up to that point.
            let list = run("4", &format!("{idx};#ff0000;"));
            assert_eq!(
                list,
                vec![ColorRequest::Set {
                    target: ColorTarget::Palette(idx as u8),
                    color: Rgb::new(255, 0, 0),
                }]
            );
        }

        for i in 0..Special::COUNT as u32 {
            let special = Special::from_u8(i as u8).unwrap();
            let list = run("4", &format!("{};#ff0000", 256 + i));
            assert_eq!(
                list,
                vec![ColorRequest::Set {
                    target: ColorTarget::Special(special),
                    color: Rgb::new(255, 0, 0),
                }]
            );
        }
    }

    // Zig: color.zig "OSC 5:" (full sweep over every special color).
    #[test]
    fn osc_5_full_sweep() {
        for i in 0..Special::COUNT as u32 {
            let special = Special::from_u8(i as u8).unwrap();
            let list = run("5", &format!("{i};#ff0000"));
            assert_eq!(
                list,
                vec![ColorRequest::Set {
                    target: ColorTarget::Special(special),
                    color: Rgb::new(255, 0, 0),
                }]
            );
        }
    }

    // Zig: color.zig "OSC 4: multiple requests".
    #[test]
    fn osc_4_multiple_requests() {
        let list = run("4", "0;#ff0000;1;#0000ff");
        assert_eq!(
            list,
            vec![
                ColorRequest::Set {
                    target: ColorTarget::Palette(0),
                    color: Rgb::new(255, 0, 0)
                },
                ColorRequest::Set {
                    target: ColorTarget::Palette(1),
                    color: Rgb::new(0, 0, 255)
                },
            ]
        );

        // Multiple requests with same index overwrite each other (i.e.
        // downstream applies in order; the list itself just has both).
        let list = run("4", "0;#ff0000;0;#0000ff");
        assert_eq!(
            list,
            vec![
                ColorRequest::Set {
                    target: ColorTarget::Palette(0),
                    color: Rgb::new(255, 0, 0)
                },
                ColorRequest::Set {
                    target: ColorTarget::Palette(0),
                    color: Rgb::new(0, 0, 255)
                },
            ]
        );
    }

    // Zig: color.zig "OSC 104:" (full sweep).
    #[test]
    fn osc_104_full_sweep() {
        for idx in 0u32..255 {
            let list = run("104", &idx.to_string());
            assert_eq!(
                list,
                vec![ColorRequest::Reset(ColorTarget::Palette(idx as u8))]
            );
        }
        for i in 0..Special::COUNT as u32 {
            let special = Special::from_u8(i as u8).unwrap();
            let list = run("104", &(256 + i).to_string());
            assert_eq!(
                list,
                vec![ColorRequest::Reset(ColorTarget::Special(special))]
            );
        }
    }

    // Zig: color.zig "OSC 104: empty index".
    #[test]
    fn osc_104_empty_index() {
        let list = run("104", "0;;1");
        assert_eq!(
            list,
            vec![
                ColorRequest::Reset(ColorTarget::Palette(0)),
                ColorRequest::Reset(ColorTarget::Palette(1)),
            ]
        );
    }

    // Zig: color.zig "OSC 104: invalid index".
    #[test]
    fn osc_104_invalid_index() {
        let list = run("104", "ffff;1");
        assert_eq!(list, vec![ColorRequest::Reset(ColorTarget::Palette(1))]);
    }

    // Zig: color.zig "OSC 104: reset all".
    #[test]
    fn osc_104_reset_all() {
        let list = run("104", "");
        assert_eq!(list, vec![ColorRequest::ResetPalette]);
    }

    // Zig: color.zig "OSC 105: reset all".
    #[test]
    fn osc_105_reset_all() {
        let list = run("105", "");
        assert_eq!(list, vec![ColorRequest::ResetSpecial]);
    }

    // Zig: color.zig "OSC 10: OSC 11: ... dynamic" (full sweep over
    // DynamicColor).
    #[test]
    fn osc_1x_dynamic_sweep() {
        let colors_and_ops = [
            (Dynamic::Foreground, "10"),
            (Dynamic::Background, "11"),
            (Dynamic::Cursor, "12"),
            (Dynamic::PointerForeground, "13"),
            (Dynamic::PointerBackground, "14"),
            (Dynamic::TektronixForeground, "15"),
            (Dynamic::TektronixBackground, "16"),
            (Dynamic::HighlightBackground, "17"),
            (Dynamic::TektronixCursor, "18"),
            (Dynamic::HighlightForeground, "19"),
        ];
        for (color, op) in colors_and_ops {
            let list = run(op, "#ff0000");
            assert_eq!(
                list,
                vec![ColorRequest::Set {
                    target: ColorTarget::Dynamic(color),
                    color: Rgb::new(255, 0, 0),
                }]
            );
        }
    }

    // Zig: color.zig "OSC 10: ... dynamic multiple".
    #[test]
    fn osc_11_dynamic_multiple() {
        let list = run("11", "#ff0000;#0000ff");
        assert_eq!(
            list,
            vec![
                ColorRequest::Set {
                    target: ColorTarget::Dynamic(Dynamic::Background),
                    color: Rgb::new(255, 0, 0)
                },
                ColorRequest::Set {
                    target: ColorTarget::Dynamic(Dynamic::Cursor),
                    color: Rgb::new(0, 0, 255)
                },
            ]
        );
    }

    // Zig: color.zig "OSC 110: ... reset dynamic".
    #[test]
    fn osc_11x_reset_dynamic_sweep() {
        let colors_and_ops = [
            (Dynamic::Foreground, "110"),
            (Dynamic::Background, "111"),
            (Dynamic::Cursor, "112"),
            (Dynamic::PointerForeground, "113"),
            (Dynamic::PointerBackground, "114"),
            (Dynamic::TektronixForeground, "115"),
            (Dynamic::TektronixBackground, "116"),
            (Dynamic::HighlightBackground, "117"),
            (Dynamic::TektronixCursor, "118"),
            (Dynamic::HighlightForeground, "119"),
        ];
        for (color, op) in colors_and_ops {
            let list = run(op, "");
            assert_eq!(list, vec![ColorRequest::Reset(ColorTarget::Dynamic(color))]);

            // xterm allows a trailing semicolon.
            let list = run(op, ";");
            assert_eq!(list, vec![ColorRequest::Reset(ColorTarget::Dynamic(color))]);

            // xterm does NOT allow any whitespace.
            let list = run(op, " ");
            assert_eq!(list, vec![]);
        }
    }
}
