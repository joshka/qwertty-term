# Stream-handler delta audit (`termio/stream_handler.zig` vs our stream/TerminalHandler)

T5 Phase-A audit (M2-F). Upstream pinned at commit `2da015cd6`
(`git -C ~/local/ghostty show 2da015cd6:src/termio/stream_handler.zig`, 1,577 LoC — note the
live checkout may be on a different commit; this audit reads the pinned blob, not the worktree).
Our side audited at main `ec4f0753` in `crates/qwertty-term-vt/src/stream.rs` (Handler trait +
`TerminalHandler`).

## Scope and the three-layer picture

Upstream has **two** concrete stream handlers, and the delta only makes sense against both:

- `src/termio/stream_handler.zig` (**termio**, the audit target): the app-side handler. Owns
  config-derived state (enquiry response, OSC color-report format, clipboard policy, default
  cursor style/blink), writes replies to the termio mailbox, and surfaces UI effects
  (bell, title, clipboard, notifications, mouse shape) to the surface mailbox.
- `src/terminal/stream_terminal.zig` (**lib**, same commit, 2,249 LoC): the embeddable default
  handler. Pure terminal-state updates plus optional `Effects` callbacks (`write_pty`, `bell`,
  `color_scheme`, `device_attributes`, `enquiry`, `size`, `title_changed`, `pwd_changed`,
  `xtversion`). With `.readonly` effects it answers *nothing*.
- **Ours**: `Stream<H: Handler>` routes parser actions onto a trait
  ([stream.rs:245](../../crates/qwertty-term-vt/src/stream.rs)); `TerminalHandler`
  ([stream.rs:1596](../../crates/qwertty-term-vt/src/stream.rs)) is the concrete impl. It sits
  **between** the two upstream layers: lib-style state updates, plus a built-in reply queue
  (`output`) that answers queries the lib layer only answers via effects. The app
  (`crates/qwertty-term/src/engine.rs`) polls state (`get_title`, `get_pwd`) and drains
  `take_output()` / `take_clipboard()` rather than implementing its own Handler.

Status legend: **done** = semantics match the applicable upstream layer(s); **partial** = core
effect present but a documented piece is missing or diverges; **missing** = action parsed (or
parseable) but has no effect on our side. "app seam" = the missing piece is upstream app-layer
behavior that needs a surfacing mechanism (T3/T4 territory), not vt-crate state.

Line references: `sh:` = `termio/stream_handler.zig` (pinned), `st:` = `stream_terminal.zig`
(pinned), `stream.rs:` = ours. The termio `vtFallible` switch (sh:203-368) enumerates the full
`Action.Tag` set, so this table is exhaustive over stream actions.

## Action-by-action table

### Printing, C0, cursor motion

| Action                           | Upstream   | Ours (stream.rs) | Status | Notes                                                                                                    |
| -------------------------------- | ---------- | ---------------- | ------ | -------------------------------------------------------------------------------------------------------- |
| `print`                          | sh:204     | 1597             | done   |                                                                                                          |
| `print_slice`                    | sh:208     | 1600             | done   |                                                                                                          |
| `print_repeat`                   | sh:212     | 1982             | done   |                                                                                                          |
| `backspace`                      | sh:214     | 1604             | done   |                                                                                                          |
| `horizontal_tab`                 | sh:215,590 | 1666             | done   | same stop-if-no-motion loop                                                                              |
| `horizontal_tab_back`            | sh:216,598 | 1675             | done   | same loop                                                                                                |
| `linefeed`                       | sh:217,606 | 1610             | done   | ours calls `Terminal::linefeed`                                                                          |
| `carriage_return`                | sh:221     | 1607             | done   |                                                                                                          |
| `index`                          | sh:269,616 | 1613             | done   |                                                                                                          |
| `next_line`                      | sh:270,620 | 1616             | done   | index + CR                                                                                               |
| `reverse_index`                  | sh:271,612 | 1620             | done   |                                                                                                          |
| `cursor_up/down/left/right`      | sh:227-230 | 1626-1637        | done   |                                                                                                          |
| `cursor_pos`                     | sh:231     | 1638             | done   |                                                                                                          |
| `cursor_col` / `cursor_row`      | sh:235-236 | 1641,1645        | done   |                                                                                                          |
| `cursor_col_relative`            | sh:237     | 1649             | done   | upstream saturating add; ours usize add — cannot overflow at u16 params, verify vs resize property tests |
| `cursor_row_relative`            | sh:241     | 1654             | done   | same note                                                                                                |
| `save_cursor` / `restore_cursor` | sh:294-295 | 1659,1662        | done   | DECSC/DECRC + ESC 7/8                                                                                    |

### Erase / edit / scroll / tabs

| Action                                          | Upstream   | Ours (stream.rs) | Status  | Notes                                                                                                                                                                         |
| ----------------------------------------------- | ---------- | ---------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `erase_display_below/above`                     | sh:246-247 | 1694             | done    |                                                                                                                                                                               |
| `erase_display_complete`                        | sh:248     | 1694             | partial | termio scrolls viewport to bottom first (sh:249); lib does not (st:188). Ours does not. `Terminal::scroll_viewport` exists (terminal/mod.rs:844) — decide layer + corpus case |
| `erase_display_scrollback`                      | sh:252     | 1694             | done    |                                                                                                                                                                               |
| `erase_display_scroll_complete`                 | sh:253     | 1694             | done    | ED 22                                                                                                                                                                         |
| `erase_line_*` (4 forms)                        | sh:254-257 | 1697             | done    | incl. `right_unless_pending_wrap`                                                                                                                                             |
| `delete_chars` / `erase_chars`                  | sh:258-259 | 1700,1703        | done    |                                                                                                                                                                               |
| `insert_lines/blanks`, `delete_lines`           | sh:260-262 | 1706-1714        | done    |                                                                                                                                                                               |
| `scroll_up` / `scroll_down`                     | sh:263-264 | 1715,1718        | done    |                                                                                                                                                                               |
| `tab_clear_current/all`, `tab_set`, `tab_reset` | sh:265-268 | 1684-1692        | done    | CTC `CSI ? 5 W` reset included                                                                                                                                                |

### Modes and margins

| Action                                           | Upstream           | Ours (stream.rs) | Status  | Notes                                                                                                                                                   |
| ------------------------------------------------ | ------------------ | ---------------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `set_mode` / `reset_mode` (core)                 | sh:273-274,654     | 1722             | partial | raw mode bit + side effects below                                                                                                                       |
| — `origin`                                       | sh:711             | 1564             | done    | cursor to 1,1                                                                                                                                           |
| — `enable_left_and_right_margin`                 | sh:713             | 1565             | done    | reset margins on disable                                                                                                                                |
| — alt screens (47/1047/1049)                     | sh:720-730         | 1571-1582        | done    | via `switch_screen_mode`                                                                                                                                |
| — `save_cursor` (1048)                           | sh:736             | 1583             | done    |                                                                                                                                                         |
| — `132_column` (DECCOLM)                         | sh:756             | 1590             | done    |                                                                                                                                                         |
| — `reverse_colors` dirty flag                    | sh:706             | —                | partial | termio sets `flags.dirty.reverse_colors`; lib no-op (st:363). Verify our renderer picks up mode 5 changes; add corpus/render case                       |
| — `autorepeat` (DECARM)                          | sh:703             | —                | done    | deliberately inert in both                                                                                                                              |
| — `enable_mode_3` resize                         | sh:745             | —                | missing | app seam: needs grid size (lib no-op st:390)                                                                                                            |
| — `synchronized_output` (2026) timer             | sh:763             | —                | missing | app seam: start timeout on enable                                                                                                                       |
| — `linefeed` mode (20) message                   | sh:767             | —                | missing | app seam: input layer must map CR→CRLF                                                                                                                  |
| — `in_band_size_reports` (2048)                  | sh:771             | —                | missing | app seam: send initial size report on enable                                                                                                            |
| — `focus_event` (1004) initial report            | sh:775             | —                | missing | app seam: report current focus on enable                                                                                                                |
| — `mouse_event_x10/normal/button/any` flags      | sh:779-814         | —                | missing | engine state: `flags.mouse_event` not ported (terminal/mod.rs:142 TODO); lib sets it (st:560-587). Also termio sets mouse shape default/text (app seam) |
| — `mouse_format_utf8/sgr/urxvt/sgr_pixels`       | sh:816-819         | —                | missing | engine state: `flags.mouse_format` not ported                                                                                                           |
| — `cursor_blinking` (12) config interplay        | sh:688             | —                | missing | app/config seam: termio ignores mode 12 when `cursor-style-blink` configured                                                                            |
| `save_mode` / `restore_mode`                     | sh:275-282         | 1726,1729        | done    | restore re-runs side effects, same as upstream                                                                                                          |
| `request_mode` / `request_mode_unknown` (DECRQM) | sh:283-284,633-652 | 1931,1940        | done    | reply via mode report encode                                                                                                                            |
| `top_and_bottom_margin`                          | sh:285             | 1733             | done    |                                                                                                                                                         |
| `left_and_right_margin`                          | sh:286             | 1737             | done    |                                                                                                                                                         |
| `left_and_right_margin_ambiguous`                | sh:287             | 1741             | done    | DECSLRM-vs-SC disambiguation                                                                                                                            |

### Charsets, attributes, protected mode, status display

| Action                                | Upstream   | Ours (stream.rs) | Status  | Notes                                                                                                                                      |
| ------------------------------------- | ---------- | ---------------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------ |
| `invoke_charset`                      | sh:226     | 1762             | done    | SO/SI/SS2/SS3/LS*                                                                                                                          |
| `configure_charset`                   | sh:339,960 | 1749             | done    |                                                                                                                                            |
| `set_attribute`                       | sh:340     | 1771             | done    | unknown SGR ignored, like upstream                                                                                                         |
| `protected_mode_off/iso/dec`          | sh:297-299 | 1779             | done    | DECSCA + SPA/EPA                                                                                                                           |
| `active_status_display`               | sh:329     | 1782             | done    | DECSASD                                                                                                                                    |
| `modify_key_format` (XTMODKEYS)       | sh:296,625 | —                | missing | `CSI > Pp;Pv m` dispatch not modeled (stream.rs:1118 comment); `flags.modify_other_keys_2` exists (terminal/mod.rs:141) but is unreachable |
| `mouse_shift_capture` (XTSHIFTESCAPE) | sh:300     | 2021             | done    |                                                                                                                                            |

### Cursor style, reset

| Action                    | Upstream   | Ours (stream.rs) | Status  | Notes                                                                                                                                                                                                             |
| ------------------------- | ---------- | ---------------- | ------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `cursor_style` (DECSCUSR) | sh:245,894 | 1793             | partial | mapping matches lib defaults (block, blink=false; st:46-49). termio resets to *config* default style/blink (sh:902-908) — needs engine setters for default style/blink (config seam, T3)                          |
| `full_reset` (RIS)        | sh:272,968 | 1789             | partial | engine reset done. lib also re-asserts default cursor style/blink post-reset (st:250-254) — verify ours matches via corpus. termio extras (mouse shape → text, color-scheme report, progress clear) are app seams |
| `decaln` (DECALN)         | sh:330,943 | 1786             | done    |                                                                                                                                                                                                                   |

### Reports and queries (reply-emitting)

| Action                                       | Upstream    | Ours (stream.rs) | Status  | Notes                                                                                                                                                                                                                    |
| -------------------------------------------- | ----------- | ---------------- | ------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| `enquiry` (ENQ)                              | sh:225,955  | 1624             | missing | termio replies configured answerback; lib via effect (st:365). Ours: no-op, no setter. Backlog: `enquiry-response`                                                                                                       |
| `device_attributes` primary                  | sh:840      | 1888             | partial | termio: `?62;22;52c`, dropping `52` when clipboard write denied. Ours: always `?62;22c`. Needs clipboard-policy seam                                                                                                     |
| `device_attributes` secondary                | sh:850      | 1898             | partial | termio hardcodes `>1;10;0c`; lib/oracle default `>1;0;0c` (ours matches lib; corpus-pinned). App should wire version via seam                                                                                            |
| `device_attributes` tertiary                 | sh:854      | 1900             | partial | termio does *not* reply (logs unimplemented); ours replies DECRPTUI with empty unit id. Divergence to resolve with corpus vs oracle                                                                                      |
| `device_status` 5 (operating)                | sh:863      | 1905             | done    | `\e[0n`                                                                                                                                                                                                                  |
| `device_status` 6 (CPR)                      | sh:865      | 1906             | done    | origin-mode-relative, same saturating subtraction                                                                                                                                                                        |
| `device_status` `?6`                         | —           | 439              | partial | upstream `reqFromInt` rejects `?6` (device_status.zig entries: `cursor_position` question=false); ours accepts `(6, _)` and replies. We are *more* permissive — corpus case + fix to match                               |
| `device_status` `?996` (color scheme)        | sh:890      | —                | missing | termio → `color_scheme_report` message; lib via `color_scheme` effect → `\e[?997;N n`. No dispatch on our side (`from_int` has no 996). Needs seam + mode 2031 story                                                     |
| `kitty_keyboard_query`                       | sh:305,981  | 1987             | done    | `\e[?Nu`                                                                                                                                                                                                                 |
| `kitty_keyboard_push/pop/set/set_or/set_not` | sh:306-325  | 1994-2009        | done    |                                                                                                                                                                                                                          |
| `size_report` (XTWINOPS 14/16/18/21)         | sh:301,1472 | —                | missing | dropped at dispatch (stream.rs:1248); no Handler method. 14/16/18 need pixel-geometry seam (lib `size` effect, st:76-79); 21 is `\e]l<title>\e\\` from `get_title` (st:735) — engine-answerable today. Backlog: XTWINOPS |
| `xtversion` (XTVERSION)                      | sh:302,996  | 1977             | partial | ours hardcodes the `libghostty` DCS reply (lib default, st:707). Needs configurable string; product string must be `qwertty-term …`, never "ghostty", when app wires it                                                  |

### OSC-driven actions

| Action                                                                    | Upstream           | Ours (stream.rs) | Status      | Notes                                                                                                                                                                                                                      |
| ------------------------------------------------------------------------- | ------------------ | ---------------- | ----------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `window_title` (OSC 0/2)                                                  | sh:331,1016        | 1812             | partial     | ours = lib (truncate 1024, st:757). termio: rejects ≥256, empty title → pwd-or-blank + `seen_title` tracking (app seam; app polls `get_title`)                                                                             |
| `report_pwd` (OSC 7)                                                      | sh:332,1131        | 1821             | partial     | ours = lib (store raw, truncate 4096, st:781). termio's URI parse + local-hostname validation is app-layer; our app's `pwd_path_from_osc7` (engine.rs:404) parses but does **not** validate hostname locality — Inbox T4   |
| `semantic_prompt` (OSC 133)                                               | sh:337,1095        | 1826             | partial     | engine effect done. termio's `start_command`/`stop_command`+exit-code surfacing (sh:1100-1114) is an app seam (needed for `jump_to_prompt`/click-move backlog)                                                             |
| `start_hyperlink` / `end_hyperlink` (OSC 8)                               | sh:335,328,825-831 | —                | **missing** | parsed (osc/mod.rs:131-133) and Screen fully supports it (`start_hyperlink` screen/mod.rs:851, `end_hyperlink` :918) but osc_dispatch drops both (stream.rs:1438). Wiring-only fix + corpus cases. Highest-value quick win |
| `mouse_shape` (OSC 22)                                                    | sh:338,1053        | 1872             | missing     | no `terminal.mouse_shape` state, no string→enum parse; upstream stores + notifies surface. Backlog: OSC 22                                                                                                                 |
| `clipboard_contents` (OSC 52)                                             | sh:336,1065        | 1875             | partial     | write path queued (`take_clipboard`); read (`?`) request not surfaced (upstream → `clipboard_read` message). App seam                                                                                                      |
| `color_operation` set/reset/reset_palette (OSC 4/5/10-19/104/105/110-119) | sh:327,1230        | 1829             | done        | matches lib (st:597) incl. palette dirty flag + mask-based `reset_palette`                                                                                                                                                 |
| `color_operation` **query**                                               | sh:1361-1441       | 1863             | missing     | `ColorRequest::Query` ignored — no OSC 4/10/11/12 query replies. Needs `osc_color_report_format` option (none/8-bit/16-bit) + terminator echo. Backlog: VT config toggles                                                  |
| `kitty_color_report` (OSC 21)                                             | sh:326,1481        | 1867             | missing     | handler is a stub. Set/reset effects (lib st:672) + query replies (termio sh:1489). Ties to our upstream finding issue-3 (OSC 21 query reply); implement per kitty spec, note upstream divergence in corpus                |
| `show_desktop_notification` (OSC 9/777)                                   | sh:333,1453        | —                | missing     | dropped at osc_dispatch (stream.rs:1446); no seam. App seam (T4)                                                                                                                                                           |
| `progress_report` (OSC 9;4)                                               | sh:334,1574        | —                | missing     | parsed (`ConemuProgressReport`) but dropped; no seam. App seam (T4)                                                                                                                                                        |

### DCS / APC / tmux

| Action                        | Upstream           | Ours (stream.rs) | Status  | Notes                                                                                                                           |
| ----------------------------- | ------------------ | ---------------- | ------- | ------------------------------------------------------------------------------------------------------------------------------- |
| `dcs_hook/put/unhook` routing | sh:358-360,372-388 | 750-768          | done    | via `dcs::Handler`                                                                                                              |
| DCS `decrqss` SGR             | sh:491             | 1955             | done    | `printAttributes` + `m`                                                                                                         |
| DCS `decrqss` DECSTBM         | sh:515             | 1957             | done    |                                                                                                                                 |
| DCS `decrqss` DECSLRM         | sh:522             | 1963             | partial | upstream replies only when DECLRMM enabled; ours replies unconditionally                                                        |
| DCS `decrqss` DECSCUSR        | sh:501             | 1969             | missing | upstream replies `{style} q` derived from cursor style + blink; ours sends nothing                                              |
| DCS `decrqss` invalid/none    | sh:483-541         | 1971             | missing | upstream *always* replies, `\eP0$r\e\\` when unrecognized; ours stays silent                                                    |
| DCS `xtgettcap` (XTGETTCAP)   | sh:467             | 777              | missing | parsed (`dcs::Command::XtGetTcap`) then dropped (stream.rs:781); needs terminfo capability map (not ported). Backlog: XTGETTCAP |
| DCS `tmux` control mode       | sh:393-465         | —                | missing | seamed (`dcs.rs` TmuxRaw); **Josh-gated**, XL, last                                                                             |
| `apc_start/put/end`           | sh:361-363,548     | 2031-2071        | done    | kitty graphics parse/execute/reply                                                                                              |
| APC glyph protocol            | sh:567             | 2069             | missing | needs font subsystem (`TODO(chunk:font-glyph-protocol)`); T2/T6 adjacency                                                       |

### Intentional no-ops (parity)

| Action                                      | Upstream   | Ours (stream.rs) | Status | Notes                                                                                                                             |
| ------------------------------------------- | ---------- | ---------------- | ------ | --------------------------------------------------------------------------------------------------------------------------------- |
| `title_push` / `title_pop` (XTWINOPS 22/23) | sh:366-368 | 2012,2016        | done   | both upstream handlers no-op; the real 10-deep stack lives in apprt (Surface mining will place it). Backlog: XTWINOPS title stack |
| `bell`                                      | sh:213,586 | 1623             | done   | lib-readonly parity; app seam for T4 (ring bell surfacing)                                                                        |

## Findings summary (ranked)

1. **OSC 8 hyperlinks unwired** — parsed, Screen implements the whole feature, dispatch drops
   it. Two-line wiring + trait methods + corpus cases. Do first when the vt gate opens.
2. **DECRQSS incomplete** — no invalid response, no DECSCUSR reply, DECSLRM unconditional.
   Reply-byte corpus cases for each.
3. **OSC color queries (4/10/11/12) unanswered** — pairs with OSC 21 (kitty color)
   set/reset/query, which is fully stubbed. **Update 2026-07-12 (T8 drift):** upstream
   `14c829883` now implements both in the lib layer (`stream_terminal.zig`) — so #28 is a
   direct port with **fixed** formats (xterm 16-bit, kitty 8-bit), NOT a configurable
   `osc_color_report_format` at the engine layer (that knob is termio-side → #35), and the
   OSC-21 "note upstream divergence" plan is void (divergence closed). See
   `t5-execution-plan.md` #28 for the corrected port.
4. **DSR divergences** — `?6` answered but upstream rejects it (we're too permissive); `?996`
   color-scheme query entirely missing (needs a light/dark seam).
5. **XTWINOPS size reports missing** — CSI 21 t is engine-answerable today (`get_title`);
   14/16/18 need a pixel-geometry seam mirroring lib's `size` effect.
6. **Mouse state flags not ported** — `flags.mouse_event`/`mouse_format` (modes 9/1000/1002/
   1003, 1005/1006/1015/1016) and `terminal.mouse_shape` (OSC 22). Input layer (T4) will need
   these; engine-side state is our territory.
7. **XTMODKEYS dead flag** — `modify_other_keys_2` exists but `CSI > 4;2 m` never reaches it.
8. **Answerback/version seams** — ENQ response and XTVERSION string are hardcoded/absent;
   need setters (config keys arrive via T3). XTVERSION must report `qwertty-term`, not any
   ghostty string, once configurable.
9. **DA nits** — primary lacks conditional clipboard bit (52); tertiary replies where termio
   stays silent; secondary matches lib/oracle but not termio's `>1;10;0c`.
10. **ED 2 viewport scroll** — termio scrolls viewport to bottom before a full erase; lib and
    ours don't. Pick a layer, add a corpus case documenting the choice.
11. **App seams needed (Inbox T3/T4, engine setters our side)** — mode side-effect messages
    (2026 timer, 2048 initial report, 1004 focus report, mode 3 resize, linefeed mode),
    OSC 9/777 notification + OSC 9;4 progress surfacing, OSC 52 read requests, OSC 133
    start/stop-command events, empty-title→pwd behavior, OSC 7 hostname validation.

## Backlog reconciliation (spec `t5-vt-complete.md`)

- **XTWINOPS complete (M)** — confirmed; scope = size reports 14/16/18/21 + title stack
  (apprt-level 10-deep semantics; engine no-op stands) + reply corpus cases. Finding 5.
- **XTGETTCAP + DECRQSS full (M)** — confirmed; DECRQSS gaps are finding 2; XTGETTCAP needs a
  terminfo map port (no terminfo module exists yet — larger than "table fill-in").
- **VT config toggles (S/M)** — confirmed: `enquiry-response`, `osc-color-report-format`
  (finding 3), plus `title-report` (CSI 21 t gating), `vt-kam-allowed` (KAM), and the two
  limits. All want engine setters + T3 Inbox plumbing.
- **OSC gaps (S each)** — OSC 21 confirmed (finding 3/kitty side); OSC 22 confirmed
  (finding 6); OSC 104/110-119 reset edges look complete on our side (mask-based reset
  matches) — corpus cases to prove it, no code expected.
- **NEW: OSC 8 hyperlink wiring** (finding 1) — not in the original backlog; insert at top.
- **NEW: DSR strictness + ?996** (finding 4), **XTMODKEYS** (finding 7), **mouse state
  flags** (finding 6), **DA polish** (finding 9), **ED 2 viewport** (finding 10) — add as S
  items.
- **Selection/word semantics, promptClickMove/jump_to_prompt** — unaffected by this audit
  except OSC 133 start/stop-command seam (finding 11) which `jump_to_prompt` consumers will
  want.
- **tmux control mode (XL)** — confirmed seam location (`dcs.rs` TmuxRaw); still Josh-gated.

Zero rows in the table above require differential-oracle changes to *land the audit*; every
"missing/partial" row that becomes code must ship with corpus cases per the method rules
(reference lib answers DECRQSS/XTGETTCAP-adjacent queries only via effects, so the vt-diff
harness may need effect wiring to referee reply bytes — check `crates/vt-diff` before the
first reply-path PR).
