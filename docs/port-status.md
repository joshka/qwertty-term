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
| Fuzz targets (parser/stream) | done (target) | `crates/ghostty-vt/fuzz` (own workspace); parser+utf8 no-panic target compiles on nightly; campaign pending `cargo install cargo-fuzz`, then `cargo +nightly fuzz run parser -- -max_total_time=60` |
| Criterion bench skeleton | done | baselines: ascii ~108 MiB/s, sgr ~104 MiB/s, utf8_mixed ~437 MiB/s (untuned) |
| Unicode table codegen (xtask) | done | `cargo xtask gen-unicode` (UCD 17.0.0 pinned, downloads gitignored); 3-stage LUT matching ghostty's format; **exact parity: 0 mismatches vs ghostty's generated table over all 1,114,112 codepoints**; analysis: `docs/analysis/unicode.md` |

## Phase 1 — VT core (`src/terminal/` → `crates/ghostty-vt`)

| Zig file | Analysis | Port | Tests | Notes |
|---|---|---|---|---|
| page.zig | done | done | 26 (Zig 46, consolidated) | Miri clean (85 lib tests, caught+fixed a Stacked Borrows bug); deferred to PageList chunk: verifyIntegrity, clone/cloneBuf+mmap pool, swapCells, Style::bg plumbing; analysis: `docs/analysis/page-memory.md`; **verify consolidation coverage during PageList chunk** |
| PageList.zig | done | done | 102 (Zig 205; 17 highlight tests → highlight chunk, tripwire-alloc N/A, resize permutations covered by 25 representative) | pools/pins/viewport/scroll/erase/clone/split/compact + full reflow; Miri: pagelist 92/92 + page 73/73 scoped (10 pathological tests normal-runner-only); Page verify_integrity completed (was no-op) — consolidated Page tests held up; analysis: `docs/analysis/pagelist.md` |
| Parser.zig | done | done | 25/25 | + table test; 14 vte differential tests, 4 divergences pinned (empty params, colon-non-m, param-overflow policy, utf8 ownership); analysis: `docs/analysis/vt-parser.md` |
| stream.zig / stream_terminal.zig | — | — | — | |
| Terminal.zig | — | — | — | 50+ inline tests |
| Screen.zig | — | — | — | |
| sgr.zig | done | done | 30 (Zig 31; 1 C-ABI-only N/A) | colon/semicolon rules only for params 4/38/48/58; fuzzer crash case `ESC[58:4:m` pinned |
| csi.zig | done | done | 5 (Zig 0) | net-new tests |
| osc.zig + osc/parsers/ | — | — | — | parallelizable per-parser |
| dcs.zig / apc.zig | — | — | — | |
| style.zig / color.zig | done | done | 8 + 26 | StyleSet done; full color port (X11 names via embedded rgb.txt, CIELAB 256-cube light/dark logic, RGB.parse grammar); analysis: `docs/analysis/terminal-state.md` |
| modes.zig / charsets.zig / Tabstops.zig | done | done | 12/3/5 (Zig 12/1/5) | modes table via macro_rules!; bitsets → plain arrays (not load-bearing) |
| hyperlink.zig | done | done | 0 (matches Zig) | PageEntry/HyperlinkSet data model |
| kitty/graphics_*.zig | — | — | — | extraction candidate (protocol model) |
| kitty/key.zig | — | — | — | |
| Selection.zig / SelectionGesture.zig | — | — | — | |
| formatter.zig | — | — | — | |
| UTF8Decoder.zig | done | done | 3/3 | |
| unicode/ (grapheme, tables) | done | done | 13 | ghostty `2da015cd6`; `grapheme_break` FSM (const-evaluated 8 KiB table), `codepoint_width`, VS15/VS16 effects; all inline tests from grapheme.zig/main.zig/c/unicode.zig ported; oracle cross-checks: 188 width + 3,915 break divergences, all classified (terminal tailorings); symbols table deferred to renderer phase |
| bitmap_allocator.zig / ref_counted_set.zig / hash_map.zig | done | done | 22/0/12 (Zig 21/0/22) | hash_map tests consolidated; unsafe boundaries documented per module |

Later phases: add tables as the phase opens (termio, font, renderer, input, config, core,
ffi, macOS).
