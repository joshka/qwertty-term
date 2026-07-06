# Port status ledger

Per-file status of the Zig→Rust port. Columns: analysis doc written, code ported, inline
tests ported (count), notes. Reference Zig tree: `~/local/ghostty` (record the commit you
ported against). Extraction candidates from the rewrite prompt are flagged inline.

Status legend: `—` not started · `WIP` · `done`

## Phase 0 — foundations

| Item | Status | Notes |
|---|---|---|
| jj workspace restructure (`work/default` + chunks) | done | 2026-07-06 |
| Cargo workspace skeleton (`crates/ghostty-vt`, `crates/spike`, `xtask`) | done | spike renamed `ghostty-spike`, all 67 tests green |
| libghostty-vt reference build (Zig) | done | ghostty `2da015cd6`; `mise exec zig@0.15.2 -- zig build -Demit-lib-vt=true` → `zig-out/lib/libghostty-vt.a`; note: header docs `max_scrollback` as lines but it is BYTES of page memory |
| Differential harness (`ghostty-vt` vs libghostty-vt) | done (scaffold) | `crates/vt-diff`, feature `reference` (off by default; trunk green without Zig artifact); `Oracle` trait ready for the Rust side; 7/7 tests incl. 3 spike fixtures matching the reference; analysis: `docs/analysis/libghostty-vt-c-api.md` |
| Fuzz targets (parser/stream) | — | after parser skeleton exists |
| Criterion bench skeleton | — | |
| Unicode table codegen (xtask) | WIP | verify against ghostty `props_table.zig` semantics |

## Phase 1 — VT core (`src/terminal/` → `crates/ghostty-vt`)

| Zig file | Analysis | Port | Tests | Notes |
|---|---|---|---|---|
| page.zig | — | — | — | port FIRST (everything sits on it) |
| PageList.zig | — | — | — | signature design; pins, offsets |
| Parser.zig | — | — | — | |
| stream.zig / stream_terminal.zig | — | — | — | |
| Terminal.zig | — | — | — | 50+ inline tests |
| Screen.zig | — | — | — | |
| sgr.zig | — | — | — | |
| csi.zig | — | — | — | |
| osc.zig + osc/parsers/ | — | — | — | parallelizable per-parser |
| dcs.zig / apc.zig | — | — | — | |
| style.zig / color.zig | — | — | — | |
| modes.zig / charsets.zig / Tabstops.zig | — | — | — | |
| hyperlink.zig | — | — | — | |
| kitty/graphics_*.zig | — | — | — | extraction candidate (protocol model) |
| kitty/key.zig | — | — | — | |
| Selection.zig / SelectionGesture.zig | — | — | — | |
| formatter.zig | — | — | — | |
| UTF8Decoder.zig | — | — | — | |
| unicode/ (grapheme, tables) | — | — | — | codegen'd tables via xtask |
| bitmap_allocator.zig / ref_counted_set.zig / hash_map.zig | — | — | — | page-internal structures |

Later phases: add tables as the phase opens (termio, font, renderer, input, config, core,
ffi, macOS).
