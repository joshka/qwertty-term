# Upstream findings status — ghostty-rs port findings vs ghostty HEAD

- **HEAD checked:** `c41c6b81a464` ("macOS: use the getOpinionatedStringContents same as
  paste for dragging (#13212)", tip of `main` after `jj git fetch` on 2026-07-07).
- **Originally surveyed at:** `2da015cd6`.
- **Repro branch:** `repro/ghostty-rs-findings` in `~/local/ghostty` (jj bookmark).
  Two commits on top of main:
  - `3405a22b14eb` — repro tests for findings 2–4 (compiles; finding 2 and 4 tests
    intentionally FAIL, finding 3 test passes as documentation).
  - `38e49a232874` (branch tip) — finding 1 test, **intentionally breaks test
    compilation**. Check out the parent to run the other tests.
- **Test command:** `mise exec zig@0.15.2 -- zig build test-lib-vt -Dtest-filter="REPRO ghostty-rs"`
- **Draft issues:** `issue-1-flattened-init.md`, `issue-2-max-scrollback-doc.md`,
  `issue-3-osc21-query-reply.md`, `issue-4-color-operation-leak.md` in this directory.
  Drafts only — nothing filed.

## Re-verification 2026-07-11 (T8) vs current main `a887df42c` — dispositions changed

Re-checked all four findings against upstream main `a887df42c` (fetched 2026-07-11, 102 commits
past the pin — see `drift.md`). **Two findings are now resolved upstream:**

- **Finding 1 — `highlight.Flattened.init` compile bugs → STILL LIVE, fileable** (pending Josh).
  `highlight.zig` at `a887df42c` still has `MultiArrayList(PageChunk)` (:146),
  `.serial = chunk.node.serial` (:151), and `.end_x = end.x` (:158).
- **Finding 2 — `max_scrollback` doc says lines, is bytes → do not file** (duplicate). Header
  still says "lines" (`terminal.h:187`); still a dup of discussion #12769.
- **Finding 3 — OSC color queries get no lib-vt reply → RESOLVED upstream, do not file.**
  `14c829883` (2026-07-07) "report OSC color queries in lib-vt" implements OSC 4/10/11/12 plus
  Kitty OSC 21 replies via the `write_pty` effect (PR #12631's successor). Now a port-side
  *feature* gap → T5 Inbox.
- **Finding 4 — `Parser.reset()` leaks `color_operation` list → RESOLVED upstream, do not file.**
  `14c829883`'s `osc.zig` reset now deinits the `color_operation` requests list. Now a port-side
  *bug to mirror* → T1 Inbox.

**Net (updated):** one fileable finding remains, Finding 1 (pending Josh's approval to file);
Finding 2 is a duplicate; Findings 3 and 4 are fixed upstream. Findings 3 (feature) and 4
(bug-mirror) are now port work routed to the owning threads' Inboxes, not upstream reports. The
original analysis below is retained for provenance; it reflects the `c41c6b81a464` (2026-07-07)
checkout.

## Finding 1 — `highlight.zig` `Flattened.init` does not compile: **CONFIRMED** (and worse than analyzed)

- `src/terminal/highlight.zig` `Flattened.init` is dead code (zero in-tree callers; the
  search subsystem builds `Flattened` values via `.empty` + manual appends), so Zig's lazy
  analysis never compiles its body.
- Forcing analysis (`_ = &Flattened.init;` in a test) yields a compile error — but **not
  the one the port analysis flagged first**. There are two distinct bugs in the function:
  1. Line 146: `var result: std.MultiArrayList(PageChunk) = .empty;` uses `PageChunk`
     (= `PageList.PageIterator.Chunk`, fields `node`/`start`/`end`) instead of the
     `Flattened.Chunk` declared just above (which adds `serial`). The append at line 151
     (`.serial = chunk.node.serial`) is the first compile error:
     `error: no field named 'serial' in struct 'terminal.PageList.PageIterator.Chunk'`.
     Even with that fixed, `.chunks = result` would be a type mismatch.
  2. Line 158: `.end_x = end.x` — the struct field is `bot_x` (the analysis's original
     finding; consistent with `clone`/`endPin`/`untracked`, where the end pin's x is
     `bot_x`).
- **Suggested fix:** line 146 → `std.MultiArrayList(Chunk)`; line 158 → `.bot_x = end.x`.
  Intended name is unambiguously `bot_x` from usage (`endPin`/`untracked` read the end x
  from `bot_x`).
- **Classification:** latent bug in dead code. Also explains why CI never catches it:
  `terminal/main.zig`'s `refAllDecls` references the `Flattened` *type* but function
  bodies are only analyzed when called.
- **Rust port note:** the port's choice (`bot_x = end.x`) matches the intended fix, but
  the port should also double-check it flattens `node.serial` into its chunks (bug 1 means
  the Zig "reference" code for that line was never type-checked).

## Finding 2 — `max_scrollback` documented as lines, actually bytes: **CONFIRMED** (doc bug)

- `include/ghostty/vt/terminal.h:172`: "Maximum number of lines to keep in scrollback
  history."
- Value flows untouched: `src/terminal/c/terminal.zig:276` → `Terminal.init` →
  `Screen.Options.max_scrollback` (`Screen.zig:255`, "maximum size of scrollback in
  bytes") → `PageList.init` `max_size` (bytes of page memory, clamped to a minimum of the
  active area + two pages, rounded to the terminal page size).
- **Repro evidence:** test `REPRO ghostty-rs finding 2` (Screen.zig): 80x2 screen,
  `max_scrollback = 2`, write 1001 lines → **999 history rows retained** (the byte value
  clamps up to PageList's minimum allocation). Header semantics would predict ≤ 2.
- **Classification:** doc bug, not API bug. Everything internal is bytes, and the
  user-facing `scrollback-limit` config (`src/config/Config.zig:1387`, default
  `10_000_000` "10MB") is documented in bytes. The header comment is the only "lines"
  claim.
- **Bonus doc bug for the same issue:** `Screen.zig` contradicts itself — `Options.max_scrollback`
  (`:253`) says "Zero means unlimited" while `Screen.init`'s doc (`:287`) says "If max
  scrollback is 0, then no scrollback is kept at all." Actual behavior: zero = none
  (`no_scrollback = opts.max_scrollback == 0`, `Screen.zig:308`); unlimited is
  `PageList.init(max_size = null)`, which `Screen` never passes. The `Options` comment is
  the stale one.
- **Rust port note:** treat the field as bytes; expose 0 = no scrollback.

## Finding 3 — OSC 21 queries get no reply: **NOT CONFIRMED AS STATED**

(Analysis was wrong for the app; real but different gap in libghostty-vt.)

- The claim "the parser produces `kitty_color_protocol` commands but no response-writer
  exists (compare OSC 4/10/11 in src/termio/stream_handler.zig)" is **wrong at HEAD and
  was already wrong at `2da015cd6`**: `src/termio/stream_handler.zig:1481`
  `kittyColorReport` handles OSC 21 query/set/reset and writes a `\x1b]21;...` reply
  (echoing the request terminator) via `messageWriter`. It has existed since commit
  `e13f9b9e8` ("terminal: kitty color", 2025-10-25), an ancestor of `2da015cd6`.
- **Likely cause of the false positive:** grepping `src/termio/` for
  `kitty_color_protocol` finds nothing because `stream.zig:2269` dispatches the command
  under a different name — `handler.vt(.kitty_color_report, v)`.
- **What IS true:** the libghostty-vt stream handler (`src/terminal/stream_terminal.zig`)
  never replies to *any* color query: `kittyColorOperation`'s `.query` arm (`:701`) and
  `colorOperation`'s `.query` arm (`:664`) are both no-ops, while other queries (DSR, DA,
  XTVERSION, glyph protocol) do reply through the `write_pty` effect. This has been so
  since the file was created (`67d8d86ef`, 2026-03-22, rename from ReadonlyStream) —
  an unimplemented feature of the lib layer, not a regression.
- **Repro evidence:** test `REPRO ghostty-rs finding 3` (stream_terminal.zig), which
  PASSES: DSR `\x1b[5n` gets `\x1b[0n` through `write_pty`; OSC 21/4/10/11 queries
  produce no output.
- **Classification:** needs-discussion. The draft issue is reframed as "libghostty-vt:
  color query OSCs (4/10/11/21) produce no reply" — a consistent lib-layer gap, possibly
  intentional (embedders can read colors via the state API). Maintainer should decide
  whether to file.
- **Rust port note:** for app-parity the port eventually needs an OSC 21 reply writer
  (model it on `kittyColorReport`); for libghostty-vt parity, dropping color queries is
  the current correct behavior.

## Finding 4 — `Parser.reset()` leaks `color_operation`'s list: **CONFIRMED** (real leak)

- `src/terminal/osc.zig` `reset()` (`:399`): `.kitty_color_protocol` gets a `deinit`
  (`:405-407`); `.color_operation` falls into the explicit no-op arm (`:411`). The
  `requests` list (`std.SegmentedList(Request, 2)`, `osc/parsers/color.zig:332`) is
  allocated with `parser.alloc` in `parseColor` and owned by `parser.command`.
- No ownership transfer exists: `stream.zig:2263` passes `v.requests` to the handler;
  both consumers (`src/termio/stream_handler.zig:327`, `src/terminal/stream_terminal.zig:260`)
  take `*const List` and never deinit. Nothing frees the list — the Zig test-suite only
  survives because `SegmentedList`'s prealloc of 2 keeps ≤ 2-request OSCs off the heap,
  and no existing test sends ≥ 3 requests in one OSC through the `Parser`.
- **Repro evidence:** test `REPRO ghostty-rs finding 4` (osc/parsers/color.zig): parse
  `\x1b]4;0;rgb:aa/bb/cc;1;rgb:bb/cc/dd;2;rgb:cc/dd/ee\x07` (3 requests) through
  `Parser` with `std.testing.allocator`, then `deinit()`. FAILS with "memory leaked";
  leak trace points at `parseGetSetAnsiColor` → `SegmentedList.growCapacity`.
- Real-world impact: every OSC 4/104 (etc.) with ≥ 3 color operations in one sequence
  leaks a few heap shelves per sequence, in both the app and libghostty-vt. Theme-setting
  scripts routinely set all 16/256 palette entries in one OSC 4.
- **Suggested fix:** either add `.color_operation => |*v| { ... v.requests.deinit(alloc) }`
  alongside the kitty arm in `reset()` (allocator availability mirrors
  `kitty_color_protocol`'s `orelse break` pattern), or move both toward the arena the
  parser was presumably meant to grow.
- **Rust port note:** ownership answer for the port: nobody frees it upstream — this is
  a genuine leak, not a transfer. The Rust `Vec` + owned-`Command` model is correct.

## Tracker cross-check (searched issues + discussions + PRs, 2026-07-07)

> Superseded for findings 3 and 4 by the 2026-07-11 re-verification above — both are now fixed
> in upstream main. This section reflects the tracker state as of 2026-07-07.

- **1 — Flattened.init:** no duplicate (9 searches, clean; only unrelated hits). New — file it.
- **2 — max_scrollback doc:** duplicate of
  [discussion #12769](https://github.com/ghostty-org/ghostty/discussions/12769) (Issue Triage,
  open, 2026-05-22), which reports the exact lines-vs-bytes doc mismatch. Do NOT file; optionally
  upvote / add the `Screen.zig` "zero" contradiction as a comment.
- **3 — color query replies:** was partial — open
  [PR #12631](https://github.com/ghostty-org/ghostty/pull/12631) "libghostty-vt: handle OSC color
  queries" implemented the lib-layer `.query` arm for OSC 10/11/12 and (via a `colorQuery` helper)
  OSC 4, but not OSC 21 (kitty). Closed [issue #7951](https://github.com/ghostty-org/ghostty/issues/7951)
  covered app-layer reply formatting (a different bug). **Now moot: `14c829883` merged the full
  OSC 4/10/11/12/21 lib-vt reply support (2026-07-07).**
- **4 — color_operation leak:** no duplicate (8 searches, clean). Leak surface was introduced by
  merged [PR #7429](https://github.com/ghostty-org/ghostty/pull/7429) "OSC: allow multiple
  set/reset/report operations per OSC" (2025-05-30), which added the `SegmentedList`-backed
  `requests`. **Now fixed upstream in `14c829883`.**

**Net (as of 2026-07-07):** two new fileable findings (1 and 4); one duplicate (2); one
in-progress-but-partial (3). See the 2026-07-11 re-verification above for the current dispositions.

## Contribution process (see `contribution-process.md` in this dir for the full writeup)

- The GitHub handle `joshka` is **not** in `.github/VOUCHED.td` and has **no** merged commits on
  `origin/main` → first-time-contributor path. Unvouched PRs are auto-closed.
- Ghostty does not take direct issues (blank issues disabled). Bugs → a **Discussion** in the
  **Issue Triage** category, template filled. Maintainers promote accepted discussions to the
  issue tracker; **PRs must implement an already-accepted issue** and come from a vouched user.
- **Order of operations:** (1) get vouched via a **Vouch Request** discussion — *Josh must write
  this himself, in his own voice; the AI policy explicitly forbids AI-generated vouch requests*;
  (2) open Issue-Triage discussions (or, for these tiny unambiguous fixes, a discussion linking a
  branch per "I've implemented a fix"); (3) once accepted → move to issue → PR. AI disclosure is
  required on issues/discussions/PRs (but not the vouch request, which must be human-written).

## Minor notes (verified, no repro needed)

1. `osc/parsers/kitty_clipboard_protocol.zig:13` — `encoding` import is dead (no
   `encoding.` reference in the file), and the `payload` doc comment (":20") says "check
   the `e` option" but the `Option` enum (`:65`) has no `e` member (id/loc/mime/name/
   password/pw/status/type).
2. `osc/parsers/iterm2.zig:4` — `simd` import is dead (no `simd.` reference).
3. `osc/parsers/kitty_clipboard_protocol.zig` — tests "OSC: 5522: example 1/3/13/15"
   have byte-identical bodies (verified by md5 after stripping the name line); 3/13/15
   add no coverage.
4. `osc/parsers/osc9.zig:535` — test name "OSC 9;3: message box -> desktop notification 2"
   is a copy-paste from the 9;2 pair; the body tests 9;3 and its sibling (`:520`) is
   named "OSC 9;3: change tab title -> desktop notification 1".
