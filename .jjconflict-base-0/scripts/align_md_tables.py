#!/usr/bin/env python3
"""Reformat Markdown tables to satisfy markdownlint-cli2 rules:

- table-pipe-style: leading_and_trailing (every row starts/ends with `|`)
- table-column-style: aligned (all pipes vertically aligned via padding,
  separator row dashes padded to match column widths, colon alignment
  markers preserved)

This script is conservative: it only touches lines it recognizes as a
Markdown table (a header row immediately followed by a valid separator
row), skips fenced code blocks, and never changes cell *content* --
only the whitespace padding between pipes. Tables containing a cell with
an escaped pipe (`\\|`) are left untouched and reported for manual review,
since splitting such rows into cells conservatively is ambiguous.

Usage:
    python3 scripts/align_md_tables.py [files...]

If no files are given, it walks the repo root (parent of this script's
`scripts/` dir) for `*.md` files, excluding `target/`, `node_modules/`,
and `work/`.
"""

from __future__ import annotations

import sys
from pathlib import Path

FENCE_PREFIXES = ("```", "~~~")

SEP_CELL_CHARS = set(":- ")


def is_fence_line(line: str) -> bool:
    stripped = line.strip()
    return stripped.startswith(FENCE_PREFIXES[0]) or stripped.startswith(FENCE_PREFIXES[1])


def looks_like_table_row(line: str) -> bool:
    stripped = line.strip()
    return stripped.startswith("|") or ("|" in stripped and not stripped.startswith("#"))


def split_row_cells(line: str):
    """Split a table row into (indent, cells, had_leading, had_trailing).

    Returns None if the line contains an unescaped-pipe-based split that's
    ambiguous due to an escaped pipe `\\|` inside a cell (caller should skip).
    """
    stripped = line.rstrip("\n")
    indent_len = len(stripped) - len(stripped.lstrip(" "))
    indent = stripped[:indent_len]
    content = stripped[indent_len:]

    if "\\|" in content:
        return None

    had_leading = content.startswith("|")
    had_trailing = content.endswith("|") and len(content) > 1

    body = content
    if had_leading:
        body = body[1:]
    if had_trailing:
        body = body[:-1]

    cells = body.split("|")
    cells = [c.strip() for c in cells]
    return indent, cells, had_leading, had_trailing


def is_separator_cell(cell: str) -> bool:
    c = cell.strip()
    if not c:
        return False
    if any(ch not in SEP_CELL_CHARS for ch in c):
        return False
    return "-" in c


def sep_alignment(cell: str) -> str:
    """Return 'left', 'right', 'center', or 'none' colon alignment for a separator cell."""
    c = cell.strip()
    left = c.startswith(":")
    right = c.endswith(":")
    if left and right:
        return "center"
    if right:
        return "right"
    if left:
        return "left"
    return "none"


def render_separator_cell(width: int, align: str) -> str:
    if align == "center":
        inner = "-" * max(width - 2, 1)
        return ":" + inner + ":"
    if align == "left":
        inner = "-" * max(width - 1, 1)
        return ":" + inner
    if align == "right":
        inner = "-" * max(width - 1, 1)
        return inner + ":"
    return "-" * max(width, 1)


def find_tables(lines):
    """Yield (start_idx, end_idx_exclusive) ranges of table blocks."""
    ranges = []
    in_fence = False
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        if is_fence_line(line):
            in_fence = not in_fence
            i += 1
            continue
        if in_fence:
            i += 1
            continue

        if looks_like_table_row(line) and line.strip().startswith("|"):
            # candidate header; check next line is a valid separator row
            if i + 1 < n:
                sep_line = lines[i + 1]
                if sep_line.strip().startswith("|") or (
                    "|" in sep_line.strip() and set(sep_line.strip()) <= SEP_CELL_CHARS
                ):
                    sep_parsed = split_row_cells(sep_line)
                    if sep_parsed is not None:
                        _, sep_cells, _, _ = sep_parsed
                        if sep_cells and all(is_separator_cell(c) for c in sep_cells):
                            header_parsed = split_row_cells(line)
                            if header_parsed is not None:
                                _, header_cells, _, _ = header_parsed
                                if len(header_cells) == len(sep_cells):
                                    # collect body rows
                                    j = i + 2
                                    while j < n:
                                        row = lines[j]
                                        if is_fence_line(row):
                                            break
                                        rs = row.strip()
                                        if not rs.startswith("|"):
                                            break
                                        j += 1
                                    ranges.append((i, j))
                                    i = j
                                    continue
        i += 1
    return ranges


def reformat_table(lines, start, end):
    """Return (new_lines, skipped_reason_or_None)."""
    parsed_rows = []
    for idx in range(start, end):
        parsed = split_row_cells(lines[idx])
        if parsed is None:
            return None, "escaped pipe in cell"
        parsed_rows.append(parsed)

    ncols = len(parsed_rows[0][1])
    for _, cells, _, _ in parsed_rows:
        if len(cells) != ncols:
            return None, "ragged row (column count mismatch)"

    sep_row_idx = 1
    sep_cells = parsed_rows[sep_row_idx][1]
    aligns = [sep_alignment(c) for c in sep_cells]

    # width per column = max(len(cell)) across all non-separator rows,
    # and at least enough for the separator's minimal rendering (3 for plain,
    # 3 for one colon, 3 for both... use markdownlint's minimum of 3 dashes
    # equivalent width, but simplest: min width 3).
    widths = [3] * ncols
    for row_i, (_, cells, _, _) in enumerate(parsed_rows):
        if row_i == sep_row_idx:
            continue
        for ci, cell in enumerate(cells):
            widths[ci] = max(widths[ci], len(cell))

    # separator cell rendering must also fit within width (colons count).
    for ci in range(ncols):
        min_sep_len = {"center": 3, "left": 2, "right": 2, "none": 1}[aligns[ci]]
        widths[ci] = max(widths[ci], min_sep_len)

    indent = parsed_rows[0][0]

    out = []
    for row_i, (_, cells, _, _) in enumerate(parsed_rows):
        rendered_cells = []
        if row_i == sep_row_idx:
            for ci in range(ncols):
                rendered_cells.append(render_separator_cell(widths[ci], aligns[ci]))
        else:
            for ci in range(ncols):
                cell = cells[ci]
                pad = widths[ci] - len(cell)
                rendered_cells.append(cell + " " * pad)
        line = indent + "| " + " | ".join(rendered_cells) + " |"
        out.append(line + "\n")

    return out, None


def process_file(path: Path):
    text = path.read_text(encoding="utf-8")
    had_trailing_newline = text.endswith("\n")
    lines = text.splitlines(keepends=True)
    if not had_trailing_newline and lines:
        lines[-1] = lines[-1] + "\n"

    table_ranges = find_tables(lines)
    if not table_ranges:
        return 0, 0, []

    new_lines = list(lines)
    tables_reformatted = 0
    skipped = []

    # process in reverse so earlier indices remain valid as we splice
    for start, end in reversed(table_ranges):
        block = new_lines[start:end]
        result, reason = reformat_table(new_lines, start, end)
        if result is None:
            skipped.append((path, start + 1, reason))
            continue
        if result != block:
            new_lines[start:end] = result
            tables_reformatted += 1
        else:
            # already fine, but still "processed"
            pass

    out_text = "".join(new_lines)
    if not had_trailing_newline:
        if out_text.endswith("\n"):
            out_text = out_text[:-1]

    changed = out_text != text
    if changed:
        path.write_text(out_text, encoding="utf-8")

    return (1 if changed else 0), tables_reformatted, skipped


def discover_files(root: Path):
    excluded_dirs = {"target", "node_modules", "work", ".git"}
    for p in sorted(root.rglob("*.md")):
        if any(part in excluded_dirs for part in p.relative_to(root).parts[:-1]):
            continue
        yield p


def main():
    args = sys.argv[1:]
    if args:
        files = [Path(a) for a in args]
    else:
        root = Path(__file__).resolve().parent.parent
        files = list(discover_files(root))

    total_files_changed = 0
    total_tables = 0
    all_skipped = []

    for f in files:
        changed, tables, skipped = process_file(f)
        total_files_changed += changed
        total_tables += tables
        all_skipped.extend(skipped)
        if changed:
            print(f"reformatted: {f} ({tables} table(s))")

    print()
    print(f"Files changed: {total_files_changed}")
    print(f"Tables reformatted: {total_tables}")
    if all_skipped:
        print(f"Tables skipped (manual handling needed): {len(all_skipped)}")
        for path, line, reason in all_skipped:
            print(f"  {path}:{line} - {reason}")


if __name__ == "__main__":
    main()
