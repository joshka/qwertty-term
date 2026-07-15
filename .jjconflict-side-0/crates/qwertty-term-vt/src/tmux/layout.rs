//! tmux layout-string parser. Port of `terminal/tmux/layout.zig`
//! (Ghostty `2da015cd6`). ADR 004 slice 2.
//!
//! A tmux window layout is a tree of panes and horizontal/vertical splits,
//! serialized as e.g. `80x24,0,0{40x24,0,0,1,40x24,40,0,2}` and (over the wire)
//! prefixed with a 4-hex-digit checksum. This ports the recursive-descent parser
//! and the checksum. Upstream leaves allocation to the caller (an arena); Rust's
//! ownership makes the tree self-freeing, so no allocator is threaded through.

/// A node in a tmux layout tree. Port of `layout.zig`'s `Layout`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Width and height of the node, in cells.
    pub width: usize,
    pub height: usize,
    /// X/Y offset from the top-left corner of the window, in cells.
    pub x: usize,
    pub y: usize,
    /// A pane (leaf) or a horizontal/vertical split (children).
    pub content: Content,
}

/// The content of a [`Layout`] node. Port of `Layout.Content`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// A leaf pane with the given pane id.
    Pane(usize),
    /// A left-to-right split (`{…}`).
    Horizontal(Vec<Layout>),
    /// A top-to-bottom split (`[…]`).
    Vertical(Vec<Layout>),
}

/// Layout parse failure. Port of `ParseError` + `error.ChecksumMismatch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    /// The layout string was malformed.
    SyntaxError,
    /// The 4-char checksum prefix did not match the layout.
    ChecksumMismatch,
}

impl Layout {
    /// Parse a layout string that includes the 4-hex-digit checksum prefix
    /// (`XXXX,<layout>`). Returns [`ParseError::ChecksumMismatch`] if the
    /// checksum is wrong, [`ParseError::SyntaxError`] if the format is invalid.
    /// Port of `parseWithChecksum`.
    pub fn parse_with_checksum(s: &[u8]) -> Result<Layout, ParseError> {
        // 4-char checksum + comma at minimum. A shorter string can't be valid.
        if s.len() < 5 {
            return Err(ParseError::SyntaxError);
        }
        if s[4] != b',' {
            return Err(ParseError::SyntaxError);
        }
        let expected = Checksum::calculate(&s[5..]).as_string();
        if !s.starts_with(&expected) {
            return Err(ParseError::ChecksumMismatch);
        }
        Self::parse(&s[5..])
    }

    /// Parse a bare layout string (no checksum prefix) into a tree. Port of
    /// `parse`.
    pub fn parse(s: &[u8]) -> Result<Layout, ParseError> {
        let mut offset = 0;
        let root = parse_next(s, &mut offset)?;
        if offset != s.len() {
            return Err(ParseError::SyntaxError);
        }
        Ok(root)
    }
}

/// Parse one node starting at `*offset`, advancing it past the node. Port of
/// `parseNext`.
fn parse_next(s: &[u8], offset: &mut usize) -> Result<Layout, ParseError> {
    // Width: up to the first `x`.
    let width = {
        let rest = &s[*offset..];
        let idx = index_of(rest, b'x').ok_or(ParseError::SyntaxError)?;
        let v = parse_usize(&rest[..idx])?;
        *offset += idx + 1; // consume `x`
        v
    };

    // Height: up to the next `,`.
    let height = {
        let rest = &s[*offset..];
        let idx = index_of(rest, b',').ok_or(ParseError::SyntaxError)?;
        let v = parse_usize(&rest[..idx])?;
        *offset += idx + 1; // consume `,`
        v
    };

    // X: up to the next `,`.
    let x = {
        let rest = &s[*offset..];
        let idx = index_of(rest, b',').ok_or(ParseError::SyntaxError)?;
        let v = parse_usize(&rest[..idx])?;
        *offset += idx + 1; // consume `,`
        v
    };

    // Y: up to any of `,{[` (the content delimiter, which we do NOT consume).
    let y = {
        let rest = &s[*offset..];
        let idx = index_of_any(rest, b",{[").ok_or(ParseError::SyntaxError)?;
        let v = parse_usize(&rest[..idx])?;
        *offset += idx; // keep the delimiter
        v
    };

    // Content, determined by the delimiter.
    let content = match s[*offset] {
        b',' => {
            *offset += 1; // consume the delimiter
            // Leaf pane id: up to `,}]`, or the end of string.
            let rest = &s[*offset..];
            let idx = index_of_any(rest, b",}]").unwrap_or(rest.len());
            let pane_id = parse_usize(&rest[..idx])?;
            *offset += idx; // consume the id, not the delimiter
            Content::Pane(pane_id)
        }
        opening @ (b'{' | b'[') => {
            let mut nodes: Vec<Layout> = Vec::new();
            *offset += 1; // move past the opening bracket
            loop {
                nodes.push(parse_next(s, offset)?);

                // We must not be at end-of-string; a closing bracket is expected.
                if *offset >= s.len() {
                    return Err(ParseError::SyntaxError);
                }

                // A comma means another child follows.
                if s[*offset] == b',' {
                    *offset += 1; // consume
                    continue;
                }

                // Otherwise the matching closing bracket must be here.
                let closing = if opening == b'{' { b'}' } else { b']' };
                if s[*offset] != closing {
                    return Err(ParseError::SyntaxError);
                }
                *offset += 1; // consume the closing bracket
                break if opening == b'{' {
                    Content::Horizontal(nodes)
                } else {
                    Content::Vertical(nodes)
                };
            }
        }
        // `index_of_any` above guarantees one of `,{[`.
        _ => unreachable!("y delimiter is one of ,{{[ "),
    };

    Ok(Layout {
        width,
        height,
        x,
        y,
        content,
    })
}

/// Parse a whole byte slice as a base-10 `usize`. Requires a non-empty run of
/// ASCII digits and nothing else. `Err(SyntaxError)` on empty input, a
/// non-digit, or overflow — matching `std.fmt.parseInt(usize, …) catch return
/// error.SyntaxError` (which also errors on overflow).
fn parse_usize(s: &[u8]) -> Result<usize, ParseError> {
    if s.is_empty() {
        return Err(ParseError::SyntaxError);
    }
    let mut v: usize = 0;
    for &b in s {
        if !b.is_ascii_digit() {
            return Err(ParseError::SyntaxError);
        }
        v = v
            .checked_mul(10)
            .and_then(|v| v.checked_add((b - b'0') as usize))
            .ok_or(ParseError::SyntaxError)?;
    }
    Ok(v)
}

/// Index of the first `needle` byte, or `None`.
fn index_of(s: &[u8], needle: u8) -> Option<usize> {
    s.iter().position(|&b| b == needle)
}

/// Index of the first byte contained in `set`, or `None`.
fn index_of_any(s: &[u8], set: &[u8]) -> Option<usize> {
    s.iter().position(|&b| set.contains(&b))
}

/// A tmux layout checksum. Port of `layout.zig`'s `Checksum`: a `u16` computed
/// by rotating right one bit and adding each byte, rendered as 4 lowercase hex
/// digits (zero-padded, matching tmux `layout-custom.c`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checksum(pub u16);

impl Checksum {
    /// Compute the checksum of a layout string. Port of `calculate`.
    pub fn calculate(s: &[u8]) -> Checksum {
        let mut result: u16 = 0;
        for &c in s {
            // Rotate right by 1 (wraparound), then add the byte.
            result = (result >> 1) | ((result & 1) << 15);
            result = result.wrapping_add(c as u16);
        }
        Checksum(result)
    }

    /// Render as 4 lowercase-hex ASCII bytes, zero-padded. Port of `asString`.
    pub fn as_string(self) -> [u8; 4] {
        const CHARSET: &[u8; 16] = b"0123456789abcdef";
        let v = self.0;
        [
            CHARSET[((v >> 12) & 0xf) as usize],
            CHARSET[((v >> 8) & 0xf) as usize],
            CHARSET[((v >> 4) & 0xf) as usize],
            CHARSET[(v & 0xf) as usize],
        ]
    }
}

#[cfg(test)]
mod tests;
