//! X11 named color table. Port of `src/terminal/x11_color.zig` (107 lines,
//! 2 inline tests).
//!
//! Ghostty comptime-parses `res/rgb.txt` (the X11 project's color database)
//! into a `StaticStringMapWithEql` keyed with ASCII case-insensitive
//! equality, plus a stable-order `entries` array for the C API. This port
//! embeds the same file (`res/rgb.txt`, copied verbatim from the Zig tree)
//! and parses it once at first use into a `HashMap<String, Rgb>` keyed by
//! lowercased name — case-insensitive lookup without a comptime string-map
//! crate dependency. [`entries`] preserves file order like the Zig
//! `entries` array (the C-API stability requirement doesn't apply to this
//! Rust-only chunk, but file order is still useful for the "first entry is
//! snow" test and for any future formatter/completion use).

use std::collections::HashMap;
use std::sync::OnceLock;

use super::Rgb;

/// A single X11 color entry: a name (in file casing) and its RGB value.
/// Port of `x11_color.zig` `Entry`.
#[derive(Debug, Clone, Copy)]
pub struct Entry {
    pub name: &'static str,
    pub color: Rgb,
}

/// This is the rgb.txt file from the X11 project, embedded verbatim. Last
/// sourced from <https://gitlab.freedesktop.org/xorg/app/rgb>. This data is
/// licensed under the MIT/X11 license while this Rust file is licensed
/// under the same license as the rest of this crate. Port of `x11_color.zig`
/// `data`.
const DATA: &str = include_str!("../../res/rgb.txt");

fn parse_entries() -> Vec<Entry> {
    let mut result = Vec::new();
    for raw_line in DATA.split('\n') {
        // Trim \r so this works with both LF and CRLF line endings.
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.is_empty() {
            continue;
        }
        let r: u8 = line[0..3].trim().parse().expect("malformed rgb.txt: r");
        let g: u8 = line[4..7].trim().parse().expect("malformed rgb.txt: g");
        let b: u8 = line[8..11].trim().parse().expect("malformed rgb.txt: b");
        let name = line[12..].trim_matches(|c| c == ' ' || c == '\t');
        result.push(Entry {
            name,
            color: Rgb::new(r, g, b),
        });
    }
    result
}

/// All X11 colors in `rgb.txt` file order. Port of `x11_color.zig`
/// `entries`.
pub fn entries() -> &'static [Entry] {
    static ENTRIES: OnceLock<Vec<Entry>> = OnceLock::new();
    ENTRIES.get_or_init(parse_entries)
}

fn build_map() -> HashMap<String, Rgb> {
    let mut map = HashMap::with_capacity(entries().len());
    for entry in entries() {
        map.insert(entry.name.to_ascii_lowercase(), entry.color);
    }
    map
}

/// Case-insensitive lookup of an X11 color name. Port of `x11_color.zig`
/// `map.get`.
pub fn get(name: &str) -> Option<Rgb> {
    static MAP: OnceLock<HashMap<String, Rgb>> = OnceLock::new();
    MAP.get_or_init(build_map)
        .get(&name.to_ascii_lowercase())
        .copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Port of x11_color.zig's unnamed `test { ... }` block.
    #[test]
    fn lookup() {
        assert_eq!(get("nosuchcolor"), None);
        assert_eq!(get("white"), Some(Rgb::new(255, 255, 255)));
        assert_eq!(get("medium spring green"), Some(Rgb::new(0, 250, 154)));
        assert_eq!(get("ForestGreen"), Some(Rgb::new(34, 139, 34)));
        assert_eq!(get("FoReStGReen"), Some(Rgb::new(34, 139, 34)));
        assert_eq!(get("black"), Some(Rgb::new(0, 0, 0)));
        assert_eq!(get("red"), Some(Rgb::new(255, 0, 0)));
        assert_eq!(get("green"), Some(Rgb::new(0, 255, 0)));
        assert_eq!(get("blue"), Some(Rgb::new(0, 0, 255)));
        assert_eq!(get("white"), Some(Rgb::new(255, 255, 255)));
        assert_eq!(get("lawngreen"), Some(Rgb::new(124, 252, 0)));
        assert_eq!(get("mediumspringgreen"), Some(Rgb::new(0, 250, 154)));
        assert_eq!(get("forestgreen"), Some(Rgb::new(34, 139, 34)));
    }

    // Port of x11_color.zig "entries".
    #[test]
    fn entries_test() {
        let all = entries();
        assert!(all.len() > 700);
        for entry in all {
            assert_eq!(get(entry.name), Some(entry.color));
        }
        assert_eq!(all[0].name, "snow");
    }
}
