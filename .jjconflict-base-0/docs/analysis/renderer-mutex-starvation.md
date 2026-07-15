# Renderer-state mutex starvation (upstream `d34b54e9b`) — port analysis

**Disposition: does not structurally apply to the port. No code change recommended.**
The port's snapshot boundary already removes the precondition upstream's fix targets;
the residual risk is theoretical and is empirically not manifesting. This document is
the resolution of T8 drift item `d34b54e9b` for the `qwertty-term-renderer` (T2) and
`qwertty-term-termio` (T4) territories.

## The upstream bug

Upstream commit `d34b54e9b` ("renderer: hand off state mutex to avoid starving frames"):

- The renderer holds a single shared `renderer_state.mutex` for the **entire**
  `updateFrame` — the heavy per-frame rebuild that reads terminal state
  (`src/renderer/generic.zig:1170`, `state.mutex.lock()` … `defer unlock`).
- The termio parse thread relocks that **same** mutex once per output batch
  (`processOutput` under the lock) and never sleeps between batches
  (`src/termio/Exec.zig:1486,1776`).
- Every mutex the platforms use is *unfair* (`os_unfair_lock` on macOS, a futex lock
  elsewhere): a running thread that unlocks and immediately relocks beats a sleeping
  waiter, because the waiter must first be woken and scheduled. So under sustained pty
  output the parse thread wins the mutex race indefinitely and the renderer **starves
  in `updateFrame` for as long as the output lasts** — dropped frames during a big
  `cat`/paste/animation.

The fix adds two atomics to `renderer.State` — a waiter count (`demand`) and a handoff
generation (`handoff_gen`) — and three methods: the renderer takes the mutex via
`lockDemand`/`unlockDemand`; the parse thread calls `yieldToDemand` between batches,
which futex-sleeps (1 ms timeout) if a demanding waiter exists so the renderer gets its
turn. The atomics are all monotonic — a scheduling hint only; the mutex still protects
the state.

## The port's architecture is different (case b)

The port does **not** hold the shared lock across the heavy per-frame work. The shared
lock is `Arc<Mutex<Engine>>` (`std::sync::Mutex`; `Engine` wraps the vt `Terminal`), and
rendering runs off an **owned `FullSnapshot`** captured under a brief lock:

### Render side — lock held only for the snapshot copy

`crates/qwertty-term/src/app.rs:653-715`, `Surface::render` (runs on the main-thread
~60 Hz pace tick, not a dedicated renderer thread):

```rust
let (mut window, range) = {
    let mut engine = self.engine();                              // LOCK ACQUIRED
    let range = engine.selection()…screen_range(…);
    let window = engine.snapshot_window_tracking(scrollback);    // copy state out
    (window, range)
};                                                               // LOCK RELEASED (guard dropped)
// … tint_selection / tint_matches / dim_window — on the owned `window`, no lock
let snapshot = FullSnapshot::from_window(window);                // no lock
render.update_frame(&snapshot, &mut grid, opts);                // HEAVY — no lock
render.sync_atlas(…);                                           // no lock
render.draw_and_present(host_layer);                            // GPU — no lock
```

The `Engine` lock is held only across the `snapshot_window_tracking` copy (which also
clears dirty tracking) — microseconds. `from_window`, `update_frame`, `sync_atlas`, and
`draw_and_present` all run **after** the guard drops. This is the snapshot boundary that
upstream's monolithic `updateFrame` lacks.

### Parse side — same shape as upstream, but contends against a tiny critical section

The `processOutput` equivalent is the sink closure at
`crates/qwertty-term/src/termio.rs:191-207`, driven by the parse loop
`reader_main` at `crates/qwertty-term-termio/src/exec.rs:866-910`:

```rust
loop {
    let slot = { … batch_ready.wait(meta) … };   // parks ONLY when the ring is empty
    sink(batch);                                  // engine.lock() → e.write(batch) → unlock
    meta.tail = (meta.tail + 1) % BUFFER_COUNT;
}
```

So the parse thread *does* relock per batch with no sleep between batches (the upstream
half is present). The difference is the other side of the contention: it now races only
a **microsecond snapshot copy** once per frame, not a whole `updateFrame`.

The codebase already documents this as the intended design
(`crates/qwertty-term-termio/src/hub.rs:27-33`):

> Upstream drives the parse sink under `renderer_state.mutex`. The R5 app has no render
> mutex (single-threaded main loop), so the app supplies a sink that locks its own
> `Arc<Mutex<Engine>>` … the same "apply behind the lock the renderer also takes"
> design, with the app's pace-tick standing in for upstream's renderer thread.

## Residual risk (why "largely", not "entirely", unnecessary)

`std::sync::Mutex` is unfair on all platforms and the port has **no `parking_lot`**
(which would give eventual fairness for free). So in principle the main-thread pace tick
could still be delayed acquiring the lock for its snapshot copy while the parse thread
relocks tightly. But this is a much weaker failure mode than upstream's:

- The contended critical sections are now **symmetric and both tiny** (snapshot copy vs.
  `write(batch)`), so the pace tick loses at most a few batch cycles per acquire, not a
  whole frame's worth of held lock. Worst case is an occasional late/dropped frame, not a
  multi-second freeze.
- The pace tick only needs the lock ~60×/s; the OS scheduler does eventually run the
  waiter. There is no unbounded hold on the renderer side to amplify the unfairness.

### Empirical check: it is not manifesting

DOOM-fire is the canonical "sustained heavy pty output" workload — exactly what triggers
upstream's bug. On current main it renders **visibly smooth (~950–1120 fps, no judder)**
(Josh's manual check, 2026-07-13; see also `docs/analysis/doomfire-smoothness.md` and the
`present_stats.rs` metric seam gated behind `QWERTTY_TERM_PRESENT_STATS`). If the pace
tick were starving on lock acquisition under heavy output, DOOM-fire would judder. It
does not.

## Options if it ever manifests

Ordered by cost. Recommendation is **(0) do nothing now** and revisit only if the present
metric shows judder that correlates with heavy pty output.

0. **Do nothing.** The snapshot boundary is the mitigation; evidence says it holds.
1. **Eventual-fairness mutex (cheap insurance).** Swap `std::sync::Mutex<Engine>` →
   `parking_lot::Mutex<Engine>`. parking_lot does a fair handoff after ~0.5 ms of
   contention, which defeats exactly this unfair-relock starvation, at ~zero steady-state
   cost. Bonus: parking_lot mutexes don't poison, simplifying `app.rs:358 lock_or_recover`.
   Touches the app crate's lock type only; no atomics, no parse-loop change. This is the
   proportionate fix if the residual risk is ever observed.
2. **Full `lockDemand`/`yieldToDemand` port.** Mirror upstream exactly: demand +
   handoff-generation atomics on the shared lock, `lockDemand` around the snapshot copy
   (`app.rs`), `yieldToDemand` between batches in the parse loop (`exec.rs`, T4). This is
   the strongest guarantee but adds cross-crate complexity (the shared lock lives in the
   **app** crate, not the renderer, so it is not even T2-core) for a bug the architecture
   already prevents. Not warranted on current evidence.

## Ownership / coordination

The shared lock lives in the **app** crate (`qwertty-term/src/app.rs`, `termio.rs`), not
in `qwertty-term-renderer`. The parse loop is **T4** (`qwertty-term-termio/src/exec.rs`).
So a real fix (option 1 or 2) would be app-level + T4, coordinated — not a T2-core change.
A T4 coordination note is filed in `docs/threads/status/t4.md`. No action is requested of
T4 now; this records that the termio `yieldToDemand` half is unnecessary given the
snapshot boundary, and flags option 1 as the cheap lever if IO-side heavy-output stalls
are ever observed.
