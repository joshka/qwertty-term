# DOOM-fire smoothness gap (post-present-pacing)

Status: **diagnosed, handed off.** Present pacing is fixed (#139). The residual
judder is a renderer + io sampling problem that lives in **T2** (`qwertty-term-renderer`)
and **T4** (`qwertty-term` app / termio), not T1 (engine). Engine throughput is already
at or above parity with Ghostty main on the vt lanes — this is not a parse/apply-speed
problem.

## Symptom

Running DOOM-fire in a `cargo run --release` window, qwertty-term reports ~1100 fps
(and after #139 presents at a steady vsync ~116 fps on a 120 Hz ProMotion panel), yet the
fire visibly **judders / arrives in chunks** rather than flowing smoothly. Side by side,
Ghostty on the same machine and grid is visibly smoother. So the problem is not "too few
frames" — we produce far more than the display can show — it is **uneven sampling of the
animation in time**.

## What #139 fixed (and confirmed by instrumentation)

Before #139 the app presented on a plain ~60 Hz `NSTimer`, which is not phase-locked to
the display. On a 120 Hz panel, presents landed at drifting offsets relative to vsync, so
high-fps content beat against the refresh and juddered. #139 moved presentation to a
vsync-synced `CADisplayLink` (`NSView.displayLink`, macOS 14+, main thread) and split the
render tick from the io-service tick so background/occluded windows still drain reply
bytes while presentation pauses.

Instrumentation (temporary `TMP_PACING` / `TMP_RENDER` env prints, since removed)
confirmed the `CADisplayLink` path is taken and fires at ~116 fps locked to vsync. **So
present timing is genuinely correct now.** The judder that remains is therefore *not*
present timing.

## Root cause of the residual judder: uneven animation sampling

Two architectural gaps, each independent of present timing:

### 1. No present backpressure (T2 — renderer)

Our renderer draws into an `IOSurface` and swaps it into a `CALayer` (the
`IOSurfaceLayer` path). A CALayer swap has **no vsync backpressure**: the render side can
produce frames as fast as it likes, and the window server picks up whatever surface is
current at each refresh. Frames produced between refreshes are simply discarded, and which
animation instant survives to the screen depends on when render happened to finish
relative to the (asynchronous) compositor pickup — so the surviving frames are unevenly
spaced in animation time even though they are evenly spaced in *display* time.

Ghostty instead presents through Metal with `hasVsync()` and **drawable backpressure**:
the render thread requests the next `CAMetalDrawable` and blocks until one is available,
so render can never outrun the display. The frames that reach the screen are sampled at an
even vsync cadence because render is *paced by* the drawable queue, not racing ahead of it.

**Fix direction for T2:** give present real backpressure. Either adopt a Metal
drawable-present path with vsync (block on the next drawable, matching Ghostty), or, if we
keep the IOSurface path, throttle surface swaps to the display-link cadence and only
render the frame that will actually be shown, so render samples the animation once per
refresh at even intervals instead of free-running.

### 2. Bursty io application (T4 — app / termio)

Even with perfect present timing, the *content* we present is sampled unevenly. Our io
path (two-stage gather → parse pipeline) applies the pty flood **as soon as it arrives, in
bursts**. DOOM-fire writes a full frame followed by its fps counter; each render tick we
apply whatever bytes happened to land since the last tick. Because the pipeline drains in
bursts rather than on a fixed animation clock, consecutive presented frames can be 1, then
3, then 1 animation-steps apart — visible as chunking.

Ghostty decouples this: it splits **render** (apply engine state) from **draw** (present)
with a fixed `DRAW_INTERVAL` (~8 ms / 120 fps) and **coalesces io wakeups**, so frames are
applied and drawn on an even cadence regardless of how bursty the pty delivery is.

Two upstream commits flagged by T8's drift watch (see `docs/threads/status/t4.md` inbox)
are directly on this path and worth mirroring:

- **`bb0ac4c72`** — the pipelined pty gather slept the full ~1.2 ms poll timeout *after*
  the parse stage had gone idle, doubling frame latency for request/response apps; fixed
  with a self-pipe wake. This is latency we still carry.
- **`d34b54e9b`** — renderer-mutex starvation (call sites in `termio/Exec.zig`, shared
  with T2).

**Fix direction for T4:** split render from draw with a fixed draw interval, coalesce io
wakeups so a burst of pty data produces one paced apply rather than a spurt, and mirror
`bb0ac4c72`'s self-pipe wake to cut the idle-poll latency.

## Why this is not T1

T1 owns the engine (stream/parser/print/page/pagelist). The engine already parses and
applies at or above Ghostty-main throughput on the vt lanes; DOOM-fire's ~1100 fps is far
above the display rate. Making the engine faster does not change *when* frames are sampled
— that is set by the renderer's present model (T2) and the io scheduler (T4). This doc is
the T1 → T2/T4 handoff for the smoothness work; T1's involvement was scoped to the
present-pacing fix (#139), which is done.

## How to reproduce / measure

- Eyeball: `cargo run --release` in any workspace, run DOOM-fire in the window (needs
  ≥120 cols; small font). Compare against Ghostty at the same grid.
- Numbers: `scripts/bench-doomfire.sh` (see `docs/benchmarks/doomfire.md`). Note that fps
  here is throughput, not smoothness — a high number can still judder. A smoothness metric
  (e.g. inter-presented-frame animation-step variance) would need to be added to quantify
  progress; the fps lane alone will not show it.
