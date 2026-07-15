# Draft issue: libghostty-vt: color query OSCs (4/10/11/21) produce no reply

<!-- ⚠️ MOSTLY SUPERSEDED — do not file as a new report. Open PR #12631
     ("libghostty-vt: handle OSC color queries", https://github.com/ghostty-org/ghostty/pull/12631)
     already implements the lib-layer .query reply for OSC 10/11/12 and OSC 4.
     The ONLY part still uncovered is OSC 21 (kitty color protocol) queries.
     If anything, comment on #12631 asking whether OSC 21 is in scope rather
     than opening this. Low priority: the app already answers OSC 21. -->


<!-- DRAFT ONLY — do not file as-is, and NOTE: the original finding
     ("OSC 21 queries get no reply, no response-writer exists") did NOT
     reproduce for the Ghostty app: src/termio/stream_handler.zig's
     kittyColorReport has replied to OSC 21 queries since e13f9b9e8
     (2025-10-25). The false positive likely came from grepping for
     "kitty_color_protocol" — stream.zig dispatches it renamed as
     "kitty_color_report". What remains is a narrower lib-layer gap,
     which may well be intentional (embedders can read colors through
     the state API). Decide whether this is worth filing at all before
     editing further. -->

## Title

`libghostty-vt: color query OSCs (4;c;?, 10;?, 11;?, 21;...=?) produce no reply`

## Body

The libghostty-vt stream handler (`src/terminal/stream_terminal.zig`) answers DA, DSR,
XTVERSION, and glyph-protocol queries through the `write_pty` effect callback, but all
color *queries* are silently dropped:

- `colorOperation` (OSC 4/5/10-19): the `.query` arm is a no-op (`stream_terminal.zig:664`).
- `kittyColorOperation` (OSC 21, kitty color protocol): the `.query` arm is a no-op
  (`stream_terminal.zig:701`).

Set and reset operations in the same sequences work fine. This has been the behavior
since the file's introduction, so it's an unimplemented feature rather than a regression.
The full Ghostty app is unaffected — `src/termio/stream_handler.zig` replies to both the
OSC 4/10/11 style queries and OSC 21 (`kittyColorReport`).

### Reproduction

With a terminal created via the C API (or the in-tree Zig test below), install a
`GHOSTTY_TERMINAL_OPT_WRITE_PTY` callback and feed:

```text
\x1b[5n              -> replies \x1b[0n   (callback plumbing works)
\x1b]21;foreground=?\x1b\\   -> no reply  (kitty replies \x1b]21;foreground=rgb:..\x1b\\)
\x1b]4;0;?\x1b\\             -> no reply  (xterm replies \x1b]4;0;rgb:..\x07)
\x1b]10;?\x1b\\              -> no reply
\x1b]11;?\x1b\\              -> no reply
```

A client waiting for the standard reply (e.g. anything probing background color for
dark/light detection, or kitty's own `kitten @ get-colors` style probing) hangs until
its timeout.

### Expected vs actual

- Expected: query replies on the PTY writer, matching the request's terminator, as the
  Ghostty app does.
- Actual: no bytes produced; only set/reset take effect.

### Notes

If this is intentional (embedders are expected to answer queries themselves via the
color readback API), a doc note in `include/ghostty/vt/terminal.h` near the effects
callbacks would prevent embedders from assuming parity with the app. Otherwise the app's
`kittyColorReport` / color-operation reply logic could move down into
`stream_terminal.zig` so both layers share it.

### Version

- Commit: `c41c6b81a464`
- Zig 0.15.2, macOS aarch64

---

*AI disclosure: this behavior was found while porting the module with AI assistance
(Claude Code); the reproduction and this report were AI-drafted and human-reviewed and
edited before filing.*
