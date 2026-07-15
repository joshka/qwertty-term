# Page hash-map backward-shift deletion (probe-length / tombstone-accumulation)

Commit-stamped record of the page offset hash-map deletion-strategy pass on
`qwertty-term-vt`'s `page::offset_map` (the cell-offset → hyperlink-id and
cell-offset → grapheme-slice maps). Ports the core of upstream Ghostty's
post-pin map cluster:

- **`fedd42e8d`** "terminal: use backward-shift deletion in page maps" — the
  core: replace tombstone deletion with backward-shift (Knuth vol. 3, §6.4,
  algorithm R).
- **`7e14347c1`** "terminal: bound page map probe lengths" — the 80% hyperlink
  load factor + `layoutForSize`. *(Deferred — see "Load factor" below.)*
- **`65f953e8e`** "terminal: avoid duplicate probes when moving cell data" —
  no-clobber insertion on cell moves. *(Already present in our port — our move
  paths already call `put_assume_capacity_no_clobber`.)*

All three are after our frozen pin `77190bd02` (pin is their ancestor; exactly
these three commits touch `hash_map.zig` in the range).

## Why tombstone deletion was slow (profile-first)

Our port used tombstone deletion: removal marks a slot `TOMBSTONE` (not free),
recycled opportunistically on the next insert whose probe path crosses it. A
fixed-capacity map cannot outgrow tombstone buildup, so probe chains grow and
lookups/inserts degrade.

A criterion microbench (`benches/hash_map.rs`, modelled on upstream's
`benchmark/HyperlinkMap.zig`) over a 4096-slot `Map<u32,u32>` confirmed the cost
is real — **but only for the representative workload**, which the first bench
got wrong:

**Same-key churn** (`remove(k); put(k)` — clear then re-set the *same* cell,
upstream's `stepChurn`) is *cheap* on our raw-tombstone map (the tombstone is
recycled in place), so backward-shift — which pays a cluster scan per removal —
is neutral-to-slower here. Upstream measured a *speedup* only because their
baseline (`7e14347c1`) carried a periodic in-place **rehash** that backward-shift
removes; **we never had that machinery**.

**Sliding-window churn** (evict the oldest key, insert a fresh never-seen key —
what actually happens as a terminal scrolls and rewrites cells at *different*
offsets) is where tombstones accumulate with nothing to recycle them, and every
probe chain lengthens toward the whole table. This is the real pathology
(upstream #13292; their OSC 8 stream 47×, map churn 100×).

## Numbers (criterion, 4096-slot map, M2 Max, machine contended — read ratios)

Both columns measured back-to-back on the same (loaded) box; the ratio holds.

| workload (per pass over the working set) | tombstone           | backward-shift      | delta              |
| ---------------------------------------- | ------------------- | ------------------- | ------------------ |
| **slide 50%** (diff-key churn)           | 705 µs              | 76 µs               | **9.3× faster**    |
| **slide 90%** (diff-key churn)           | 13.84 ms            | 770 µs              | **18× faster**     |
| churn 50% (same-key)                     | 22.8 µs             | 20.7 µs             | ~tie               |
| churn 90% (same-key)                     | 216 µs              | 524 µs              | 2.4× slower        |
| churn 100% (same-key)                    | 17.0 ms             | 28.9 ms             | 1.7× slower        |
| lookup 50% / 90% / 100%                  | 3.8 / 17.6 / 141 µs | 3.8 / 16.6 / 140 µs | tie / better / tie |

The lever is a **robustness / tail-latency** win: 9–18× on the representative
cell-churn workload and canonical (tombstone-free) lookup chains, at the cost of
a narrow same-key-churn regression at high load (+84 ns/op on a near-full map,
only for pathologically dense same-cell rewrites). Matches upstream's decision.

## The change (backward-shift deletion — oracle-neutral)

`offset_map.rs`: `Metadata` loses `TOMBSTONE`/`is_tombstone`/`remove` — a slot
is now only free (all-zero byte) or used. `remove_by_index` implements
backward-shift: walk the cluster forward from the hole; any entry whose home
slot lies cyclically within `[home, j)` moves into the hole, advancing the hole,
until a free slot ends the cluster (bounded to one full cycle for a 100%-full
table). `get_or_put` drops tombstone recycling and asserts the probe ends at a
free slot (capacity was guaranteed). `put_assume_capacity_no_clobber` asserts a
free slot exists. Backward-shift needs only `key.hash64()` (offset keys are
self-hashing — no context, unlike `RefCountedSet`).

This is an internal algorithm swap: **same memory layout, same capacities, same
observable output** (get/put/remove/contains/count return identically; only the
post-removal slot arrangement and iteration order differ, and no observable VT
output depends on map iteration order). Therefore **oracle-neutral** — no pin
bump. Verified below.

## Load factor (`7e14347c1`) — PR-2, implemented, ORACLE-NEUTRAL (no pin bump)

Upstream additionally caps the hyperlink map at an 80% load factor to bound
probe length on a fully-populated map and to structurally dodge the same-key
regression (which only bites at 90–100% load). PR-2 ports this: a defaulted
const generic `OffsetHashMap<K, V, const MAX_LOAD: u8 = 100>` (grapheme map keeps
the 100% default; hyperlink map = `…, 80`), a `layout_for_size` that scales the
requested entry count up to the raw slot count the load factor needs (rounding to
a power of two), `max_load()` used as the insertion ceiling in `get_or_put` /
`ensure_unused_capacity`, and `hyperlink_capacity()` returning `max_load()`. The
page hyperlink-map layout now requests the raw entry count and lets
`layout_for_size` do the scaling+rounding (no double-rounding).

This *does* change the hyperlink map's raw capacity and per-page memory layout,
so — unlike backward-shift — it was not obviously oracle-neutral. **Evaluated on
evidence: it is.** Page growth on a full hyperlink map is lossless (Ghostty grows
the page via `increaseCapacity`, it never drops hyperlinks), so *when* a page
grows (at 80% vs 100% fill) is invisible to observable output. Confirmed: the
full `vt-diff --features reference` suite — corpus + AFL + hand + formatter +
generative sweep — is **0-divergence against the `77190bd02` oracle** with PR-2
applied. So **no pin bump is required** for the full faithful port. The 100%-load
`get_or_put`/`churn`/`lookup` cliffs (see numbers above) are now structurally
unreachable for the hyperlink map, which operates at ≤80% fill.

## Verification

- **Differential (the primary guarantee).** Against the `77190bd02` reference
  oracle the generative sweep is **0-divergence** with this change; corpus +
  AFL-corpus + hand differential + formatter differential all green. (During
  initial verification the installed oracle was stale — see note — so this was
  first established *relatively*: the sweep reported the identical 259
  divergence seeds with and without the change, none hyperlink/grapheme;
  after the oracle was rebuilt to the documented pin it is a clean zero.)
- **Direct equivalence / correctness** (`page::offset_map::tests`): ported
  upstream's new backward-shift tests — `assert_canonical` (every used entry
  reachable from its home without crossing a free slot), colliding-cluster
  removal across the wraparound, removal from a completely full table, and a
  20k-op random differential against `std::collections::HashMap`.
- **Miri** (`page::offset_map`): 15/15 clean (186 s), including the 20k-op
  oracle differential — the unsafe backward-shift pointer probing is UB-clean.
- **Fuzz** (`resize`, 3 min): 85 257 runs, no crash/panic/leak. Resize/reflow
  drives `move_cells`, which removes+reinserts map entries under backward-shift.
- Full gate: `cargo check --all-targets` (zero warnings), `fmt`, `clippy -D
  warnings`, release lane.

## Note: pre-existing oracle staleness (found + FIXED during this pass)

While verifying, the installed reference lib `~/local/ghostty/zig-out/lib/
libghostty-vt.a` was found dated Jul 7 — the old `2da015cd6`-era artifact,
despite `ffi.rs`/`AGENTS.md` documenting the pin as `77190bd02`. The prior
pin-bump session's install had only updated the symlinks/xcframework (Jul 15),
not the actual `.a`/`.dylib` — a partial install. That stale oracle (predating
upstream's `no_scrollback` change) is what left main's generative sweep red at
259 scrollback-class divergences — orthogonal to this lever.

**Fixed:** the correct `77190bd02` build already existed un-installed in the
`~/local/ghostty-pin77190` worktree (Jul 15 08:50); installed its `.a`/`.dylib`
into `~/local/ghostty/zig-out/lib` (stale ones backed up to
`lib-backup-stale-jul7-2da015cd6/`). The default `vt-diff --features reference`
sweep now passes at **0 divergences**, restoring the repo's zero-divergence
invariant for every thread. `ffi.rs`/`AGENTS.md` were already correct.
