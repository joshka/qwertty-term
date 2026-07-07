# Renderer R3: first-pixels MSL port + pipeline descriptions

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse --short 2da015cd6`). Scope:
`src/renderer/shaders/shaders.metal` (853 lines — the first-pixels subset
only) and `src/renderer/metal/shaders.zig` (454 lines — the pipeline
description table and wire structs, wire structs already ported and frozen
in R1). Rust ports live at `crates/ghostty-renderer/src/shaders/{mod.rs,
ghostty.metal,color_math.rs,smoke.rs}`. Working-copy commit at time of
writing: `9a9b82b05604`.

This is chunk R3: the shader layer between R1's wire structs/GPU-resource
primitives and R2's frame/pipeline-construction machinery (not yet landed as
of this chunk — R3 only defines the pipeline *description* table as plain
data; wiring `PipelineDescription` into an actual `MTLRenderPipelineState`
via `objc2-metal` is R2's `Pipeline`/`Frame` types).

## What was ported

The MSL functions upstream's `shaders.metal` first-pixels pipelines need,
copied **verbatim** (plan decision 6: "port MSL verbatim... embed source;
runtime `newLibraryWithSource` first"), including upstream's own comments:

- The shared color-math helpers (`~34-180` in upstream): `srgb_to_display_p3`,
  the two `linearize`/`unlinearize` overloads (vector and scalar), `luminance`,
  `contrast_ratio`, `contrasted_color`, `load_color`.
- `full_screen_vertex` — the single-triangle-clipped-to-viewport trick shared
  by `bg_color` and `cell_bg`.
- `bg_color_fragment` — solid background color fill.
- `cell_bg_fragment` — per-cell background color, with the padding-extend
  edge-clamping logic (`EXTEND_LEFT/RIGHT/UP/DOWN`).
- `cell_text_vertex` / `cell_text_fragment` — the glyph-quad vertex shader
  and the two-atlas (grayscale/color) sampling fragment shader, including the
  minimum-contrast and cursor-color-override logic.

No line was altered from the upstream source; `ghostty.metal`'s only
additions are a file-level doc comment recording provenance and scope. The
file compiles standalone (`#include <metal_stdlib>` + `using namespace
metal;` needs nothing else), so no MSL-compilation-forced edits were needed —
the "adjust only what MSL compilation requires" allowance in the task turned
out to be unnecessary for this subset.

From `shaders.zig`'s `pipeline_descs` table, the three first-pixels entries
were ported into `PIPELINE_DESCRIPTIONS`: `bg_color`, `cell_bg`, `cell_text`
(function names, step function, blending flag), plus `cell_text`'s vertex
attribute layout (`CellText`'s `autoAttribute`-derived format/offset table).

## What was skipped (and why)

- **`image_vertex`/`image_fragment`** and **`bg_image_vertex`/
  `bg_image_fragment`** (plus their supporting structs/enums
  `ImageVertexIn/Out`, `BgImageVertexIn/Out`, `BgImagePosition`, `BgImageFit`,
  `BgImageRepeat`) — explicitly out of scope per the chunk brief ("SKIP
  image/bg_image pairs — later"; roadmap parks these at R6, "kitty/bg
  images"). `wire.rs`'s `Image`/`BgImage` structs are already frozen (R1) so
  R6 has a stable target to emit into, but the shader code that reads them is
  not yet ported.
- **The `image`/`bg_image` pipeline descriptions** in `pipeline_descs` — same
  deferral; `PIPELINE_DESCRIPTIONS` covers only the first three of upstream's
  five entries.
- **Custom/post-process shader plumbing** (`Shaders.post_pipelines`,
  `initPostPipelines`, `initPostPipeline`, the `main0`-named SPIR-V-Cross
  entry point convention) — R8 ("shadertoy") per the roadmap; nothing in this
  chunk's scope touches it.
- **Actual `MTLRenderPipelineState` construction** (`Pipeline.init`,
  `MTLVertexDescriptor`/`MTLRenderPipelineDescriptor` setup,
  `newRenderPipelineStateWithDescriptor:error:`) — that's R2's `Pipeline`
  type (declared as an uninhabited placeholder in `metal/mod.rs` per R1's
  deferrals list) consuming this chunk's `PipelineDescription` table as
  input. R3 stops at "here is the data describing each pipeline," matching
  the acceptance criteria ("sizeof/layout tests... MSL compiles at runtime;
  color-math golden values" — no pipeline-state acceptance criterion).
- **Build-time `metallib` via `xcrun metal`** — plan decision 6 explicitly
  sequences this after the runtime-compile path; only `newLibraryWithSource`
  is in scope for chunk R3.

## The buffer-index / vertex-layout contract

Frozen by R1 (plan decision 5, restated in `wire.rs`'s module doc): buffer
index 0 is vertex/instance data, index 1 is uniforms, 2+ are extras. The
ported MSL's `[[buffer(N)]]` annotations follow this exactly —
`cell_bg_fragment` and `cell_text_vertex` both take `uniforms [[buffer(1)]]`
and an extra per-cell background-color array at `[[buffer(2)]]`
(`cells`/`bg_colors` respectively; two different fragment/vertex stages
reading the same logical "flattened grid of cell background colors" buffer,
per upstream's own duplication). Neither ported fragment/vertex function
declares a buffer-0 binding directly — for `cell_text_vertex`, buffer 0 is
implicit: it's where the `CellTextVertexIn` `[[stage_in]]` struct's
per-instance attributes are fetched from, per the vertex descriptor built by
`Pipeline.init`/`autoAttribute`, not a `constant T*` parameter in the
function signature.

`CELL_TEXT_ATTRIBUTES` in `shaders/mod.rs` is the Rust mirror of
`autoAttribute(CellText, ...)`: one entry per struct field in declaration
order, giving the shader-side attribute index (`0..6`, matching
`[[attribute(N)]]` in `CellTextVertexIn`), the Metal vertex format the
field's type maps to (`autoAttribute`'s `switch (FT)` — e.g. `[2]u32 ->
uint2`, `[4]u8 -> uchar4`), and the byte offset within the instance struct
(`@offsetOf`). Every offset in that table is asserted, by the
`layout_pins_match_wire_offsets` test, to equal
`std::mem::offset_of!(wire::CellText, <field>)` — i.e. it doesn't hardcode a
second copy of the frozen layout, it reads the same offsets `wire.rs`'s own
tests already pin, so the two can never silently diverge. The stride
(`layout.stride` in upstream `Pipeline.init`, `@sizeOf(V)`) is likewise
`size_of::<wire::CellText>()`, not a literal `32` — though `wire.rs` still
carries upstream's own inline "minimize this struct" assertion as the
ground truth for that number. `bg_color`/`cell_bg` have no vertex buffer at
all (`vertex_attributes: None`, `stride: 0`): their vertex shader is the
`[[vertex_id]]`-only `full_screen_vertex`, no `[[stage_in]]` struct.

Blending is modeled as a single bool-wrapping struct
(`Blending::{DISABLED, PREMULTIPLIED_OVER}`) rather than a full blend
descriptor, because upstream's `Pipeline.init` only ever configures one fixed
premultiplied "over" blend (`rgbBlendOperation`/`alphaBlendOperation = add`,
`sourceRGBBlendFactor`/`sourceAlphaBlendFactor = one`,
`destinationRGBBlendFactor`/`destinationAlphaBlendFactor =
one_minus_source_alpha`) whenever `blending_enabled` is true — there is no
per-pipeline variation in *how* blending is done, only *whether* it's on.
`bg_color` disables it (first pass, draws onto an undefined/cleared target);
`cell_bg` and `cell_text` enable it (each subsequent pass blends over what
came before).

## P3-vs-sRGB handling

Ghostty's render target is always Display P3 (R1's `Target::new`: the
backing IOSurface is tagged with `kCGColorSpaceDisplayP3`), but colors
arriving from terminal/config state are conventionally sRGB. `load_color`
(the one function every fragment/vertex shader in this subset funnels colors
through) is the single point where this is reconciled:

1. Decode the 4-byte RGBA input (`0..255 -> 0.0..1.0`).
2. If the color is **already** P3 and blending is non-linear, it's already
   correct — premultiply and return immediately (fast path, avoids
   unnecessary linearize/unlinearize round-trips).
3. Otherwise, linearize (gamma-encoded sRGB and gamma-encoded P3 share the
   same sRGB transfer function, so one `linearize` call handles both cases —
   the only difference is the *primaries*, handled next).
4. If the input was sRGB (not already P3), apply `srgb_to_display_p3` — a
   3x3 matrix multiply in linear space (composed from a D50-adapted sRGB->XYZ
   matrix and an XYZ->Display-P3 matrix; upstream notes this should ideally
   be a uniform-supplied matrix rather than hardcoded, unchanged in this
   port).
5. If the caller wants gamma-encoded output (`linear=false`), unlinearize.
6. Premultiply by alpha and return.

The `use_display_p3` uniform bool is therefore not "is P3 support enabled" —
it's "are the *input* colors already expressed in P3" (set based on how the
config/terminal state produced them), decoupled from `use_linear_blending`
(whether the *pipeline* wants linear or gamma-encoded output) and
`use_linear_correction` (see below). `cell_text_vertex` always requests
`linear=true` from `load_color` regardless of `use_linear_blending`, because
contrast-ratio math (`contrasted_color`) needs linear luminances to be
correct per the WCAG formula — the fragment shader re-applies gamma encoding
itself if the pipeline isn't doing linear blending (see next section).

## The linear-blend weight-correction math (luminance-based alpha remap)

This is `cell_text_fragment`'s `ATLAS_GRAYSCALE` branch, gated by
`uniforms.use_linear_correction`, and is the subtlest piece of numerically
pinned behavior in this chunk (golden-tested in
`shaders/color_math.rs::linear_correction_alpha_*`).

**The problem it solves:** a font rasterizer's alpha-coverage mask (e.g. "35%
of this pixel is covered by the glyph") is a *linear* coverage fraction, but
traditional gamma-incorrect terminal renderers blend it directly in sRGB
(gamma-encoded) space: `result_srgb = lerp(bg_srgb, fg_srgb, coverage)`. That
is not physically correct (blending should happen in linear light), but
decades of terminals/text-renderers do it that way, and it produces text
that *looks* a certain weight/thickness that users are calibrated to expect.
If Ghostty instead does the physically-correct thing — blend in *linear*
space (`result_linear = lerp(bg_linear, fg_linear, coverage)`, then
gamma-encode the result) — the same coverage value produces visibly
different (typically thinner-looking, for dark-text-on-light-background)
edges, because sRGB gamma is not a linear remapping of perceptual lightness.

**The fix — don't change the blend, change the alpha fed into it:** rather
than switch the blend equation, `use_linear_correction` computes a
*different* alpha value that, when used in the physically-correct linear
blend, reproduces the same **luminance** the naive gamma-space blend would
have produced. Concretely (`linear_correction_alpha` in the Rust mirror,
matching the shader 1:1):

1. Take the (already-linear) foreground and background luminances `fg_l`,
   `bg_l` (`luminance()` — WCAG relative luminance).
2. Unlinearize both back to gamma space, blend them there with the *original*
   linear coverage `a` as the naive gamma-space renderer would:
   `naive_gamma_blend = unlinearize(fg_l) * a + unlinearize(bg_l) * (1 - a)`.
3. Linearize that back: `blend_l = linearize(naive_gamma_blend)` — this is
   "the luminance the naive approach would have produced, expressed in
   linear space."
4. Solve for the alpha that reproduces `blend_l` under *linear*
   interpolation between `bg_l` and `fg_l`: `a' = (blend_l - bg_l) / (fg_l -
   bg_l)`, clamped to `[0, 1]`.
5. Use `a'` (not the original `a`) as the actual blend weight for the real
   (color, not just luminance) linear blend that follows.

The dead-band (`abs(fg_l - bg_l) > 0.001`) exists because step 4's
denominator degenerates as `fg_l -> bg_l`; upstream's comment notes this
"avoid[s] numbers going haywire" — the golden test
`linear_correction_alpha_no_op_when_luminances_are_close` pins that the
function is a no-op (returns the input `a` unchanged) inside that band. The
boundary golden test confirms `a=0`/`a=1` map to themselves regardless of the
luminance gap (the remap is anchored at the endpoints by construction: at
`a=0`/`a=1`, `blend_l` collapses to exactly `bg_l`/`fg_l`). The directional
golden test (`linear_correction_alpha_black_text_on_white_bg_darkens_faster`)
pins the concrete numeric claim upstream's comment makes in prose ("yields
virtually identical results for grayscale blending... very similar but
non-identical for color blending"): for black-on-white text at 50% linear
coverage, the corrected alpha is `0.7859588595177676`, well above the naive
`0.5` — confirming the remap moves alpha in the direction that makes
linear-correct blending "look like" the heavier, gamma-incorrect blending
users expect.

Applied only in the grayscale-atlas branch (`ATLAS_COLOR`, i.e. pre-rendered
color emoji/symbol glyphs, is unaffected — upstream assumes those are
already premultiplied and doesn't attempt luminance correction on them).

## Golden values chosen (`shaders/color_math.rs`)

All of the following are Rust reimplementations (test-only, deliberately not
shared code with `ghostty.metal`) of the exact scalar formulas in the MSL,
pinned against independently-documented constants rather than "whatever the
current code computes":

- `linearize(0.0) == 0.0`, `linearize(1.0) == 1.0` — sRGB transfer function
  fixed points, exact by construction.
- `linearize(0.5) == 0.21404114048223255` — the textbook "sRGB 50% gray is
  ~21.4% linear light" fact, computed independently in Python and pinned to
  17 significant digits.
- `unlinearize(linearize(v)) == v` and vice versa for a spread of sample
  points — round-trip fidelity of the two piecewise branches.
- The sRGB breakpoint pair (`0.04045` gamma-side, `0.0031308` linear-side):
  pinned with a *loose* (`1e-7`) tolerance rather than exact equality,
  because these are independently-rounded published constants, not exact
  images of each other under the formula (evaluating the curve at `0.04045`
  gives `~0.00313080496`, about `5e-9` off from `0.0031308` — a known,
  harmless quirk of the sRGB spec's own rounding, now recorded so a future
  reader doesn't "fix" the test into a false exact match).
- `luminance(red/green/blue/white/black)` == the WCAG coefficients themselves
  (`0.2126`/`0.7152`/`0.0722`) and `1.0`/`0.0` for white/black — exact by
  construction of the dot product.
- `contrast_ratio(black, white) == 21.0` — the canonical WCAG maximum-contrast
  boundary value, `(1.0 + 0.05) / (0.0 + 0.05)`.
- `contrast_ratio(gray, gray) == 1.0` — minimum possible ratio for identical
  luminances.
- A WCAG AA `4.5:1` boundary case: algebraically solved for the linear gray
  level that produces exactly `4.5` against white, confirming the formula
  shape (not just the two extremes).
- The linear-correction alpha remap's fixed points (`a=0`, `a=1`), its
  dead-band no-op behavior, and the black-on-white-at-50%-coverage golden
  value (`0.7859588595177676`) described above.

## Runtime-compile smoke test (`shaders/smoke.rs`)

`embedded_msl_compiles_and_exposes_ported_functions` calls
`MTLCreateSystemDefaultDevice()` directly (not R1/R2's `metal::test_metal()`
— deliberately no coordination with R2, which owns `metal/mod.rs`), compiles
[`shaders::SOURCE`] via `newLibraryWithSource:options:error:`, and asserts
all 5 ported function names (`full_screen_vertex`, `bg_color_fragment`,
`cell_bg_fragment`, `cell_text_vertex`, `cell_text_fragment`) resolve via
`newFunctionWithName:`. Skips gracefully (`SKIP:` to stderr, early return) if
no Metal device is present, matching R1's CI-friendliness pattern. **Result
on this development machine: PASS** — the library compiled and all 5
function names resolved, i.e. this is not just a skip-and-hope test; it
exercised real Metal shader compilation of the ported source in this run.

## Deferrals

- `image`/`bg_image` shader pairs, structs, and pipeline descriptions — R6.
- Post-process/custom shader pipeline (`main0` convention, SPIR-V-Cross
  compatibility) — R8.
- Actual `MTLRenderPipelineState`/`MTLVertexDescriptor` construction consuming
  `PipelineDescription` — R2's `Pipeline` type.
- Build-time `metallib` via `xcrun metal` in `build.rs` — later per plan
  decision 6 (explicitly sequenced after runtime-compile).
- Config-driven selection of `use_display_p3`/`use_linear_blending`/
  `use_linear_correction`/`min_contrast` uniform values — those uniforms are
  consumed as given by this chunk's shaders; producing them from config is
  R2+/R4 territory (R1's `renderer-r1.md` already notes `linear_blending`
  as a config-plumbing stand-in).
