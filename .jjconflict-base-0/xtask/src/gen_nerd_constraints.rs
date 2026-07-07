//! `cargo xtask gen-nerd-constraints [<path-to-nerd_font_attributes.zig>]`
//! — mechanically translates upstream's checked-in, codegen'd
//! `src/font/nerd_font_attributes.zig` into
//! `crates/ghostty-font/src/nerd_font_constraints.rs`.
//!
//! Upstream `nerd_font_attributes.zig` is itself generated (by
//! `nerd_font_codegen.py`, from the Nerd Fonts font-patcher script) and checked
//! in with a "DO NOT EDIT BY HAND" header. Rather than re-run the Python
//! pipeline (which needs the upstream font-patcher sources), we translate the
//! already-generated Zig switch table one-to-one into a Rust `match`. The
//! translation is purely syntactic: every case label (`0xXXXX,` or a
//! `0xAAAA...0xBBBB,` inclusive range) and every struct field is copied
//! verbatim, so the emitted table is byte-for-byte semantically identical to
//! upstream's.
//!
//! The default path is the pinned upstream checkout used for the port. Pass an
//! explicit path to regenerate against a different upstream revision.

use std::error::Error;
use std::fs;

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const DEFAULT_SRC: &str = "/tmp/ghostty-2da015/src/font/nerd_font_attributes.zig";
const OUT: &str = "crates/ghostty-font/src/nerd_font_constraints.rs";

/// One parsed switch arm: the codepoint labels it covers and the constraint
/// fields it sets (defaults omitted upstream, so we only carry the set ones).
struct Arm {
    /// Inclusive codepoint ranges (`start..=end`; a single cp is `cp..=cp`).
    ranges: Vec<(u32, u32)>,
    /// `(field_name, value_token)` pairs, in source order.
    fields: Vec<(String, String)>,
}

pub fn run(args: &[String]) -> Result<()> {
    let src_path = args.first().map(String::as_str).unwrap_or(DEFAULT_SRC);
    let src = fs::read_to_string(src_path)
        .map_err(|e| format!("read {src_path}: {e} (pass the path to nerd_font_attributes.zig)"))?;

    let arms = parse(&src)?;
    let total_cps: u64 = arms
        .iter()
        .flat_map(|a| a.ranges.iter())
        .map(|(a, b)| (*b - *a) as u64 + 1)
        .sum();
    eprintln!(
        "gen-nerd-constraints: {} arms, {total_cps} codepoints from {src_path}",
        arms.len()
    );

    let rendered = render(&arms, src_path);
    fs::write(OUT, rendered)?;
    eprintln!("gen-nerd-constraints: wrote {OUT}");
    Ok(())
}

/// Parse the `getConstraint` switch body into arms.
fn parse(src: &str) -> Result<Vec<Arm>> {
    // Isolate the switch body between `return switch (cp) {` and `else => null;`.
    let start = src
        .find("switch (cp)")
        .ok_or("no `switch (cp)` in source")?;
    let body = &src[start..];

    let mut arms = Vec::new();
    let mut pending_ranges: Vec<(u32, u32)> = Vec::new();
    let mut cur_fields: Option<Vec<(String, String)>> = None;

    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("else =>") {
            break;
        }
        // A case label line: `0xXXXX,` or `0xAAAA...0xBBBB,`.
        if line.starts_with("0x") && line.ends_with(',') && !line.contains('=') {
            let label = line.trim_end_matches(',').trim();
            pending_ranges.push(parse_label(label)?);
            continue;
        }
        // Start of an arm body: `=> .{`.
        if line.starts_with("=>") {
            cur_fields = Some(Vec::new());
            continue;
        }
        // End of an arm body: `},` — flush.
        if line == "}," {
            let fields = cur_fields.take().unwrap_or_default();
            arms.push(Arm {
                ranges: std::mem::take(&mut pending_ranges),
                fields,
            });
            continue;
        }
        // A field line inside an arm body: `.name = value,`.
        if let Some(fields) = cur_fields.as_mut()
            && let Some(rest) = line.strip_prefix('.')
        {
            let rest = rest.trim_end_matches(',');
            if let Some((name, value)) = rest.split_once(" = ") {
                fields.push((name.trim().to_string(), value.trim().to_string()));
            }
        }
    }

    Ok(arms)
}

/// Parse a case label: `0xXXXX` or `0xAAAA...0xBBBB` → inclusive `(start,end)`.
fn parse_label(label: &str) -> Result<(u32, u32)> {
    if let Some((a, b)) = label.split_once("...") {
        Ok((parse_hex(a)?, parse_hex(b)?))
    } else {
        let v = parse_hex(label)?;
        Ok((v, v))
    }
}

fn parse_hex(s: &str) -> Result<u32> {
    let s = s.trim().trim_start_matches("0x");
    u32::from_str_radix(s, 16).map_err(|e| format!("bad hex {s:?}: {e}").into())
}

/// Map a Zig enum field value (`.center1`, `.icon`, `.fit_cover1`, a float
/// literal, or an int) to its Rust form.
fn map_value(name: &str, value: &str) -> String {
    match name {
        "size" => format!("size: Size::{}", pascal(value.trim_start_matches('.'))),
        "align_vertical" => {
            format!(
                "align_vertical: Align::{}",
                pascal(value.trim_start_matches('.'))
            )
        }
        "align_horizontal" => format!(
            "align_horizontal: Align::{}",
            pascal(value.trim_start_matches('.'))
        ),
        "height" => format!("height: Height::{}", pascal(value.trim_start_matches('.'))),
        "max_constraint_width" => format!("max_constraint_width: {value}"),
        "max_xy_ratio" => format!("max_xy_ratio: Some({})", float_lit(value)),
        // Float-valued fields.
        "pad_top" | "pad_bottom" | "pad_left" | "pad_right" | "relative_width"
        | "relative_height" | "relative_x" | "relative_y" => {
            format!("{name}: {}", float_lit(value))
        }
        other => format!("{other}: {value}"),
    }
}

/// Turn a snake_case enum variant into PascalCase (`fit_cover1` → `FitCover1`).
fn pascal(s: &str) -> String {
    let mut out = String::new();
    for part in s.split('_') {
        let mut chars = part.chars();
        if let Some(c) = chars.next() {
            out.extend(c.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Ensure a float literal has a decimal point so it's an `f64` in Rust.
fn float_lit(v: &str) -> String {
    if v.contains('.') || v.contains('e') || v.contains('E') {
        v.to_string()
    } else {
        format!("{v}.0")
    }
}

fn render(arms: &[Arm], src_path: &str) -> String {
    let mut s = String::new();
    s.push_str(&format!(
        "//! Nerd Fonts glyph constraint table (GENERATED — DO NOT EDIT BY HAND).\n\
         //!\n\
         //! Mechanically translated from upstream `src/font/nerd_font_attributes.zig`\n\
         //! (itself codegen'd by `nerd_font_codegen.py` from the Nerd Fonts font-patcher)\n\
         //! by `cargo run -p xtask -- gen-nerd-constraints`. Source: `{src_path}`.\n\
         //!\n\
         //! Every arm's codepoint ranges and constraint fields are copied verbatim from\n\
         //! the upstream switch, so [`get_constraint`] is semantically identical to\n\
         //! upstream's `getConstraint`. Regenerate rather than hand-editing.\n\n\
         // Float literals are copied verbatim from upstream (which carries full\n\
         // decimal expansions like 0.7142857142857143); the extra digits beyond f64\n\
         // precision are intentional for byte-for-byte parity, not a mistake.\n\
         #![allow(clippy::excessive_precision)]\n\n\
         use crate::constraint::{{Align, Constraint, Height, Size}};\n\n\
         /// The constraint for `cp` per the Nerd Fonts attribute table, or `None`\n\
         /// (upstream `nerd_font_attributes.getConstraint`).\n\
         pub fn get_constraint(cp: u32) -> Option<Constraint> {{\n\
         \x20   Some(match cp {{\n",
    ));

    for arm in arms {
        // Case labels: `0xAAAA..=0xBBBB | 0xCCCC => ...`.
        let labels: Vec<String> = arm
            .ranges
            .iter()
            .map(|(a, b)| {
                if a == b {
                    format!("0x{a:X}")
                } else {
                    format!("0x{a:X}..=0x{b:X}")
                }
            })
            .collect();
        s.push_str(&format!(
            "        {} => Constraint {{\n",
            labels.join(" | ")
        ));
        for (name, value) in &arm.fields {
            s.push_str(&format!("            {},\n", map_value(name, value)));
        }
        s.push_str("            ..Constraint::NONE\n");
        s.push_str("        },\n");
    }

    s.push_str("        _ => return None,\n");
    s.push_str("    })\n}\n");
    // Ensure trailing formatting is rustfmt-clean-ish; a final rustfmt pass is
    // recommended by the caller.
    s
}
