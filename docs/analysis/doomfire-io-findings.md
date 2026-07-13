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
`d34b54e9b` does not address it. Closing it means applying engine state on an even cadence
(a fixed apply-interval / io-coalescing change) and/or T2's present-backpressure work, which
T2's doc argues are intertwined and best done together with a metric — a larger, coordinated
item, deliberately not taken on blind here.
