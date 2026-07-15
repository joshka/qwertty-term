# How to get these findings fixed in ghostty-org/ghostty

Sourced from `CONTRIBUTING.md`, `AI_POLICY.md`, `.github/VOUCHED.td`, the discussion
templates, and a live check of the tracker on 2026-07-07 (HEAD `c41c6b81a464`).

## Your standing

- **`joshka` is NOT vouched** — not present in `.github/VOUCHED.td`, and has **zero merged
  commits** on `origin/main`. So you're on the *first-time contributor* path, not the
  "prior contributor" grandfather clause.
- **Consequence:** any PR you open while unvouched is **auto-closed by a bot**. You must be
  vouched first.

## The pipeline (how work actually flows here)

Ghostty deliberately does NOT use the issue tracker as an inbox:

1. **Blank issues are disabled.** `Features, Bug Reports, Questions` all start in
   **Discussions** (`https://github.com/ghostty-org/ghostty/discussions/new/choose`).
2. Bug reports go in the **Issue Triage** discussion category (filled-in template).
3. A maintainer triages the discussion; once it's a well-scoped, accepted, actionable
   item, **they** move it to the **issue tracker**. "All issues are actionable."
4. **Pull requests must implement an already-accepted issue.** A PR for something not
   previously discussed "may be closed or remain stale." PRs are not for design discussion.

So the ideal order for each finding is: **Discussion → (maintainer accepts) → Issue → PR.**
For a tiny, unambiguous fix there's also the sanctioned shortcut (CONTRIBUTING "I've
implemented a fix"): *if there's no issue, open a discussion and link to your branch.*

## Step 0 — Get vouched (do this once, first)

Open a **Vouch Request** discussion:
`https://github.com/ghostty-org/ghostty/discussions/new?category=vouch-request`

Template asks two things ("What do you want to change?", "Why?") plus three acknowledgement
checkboxes (read CONTRIBUTING, agree to AI policy, **"I wrote this vouch request myself, in
my own voice, without AI generating it."**).

> ⚠️ **You (Josh) must write the vouch request yourself.** CONTRIBUTING §First-Time
> Contributors and the template both require it be in your own voice and NOT AI-generated.
> I have deliberately not drafted it. Keep it concise: e.g., "I found and want to fix a
> couple of small terminal/libghostty-vt defects while doing a Rust port of the VT engine
> (a dead `highlight.zig` function that doesn't compile; a memory leak in OSC
> `color_operation` cleanup). I have minimal Zig repros for both." A maintainer replies
> `!vouch` to approve.

## Step 1 — Report the two new findings as Issue-Triage discussions

Only findings **1** and **4** are new (see `findings-status.md` for why 2 and 3 aren't).
Use the drafts `issue-1-flattened-init.md` and `issue-4-color-operation-leak.md` as the
*content*, but paste them into the **Issue Triage discussion** template, not a raw issue.

Template quirks for library/code-level defects (most fields assume a runtime user bug):
- **Issue Description / Expected / Actual / Reproduction Steps** — map directly to the
  draft. For repro, give the `zig build test -Dtest-filter=...` snippet.
- **Ghostty Version** — `ghostty +version` output, or just state the commit
  `c41c6b81a464` and Zig `0.15.2` since these are source-level.
- **OS Version** — your macOS (Darwin 24.6.0). Note it's platform-independent.
- **Minimal Ghostty Configuration** — "N/A, source-level defect" is fine.
- Tick the acknowledgement checkboxes (FAQ reviewed, searched for duplicates — you have,
  results in `findings-status.md`; backticks around code).
- **Keep the AI-disclosure line** from each draft (AI policy: all AI use disclosed on
  discussions/issues, human-reviewed and trimmed).

Both are small enough that a maintainer may just say "send a PR." Because the fixes are
one-liners, the branch-link shortcut is attractive: mention in the discussion that a repro
+ fix branch is ready.

## Step 2 — Finding 2: don't file, engage the existing thread

Duplicate of **discussion #12769**. Don't open anything new. If you want, upvote it and/or
add the one detail it's missing — the `Screen.zig` `Options.max_scrollback` "Zero means
unlimited" comment contradicting `Screen.init`'s "zero = no scrollback" — as a single
comment. (CONTRIBUTING asks people not to add low-value "me too" comments; this is a real
addition, so it's fine.)

## Step 3 — Finding 3: comment on the in-flight PR, don't file

Open **PR #12631** ("libghostty-vt: handle OSC color queries") already implements the
lib-layer reply for OSC 10/11/12 and OSC 4. It does **not** cover OSC 21 (kitty color
protocol) queries, whose `.query` arm in `kittyColorOperation` stays a no-op. Lightest
touch: a comment on #12631 asking whether OSC 21 is in scope, or wait for it to land and
re-check. Low priority — the *app* already answers OSC 21 via `kittyColorReport`; only the
embedder/libghostty-vt path is affected, and that may be intentional. Not worth a fresh
discussion given #12631 owns this territory.

## Step 4 — After acceptance: open PRs

Once vouched and an issue exists (or a maintainer green-lights the branch):
- Fork, branch, implement the fix. For these:
  - **Finding 1:** in `Flattened.init`, `MultiArrayList(PageChunk)` → `MultiArrayList(Chunk)`
    and `.end_x` → `.bot_x` (2-line fix; consider adding a compile guard/test so the dead
    function stays checked).
  - **Finding 4:** add a `.color_operation => |*v| { v.requests.deinit(alloc) }` arm to
    `Parser.reset()` mirroring the `kitty_color_protocol` allocator-optional pattern.
- Follow repo conventions (`zig fmt`, targeted tests). Reference the accepted issue in the
  PR. Include the AI-disclosure statement describing tool + extent of assistance.
- **The Critical Rule:** you must be able to explain the change without AI. Both fixes are
  small and you have the full analysis in `findings-status.md`, so this is satisfiable.

## Note on the repro branch

The repro tests live on jj bookmark `repro/ghostty-rs-findings` in `~/local/ghostty`. The
finding-1 test intentionally breaks compilation, so a PR branch should carry the *fix* plus
a compiling test, not that tripwire test verbatim. The finding-4 leak test converts cleanly
into a real regression test (it passes once the deinit arm is added).
