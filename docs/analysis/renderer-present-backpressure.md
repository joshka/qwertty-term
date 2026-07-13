# Renderer present backpressure — findings for #141 (DOOM-fire judder, T2 half)

T2 investigation of T1's `docs/analysis/doomfire-smoothness.md` handoff, 2026-07-13.
Read the present/pacing path end-to-end before choosing a fix; the premise shifted.

## What the handoff assumed vs what the code actually does

The handoff's T2 framing is *"render free-runs with no vsync backpressure; frames are
unevenly sampled."* After reading the post-#139 code, **render is already vsync-driven,
not free-running:**

- Render fires **only** from `tick_render` (`app.rs:3630`), which is called **only** by
  the `CADisplayLink` callback `ghosttyRenderTick:` (`app.rs:4916`) — one render+present
  per vsync tick. The free-running ~60 Hz `NSTimer` runs `tick_service` (io only, **no
  render**). So render cannot outrun the display on the interactive path.
- `#139` already phase-locked render to vsync. The "render produces as fast as it likes"
  premise is outdated for the interactive path (it *is* still true on the GUI-smoke /
  no-screen fallback `start_pace_timer` path, a plain 60 Hz `NSTimer`).

**Where the real T2 gap is:** the present itself is fire-and-forget. `draw_and_present`
ends with a bare `layer.set_surface_sync` → `CALayer.setContents(IOSurface)`
(`present.rs:194`, `layer.rs:177`), the only blocking being a **GPU-side**
`waitUntilCompleted` in `frame.complete(true)` (`frame.rs:108`). `SwapChainMode::Sync`
keeps **1 frame in flight** (`engine.rs:248`). There is no `CAMetalDrawable`/
`presentDrawable:` queue, so nothing couples the swap to the display's *scanout* — the
display-link callback cadence alone spaces frames, and any main-thread jitter in that
callback (or variable render time landing the `setContents` at a jittery offset before the
compositor's async pickup) shows up as uneven presents.

## The two honest caveats

1. **No smoothness metric exists.** The handoff itself notes the fps lane measures
   throughput, not smoothness; validating any fix needs an inter-presented-frame
   *animation-step variance* metric that isn't built yet. **Building a present change blind
   (no way to measure improvement or regression) is the main risk here.**
2. **The dominant residual judder is likely the T4 half, not T2.** The handoff's own
   analysis attributes the "1, then 3, then 1 animation-steps apart" chunking to bursty io
   apply (no render/draw split, no io coalescing) — a T4 item. Since render is already
   vsync-paced, a T2 present-backpressure change may move the needle only modestly until
   T4's io half lands.

## Architecture constraint

Adopting a Metal drawable-present path (block on `nextDrawable`, matching Ghostty's
`hasVsync()` backpressure) **reverses locked decision 2** in
`docs/plans/m3-first-pixels.md:14` ("IOSurface-on-CALayer, **not** CAMetalLayer/
nextDrawable") — it needs an ADR. The **less invasive, in-architecture** fix is the one
decision 3 already anticipates: **triple buffering (`SwapChainMode::Async`, permits=3) +
aligning IOSurface swaps to the display-link tick**, which stays entirely within the
IOSurface path.

## Recommended path (metric-first, in-architecture)

1. **Add a smoothness metric** (inter-presented-frame animation-step variance, or present
   inter-arrival jitter) so any change is measurable — otherwise we're flying blind.
2. **Scoped in-architecture fix:** enable `Async` triple-buffering (permits=3, the guarded
   `debug_assert_eq!(mode, Sync)` and the engine-level kitty image buffers must go per-slot
   first — R6 slice-1's noted async-safety follow-up) so a complete recent frame is always
   available to the compositor, and align the swap to the display-link tick.
3. **Only if (2) proves insufficient**, escalate to a Metal drawable-present ADR.
4. Alternatively, **defer T2 until T4's io-coalescing half lands, then re-measure** —
   because the dominant judder source is likely there and T2's marginal gain is unclear
   without a metric.

Note the coupling to R6: enabling `Async` requires moving the engine-level kitty image
texture cache + instance buffers **per-slot** (the `debug_assert_eq!(mode, Sync)` guard at
`engine.rs:1154` exists precisely because they aren't yet) — so the present-backpressure
work and that R6 async-safety follow-up (#19) are the same change.

## Running the measurement (wired 2026-07-13)

The `PresentStats` primitive is now fed from the live present path by an env-gated
recorder in the renderer (`present_stats::PresentStatsRecorder`, held by `Engine`) —
**no app change**. To get real numbers on a DOOM-fire (or any) run:

```sh
QWERTTY_TERM_ASSERT_PRESENT=1 \   # makes the host present via the readback path
QWERTTY_TERM_PRESENT_STATS=1 \    # enables the recorder
QWERTTY_TERM_PRESENT_STATS_EVERY=120 \  # report cadence in frames (default 120)
  cargo run -p qwertty-term --release
```

It prints a running line to stderr every `EVERY` frames:

```text
PRESENT_STATS frames=120 cadence_ms=8.62±0.31 content_step=1.94±2.63 judder_cv=1.36
```

Read it as: `cadence_ms=mean±stddev` (present interval; the ± is present-cadence
jitter — should be small with #139's vsync), and `judder_cv` (coefficient of
variation of the per-present content step — the animation-step-evenness / judder
metric; ~0 is smooth, large is chunky). **A small cadence ± with a large judder_cv
confirms the residual judder is uneven *sampling* (T4's io-apply burstiness), not the
present path** — which decides whether the fix belongs to T2 or T4 before either is
built. Caveat: the readback path adds a full-frame CPU readback per present, so the
absolute cadence is measurement-perturbed; the *ratios* (jitter, CV) are the signal.
