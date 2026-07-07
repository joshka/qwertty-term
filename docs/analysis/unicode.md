# Unicode subsystem (`src/unicode/`)

Surveyed against ghostty commit `2da015cd6` (uucode dependency `0.2.0`, UCD **17.0.0**).

Ghostty's unicode subsystem answers exactly two questions on the terminal hot path:

1. **Width**: how many cells does a codepoint / grapheme cluster occupy (0, 1, or 2)?
2. **Segmentation**: is there a grapheme cluster break between two adjacent codepoints
   (streaming, for mode 2027 grapheme clustering in `Terminal.print`)?

Everything is precomputed into compact multi-stage lookup tables at build time; the runtime
does no UCD parsing and no per-rule evaluation for property lookup.

## File map

| File                                      | Role                                                                    |
| ----------------------------------------- | ----------------------------------------------------------------------- |
| `src/unicode/main.zig`                    | Public API: `codepointWidth`, `graphemeBreak`, `graphemeWidth`, `table` |
| `src/unicode/props.zig`                   | `Properties` packed struct — the per-codepoint payload                  |
| `src/unicode/props_table.zig`             | Binds the codegen'd stage arrays into `lut.Tables(Properties)`          |
| `src/unicode/props_uucode.zig`            | Codegen exe: uucode → 3-stage LUT → Zig source on stdout                |
| `symbols_table.zig`, `symbols_uucode.zig` | Same pattern, `bool` symbol table (renderer-only)                       |
| `src/unicode/lut.zig`                     | Generic 3-stage LUT generator + runtime accessor                        |
| `src/unicode/grapheme.zig`                | Grapheme break FSM (precomputed) + cluster width measurement            |
| `src/build/UnicodeTables.zig`             | Build plumbing: runs codegen exes, captures stdout as modules           |
| `src/build/uucode_config.zig`             | ghostty's `width`/`is_symbol` derivations (the width policy)            |

Consumers: `Terminal.zig` (`print` width + grapheme clustering, lines ~451/961/1006),
`Screen.zig` (~3248), `Surface.zig` (~2572), `lib_vt.zig` / `terminal/c/unicode.zig` (C API
`ghostty_cp_width`/`ghostty_grapheme_width`), `renderer/cell.zig` (symbols table),
`benchmark/{GraphemeBreak,CodepointWidth,IsSymbol}.zig`.

## Per-codepoint properties (`props.zig`)

```zig
pub const Properties = packed struct {
    width: u2 = 0,                       // clamped [0,2]
    width_zero_in_grapheme: bool = false,
    grapheme_break: uucode.x.types.GraphemeBreakNoControl = .other, // u5, 17 values
    emoji_vs_base: bool = false,
};
```

Adding fields makes the LUT less compressible; ghostty gates changes on `src/bench`
benchmarks (comment at `props.zig:3-5`).

### `grapheme_break`: the `GraphemeBreakNoControl` class (u5, 17 values)

uucode extends UAX #29 `Grapheme_Cluster_Break` (`uucode src/types.zig:127-152`,
`src/x/types_x/grapheme.zig`): `other, prepend, regional_indicator, spacing_mark, l, v, t,
lv, lvt, zwj, zwnj, extended_pictographic, emoji_modifier_base, emoji_modifier,
indic_conjunct_break_extend, indic_conjunct_break_linker, indic_conjunct_break_consonant`.

Derivation from UCD (`uucode src/build/tables.zig:1008-1070`), in priority order:

1. `Emoji_Modifier` (emoji-data.txt) → `emoji_modifier` (asserts GCB=Extend).
2. `Emoji_Modifier_Base` → `emoji_modifier_base` (asserts GCB=Other, ExtPict).
3. `Extended_Pictographic` → `extended_pictographic` (asserts GCB=Other).
4. Else by `InCB` (DerivedCoreProperties.txt): `InCB=Extend` → `zwj` if cp==U+200D else
   `indic_conjunct_break_extend`; `InCB=Linker` → `indic_conjunct_break_linker`;
   `InCB=Consonant` → `indic_conjunct_break_consonant`.
5. Else the raw GCB value, except GCB=Extend which must be U+200C → `zwnj` (uucode asserts
   every GCB=Extend cp is either InCB=Extend/Linker or ZWNJ).

The classic UAX #29 `Extend` class is therefore split into `zwnj ∪ indic_conjunct_break_extend
∪ indic_conjunct_break_linker` (helper `isExtend`, `uucode src/x/grapheme.zig:701-705`).

The "NoControl" variant collapses `control`, `cr`, `lf` → `other`
(`uucode src/x/config_x/grapheme_break.zig:17-23`): ghostty filters control characters
before segmentation ever runs (`grapheme.zig:32-36` doc comment), so the table doesn't spend
states on them. **This means `graphemeBreak` gives garbage for raw control input by design.**

### `width` (u2): ghostty's terminal width policy

Formula lives in **ghostty's** uucode build config (`src/build/uucode_config.zig:12-38`,
`computeWidth`):

```text
width = 0                       if wcwidth_zero_in_grapheme
                                   and not is_emoji_modifier
                                   and grapheme_break_no_control != prepend
      = min(2, wcwidth_standalone)  otherwise
```

`wcwidth_standalone` / `wcwidth_zero_in_grapheme` come from uucode's wcwidth extension
(`uucode src/x/config_x/wcwidth.zig`, `compute`), in this exact order:

- gc ∈ {Cc, Cs, Zl, Zp} → 0 (controls, surrogates, line/para separators)
- U+00AD SOFT HYPHEN → 1 (terminal compat exception to default-ignorable)
- `Default_Ignorable_Code_Point` → 0 (ZWJ/ZWNJ, VS15/VS16, tags, bidi controls…)
- U+2E3A TWO-EM DASH → 2; U+2E3B THREE-EM DASH → 3 (ghostty clamps to 2)
- `East_Asian_Width ∈ {W, F}` → 2 (uucode applies `# @missing: …; W` directives from
  `extracted/DerivedEastAsianWidth.txt`, so *unassigned* codepoints in CJK-default ranges
  are already Wide — `uucode src/build/Ucd.zig:1021-1046`)
- GCB = Regional_Indicator → 2 (UTS #51: lone RI renders as letter-in-box)
- else → 1 (includes East Asian Ambiguous → narrow, and lone combining marks → 1 via the
  "defective combining sequence rendered on NBSP base" rule)

Special case: U+20E3 COMBINING ENCLOSING KEYCAP gets `wcwidth_standalone = 2`.

`wcwidth_zero_in_grapheme` = true when: standalone width is 0, or `is_emoji_modifier`, or
gc ∈ {Mn, Me} (incl. keycap), or GCB ∈ {V, T} (Hangul jamo vowels/trailers, Kirat Rai), or
GCB = Prepend. It means "contributes no width *inside* a multi-codepoint cluster".

The escape hatches in `computeWidth` are deliberate (comment `uucode_config.zig:23-32`):
emoji modifiers keep standalone width 2 so lone skin tones render as color patches; Prepend
keeps width 1 so lone rephas don't vanish.

Net effect: `width` is 0 for C0/C1, surrogates, Zl/Zp, default-ignorables, and combining
marks; 2 for EAW W/F, regional indicators, emoji modifiers, and the em-dashes; 1 otherwise.
For cp > 0x10FFFF (u21 max is 0x1FFFFF), `props_uucode.get` returns
`{width=1, width_zero_in_grapheme=true, grapheme_break=other, emoji_vs_base=false}`
(`props_uucode.zig:9-15`).

### `emoji_vs_base` (bool)

True iff the cp appears as the *base* of both a text-style and an emoji-style sequence in
`emoji/emoji-variation-sequences.txt` (uucode asserts is_text == is_emoji,
`uucode src/build/tables.zig:981-987`). Used to validate VS15/VS16 sequences — a selector
after a non-base is *ignored*, not applied.

## The 3-stage lookup table (`lut.zig`)

Based on <https://here-be-braces.com/fast-lookup-of-unicode-properties/>.

- Codepoint space `0..=0x1FFFFF` is split into **256-codepoint blocks** (`block_size = 256`,
  `lut.zig:24`).
- **stage3**: deduplicated array of distinct `Properties` values (linear-scan dedup,
  `lut.zig:70-81`).
- **stage2**: concatenation of deduplicated *blocks*; each entry is a u16 index into stage3.
  Blocks are dedup'd by content hash (`lut.zig:95-103`).
- **stage1**: one u16 per block (8192 entries for u21), holding the *offset of the block's
  first entry* in stage2.

Lookup (`lut.zig:143-147`):

```zig
stage3[stage2[stage1[cp >> 8] + (cp & 0xFF)]]
```

All three lengths are asserted ≤ u16::MAX (`lut.zig:107-110`). Codegen writes the arrays as
Zig source (`writeZig`); the build (`src/build/UnicodeTables.zig`) compiles
`props_uucode.zig`/`symbols_uucode.zig` as host exes, runs them, captures stdout, and wires
the result in as the anonymous imports `unicode_tables`/`symbols_tables`, which
`props_table.zig`/`symbols_table.zig` bind at comptime.

## Grapheme break FSM (`grapheme.zig`)

### State (`uucode.grapheme.BreakState`, u3, 5 values)

`default, regional_indicator, extended_pictographic, indic_conjunct_break_consonant,
indic_conjunct_break_linker` (`uucode src/grapheme.zig:197-203`). Carried by the caller
between consecutive `graphemeBreak(cp1, cp2, &state)` calls (cp2 of one call is cp1 of the
next). The states cover the three context-sensitive UAX #29 rules: GB12/13 (RI pairing
parity), GB11 (emoji ZWJ sequences), GB9c (Indic conjunct sequences: consonant, then
extend/linker runs, ending at a consonant).

### The rule kernel (`computeGraphemeBreakNoControl`, uucode `src/x/grapheme.zig:472-680`)

A direct port of UAX #29 with three deliberate tailorings:

1. **No GB3/GB4/GB5**: control/CR/LF rules are compiled out (classes collapsed to `other`);
   the terminal filters controls upstream.
2. **Emoji modifier tailoring** (UTS #51 ED-13, comment `src/x/grapheme.zig:686-700`):
   `emoji_modifier` is *removed* from Extend. An emoji modifier only continues a cluster
   immediately after `emoji_modifier_base` (entering the `extended_pictographic` state) or
   within an existing ExtPict sequence. So `"a" + skin-tone` **breaks** (diverges from
   stock UAX #29 / GraphemeBreakTest.txt, where Emoji_Modifier is Extend).
3. **State resets** precede the rules: on entry, the current state is invalidated back to
   `default` if gb1/gb2 are not plausible members of the in-flight sequence
   (`src/x/grapheme.zig:478-537`). This makes the function total over all (state, gb1, gb2)
   triples, which is what makes the precomputation below valid.

Rule order: GB6/7/8 (Hangul), GB9a (SpacingMark), GB9b (Prepend), GB9c (Indic, stateful),
GB11 (emoji, stateful), GB12/13 (RI parity via state toggle), GB9 (Extend|ZWJ catch-all),
GB999 (break).

### Precomputed transition table (`grapheme.zig:146-206`)

All `(state: u3, gb1: u5, gb2: u5)` triples are evaluated at comptime into

```zig
Key   = packed struct(u13) { state: u3, gb1: u5, gb2: u5 }   // state = low bits
Value = packed struct(u4)  { result: bool, state: u3 }
data  : [8192]Value                                           // 8 KiB (asserted)
```

`graphemeBreak` (`grapheme.zig:37-47`) is then: two `table.get` property lookups + one array
index + state store. A slow exhaustive verifier against plain uucode exists as an optional
`main()` (`grapheme.zig:215-260`).

## Cluster width and VS15/VS16 (`grapheme.zig`)

`graphemeWidthEffect(prev, cp) → {ignore, no_change, wide, narrow}` (`grapheme.zig:58-83`)
is the width-decision kernel for a codepoint that *continues* a cluster (caller must have
already established no-break):

- `cp == VS16 (U+FE0F)`: if `table.get(prev).emoji_vs_base` → `wide`, else **`ignore`**.
- `cp == VS15 (U+FE0E)`: if base valid → `narrow`, else `ignore`.
- else if `!table.get(cp).width_zero_in_grapheme` → `wide` (any width-contributing
  continuation makes the cluster ≥ 2 because the base was ≥ 1; e.g. spacing marks,
  Hangul V after L is *not* here — V/T are width_zero_in_grapheme).
- else → `no_change`.

**`ignore` contract**: the terminal does not store invalid variation selectors in the cell;
the caller must also *roll back the break state* to what it was before the selector and keep
`prev` unchanged (doc comment `grapheme.zig:49-57`; `Terminal.zig:1006` area implements
this; `graphemeWidth` does the same via `state_before`).

`graphemeWidth(T, cps) → {len, width}` (`grapheme.zig:96-140`) measures the first cluster:
starts with `width = table.get(cps[0]).width`, then loops `graphemeBreak` +
`graphemeWidthEffect`, applying `wide→2`, `narrow→1`, `ignore→state rollback`. For u32-typed
input (C API), cp > 0x10FFFF acts as: width-1 singleton if first, cluster terminator
otherwise (`grapheme.zig:100-118`). Mirrors `Terminal.print` under mode 2027.

Note the width model: cluster width is **not** a sum; it is base width possibly bumped to 2
(or forced 1/2 by VS15/16). uucode's own `x.grapheme.wcwidth` sums Devanagari/Hangul
contributions — ghostty's simpler bump model plus `width_zero_in_grapheme` reproduces the
same results for terminal-relevant sequences because any non-zero continuation forces 2.

## Symbols table (`symbols_table.zig` / `symbols_uucode.zig`)

Second LUT instance with `Elem = bool`: true for private-use gc or codepoints in
symbol-heavy blocks (arrows, dingbats, emoticons, misc symbols, enclosed alphanumerics,
misc symbols & pictographs, transport & map) — formula in `uucode_config.zig:computeIsSymbol`.
Sole runtime consumer is `renderer/cell.zig` (font/glyph decisions). **Not needed for
`ghostty-vt`; defer to the renderer/font phase.**

## Inline tests (the conformance anchors)

- `main.zig` "codepointWidth": narrow/control/zero-width/wide spot checks incl. 0x10FFFF→1,
  surrogate→0, VS16→0, RI→2, U+2E3B→2.
- `grapheme.zig`: "emoji modifier" (base+modifier no-break, `"` + modifier breaks), "long
  emoji zwj sequences" (family ZWJ chain), "variation selectors" (effects + `ignore`
  double-VS16 case + `len=3 width=1` for `x,FE0F,FE0F`), "emoji sequences" (keycap `#,FE0F,20E3`
  → 2; bare `1,20E3` → 1; wave+skin-tone → 2), "spacing marks can widen narrow clusters"
  (scan finds an Mc that bumps `a`+Mc to width 2), "segmentation" (RI pairing len=2 width=2,
  lone RI width 2, defective `0301 0302` → len 2 width 0), "u32 invalid codepoints stand
  alone".
- `terminal/c/unicode.zig`: C-API wrappers of the same (out-of-range u32 behavior).
- `props_uucode.zig`/`symbols_uucode.zig`: exhaustive parity of LUT vs direct uucode get.

## Port notes (Rust)

- Rust `char` cannot exceed 0x10FFFF, so the Rust tables cover `0..=0x10FFFF` (4352 blocks)
  instead of u21's 8192; `properties(cp: u32)` returns ghostty's out-of-range fallback for
  larger values, preserving the C-API-visible semantics.
- The transition-table precomputation moves from Zig comptime to a Rust `const fn` running
  the same ported rule kernel at compile time — no codegen needed for the 8 KiB FSM table.
- The property LUT is codegen'd by `cargo xtask gen-unicode` from UCD 17.0.0 via `ucd-parse`
  (+ small hand parsers for `@missing` EAW directives, InCB values, and
  emoji-variation-sequences, which ucd-parse does not model), reproducing the uucode
  derivations documented above. Generated file: `crates/ghostty-vt/src/unicode/tables.rs`
  (stage1 = 4352, stage2 = 31488 = 123 blocks, stage3 = 29 unique property sets; ghostty's
  Zig table is 8192/31744/30, the delta being exactly the >U+10FFFF fallback rows).
- Verified: per-codepoint parity with ghostty's actual generated `props.zig` (from the
  ghostty build cache at the surveyed commit) over all of `0..=0x10FFFF` — 0 mismatches —
  and exhaustive cross-checks against `unicode-width`/`unicode-segmentation` (both UCD
  17.0.0) with an allowlist documenting every intentional divergence
  (`crates/ghostty-vt/tests/unicode_crosscheck.rs`).
