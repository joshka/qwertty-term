# Draft issue: libghostty-vt: max_scrollback documented as "lines" but is bytes

<!-- ⛔ DO NOT FILE — DUPLICATE. This is already reported (and open) as
     https://github.com/ghostty-org/ghostty/discussions/12769 (Issue Triage,
     2026-05-22). Kept only for reference. The one thing #12769 lacks is the
     Screen.zig "Zero means unlimited" vs "zero = no scrollback" contradiction
     noted below — worth adding as a comment there, not as a new report. -->

<!-- DRAFT ONLY — do not file as-is. Review, edit, and file manually.
     Per Ghostty's AI policy, the disclosure line at the bottom must be kept
     (edit it to reflect your actual review). -->

## Title

`libghostty-vt: GhosttyTerminalOptions.max_scrollback documented as "lines" but is bytes`

## Body

`include/ghostty/vt/terminal.h` documents:

```c
  /** Maximum number of lines to keep in scrollback history. */
  size_t max_scrollback;
```

The value is actually **bytes of page memory**: it passes untouched through
`src/terminal/c/terminal.zig` into `Screen.Options.max_scrollback`, which is documented
as "maximum size of scrollback in bytes" and feeds `PageList.init`'s `max_size` (bytes,
rounded to the terminal page size, clamped to a minimum of the active area plus two
pages). The user-facing app config for the same value (`scrollback-limit`) is likewise
bytes (default 10MB), so bytes is clearly the intended unit — the C header comment is
the only place claiming lines.

### Reproduction

Any small value shows it. In-tree Zig equivalent (the C API forwards verbatim):

```zig
var s = try Screen.init(alloc, .{ .cols = 80, .rows = 2, .max_scrollback = 2 });
// write 1001 numbered lines...
```

With `max_scrollback = 2` — which a C-header reader would take as "keep 2 lines" — all
**999 history rows are retained**, because 2 bytes is clamped up to PageList's minimum
page allocation. Conversely a large terminal with `max_scrollback = 10_000` keeps far
fewer than 10,000 lines. Retention scales with bytes-per-row (i.e. with `cols`), which
is impossible under a "lines" reading.

### Expected vs actual

- Expected (per header): scrollback history capped at `max_scrollback` *lines*.
- Actual: scrollback capped at roughly `max_scrollback` *bytes* of page memory
  (rounded to page size, minimum enforced), and `0` means *no scrollback*.

### Suggested fix

Reword the header doc, e.g.:

```c
  /**
   * Maximum amount of scrollback history to keep, in bytes of page
   * memory. Rounded to the internal page size; values smaller than the
   * active screen are clamped up. Zero keeps no scrollback at all.
   */
  size_t max_scrollback;
```

While here: `src/terminal/Screen.zig` contradicts itself about zero —
`Options.max_scrollback`'s comment says "Zero means unlimited" while `Screen.init`'s doc
says "If max scrollback is 0, then no scrollback is kept at all". The latter matches the
implementation (`no_scrollback = opts.max_scrollback == 0`); the `Options` comment is
stale. (Unlimited is `PageList.init(max_size = null)`, which `Screen` never passes.)

### Version

- Commit: `c41c6b81a464` (also present at `2da015cd6`)
- Zig 0.15.2, macOS aarch64

---

*AI disclosure: this defect was found while porting the module with AI assistance
(Claude Code); the reproduction and this report were AI-drafted and human-reviewed and
edited before filing.*
