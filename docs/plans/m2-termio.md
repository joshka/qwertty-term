# Plan: M2 daily-drivable (termio + input completion)

Zig refs at `2da015cd6`. Spine: A → B/C → D → E → M/N; input track independent until M.
Sizing detail lives in the termio/input discovery report (2026-07-07); key excerpts inline.

## Decisions (locked)

1. **PTY via rustix** (`rustix::pty` + `rustix::termios`), not portable-pty — we need
   termios polling (200ms password detection), IUTF8, and exact resize semantics. The spike
   keeps portable-pty until chunk E swaps it out.
2. **The tokio question is settled by chunk C, not by taste.** Build Thread.zig's exact
   semantics twice — (a) OS thread + mio/polling + timer wheel, (b) tokio current-thread —
   drive both with the same synthetic load (mailbox floods, resize-coalesce 25ms timer,
   sync-output 1s timeout), measure wakeup latency p50/p99 + idle CPU, write the ADR.
   Whichever wins, the MAILBOX API stays identical (Exec/Termio code is independent of the choice).
3. **Preserve the backpressure-unlock trick**: upstream unlocks the renderer mutex while
   blocking on a full write queue (deadlock avoidance). This must exist explicitly in the
   Rust design — a naive bounded-channel send while holding the render lock deadlocks.
4. **Exec's two-stage read pipeline ports as-is**: io-reader thread + io-gather thread over
   rotating ring buffers (one mutex+condvar). Do not "simplify" to a single reader — the
   design bridges kernel refill gaps; benchmark any deviation against `seq`/`cat` floods.
5. **Shell integration scripts copy VERBATIM** from upstream src/shell-integration/ (they're
   shell code, not Zig); only the injection logic (env, XDG dirs) is ported.
6. **Surface.zig lands last and adapter-first**: it has zero upstream tests. Port it against
   the existing spike Engine seam so the headless engine_pty E2E harness keeps verifying
   behavior; decompose internally (render-coord / input-routing / clipboard / lifecycle)
   only after it works.

## Chunks

| #   | Chunk                                      | Model                              | Zig LoC | Notes                                                                                  |
| --- | ------------------------------------------ | ---------------------------------- | ------- | -------------------------------------------------------------------------------------- |
| A   | PTY primitive                              | Sonnet                             | 546     | rustix; real-shell winsize/termios tests                                               |
| B   | plumbing (Options/message/mailbox/backend) | Sonnet                             | 384     | message union 1:1; backpressure-unlock in mailbox                                      |
| C   | runtime spike + ADR                        | Opus                               | 531     | see decision 2; timeboxed; docs/adr/                                                   |
| D   | Exec                                       | Opus, priority ladder              | 2,143   | after C; the XL; 11 upstream tests                                                     |
| E   | Termio hub + spike swap to real PTY        | Opus                               | 800     | retires portable-pty                                                                   |
| F   | stream_handler delta                       | Sonnet                             | 1,577   | most exists in ghostty-vt stream; port the mailbox-effect delta only (enumerate first) |
| G   | shell integration                          | Sonnet                             | 1,032   | after D; 20 upstream tests                                                             |
| J   | legacy key encode                          | Opus                               | ~2,100  | fills the seam input-encode left; 92-test file                                         |
| M   | Surface.zig single-surface                 | Opus, multi-pass like Terminal.zig | 6,036   | decision 6; join point                                                                 |
| N   | App slice + surface_mouse                  | Sonnet                             | ~860    | parallel with M                                                                        |

Exit: Josh uses the window for an hour. Hidden acceptance: `seq 1 100000` smooth, vim/tmux
mouse+keys correct (kitty + legacy), paste safe, OSC133 prompts marked (G), exit/restart
clean.
