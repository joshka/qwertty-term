#!/usr/bin/env python3
r"""Regenerate the XTGETTCAP capability rows for `src/terminfo.rs` from upstream.

Parses Ghostty's terminfo Source (`src/terminfo/ghostty.zig`) and emits the
`CAPABILITIES` table rows. We translate each Zig string literal to its RUNTIME
byte form (halving the backslashes: Zig source `\\E` -> runtime `\E`); the value
is stored in Rust as a raw byte string `br"..."` byte-identical to the Zig
runtime string. The xtgettcapMap transform (`\E`->ESC, `^X`->ctrl, `%`-verbatim)
is applied at query time in Rust (`encode_string_cap`), mirroring
`Source.zig:88-115`.

Usage:
    gen_terminfo.py [path/to/ghostty/src/terminfo/ghostty.zig]

Default source path is ~/local/ghostty/src/terminfo/ghostty.zig. The emitted
rows are pasted between the `static CAPABILITIES` brackets in src/terminfo.rs;
the `TN` value is overridden to `qwertty-term` at query time, not here.
"""
import os
import re
import sys

default_src = os.path.expanduser("~/local/ghostty/src/terminfo/ghostty.zig")
src_path = sys.argv[1] if len(sys.argv) > 1 else default_src
src = open(src_path).read()

# Grab the capabilities = &.{ ... }; block.
m = re.search(r"\.capabilities = &\.\{(.*)\},\s*\};", src, re.S)
body = m.group(1)

# Each entry: .{ .name = "NAME", .value = .{ .KIND = VAL } }
# KIND: boolean = {} | numeric = N | string = "..."
entry_re = re.compile(
    r'\.\{\s*\.name\s*=\s*"((?:[^"\\]|\\.)*)"\s*,\s*\.value\s*=\s*\.\{\s*\.(\w+)\s*=\s*'
    r'(?:\{\}|(\d+)|"((?:[^"\\]|\\.)*)")\s*\}\s*\}',
    re.S,
)


def zig_unescape(s):
    """Zig string literal body -> runtime bytes."""
    out = []
    i = 0
    while i < len(s):
        c = s[i]
        if c == "\\":
            nxt = s[i + 1]
            mapping = {"\\": "\\", '"': '"', "n": "\n", "r": "\r", "t": "\t"}
            if nxt in mapping:
                out.append(mapping[nxt])
                i += 2
            elif nxt == "x":
                out.append(chr(int(s[i + 2 : i + 4], 16)))
                i += 4
            else:
                raise SystemExit(f"unhandled zig escape \\{nxt} in {s!r}")
        else:
            out.append(c)
            i += 1
    return "".join(out)


def rust_bytestr(runtime):
    """Emit a Rust byte-string literal for these runtime bytes."""
    if all(0x20 <= ord(ch) < 0x7F for ch in runtime) and '"' not in runtime:
        return 'br"' + runtime + '"'
    esc = []
    for ch in runtime:
        b = ord(ch)
        if ch == "\\":
            esc.append("\\\\")
        elif ch == '"':
            esc.append('\\"')
        elif 0x20 <= b < 0x7F:
            esc.append(ch)
        else:
            esc.append(f"\\x{b:02x}")
    return 'b"' + "".join(esc) + '"'


rows = []
for mm in entry_re.finditer(body):
    name, kind, num, strval = mm.group(1), mm.group(2), mm.group(3), mm.group(4)
    if kind == "boolean":
        rows.append(f'    (b"{name}", CapValue::Boolean),')
    elif kind == "numeric":
        rows.append(f'    (b"{name}", CapValue::Numeric({num})),')
    elif kind == "string":
        rows.append(f'    (b"{name}", CapValue::Str({rust_bytestr(zig_unescape(strval))})),')
    else:
        raise SystemExit(f"unknown kind {kind}")

sys.stderr.write(f"parsed {len(rows)} capabilities from {src_path}\n")
print("\n".join(rows))
