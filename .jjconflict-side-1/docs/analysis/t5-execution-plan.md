# T5 execution plan — ready-to-code notes for the near-term backlog

Gate-blocked prep (T1 owns the vt crate while active). This turns the top backlog issues into
land-in-one-sitting work: exact files, upstream refs, wire bytes, and corpus cases. Written
against main `8e732771`; re-verify line numbers when the gate opens (T1's perf work moves
`stream.rs`). Corpus format: one dir under `crates/vt-diff/corpus/<group>/<case>/` with
`input.esc` (escaped stream, see `vt_diff::decode_escaped_stream`) + optional `size.txt`; the
oracle compares screen dump **and** reply bytes against `libghostty-vt`. Run with
`cargo test -p vt-diff --features reference`.

**Ordering caveat:** #26 edits `osc_dispatch`; #27/#28/#31 edit `csi_dispatch`/`TerminalHandler`
reply methods — the same regions T1's queued "sgr/cursor dispatch package" touches. Land after
T1's PR #15 + sgr package merge, rebasing onto them, to avoid churn.

---

## #26 — OSC 8 hyperlinks (top quick win)

**Diagnosis:** parsed (`osc/mod.rs` `Command::HyperlinkStart{id,uri}` / `HyperlinkEnd`), Screen
implements it (`screen/mod.rs:851` `start_hyperlink(uri: &[u8], id: Option<&[u8]>) -> Result<(),()>`,
`:918` `end_hyperlink()`), but `osc_dispatch` drops both arms (`stream.rs:1438`). Upstream wires
them (`stream_handler.zig:335,328` → `startHyperlink`/`endHyperlink`; lib `stream_terminal.zig:256-257`).

**Code:**

1. `Handler` trait (`stream.rs` ~371, OSC-driven block): add
   `fn start_hyperlink(&mut self, uri: &str, id: Option<&str>) {}` and
   `fn end_hyperlink(&mut self) {}`.
2. `osc_dispatch` (`stream.rs:1438`): replace the `HyperlinkStart | HyperlinkEnd` no-op with
   `C::HyperlinkStart { id, uri } => self.handler.start_hyperlink(&uri, id.as_deref())` and
   `C::HyperlinkEnd => self.handler.end_hyperlink()`.
3. `TerminalHandler` impl: `start_hyperlink` calls
   `self.terminal.screen_mut().start_hyperlink(uri.as_bytes(), id.map(|s| s.as_bytes()))`
   (discard the `Result`); `end_hyperlink` → `self.terminal.screen_mut().end_hyperlink()`.
   (Ignore the `Err(())` like
   the existing internal callers at `screen/mod.rs:662,2108` do.)

**Corpus** (`corpus/hyperlink/`, screen-dump diff — hyperlink IDs surface in the formatter dump):

- `osc8_basic/input.esc`: `\e]8;;https://example.com\e\\link\e]8;;\e\\` then text.
- `osc8_id/input.esc`: `\e]8;id=abc;https://a.example\e\\A\e]8;id=abc;https://a.example\e\\B\e]8;;\e\\`
  (same id spanning a gap → one logical link).
- `osc8_reset_on_empty/input.esc`: open a link, print, `\e]8;;\e\\`, print unlinked text.
- BEL terminator variant: `\e]8;;https://x.example\aY\e]8;;\a`.

Verify the reference build attributes hyperlinks in its dump; if the dump doesn't surface link
identity, add an `#[ignore]` note and lean on `screen/tests.rs` unit coverage instead.

---

## #27 — DECRQSS completeness

**Diagnosis:** `TerminalHandler::decrqss` (`stream.rs:1949`) answers SGR/DECSTBM/DECSLRM but:
(a) never emits the invalid response, (b) no DECSCUSR reply, (c) DECSLRM unconditional.
Upstream `stream_handler.zig:475-544`.

**Exact wire bytes** (prefix `\x1bP{valid}$r`, terminator `\x1b\\`; `valid`=1 if payload else 0):

- **Invalid / `Decrqss::None`:** `\x1bP0$r\x1b\\` (upstream *always* replies; sh:483-541,535).
  Ours currently returns silently — change the empty-body branch to emit this.
- **DECSCUSR (`Decrqss::Decscusr`):** payload `{n} q`, `n` from cursor style + `cursor_blinking`
  mode (sh:501-513). Map our `screen::cursor::CursorStyle` + `modes.get(CursorBlinking)`:
  Block→`blink?1:2`, Underline→`blink?3:4`, Bar→`blink?5:6`, **BlockHollow→`blink?1:2`**
  (hollow reported as block, matching sh:510). Reply `\x1bP1$r{n} q\x1b\\`.
- **DECSLRM (`Decrqss::Decslrm`):** only when `modes.get(EnableLeftAndRightMargin)` (sh:525);
  else emit the invalid `\x1bP0$r\x1b\\`. Payload `{left+1};{right+1}s`.
- SGR / DECSTBM: unchanged (already correct).

**Corpus** (`corpus/reply_diffing/`, sibling to the existing `decrqss_sgr_scope`):

- `decrqss_invalid/input.esc`: `\eP\$q\e\\` (unrecognized request → expect `\eP0\$r\e\\`).
- `decrqss_decscusr_default/input.esc`: `\eP\$q q\e\\` (default cursor → expect `2 q` payload).
- `decrqss_decscusr_bar_blink/input.esc`: `\e[5 q\eP\$q q\e\\` (set blinking bar, then query →
  `5 q`).
- `decrqss_decslrm_disabled/input.esc`: `\eP\$qs\e\\` with DECLRMM off → expect invalid `0$r`.
- `decrqss_decslrm_enabled/input.esc`: `\e[?69h\e[3;10s\eP\$qs\e\\` → expect `3;10s`.

(`\eP$q...\e\\` is the DECRQSS request: DCS `$q` + setting's final byte + ST.)

---

## #28 — OSC color-report queries (OSC 4/10/11/12) + OSC 21 kitty color

**UPDATED 2026-07-12 (T8 drift):** upstream landed `14c829883` "terminal: report OSC color
queries in lib-vt" — it implements exactly this in **`stream_terminal.zig`** (the lib layer our
`TerminalHandler` mirrors), so #28 is now a **direct port of that commit**, NOT a roll-our-own
"per kitty spec, note divergence" (that framing in stream-handler-delta.md finding 3 predates
the upstream fix — the finding-issue-3 divergence is closed). Verify the reference build for the
corpus is at/after `14c829883` (it is, on `~/local/ghostty` main); pin wire bytes to it.

**Key correction:** the lib layer uses **fixed formats** — xterm queries always 16-bit, kitty
always 8-bit. There is **no** `None/8-bit/16-bit` knob at this layer. The `osc-color-report-format`
config toggle (none/8/16) is a *termio*-handler concern (`stream_handler.zig`), so it belongs to
**#35**, layered as an app-side override — do NOT add it to the core `TerminalHandler` reply. My
earlier "add `osc_color_report_format` option" note was wrong; ignore it.

**Diagnosis:** `ColorRequest::Query` ignored (`stream.rs:1863`); `kitty_color` stub (`:1867`).
Port targets: `stream_terminal.zig@14c829883` `colorOperation` (:601-687, incl.
`writeXtermColorReport` :688-720) and `kittyColorOperation` (:722-780).

**Helpers to port first** (don't exist on our side yet):

- `Rgb::encode_rgb16(w)` → `rgb:{r*257:04x}/{g*257:04x}/{b*257:04x}` (each byte doubled: 0x12 →
  0x1212). Confirmed by upstream test: `\x1b]4;2;rgb:1212/3434/5656\x1b\\` for input `12/34/56`.
- `Rgb::encode_rgb8(w)` → `rgb:{r:02x}/{g:02x}/{b:02x}`.
- `Terminal::color_for_xterm(target)` → resolved `Option<Rgb>`; **cursor falls back to
  foreground** when no cursor color set; returns `None` (skip) for unsupported targets.
- `Terminal::color_for_kitty(key)` → `Option<Rgb>`; plus `key.has_terminal_query_color()` to
  decide empty-value vs skip for unset keys.

**Exact wire bytes** (terminator = the request's own terminator, `\x1b\\` or `\x07`, **preserved**):

- Xterm palette query: `\x1b]4;{i};` + `encode_rgb16` + term.
- Xterm dynamic (fg=10/bg=11/cursor=12): `\x1b]{10|11|12};` + `encode_rgb16` + term.
- Skip (no output) when `color_for_xterm` returns `None`.
- Multiple queries coalesce into one write (upstream accumulates into one `Allocating` writer).

**OSC 21 (kitty color):** set/reset already plumbed via `color_operation` (`stream.rs:1829`);
port the query path: write prefix `\x1b]21` once (on first output), then per key
`;{key}=` + `encode_rgb8`; unset-but-terminal-backed key (`has_terminal_query_color`) →
`;{key}=` (empty value); unsupported key → skip. Terminator appended once at end.

**Parser note:** the commit also fixes an alloc leak — "OSC parser releases color operation
request lists during reset." Our Rust `osc::Parser` frees on `reset` via `Vec` drop, so this is
likely a no-op for us — but confirm `reset()` clears any retained `ColorList` and add a
multi-query test (the leak surfaced under repeated queries).

**Corpus** (`corpus/reply_diffing/`; pin against reference ≥ `14c829883`):

- **Update the existing `osc_color_query_no_reply` case** — it currently asserts *no* reply;
  against the new reference it now *replies*. Rename to `osc_color_query_palette` / adjust, or
  it will diff-fail. (Do NOT leave it as-is.)
- `osc_color_query_palette_16bit/input.esc`: `\e]4;1;?\e\\` → `\e]4;1;rgb:XXXX/XXXX/XXXX\e\\`.
- `osc_color_query_fg_bg/input.esc`: `\e]10;?\e\\\e]11;?\e\\`.
- `osc_color_query_set_then_read/input.esc`: `\e]4;2;rgb:12/34/56\e\\\e]4;2;?\e\\` → expect
  `rgb:1212/3434/5656` (the doubled 16-bit form; matches upstream test).
- `osc_color_query_cursor_fallback/input.esc`: `\e]12;?\e\\` with no cursor color set → expect
  the foreground color reported (cursor→fg fallback).
- `osc21_kitty_query/input.esc`: `\e]21;foreground=?\e\\` → `\e]21;foreground=rgb:XX/XX/XX\e\\`
  (8-bit). Now that upstream implements it, expect agreement — no `SKIP` needed unless a live
  diff proves otherwise.

Pairs with #35 (the `osc-color-report-format` config key gates `None`/`8-bit`/`16-bit`).

---

## #31 — DSR strictness (`?6` too permissive; `?996` missing)

**Diagnosis:** `DeviceStatusReq::from_int` (`stream.rs:436`) accepts `(6, _)` — so the private
`CSI ? 6 n` wrongly gets a CPR reply. Upstream `device_status.zig` entries:
`operating_status`=5 (q=false), `cursor_position`=6 (q=false), `color_scheme`=996 (q=true).

**Code:**

1. `from_int`: change `(6, _) => …CursorPosition` to `(6, false) => …CursorPosition`. Add
   `(996, true) => Some(DeviceStatusReq::ColorScheme)`. Everything else `None`.
2. Add `DeviceStatusReq::ColorScheme` variant + a `device_status` handler arm. Reply
   `\x1b[?997;1n` (dark) / `\x1b[?997;2n` (light) — `device_status.zig` `encodeColorSchemeReport`.
   The light/dark value needs a **seam**: a `color_scheme: Option<ColorScheme>` setter on
   `TerminalHandler` (theme source is T4). With no scheme set, upstream's `color_scheme` effect
   returns null → **no reply** (sh:890, lib `stream_terminal.zig` returns early). Match that:
   `None` scheme → emit nothing.

**Corpus** (`corpus/reply_diffing/`):

- `dsr_private_6_no_reply/input.esc`: `\e[?6n` → expect **no** reply (regression guard for the
  fix; the reference rejects it).
- `dsr_operating_status/input.esc`: `\e[5n` → `\e[0n` (may already exist — check before adding).
- `dsr_color_scheme_default/input.esc`: `\e[?996n` → no reply when no scheme seam set (documents
  the default-silent behavior; a scheme-set variant waits on the T4 theme seam).

`?996` full round-trip (light/dark reply) is gated on the theme seam — file that half as a
follow-up when T4's color-scheme source exists; land the strictness fix + silent-default now.

---

## Cross-cutting: reply-path harness check (do first)

The reference lib answers reply-emitting queries only through effect callbacks; the vt-diff
harness already wires `GHOSTTY_TERMINAL_OPT_WRITE_PTY` into `reply_buf`
(`crates/vt-diff/src/reference.rs:15`) and exposes `RustTerminal::output()`
(`rust_engine.rs:52`). The existing `reply_diffing` cases (da1/da2/decrqss_sgr/dsr_cursor/
kitty_keyboard_query) prove reply bytes are already diffed — so **no harness changes are needed**
for #26–#31; new cases just drop into `reply_diffing/`. Confirm by running the suite before
starting, so a pre-existing red isn't misattributed to the new work.
