# Surface.zig mining table (M2-M/N) — engine-adjacent behaviors

T5 ongoing mining doc (spec: `docs/threads/t5-vt-complete.md`). Upstream `src/Surface.zig`
pinned at `2da015cd6` (6,036 LoC; read via `git show 2da015cd6:src/Surface.zig`, not the live
worktree). Goal: find behaviors the *engine* (`qwertty-term-vt`) must expose so the app layer
can replicate Surface semantics — port engine-side pieces (T5 territory), Inbox app-side ones
(T3/T4). Companion to [stream-handler-delta.md](stream-handler-delta.md), which covers the
stream-handler side of the same seams.

Status: **session 2** — high-signal callbacks tabled (session 1) + the mouse/scroll input
pipeline, `performBindingAction` inventory, and the key-input/clipboard-read seams mined
(session 2, gate-blocked docs work). `sf:` = Surface.zig pinned line.

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

## Mouse / scroll input pipeline (session 2 mining, sf:3422-4704)

The whole mouse-input path is app/input-layer (T4), but it reads a small, precise set of
**engine** state — this pins exactly what #33 (mouse state flags) must expose and no more.

| Behavior                                                                                                                                                                                                                                       | Upstream                               | Engine state read                                                                               | Ours today                                                                                           | Owner                                            |
| ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------- | ----------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------- | ------------------------------------------------ |
| Mouse report encoding: `mouseReport` builds `mouse_encode.Options.fromTerminal(&terminal, size)` which reads **only** `flags.mouse_event` + `flags.mouse_format` (+ renderer size); the encoder (`input/mouse_encode.zig`) is pure input-layer | sf:3645-3707; `mouse_encode.zig:41-49` | `flags.mouse_event`, `flags.mouse_format` (that is the entire engine seam for mouse reports)    | missing (terminal/mod.rs:142 TODO)                                                                   | T5 engine (2 flags); T4 owns `mouse_encode` port |
| Mouse-report gate: `mouseCaptured` / `isMouseReporting` = `flags.mouse_event != .none`                                                                                                                                                         | sf:3741-3748,3654                      | `flags.mouse_event`                                                                             | missing                                                                                              | T5 flag; T4 gate                                 |
| Shift-capture resolution: `mouseShiftCapture` folds config (`never`/`always`/`false`/`true`) over `flags.mouse_shift_capture` tri-state                                                                                                        | sf:3711-3739                           | `flags.mouse_shift_capture` (**exists**, terminal/mod.rs:126 `MouseShiftCapture`)               | done                                                                                                 | T4 consumes; config via T3                       |
| Faux-scroll → cursor keys: on alt screen, `mouse_event == .none`, **and mode 1007 (`mouse_alternate_scroll`)** enabled, wheel emits `\x1bOA/B` (cursor-keys app mode) or `\x1b[A/B` per DECCKM                                                 | sf:3531-3560                           | `screens.active_key == .alternate`, `flags.mouse_event`, mode 1007, mode `cursor_keys` (DECCKM) | mode 1007 **exists** (modes.rs:153, default on); DECCKM exists; `mouse_event` missing (the only gap) | T4 wiring; T5 the one flag                       |
| Scroll clears selection when mouse-reporting active                                                                                                                                                                                            | sf:3521-3526                           | `flags.mouse_event` + selection API (exists)                                                    | blocked on flag                                                                                      | T4                                               |

**Net for #33:** the engine work is exactly two `Flags` fields — `mouse_event: MouseEvent`
(none/x10/normal/button/any) and `mouse_format: MouseFormat` (x10/utf8/sgr/urxvt/sgr_pixels) —
plus their mode side-effects (modes 9/1000/1002/1003 and 1005/1006/1015/1016) and OSC 22 for
`mouse_shape`. Everything downstream (`mouse_encode`, click-count, drag, autoscroll) is T4 and
reads only these. No larger engine surface is implied by the callbacks.

## performBindingAction inventory (sf:4775-5793) — engine primitives that keybinds consume

Skimmed for the engine primitives keybind actions call (full binding→action map is T3
territory). Engine-adjacent ones relevant to the T5 backlog:

- **Scroll actions** (`scroll_to_top`/`scroll_to_bottom`/`scroll_page_up|down`/
  `scroll_page_fractional`/`scroll_page_lines`): all resolve to `Screen`/viewport scroll
  primitives we already have (`scroll_viewport`). T3 wiring, no engine gap.
- **`jump_to_prompt`** (prev/next N): scans OSC 133 prompt zones over the screen and scrolls
  the viewport to the target prompt row — the missing `prompt`-zone navigation query flagged in
  the engine-findings table above. T5 engine primitive; T3 binds it. Pairs with `promptClickMove`.
- **`write_scrollback_file`** / **`select_all`** / clipboard actions: read existing Screen/
  selection APIs; T4/T3.
- **`reset`** (RIS-equivalent binding) → `terminal.fullReset` (exists).

No new engine gaps beyond `jump_to_prompt`/`promptClickMove` (already tabled). The binding
surface is otherwise satisfied by existing Screen/Terminal primitives.

## Corrections to the initial pass

- **Preedit dirty bit: verified present.** `flags.dirty.preedit` exists
  (terminal/mod.rs:122, `Dirty { … preedit: bool … }`). The engine-findings row's
  "dirty-bit presence unverified" is resolved — the seam is complete engine-side; preedit
  rendering itself is T2/T4.

## Key-input & clipboard-read seams (session 2 cont, sf:2605-3281, 5794-6034)

Mined the key-input entry and the OSC 52 read-reply path. The actual key *encoder*
(`input/KeyEncoder.zig`, reads `flags.modify_other_keys_2` + `kitty_keyboard` state) is T4;
Surface only gates on a few engine modes/flags — tabled here since two feed the T5 backlog.

| Behavior                                                                                                                                                                | Upstream                        | Engine state read                                             | Ours today                | Owner                                                                                                                            |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------- | ------------------------------------------------------------- | ------------------------- | -------------------------------------------------------------------------------------------------------------------------------- |
| **KAM gate:** `keyCallback` returns `.consumed` (swallows the key, no encode) when mode 2 (`disable_keyboard`, KAM) is set                                              | sf:2702                         | mode `disable_keyboard` (2, ANSI)                             | **exists** (modes.rs:117) | T5: #35 `vt-kam-allowed` is a *config-gating* toggle over this mode, NOT a missing primitive; T4 honors the gate in its key path |
| Bracketed paste: `paste.Options.fromTerminal(&terminal)` reads mode 2004; paste framed with `\e[200~`…`\e[201~`, and unsafe-paste detection scans for a stray `\e[201~` | sf:5842-5883; `input/paste.zig` | mode `bracketed_paste` (2004)                                 | **exists** (modes.rs:163) | T4 (paste encode + protection); no engine gap                                                                                    |
| Password-input concealment: `flags.password_input` toggled by a surface message; conceals input                                                                         | sf:1391-1394                    | `flags.password_input` (**exists**, terminal/mod.rs)          | done                      | T4 wiring                                                                                                                        |
| `disable_keyboard`/kitty-keyboard disable on child exit (`kitty_keyboard.set(.set,.disabled)`)                                                                          | sf:1292                         | kitty_keyboard (exists)                                       | done                      | T4                                                                                                                               |
| XTMODKEYS (`modify_other_keys_2`) consumption: read by `KeyEncoder`, not Surface directly                                                                               | `input/KeyEncoder.zig`          | `flags.modify_other_keys_2` (exists, but **never set** — #36) | flag present, unreachable | T5 #36 wires the `CSI > 4;2 m` setter; T4 owns the encoder                                                                       |

**OSC 52 read reply — exact wire format** (refines the delta-audit "OSC 52 read seam"):
`completeClipboardReadOSC52` (sf:5923) replies `\x1b]52;{kind};{base64}\x1b\\` where `kind` ∈
`{c: standard, s: selection, p: primary}` and `base64` is standard-alphabet encoding of the
clipboard bytes (empty clipboard still replies, with empty payload). Gated on `clipboard-read`
config: `deny` → no request; `ask` → requires user confirm (`error.UnauthorizedPaste` until
confirmed); `allow` → immediate. **Engine seam:** our `Handler::clipboard(kind, "?")` already
recognizes the read request and returns without queuing (stream.rs:1875) — the app must own the
OS-clipboard read + this base64 reply encode. Purely T4; the format is now pinned here so the
implementer needn't re-derive it.

- **KAM & bracketed-paste modes present.** Both `disable_keyboard` (2) and `bracketed_paste`
  (2004) exist in `modes.rs` — so #35's `vt-kam-allowed` and any paste work are config/wiring,
  not new engine state.

## Not yet mined (next sessions)

| Range                         | Lines          | Expected yield                                        |
| ----------------------------- | -------------- | ----------------------------------------------------- |
| `handleMessage` detail        | 960-1713       | per-message app seam specs (partially skimmed)        |
| `cursorPosCallback` detail    | 4512-4704      | drag selection, autoscroll (mouse-report path tabled) |
| `dumpText` / `getProcessInfo` | 1905-2034,6034 | test/dump helpers, process info seams                 |
