//! Nerd Fonts glyph constraint table (GENERATED — DO NOT EDIT BY HAND).
//!
//! Mechanically translated from upstream `src/font/nerd_font_attributes.zig`
//! (itself codegen'd by `nerd_font_codegen.py` from the Nerd Fonts font-patcher)
//! by `cargo run -p xtask -- gen-nerd-constraints`. Source: `/tmp/ghostty-2da015/src/font/nerd_font_attributes.zig`.
//!
//! Every arm's codepoint ranges and constraint fields are copied verbatim from
//! the upstream switch, so [`get_constraint`] is semantically identical to
//! upstream's `getConstraint`. Regenerate rather than hand-editing.

// Float literals are copied verbatim from upstream (which carries full
// decimal expansions like 0.7142857142857143); the extra digits beyond f64
// precision are intentional for byte-for-byte parity, not a mistake.
#![allow(clippy::excessive_precision)]

use crate::constraint::{Align, Constraint, Height, Size};

/// The constraint for `cp` per the Nerd Fonts attribute table, or `None`
/// (upstream `nerd_font_attributes.getConstraint`).
pub fn get_constraint(cp: u32) -> Option<Constraint> {
    Some(match cp {
        0x2630 => Constraint {
            size: Size::Cover,
            height: Height::Icon,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            pad_left: 0.05,
            pad_right: 0.05,
            pad_top: 0.05,
            pad_bottom: 0.05,
            ..Constraint::NONE
        },
        0x276C..=0x276D => Constraint {
            size: Size::Cover,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7142857142857143,
            relative_height: 0.8910614525139665,
            relative_x: 0.1428571428571428,
            relative_y: 0.0349162011173184,
            pad_top: 0.15,
            pad_bottom: 0.15,
            ..Constraint::NONE
        },
        0x276E..=0x276F => Constraint {
            size: Size::Cover,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9885714285714285,
            relative_height: 0.8910614525139665,
            relative_x: 0.0057142857142857,
            relative_y: 0.0125698324022346,
            pad_top: 0.15,
            pad_bottom: 0.15,
            ..Constraint::NONE
        },
        0x2770..=0x2771 => Constraint {
            size: Size::Cover,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            pad_top: 0.15,
            pad_bottom: 0.15,
            ..Constraint::NONE
        },
        0xE0A0..=0xE0A3 | 0xE0CF => Constraint {
            size: Size::FitCover1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            ..Constraint::NONE
        },
        0xE0B0 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            pad_left: -0.03,
            pad_right: -0.03,
            pad_top: -0.005,
            pad_bottom: -0.005,
            max_xy_ratio: Some(0.7),
            ..Constraint::NONE
        },
        0xE0B1 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            max_xy_ratio: Some(0.7),
            ..Constraint::NONE
        },
        0xE0B2 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            pad_left: -0.03,
            pad_right: -0.03,
            pad_top: -0.005,
            pad_bottom: -0.005,
            max_xy_ratio: Some(0.7),
            ..Constraint::NONE
        },
        0xE0B3 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            max_xy_ratio: Some(0.7),
            ..Constraint::NONE
        },
        0xE0B4 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            pad_left: -0.03,
            pad_right: -0.03,
            pad_top: -0.005,
            pad_bottom: -0.005,
            max_xy_ratio: Some(0.59),
            ..Constraint::NONE
        },
        0xE0B5 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            max_xy_ratio: Some(0.5),
            ..Constraint::NONE
        },
        0xE0B6 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            pad_left: -0.03,
            pad_right: -0.03,
            pad_top: -0.005,
            pad_bottom: -0.005,
            max_xy_ratio: Some(0.59),
            ..Constraint::NONE
        },
        0xE0B7 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            max_xy_ratio: Some(0.5),
            ..Constraint::NONE
        },
        0xE0B8 | 0xE0BC => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            pad_left: -0.025,
            pad_right: -0.025,
            pad_top: -0.005,
            pad_bottom: -0.005,
            ..Constraint::NONE
        },
        0xE0B9 | 0xE0BD => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            ..Constraint::NONE
        },
        0xE0BA | 0xE0BE => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            pad_left: -0.025,
            pad_right: -0.025,
            pad_top: -0.005,
            pad_bottom: -0.005,
            ..Constraint::NONE
        },
        0xE0BB | 0xE0BF => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            ..Constraint::NONE
        },
        0xE0C0 | 0xE0C8 => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            pad_left: -0.025,
            pad_right: -0.025,
            pad_top: -0.005,
            pad_bottom: -0.005,
            ..Constraint::NONE
        },
        0xE0C1 => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            ..Constraint::NONE
        },
        0xE0C2 | 0xE0CA => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            pad_left: -0.025,
            pad_right: -0.025,
            pad_top: -0.005,
            pad_bottom: -0.005,
            ..Constraint::NONE
        },
        0xE0C3 => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            ..Constraint::NONE
        },
        0xE0C4 => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            max_xy_ratio: Some(0.86),
            ..Constraint::NONE
        },
        0xE0C5 => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            max_xy_ratio: Some(0.86),
            ..Constraint::NONE
        },
        0xE0C6 => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            max_xy_ratio: Some(0.78),
            ..Constraint::NONE
        },
        0xE0C7 => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            max_xy_ratio: Some(0.78),
            ..Constraint::NONE
        },
        0xE0CC => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            pad_left: -0.01,
            pad_right: -0.01,
            pad_top: -0.005,
            pad_bottom: -0.005,
            max_xy_ratio: Some(0.85),
            ..Constraint::NONE
        },
        0xE0CD => Constraint {
            size: Size::Stretch,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            max_xy_ratio: Some(0.865),
            ..Constraint::NONE
        },
        0xE0CE | 0xE0D0..=0xE0D1 => Constraint {
            size: Size::FitCover1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            ..Constraint::NONE
        },
        0xE0D2 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            pad_left: -0.01,
            pad_right: -0.01,
            pad_top: -0.005,
            pad_bottom: -0.005,
            max_xy_ratio: Some(0.7),
            ..Constraint::NONE
        },
        0xE0D4 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            pad_left: -0.01,
            pad_right: -0.01,
            pad_top: -0.005,
            pad_bottom: -0.005,
            max_xy_ratio: Some(0.7),
            ..Constraint::NONE
        },
        0xE0D6 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            pad_left: -0.025,
            pad_right: -0.025,
            pad_top: -0.005,
            pad_bottom: -0.005,
            max_xy_ratio: Some(0.7),
            ..Constraint::NONE
        },
        0xE0D7 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            pad_left: -0.025,
            pad_right: -0.025,
            pad_top: -0.005,
            pad_bottom: -0.005,
            max_xy_ratio: Some(0.7),
            ..Constraint::NONE
        },
        0xE300 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8984375000000000,
            relative_y: 0.0986328125000000,
            ..Constraint::NONE
        },
        0xE301 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8798828125000000,
            relative_y: 0.1171875000000000,
            ..Constraint::NONE
        },
        0xE302 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7646484375000000,
            relative_y: 0.2314453125000000,
            ..Constraint::NONE
        },
        0xE303 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8789062500000000,
            relative_y: 0.1171875000000000,
            ..Constraint::NONE
        },
        0xE304 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9755859375000000,
            relative_y: 0.0244140625000000,
            ..Constraint::NONE
        },
        0xE305 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9960937500000000,
            relative_y: 0.0019531250000000,
            ..Constraint::NONE
        },
        0xE306 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9863281250000000,
            relative_y: 0.0097656250000000,
            ..Constraint::NONE
        },
        0xE307 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9951171875000000,
            relative_y: 0.0039062500000000,
            ..Constraint::NONE
        },
        0xE308 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9785156250000000,
            relative_y: 0.0195312500000000,
            ..Constraint::NONE
        },
        0xE309 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9736328125000000,
            relative_y: 0.0214843750000000,
            ..Constraint::NONE
        },
        0xE30A | 0xE35F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9648437500000000,
            relative_y: 0.0302734375000000,
            ..Constraint::NONE
        },
        0xE30B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8437500000000000,
            relative_y: 0.1513671875000000,
            ..Constraint::NONE
        },
        0xE30C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8027343750000000,
            relative_y: 0.1835937500000000,
            ..Constraint::NONE
        },
        0xE30D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7753906250000000,
            relative_y: 0.1083984375000000,
            ..Constraint::NONE
        },
        0xE30E | 0xE365 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9833984375000000,
            relative_y: 0.0166015625000000,
            ..Constraint::NONE
        },
        0xE30F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9716796875000000,
            relative_y: 0.0263671875000000,
            ..Constraint::NONE
        },
        0xE310 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6621093750000000,
            relative_y: 0.0986328125000000,
            ..Constraint::NONE
        },
        0xE311 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6425781250000000,
            relative_y: 0.1171875000000000,
            ..Constraint::NONE
        },
        0xE312 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5322265625000000,
            relative_y: 0.2314453125000000,
            ..Constraint::NONE
        },
        0xE313 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6416015625000000,
            relative_y: 0.1181640625000000,
            ..Constraint::NONE
        },
        0xE314 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7382812500000000,
            relative_y: 0.0195312500000000,
            ..Constraint::NONE
        },
        0xE315 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6787109375000000,
            relative_y: 0.1357421875000000,
            ..Constraint::NONE
        },
        0xE316 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7480468750000000,
            relative_y: 0.0097656250000000,
            ..Constraint::NONE
        },
        0xE317 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7529296875000000,
            relative_y: 0.0048828125000000,
            ..Constraint::NONE
        },
        0xE318 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7314453125000000,
            relative_y: 0.0263671875000000,
            ..Constraint::NONE
        },
        0xE319 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7402343750000000,
            relative_y: 0.0195312500000000,
            ..Constraint::NONE
        },
        0xE31A | 0xE35E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7294921875000000,
            relative_y: 0.0283203125000000,
            ..Constraint::NONE
        },
        0xE31B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6074218750000000,
            relative_y: 0.1503906250000000,
            ..Constraint::NONE
        },
        0xE31C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7363281250000000,
            relative_y: 0.0224609375000000,
            ..Constraint::NONE
        },
        0xE31D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7460937500000000,
            relative_y: 0.0126953125000000,
            ..Constraint::NONE
        },
        0xE31E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.2675781250000000,
            relative_y: 0.3310546875000000,
            ..Constraint::NONE
        },
        0xE31F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7363281250000000,
            relative_y: 0.0986328125000000,
            ..Constraint::NONE
        },
        0xE320 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7177734375000000,
            relative_y: 0.1171875000000000,
            ..Constraint::NONE
        },
        0xE321 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8085937500000000,
            relative_y: 0.0253906250000000,
            ..Constraint::NONE
        },
        0xE322 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7509765625000000,
            relative_y: 0.0839843750000000,
            ..Constraint::NONE
        },
        0xE323 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8281250000000000,
            relative_y: 0.0097656250000000,
            ..Constraint::NONE
        },
        0xE324 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8349609375000000,
            ..Constraint::NONE
        },
        0xE325 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8154296875000000,
            relative_y: 0.0214843750000000,
            ..Constraint::NONE
        },
        0xE326 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8144531250000000,
            relative_y: 0.0195312500000000,
            ..Constraint::NONE
        },
        0xE327 | 0xE361 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8076171875000000,
            relative_y: 0.0273437500000000,
            ..Constraint::NONE
        },
        0xE328 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6845703125000000,
            relative_y: 0.1503906250000000,
            ..Constraint::NONE
        },
        0xE329 | 0xE367 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8173828125000000,
            relative_y: 0.0175781250000000,
            ..Constraint::NONE
        },
        0xE32A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8105468750000000,
            relative_y: 0.0263671875000000,
            ..Constraint::NONE
        },
        0xE32B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5175781250000000,
            relative_y: 0.2421875000000000,
            ..Constraint::NONE
        },
        0xE32C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6992187500000000,
            relative_y: 0.1005859375000000,
            ..Constraint::NONE
        },
        0xE32D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6787109375000000,
            relative_y: 0.1201171875000000,
            ..Constraint::NONE
        },
        0xE32E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5654296875000000,
            relative_y: 0.2324218750000000,
            ..Constraint::NONE
        },
        0xE32F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7714843750000000,
            relative_y: 0.0273437500000000,
            ..Constraint::NONE
        },
        0xE330 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7148437500000000,
            relative_y: 0.0830078125000000,
            ..Constraint::NONE
        },
        0xE331 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7919921875000000,
            relative_y: 0.0097656250000000,
            ..Constraint::NONE
        },
        0xE332 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7871093750000000,
            relative_y: 0.0126953125000000,
            ..Constraint::NONE
        },
        0xE333 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7714843750000000,
            relative_y: 0.0263671875000000,
            ..Constraint::NONE
        },
        0xE334 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7773437500000000,
            relative_y: 0.0195312500000000,
            ..Constraint::NONE
        },
        0xE335 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7714843750000000,
            relative_y: 0.0283203125000000,
            ..Constraint::NONE
        },
        0xE336 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6503906250000000,
            relative_y: 0.1503906250000000,
            ..Constraint::NONE
        },
        0xE337 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7753906250000000,
            relative_y: 0.0234375000000000,
            ..Constraint::NONE
        },
        0xE338 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7792968750000000,
            relative_y: 0.0185546875000000,
            ..Constraint::NONE
        },
        0xE339 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8445945945945946,
            ..Constraint::NONE
        },
        0xE33A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5283203125000000,
            relative_y: 0.2324218750000000,
            ..Constraint::NONE
        },
        0xE33B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5449218750000000,
            relative_y: 0.2148437500000000,
            ..Constraint::NONE
        },
        0xE33C..=0xE33D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5273437500000000,
            relative_y: 0.2324218750000000,
            ..Constraint::NONE
        },
        0xE33E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.3293918918918919,
            relative_y: 0.6706081081081081,
            ..Constraint::NONE
        },
        0xE33F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5200000000000000,
            relative_y: 0.2707692307692308,
            ..Constraint::NONE
        },
        0xE340 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8307692307692308,
            relative_y: 0.0861538461538462,
            ..Constraint::NONE
        },
        0xE341 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8327702702702703,
            relative_y: 0.0050675675675676,
            ..Constraint::NONE
        },
        0xE344 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5307692307692308,
            relative_y: 0.2092307692307692,
            ..Constraint::NONE
        },
        0xE345 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5332112630208333,
            relative_y: 0.2040934244791667,
            ..Constraint::NONE
        },
        0xE347 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8307692307692308,
            relative_y: 0.1246153846153846,
            ..Constraint::NONE
        },
        0xE349 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5307967032967034,
            relative_y: 0.2615384615384616,
            ..Constraint::NONE
        },
        0xE34C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8659995118379302,
            relative_y: 0.1340004881620698,
            ..Constraint::NONE
        },
        0xE34D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9890163534293386,
            relative_y: 0.0002440810349036,
            ..Constraint::NONE
        },
        0xE34F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5751953125000000,
            relative_y: 0.1142578125000000,
            ..Constraint::NONE
        },
        0xE351 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6533203125000000,
            relative_y: 0.1328125000000000,
            ..Constraint::NONE
        },
        0xE352 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5215384615384615,
            relative_y: 0.2846153846153846,
            ..Constraint::NONE
        },
        0xE353 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8308012820512821,
            relative_y: 0.1230448717948718,
            ..Constraint::NONE
        },
        0xE354..=0xE356 | 0xE358..=0xE359 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9935233160621761,
            relative_y: 0.0025906735751295,
            ..Constraint::NONE
        },
        0xE357 | 0xE3A9 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9961139896373057,
            ..Constraint::NONE
        },
        0xE35A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9935233160621761,
            relative_y: 0.0012953367875648,
            ..Constraint::NONE
        },
        0xE35B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9987046632124352,
            relative_y: 0.0012953367875648,
            ..Constraint::NONE
        },
        0xE360 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7695312500000000,
            relative_y: 0.0302734375000000,
            ..Constraint::NONE
        },
        0xE362 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9902343750000000,
            relative_y: 0.0097656250000000,
            ..Constraint::NONE
        },
        0xE363 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7900390625000000,
            relative_y: 0.0097656250000000,
            ..Constraint::NONE
        },
        0xE364 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8251953125000000,
            relative_y: 0.0097656250000000,
            ..Constraint::NONE
        },
        0xE366 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7832031250000000,
            relative_y: 0.0166015625000000,
            ..Constraint::NONE
        },
        0xE369 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.4902343750000000,
            relative_y: 0.2548828125000000,
            ..Constraint::NONE
        },
        0xE36B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9333658774713205,
            relative_y: 0.0266048328044911,
            ..Constraint::NONE
        },
        0xE36C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7076171875000000,
            relative_y: 0.1083984375000000,
            ..Constraint::NONE
        },
        0xE36D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8427734375000000,
            relative_y: 0.0625000000000000,
            ..Constraint::NONE
        },
        0xE36E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7529721467391304,
            relative_y: 0.0956606657608696,
            ..Constraint::NONE
        },
        0xE36F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6835937500000000,
            relative_y: 0.1250000000000000,
            ..Constraint::NONE
        },
        0xE370 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8642578125000000,
            relative_y: 0.0625000000000000,
            ..Constraint::NONE
        },
        0xE371 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6103515625000000,
            relative_y: 0.1933593750000000,
            ..Constraint::NONE
        },
        0xE372 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7949218750000000,
            relative_y: 0.0576171875000000,
            ..Constraint::NONE
        },
        0xE373 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8652343750000000,
            relative_y: 0.0058593750000000,
            ..Constraint::NONE
        },
        0xE374 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.3154296875000000,
            relative_y: 0.2861328125000000,
            ..Constraint::NONE
        },
        0xE375 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6772460937500000,
            relative_y: 0.1303710937500000,
            ..Constraint::NONE
        },
        0xE376 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6992187500000000,
            relative_y: 0.1337890625000000,
            ..Constraint::NONE
        },
        0xE377 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7314453125000000,
            relative_y: 0.1552734375000000,
            ..Constraint::NONE
        },
        0xE378 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7314453125000000,
            relative_y: 0.1542968750000000,
            ..Constraint::NONE
        },
        0xE379 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5751953125000000,
            relative_y: 0.1826171875000000,
            ..Constraint::NONE
        },
        0xE37A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5263671875000000,
            relative_y: 0.2285156250000000,
            ..Constraint::NONE
        },
        0xE37B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5751953125000000,
            relative_y: 0.1835937500000000,
            ..Constraint::NONE
        },
        0xE37D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9003906250000000,
            relative_y: 0.0957031250000000,
            ..Constraint::NONE
        },
        0xE37E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6015625000000000,
            relative_y: 0.2324218750000000,
            ..Constraint::NONE
        },
        0xE37F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5200000000000000,
            relative_y: 0.2784615384615385,
            ..Constraint::NONE
        },
        0xE380 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5200000000000000,
            relative_y: 0.2630769230769231,
            ..Constraint::NONE
        },
        0xE38E..=0xE391 | 0xE394 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.4990253411306043,
            relative_height: 0.9987012987012988,
            relative_x: 0.4996751137102014,
            ..Constraint::NONE
        },
        0xE392..=0xE393 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.4996751137102014,
            relative_height: 0.9987012987012988,
            relative_x: 0.4990253411306043,
            ..Constraint::NONE
        },
        0xE395 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5471085120207927,
            relative_height: 0.9987012987012988,
            relative_x: 0.4515919428200130,
            ..Constraint::NONE
        },
        0xE396 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5945419103313840,
            relative_height: 0.9987012987012988,
            relative_x: 0.4041585445094217,
            ..Constraint::NONE
        },
        0xE397 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6426250812215725,
            relative_x: 0.3573749187784275,
            ..Constraint::NONE
        },
        0xE398 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6900584795321637,
            relative_x: 0.3099415204678362,
            ..Constraint::NONE
        },
        0xE399 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7381416504223521,
            relative_x: 0.2618583495776478,
            ..Constraint::NONE
        },
        0xE39A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7855750487329435,
            relative_x: 0.2144249512670565,
            ..Constraint::NONE
        },
        0xE39B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9987004548408057,
            relative_height: 0.9987012987012988,
            ..Constraint::NONE
        },
        0xE39C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8323586744639376,
            relative_height: 0.9935064935064936,
            ..Constraint::NONE
        },
        0xE39D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7855750487329435,
            relative_height: 0.9948051948051948,
            ..Constraint::NONE
        },
        0xE39E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7381416504223521,
            relative_height: 0.9961038961038962,
            ..Constraint::NONE
        },
        0xE39F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6907082521117609,
            relative_height: 0.9961038961038962,
            ..Constraint::NONE
        },
        0xE3A0 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6426250812215725,
            relative_height: 0.9961038961038962,
            ..Constraint::NONE
        },
        0xE3A1 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5945419103313840,
            relative_height: 0.9974025974025974,
            ..Constraint::NONE
        },
        0xE3A2..=0xE3A3 | 0xE3A5 | 0xE3A7..=0xE3A8 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.4990253411306043,
            relative_height: 0.9987012987012988,
            ..Constraint::NONE
        },
        0xE3A4 | 0xE3A6 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.4996751137102014,
            relative_height: 0.9987012987012988,
            ..Constraint::NONE
        },
        0xE3AA => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9902343750000000,
            relative_y: 0.0078125000000000,
            ..Constraint::NONE
        },
        0xE3AB => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7900390625000000,
            relative_y: 0.0058593750000000,
            ..Constraint::NONE
        },
        0xE3AC => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8251953125000000,
            relative_y: 0.0078125000000000,
            ..Constraint::NONE
        },
        0xE3AD => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7519531250000000,
            relative_y: 0.0068359375000000,
            ..Constraint::NONE
        },
        0xE3AE => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6152343750000000,
            relative_y: 0.2324218750000000,
            ..Constraint::NONE
        },
        0xE3AF | 0xE3B3 | 0xE3B5..=0xE3BB => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9986072423398329,
            relative_y: 0.0013927576601671,
            ..Constraint::NONE
        },
        0xE3B0..=0xE3B2 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9958217270194986,
            relative_y: 0.0041782729805014,
            ..Constraint::NONE
        },
        0xE3C1 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6590187942396876,
            relative_y: 0.1349768123016842,
            ..Constraint::NONE
        },
        0xE3C2 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7939956065413717,
            ..Constraint::NONE
        },
        0x23FB..=0x23FE
        | 0x2665
        | 0x26A1
        | 0x2B58
        | 0xE000..=0xE00A
        | 0xE200..=0xE2A9
        | 0xE342..=0xE343
        | 0xE346
        | 0xE348
        | 0xE34A..=0xE34B
        | 0xE34E
        | 0xE350
        | 0xE35C..=0xE35D
        | 0xE368
        | 0xE36A
        | 0xE37C
        | 0xE381..=0xE38D
        | 0xE3B4
        | 0xE3BC..=0xE3C0
        | 0xE3C3..=0xE3E3
        | 0xE5FA..=0xE6B8
        | 0xE700..=0xE8EF
        | 0xEA60
        | 0xEA62..=0xEA7C
        | 0xEA7E..=0xEA88
        | 0xEA8A..=0xEA8C
        | 0xEA8F..=0xEA98
        | 0xEAA3..=0xEAB3
        | 0xEAB8..=0xEAC7
        | 0xEAC9
        | 0xEACC..=0xEAD3
        | 0xEAD7..=0xEB09
        | 0xEB0B..=0xEB42
        | 0xEB44..=0xEB4E
        | 0xEB50..=0xEB6D
        | 0xEB72..=0xEB89
        | 0xEB8B..=0xEB99
        | 0xEB9B..=0xEBD4
        | 0xEBD7..=0xEC06
        | 0xEC08..=0xEC0A
        | 0xEC0D..=0xEC1E
        | 0xED00..=0xEDFF
        | 0xEE0C..=0xEFCE
        | 0xF000..=0xF004
        | 0xF006..=0xF025
        | 0xF028..=0xF02A
        | 0xF02C..=0xF030
        | 0xF034
        | 0xF036..=0xF043
        | 0xF045
        | 0xF047
        | 0xF053..=0xF05F
        | 0xF062
        | 0xF064..=0xF076
        | 0xF079..=0xF07D
        | 0xF07F..=0xF088
        | 0xF08A..=0xF0A3
        | 0xF0A6..=0xF0D6
        | 0xF0DB
        | 0xF0DF..=0xF0FF
        | 0xF108..=0xF12F
        | 0xF131..=0xF140
        | 0xF142..=0xF152
        | 0xF155
        | 0xF15A..=0xF174
        | 0xF176
        | 0xF179..=0xF181
        | 0xF183..=0xF220
        | 0xF223
        | 0xF22E..=0xF254
        | 0xF259
        | 0xF25C..=0xF381
        | 0xF400..=0xF415
        | 0xF417..=0xF423
        | 0xF425..=0xF430
        | 0xF435..=0xF437
        | 0xF439..=0xF43D
        | 0xF43F..=0xF442
        | 0xF446..=0xF449
        | 0xF44C..=0xF45B
        | 0xF45D..=0xF45F
        | 0xF462..=0xF466
        | 0xF468..=0xF46B
        | 0xF46D..=0xF46F
        | 0xF471..=0xF475
        | 0xF477..=0xF479
        | 0xF47F..=0xF48A
        | 0xF48C..=0xF492
        | 0xF494..=0xF499
        | 0xF49B..=0xF4C2
        | 0xF4C4..=0xF4EE
        | 0xF4F3..=0xF51C
        | 0xF51E..=0xF533
        | 0xF0001..=0xF1AF0 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            ..Constraint::NONE
        },
        0xEA61 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7513020833333334,
            relative_height: 0.9291573452647278,
            relative_x: 0.0846354166666667,
            relative_y: 0.0708426547352722,
            ..Constraint::NONE
        },
        0xEA7D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8394854586129754,
            relative_height: 0.8751387347391787,
            relative_x: 0.0917225950782998,
            relative_y: 0.0416204217536071,
            ..Constraint::NONE
        },
        0xEA99 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9395973154362416,
            relative_height: 0.4778024417314096,
            relative_x: 0.0302013422818792,
            relative_y: 0.2269700332963374,
            ..Constraint::NONE
        },
        0xEA9A | 0xEAA1 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7673378076062640,
            relative_height: 0.8523862375138734,
            relative_x: 0.1526845637583893,
            relative_y: 0.0754716981132075,
            ..Constraint::NONE
        },
        0xEA9B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8590604026845637,
            relative_height: 0.7613762486126526,
            relative_x: 0.0721476510067114,
            relative_y: 0.0871254162042175,
            ..Constraint::NONE
        },
        0xEA9C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8590604026845637,
            relative_height: 0.7574916759156493,
            relative_x: 0.0721476510067114,
            relative_y: 0.0832408435072142,
            ..Constraint::NONE
        },
        0xEA9D | 0xEAA0 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.4082774049217002,
            relative_height: 0.5077691453940066,
            relative_x: 0.2863534675615212,
            relative_y: 0.2763596004439512,
            ..Constraint::NONE
        },
        0xEA9E..=0xEA9F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5117449664429530,
            relative_height: 0.4051054384017758,
            relative_x: 0.2136465324384788,
            relative_y: 0.3068812430632630,
            ..Constraint::NONE
        },
        0xEAA2 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8061116965226555,
            relative_height: 0.9438247156716689,
            relative_x: 0.0679662802950474,
            relative_y: 0.0147523709167545,
            ..Constraint::NONE
        },
        0xEAB4 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9945482866043613,
            relative_height: 0.5264797507788161,
            relative_y: 0.2024922118380062,
            ..Constraint::NONE
        },
        0xEAB5 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5264797507788161,
            relative_height: 0.9945482866043613,
            relative_x: 0.2024922118380062,
            relative_y: 0.0054517133956386,
            ..Constraint::NONE
        },
        0xEAB6 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5264797507788161,
            relative_height: 0.9945482866043613,
            relative_x: 0.2710280373831775,
            ..Constraint::NONE
        },
        0xEAB7 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9945482866043613,
            relative_height: 0.5264797507788161,
            relative_x: 0.0054517133956386,
            relative_y: 0.2710280373831775,
            ..Constraint::NONE
        },
        0xEAD4..=0xEAD5 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7069825436408977,
            relative_x: 0.1483790523690773,
            ..Constraint::NONE
        },
        0xEAD6 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8780760626398211,
            relative_y: 0.0687919463087248,
            ..Constraint::NONE
        },
        0xEB43 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7335766423357665,
            relative_height: 0.9996188152778837,
            relative_x: 0.1991657977059437,
            relative_y: 0.0003811847221163,
            ..Constraint::NONE
        },
        0xEB6E | 0xEB71 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.4954604409857328,
            relative_y: 0.2522697795071336,
            ..Constraint::NONE
        },
        0xEB6F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.4973958333333333,
            relative_x: 0.2493489583333333,
            ..Constraint::NONE
        },
        0xEB70 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.4973958333333333,
            relative_height: 0.9961089494163424,
            relative_x: 0.2493489583333333,
            relative_y: 0.0038910505836576,
            ..Constraint::NONE
        },
        0xEB8A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.3468834688346883,
            relative_height: 0.3353615785256410,
            relative_x: 0.2642276422764228,
            relative_y: 0.3313050881410256,
            ..Constraint::NONE
        },
        0xEB9A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8740779768177028,
            relative_height: 0.9438247156716689,
            relative_x: 0.0679662802950474,
            relative_y: 0.0147523709167545,
            ..Constraint::NONE
        },
        0xEBD5 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9322210636079249,
            relative_height: 0.9318897917604415,
            relative_y: 0.0681102082395584,
            ..Constraint::NONE
        },
        0xEBD6 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9996446423917936,
            relative_y: 0.0003553576082064,
            ..Constraint::NONE
        },
        0xEC07 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.3495911047345768,
            relative_height: 0.3355179398148149,
            relative_x: 0.2615335565120357,
            relative_y: 0.3311487268518519,
            ..Constraint::NONE
        },
        0xEC0B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9327424400417101,
            relative_height: 0.9996188152778837,
            relative_y: 0.0003811847221163,
            ..Constraint::NONE
        },
        0xEC0C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8008342022940563,
            relative_x: 0.1991657977059437,
            ..Constraint::NONE
        },
        0xEE00 | 0xEE03 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::End,
            align_vertical: Align::Center1,
            relative_width: 0.8681172291296625,
            relative_height: 0.8626692456479691,
            relative_x: 0.1314387211367673,
            relative_y: 0.0686653771760155,
            pad_left: -0.025,
            pad_right: -0.025,
            pad_top: -0.005,
            pad_bottom: -0.005,
            ..Constraint::NONE
        },
        0xEE01 | 0xEE04 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8626692456479691,
            relative_y: 0.0686653771760155,
            pad_left: -0.05,
            pad_right: -0.05,
            pad_top: -0.005,
            pad_bottom: -0.005,
            ..Constraint::NONE
        },
        0xEE02 | 0xEE05 => Constraint {
            size: Size::Stretch,
            max_constraint_width: 1,
            align_horizontal: Align::Start,
            align_vertical: Align::Center1,
            relative_width: 0.8685612788632326,
            relative_height: 0.8626692456479691,
            relative_y: 0.0686653771760155,
            pad_left: -0.025,
            pad_right: -0.025,
            pad_top: -0.005,
            pad_bottom: -0.005,
            ..Constraint::NONE
        },
        0xEE06 => Constraint {
            size: Size::Cover,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7059415911379657,
            relative_height: 0.2234524408656266,
            relative_x: 0.1470292044310171,
            relative_y: 0.7765475591343735,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            ..Constraint::NONE
        },
        0xEE07 => Constraint {
            size: Size::Cover,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5000000000000000,
            relative_height: 0.7498741821841973,
            relative_x: 0.5000000000000000,
            relative_y: 0.2501258178158027,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            ..Constraint::NONE
        },
        0xEE08 => Constraint {
            size: Size::Cover,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6299093655589124,
            relative_height: 0.8535480624056366,
            relative_x: 0.3700906344410876,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            ..Constraint::NONE
        },
        0xEE09 => Constraint {
            size: Size::Cover,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.4997483643683945,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            ..Constraint::NONE
        },
        0xEE0A => Constraint {
            size: Size::Cover,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6299093655589124,
            relative_height: 0.8535480624056366,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            ..Constraint::NONE
        },
        0xEE0B => Constraint {
            size: Size::Cover,
            max_constraint_width: 1,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5000000000000000,
            relative_height: 0.7498741821841973,
            relative_y: 0.2501258178158027,
            pad_left: 0.015,
            pad_right: 0.015,
            pad_top: 0.015,
            pad_bottom: 0.015,
            ..Constraint::NONE
        },
        0xF005 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9999664113932554,
            relative_y: 0.0000335886067446,
            ..Constraint::NONE
        },
        0xF026..=0xF027 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9786184354605580,
            relative_y: 0.0103951316192896,
            ..Constraint::NONE
        },
        0xF02B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9758052740827267,
            relative_y: 0.0238869355863696,
            ..Constraint::NONE
        },
        0xF031..=0xF033 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9987922705314010,
            relative_y: 0.0006038647342995,
            ..Constraint::NONE
        },
        0xF035 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9989935587761675,
            relative_y: 0.0004025764895330,
            ..Constraint::NONE
        },
        0xF044 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9925925925925926,
            ..Constraint::NONE
        },
        0xF046 | 0xF153..=0xF154 | 0xF158 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8751322751322751,
            relative_y: 0.0624338624338624,
            ..Constraint::NONE
        },
        0xF048 | 0xF04A | 0xF04E | 0xF051 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8577706898990622,
            relative_y: 0.0711892586341537,
            ..Constraint::NONE
        },
        0xF049 | 0xF050 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8579450878868969,
            relative_y: 0.0710148606463189,
            ..Constraint::NONE
        },
        0xF04B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9997041418532618,
            relative_y: 0.0002958581467381,
            ..Constraint::NONE
        },
        0xF04C..=0xF04D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8572940020656472,
            relative_y: 0.0713404035569438,
            ..Constraint::NONE
        },
        0xF04F => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7138835298072554,
            relative_y: 0.1433479295317200,
            ..Constraint::NONE
        },
        0xF052 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9999748091795350,
            ..Constraint::NONE
        },
        0xF060..=0xF061 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8567975830815709,
            relative_y: 0.0719033232628399,
            ..Constraint::NONE
        },
        0xF063 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9987915407854985,
            relative_y: 0.0006042296072508,
            ..Constraint::NONE
        },
        0xF077 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5700483091787439,
            relative_y: 0.2862318840579710,
            ..Constraint::NONE
        },
        0xF078 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5700483091787439,
            relative_y: 0.1437198067632850,
            ..Constraint::NONE
        },
        0xF07E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.4989429175475687,
            relative_y: 0.2505285412262157,
            ..Constraint::NONE
        },
        0xF089 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9998488512696494,
            relative_y: 0.0001511487303507,
            ..Constraint::NONE
        },
        0xF0A4..=0xF0A5 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7502645502645503,
            relative_y: 0.1248677248677249,
            ..Constraint::NONE
        },
        0xF0D7 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.4281400966183575,
            relative_y: 0.2053140096618357,
            ..Constraint::NONE
        },
        0xF0D8 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.4281400966183575,
            relative_y: 0.3472222222222222,
            ..Constraint::NONE
        },
        0xF0D9 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7140772371750631,
            relative_y: 0.1333462732919255,
            ..Constraint::NONE
        },
        0xF0DA => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7140396210163651,
            relative_y: 0.1333838894506235,
            ..Constraint::NONE
        },
        0xF0DC => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            ..Constraint::NONE
        },
        0xF0DD => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            relative_height: 0.4275362318840580,
            relative_y: 0.0012077294685990,
            ..Constraint::NONE
        },
        0xF0DE => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            relative_height: 0.4287439613526570,
            relative_y: 0.5712560386473430,
            ..Constraint::NONE
        },
        0xF100..=0xF101 | 0xF104..=0xF105 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8573155985489722,
            relative_y: 0.0713422007255139,
            ..Constraint::NONE
        },
        0xF102 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9286577992744861,
            relative_y: 0.0713422007255139,
            ..Constraint::NONE
        },
        0xF103 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9286577992744861,
            ..Constraint::NONE
        },
        0xF106..=0xF107 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.5000000000000000,
            relative_y: 0.2853688029020556,
            ..Constraint::NONE
        },
        0xF130 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9998602571268865,
            ..Constraint::NONE
        },
        0xF141 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.2593984962406015,
            relative_y: 0.3696741854636592,
            ..Constraint::NONE
        },
        0xF156 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8752505446623093,
            relative_y: 0.0623155929038282,
            ..Constraint::NONE
        },
        0xF157 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8756468797564688,
            relative_y: 0.0624338624338624,
            ..Constraint::NONE
        },
        0xF159 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8756067947646895,
            relative_y: 0.0623492063492063,
            ..Constraint::NONE
        },
        0xF175 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9989423585404548,
            relative_y: 0.0005288207297726,
            ..Constraint::NONE
        },
        0xF177..=0xF178 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6250661025912215,
            relative_y: 0.1877313590692755,
            ..Constraint::NONE
        },
        0xF182 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9998046921689268,
            ..Constraint::NONE
        },
        0xF221 | 0xF224..=0xF226 | 0xF228 | 0xF22A | 0xF22C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9994854643684076,
            ..Constraint::NONE
        },
        0xF222 | 0xF227 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.8746819883943630,
            relative_y: 0.0624017379870223,
            ..Constraint::NONE
        },
        0xF229 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9370837263813853,
            relative_y: 0.0624017379870223,
            ..Constraint::NONE
        },
        0xF22B | 0xF22D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6874767744332962,
            relative_y: 0.1560043449675557,
            ..Constraint::NONE
        },
        0xF255..=0xF256 | 0xF25A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9993997599039616,
            ..Constraint::NONE
        },
        0xF257 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7810124049619848,
            relative_y: 0.0935945806894186,
            ..Constraint::NONE
        },
        0xF258 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.7498142113988452,
            relative_y: 0.1247927742525582,
            ..Constraint::NONE
        },
        0xF25B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.9975006099019084,
            ..Constraint::NONE
        },
        0xF416 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_height: 0.6090604026845637,
            relative_y: 0.2119686800894855,
            ..Constraint::NONE
        },
        0xF424 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5019531250000000,
            relative_height: 0.5755033557046980,
            relative_x: 0.2480468750000000,
            relative_y: 0.2108501118568233,
            ..Constraint::NONE
        },
        0xF431 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6240234375000000,
            relative_height: 0.7695749440715883,
            relative_x: 0.2031250000000000,
            relative_y: 0.1420581655480984,
            ..Constraint::NONE
        },
        0xF432 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6718750000000000,
            relative_height: 0.7147651006711410,
            relative_x: 0.1875000000000000,
            relative_y: 0.1610738255033557,
            ..Constraint::NONE
        },
        0xF433 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6240234375000000,
            relative_height: 0.7695749440715883,
            relative_x: 0.2041015625000000,
            relative_y: 0.0883668903803132,
            ..Constraint::NONE
        },
        0xF434 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6718750000000000,
            relative_height: 0.7147651006711410,
            relative_x: 0.1406250000000000,
            relative_y: 0.1599552572706935,
            ..Constraint::NONE
        },
        0xF438 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.2436523437500000,
            relative_height: 0.4560546875000000,
            relative_x: 0.3813476562500000,
            relative_y: 0.2719726562500000,
            ..Constraint::NONE
        },
        0xF43E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5029296875000000,
            relative_height: 0.5755033557046980,
            relative_x: 0.2500000000000000,
            relative_y: 0.2136465324384788,
            ..Constraint::NONE
        },
        0xF443 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7500000000000000,
            relative_x: 0.1250000000000000,
            ..Constraint::NONE
        },
        0xF444..=0xF445 | 0xF4C3 | 0xF51D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5000000000000000,
            relative_height: 0.5000000000000000,
            relative_x: 0.2500000000000000,
            relative_y: 0.2500000000000000,
            ..Constraint::NONE
        },
        0xF44A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.2436523437500000,
            relative_height: 0.4560546875000000,
            relative_x: 0.3750000000000000,
            relative_y: 0.2719726562500000,
            ..Constraint::NONE
        },
        0xF44B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.4560546875000000,
            relative_height: 0.2436523437500000,
            relative_x: 0.2719726562500000,
            relative_y: 0.3188476562500000,
            ..Constraint::NONE
        },
        0xF45C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5019531250000000,
            relative_height: 0.5749440715883669,
            relative_x: 0.2480468750000000,
            relative_y: 0.2114093959731544,
            ..Constraint::NONE
        },
        0xF460 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.3593750000000000,
            relative_height: 0.6240234375000000,
            relative_x: 0.3750000000000000,
            relative_y: 0.1884765625000000,
            ..Constraint::NONE
        },
        0xF461 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6237816764132553,
            relative_height: 0.9988851727982163,
            relative_x: 0.1881091617933723,
            ..Constraint::NONE
        },
        0xF467 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5639648437500000,
            relative_height: 0.5649414062500000,
            relative_x: 0.2187500000000000,
            relative_y: 0.2177734375000000,
            ..Constraint::NONE
        },
        0xF46C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5039062500000000,
            relative_height: 0.5771812080536913,
            relative_x: 0.2490234375000000,
            relative_y: 0.2091722595078300,
            ..Constraint::NONE
        },
        0xF470 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9926757812500000,
            relative_height: 0.2690429687500000,
            relative_y: 0.6865234375000000,
            ..Constraint::NONE
        },
        0xF476 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8732325694783033,
            relative_x: 0.0633837152608484,
            ..Constraint::NONE
        },
        0xF47A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.5843079922027290,
            relative_height: 0.9509476031215162,
            relative_x: 0.2066276803118908,
            relative_y: 0.0234113712374582,
            ..Constraint::NONE
        },
        0xF47B..=0xF47C => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.6250000000000000,
            relative_height: 0.3593750000000000,
            relative_x: 0.1875000000000000,
            relative_y: 0.3281250000000000,
            ..Constraint::NONE
        },
        0xF47D => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.3593750000000000,
            relative_height: 0.6240234375000000,
            relative_x: 0.2656250000000000,
            relative_y: 0.1875000000000000,
            ..Constraint::NONE
        },
        0xF47E => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.4560546875000000,
            relative_height: 0.2436523437500000,
            relative_x: 0.2719726562500000,
            relative_y: 0.3750000000000000,
            ..Constraint::NONE
        },
        0xF48B => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7187500000000000,
            relative_height: 0.0937500000000000,
            relative_x: 0.1250000000000000,
            relative_y: 0.4687500000000000,
            ..Constraint::NONE
        },
        0xF493 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8313840155945419,
            relative_height: 0.9509476031215162,
            relative_x: 0.0843079922027290,
            relative_y: 0.0234113712374582,
            ..Constraint::NONE
        },
        0xF49A => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.8727450024378351,
            relative_x: 0.0633837152608484,
            ..Constraint::NONE
        },
        0xF4EF | 0xF4F2 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.7142857142857143,
            relative_x: 0.1428571428571428,
            ..Constraint::NONE
        },
        0xF4F0 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9642857142857143,
            relative_height: 0.7407407407407407,
            relative_y: 0.1111111111111111,
            ..Constraint::NONE
        },
        0xF4F1 => Constraint {
            size: Size::FitCover1,
            height: Height::Icon,
            align_horizontal: Align::Center1,
            align_vertical: Align::Center1,
            relative_width: 0.9642857142857143,
            relative_height: 0.7407407407407407,
            relative_x: 0.0357142857142857,
            relative_y: 0.1111111111111111,
            ..Constraint::NONE
        },
        _ => return None,
    })
}
