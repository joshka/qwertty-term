# DOOM-fire io findings — T4 half of #141

T4 investigation of the io side of `docs/analysis/doomfire-smoothness.md` (T1's handoff),
building on T2's `renderer-present-backpressure.md`. 2026-07-13.

## Shipped

- **`bb0ac4c72` self-pipe wake** (#148) — the request/response latency fix (the exact
  `fps -fire` fix). The gather stage no longer sleeps the ~1 ms bridge poll after the parse
  stage goes idle; it delivers immediately (parse-idle early-deliver) or is woken by a
  self-pipe. Mirrored faithfully into `qwertty-term-termio/src/exec.rs`.

## `d34b54e9b` (renderer-mutex handoff) — measured, NOT needed here

Upstream `d34b54e9b` adds a demand-handoff to `renderer.State.mutex` so the termio parse
loop can't starve the renderer's `updateFrame`, because upstream's parse loop relocks that
mutex per batch and never sleeps between batches. The T8 drift note filed it as T4-owned
(the `yieldToDemand` call sites live in `Exec.zig`).

**Architecture note:** in this port there is no `renderer.State` mutex. The shared terminal
state is the app's `Arc<Mutex<Engine>>` (`Surface.engine`); the render snapshot
(`Surface::render` under `tick_render`, post-#139 vsync path) and the parse-apply
(`termio.rs`'s sink → `engine.lock().write(batch)`) contend on it. So the handoff, if
needed, would be **entirely app-side** — not T2-blocked, contrary to an earlier status note.

**Measurement (T2's #141 `PresentStats`, release build, readback path):**

```text
QWERTTY_TERM_ASSERT_PRESENT=1 QWERTTY_TERM_PRESENT_STATS=1 QWERTTY_TERM_PRESENT_STATS_EVERY=120

flood (yes, ~68-col line):  cadence_ms=8.40±1.2–2.8   judder_cv=3.7–5.6
idle  (sleep 30, no output): cadence_ms=8.41±1.4–2.9   judder_cv=n/a (content_step≈0 → CV blows up)
```

The present-cadence jitter under a sustained pty flood is **identical to idle** (same
`±` on the same ~8.4 ms mean, zero pty output vs saturating output). If the parse thread
were starving the render on the engine lock, flood jitter would exceed idle jitter — it
does not. So **there is no render lock-starvation to fix.**

Why our pipeline avoids it: the sink locks the engine *only* for the `write(batch)` call and
releases immediately (`termio.rs:203`), and the two-stage gather→parse pipeline hands off
through condvars — unlike upstream's single parse loop that held/relocked the state mutex
greedily. Parsing a 64 KiB batch is tens of µs, far below the 8.4 ms present interval, so
the render's occasional lock wait is negligible.

**Decision:** do **not** implement the `d34b54e9b` demand-handoff. It would add atomics +
a wait primitive to the engine-lock hot path to fix a starvation that measurement shows
does not occur here. Revisit only if a future change (e.g. a much heavier per-batch apply,
or a renderer-thread redesign) makes flood cadence jitter diverge from idle.

## The real residual: uneven sampling (`judder_cv`), not lock contention

Under flood, `judder_cv ≈ 3.7–5.6` (high = chunky) while cadence `±` stays small — which,
per T2's `renderer-present-backpressure.md`, is the signature of uneven *sampling*: the
parse thread applies the pty flood in bursts, so consecutive vsync presents advance the
animation by 1/3/1 steps. This is real, but it is **not** a mutex-starvation problem and
`d34b54e9b` does not address it.

### io-coalescing / draw-split — the T4 half is already done (#139); batch size is the wrong lever

The T1 handoff's T4 ask was "split render from draw with a fixed draw interval and coalesce
io wakeups." **#139 already did this:** render fires only from `tick_render` on the vsync
`CADisplayLink` (a fixed-cadence timer), io runs on the separate `tick_service`, and the
render is *not* triggered by io wakeups at all (it's timer-driven), so there is nothing left
to coalesce on the render path.

The only remaining T4-side lever is the pty **batch granularity** — the gather stage packs up
to `BUFFER_CAPACITY` (64 KiB) per batch, so the parse applies multi-frame chunks at once and
the engine jumps in steps. Measured (yes-flood, `judder_cv` at frame 720 / `throughput_cat`):

```text
BUFFER_CAPACITY   judder_cv   throughput
  64 KiB (ship)     ~3.75       105 MiB/s
  16 KiB            ~4.0         43 MiB/s   (worse-of-both)
   4 KiB            ~2.64        39 MiB/s   (fails the >40 floor)
```

Only a *very* small batch (4 KiB) meaningfully lowers judder, and it craters bulk throughput
~2.7× (below the CI floor) — and even then judder_cv ~2.6 is still chunky. **Decisive point:
upstream uses the same 64 KiB `buffer_capacity` yet is visibly smoother** — so upstream's
smoothness does *not* come from batch size. Shrinking ours would trade a large, real
throughput regression for a partial, still-chunky smoothness gain that doesn't match how
upstream gets there.

**Decision:** do not change the batch size. The residual uneven-sampling judder is the
**render/present path** — T2's present-backpressure work (`Async` triple-buffering aligned to
the display link, or a Metal drawable-present ADR; see `renderer-present-backpressure.md`),
which keeps a complete recent frame available to the compositor at an even cadence. T4's io
half (the draw/render split) is complete; the ball is in T2's court, and the batch-size lever
is a dead end. Re-measure with a real DOOM-fire run (needs Zig 0.14) once T2's present change
lands.
