# Surface.zig mining table (M2-M/N) — engine-adjacent behaviors

T5 ongoing mining doc (spec: `docs/threads/t5-vt-complete.md`). Upstream `src/Surface.zig`
pinned at `2da015cd6` (6,036 LoC; read via `git show 2da015cd6:src/Surface.zig`, not the live
worktree). Goal: find behaviors the *engine* (`qwertty-term-vt`) must expose so the app layer
can replicate Surface semantics — port engine-side pieces (T5 territory), Inbox app-side ones
(T3/T4). Companion to [stream-handler-delta.md](stream-handler-delta.md), which covers the
stream-handler side of the same seams.

Status: **initial pass** — the high-signal callbacks are tabled; the ranges under
"Not yet mined" remain for later sessions. `sf:` = Surface.zig pinned line.

## Engine-adjacent findings

| Behavior                                                                                                                                                        | Upstream                                       | Engine primitive needed                                                                                   | Ours today                                                              | Owner                                     |
| --------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------- | --------------------------------------------------------------------------------------------------------- | ----------------------------------------------------------------------- | ----------------------------------------- |
| Mouse shape protocol: shape from `terminal.mouse_shape` pushed to apprt on change, on message (`sf:1034`), key events (`sf:2745`), cursor pos (`sf:4532`)       | sf:1034,2741-2769,4532                         | `terminal.mouse_shape: MouseShape` state + OSC 22 string→enum parse                                       | missing (stream-handler-delta finding 6)                                | T5 engine; T4 consumes                    |
| Mouse reporting gate: every mouse path branches on `flags.mouse_event` / `flags.mouse_format` (`sf:2726,2761,3534,3642,3654,3744,4371,4604`)                    | sf:2726+                                       | `flags.mouse_event`, `flags.mouse_format` set by modes 9/1000/1002/1003 and 1005/1006/1015/1016           | missing (terminal/mod.rs:142 TODO)                                      | T5 engine; T4 input encodes reports       |
| Focus: `flags.focused` updated on focus change; focus reports (mode 1004) sent via io message                                                                   | sf:3304-3391                                   | `flags.focused` (exists, terminal/mod.rs:145) + focus-report encoder                                      | flag done; report emission missing (app seam per delta finding 11)      | T4 wiring; encoder T5                     |
| Color-scheme report (mode 2031 / DSR ?996): on OS theme change, queue `color_scheme_report` unless unchanged; force=false respects mode 2031                    | sf:4705-4724                                   | light/dark seam + `\e[?997;Nn` encoder (device_status.zig)                                                | missing (delta finding 4)                                               | T5 encoder + seam; T4 theme source        |
| In-band size reports (mode 2048) + resize: sizeCallback resizes terminal and sends size report when enabled                                                     | sf:2467-2525                                   | size-report encoders (`size_report.zig` styles: mode 2048, CSI 14/16/18 t) taking a pixel-geometry struct | missing (delta finding 5; XTWINOPS backlog)                             | T5 encoders; T4 supplies geometry         |
| promptClickMove: click-in-prompt → count of left/right arrow presses computed from OSC 133 zones, arrows chosen by `cursor_keys` mode                           | sf:4240-4266 (uses `screen.promptClickMove`)   | `Screen::prompt_click_move(pin) -> {left, right}`                                                         | missing; no equivalent in screen/mod.rs                                 | T5 engine (backlog item); T4 click wiring |
| jump_to_prompt / scroll-to-prompt binding: viewport scroll by prompt zones                                                                                      | performBindingAction sf:4775+ (unmined detail) | prompt-zone scan over semantic-prompt rows (`scroll_viewport` variant or row query)                       | partial — semantic prompt rows stored; navigation query missing         | T5 engine; T3 binding                     |
| Selection word semantics: double-click select uses `selection_word_chars` config as boundary codepoints (`sf:1197,3976,4090,4482,4673`)                         | sf:320,399                                     | `Screen::select_word(pin, boundary_codepoints)`                                                           | **done** (screen/mod.rs:1524) — expose via engine API + document for T4 | T4 consume; T5 doc only                   |
| Kitty keyboard disable on message (`sf:1292`): app force-disables kitty keyboard (child exit path)                                                              | sf:1292                                        | `kitty_keyboard.set(.set, .disabled)` — exists                                                            | done (screen kitty_key)                                                 | T4 wiring                                 |
| Preedit interplay: preedit lives in renderer state, but sets `terminal.flags.dirty.preedit`; IME position from cursor + preedit width (`imePoint` sf:2090-2140) | sf:2526-2601,2090                              | `flags.dirty.preedit` dirty bit; cursor position query (exists)                                           | dirty-bit presence unverified — check `Flags::dirty` when gate opens    | T5 verify; T2/T4 own preedit rendering    |
| OSC 52 read completion: `completeClipboardRequest` encodes base64 reply `\e]52;kind;<b64>\e\\` back to pty                                                      | sf:5794+                                       | reply encoder for clipboard read (pairs with delta finding: OSC 52 read seam)                             | missing                                                                 | T5 encoder; T4 clipboard I/O              |
| Scroll-wheel → arrow keys on alt screen when mouse reporting off (`faux scrolling`), gated on `flags.mouse_event == .none`                                      | sf:3534+                                       | needs mouse_event flags (above) + alt-screen query (exists)                                               | blocked on mouse flags                                                  | T4; T5 flags                              |

## App-side observations (Inbox candidates, not engine work)

- `handleMessage` (sf:960-1713) is the consumer catalog for every stream-handler surface
  message: `set_title`, `pwd_change`, `set_mouse_shape` (sf:1034), `clipboard_read/write`,
  `color_change`, `report_title`, `ring_bell`, `progress_report`, `desktop_notification`,
  `start/stop_command`. Our app has no equivalent bus — it polls engine state. Each delta-audit
  "app seam" finding should name its `handleMessage` arm when Inboxed to T4.
- `focusCallback` (sf:3304) also synthesizes key-release events on focus loss (pressed-key
  cleanup) — pure input-layer behavior, T4.
- `colorSchemeCallback` (sf:4705) keeps a `config_conditional_state.theme` for conditional
  config — T3 adjacency.

## Not yet mined (next sessions)

| Range                         | Lines          | Expected yield                                          |
| ----------------------------- | -------------- | ------------------------------------------------------- |
| `handleMessage` detail        | 960-1713       | per-message app seam specs (partially skimmed)          |
| `keyCallback` + encoding      | 2605-3281      | kitty keyboard encode paths, modify_other_keys use, IME |
| `scrollCallback`              | 3392-3602      | scroll units, mouse wheel reports, faux-scroll detail   |
| `mouseButtonCallback`         | 3741-4447      | report encoding, click-count selection, link click      |
| `cursorPosCallback`           | 4512-4704      | motion reports, drag selection, autoscroll              |
| `performBindingAction`        | 4775-5793      | full binding→engine-primitive inventory (T3 overlap)    |
| `dumpText` / `getProcessInfo` | 1905-2034,6034 | test/dump helpers, process info seams                   |
