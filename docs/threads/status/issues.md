# issues status — issue triage (bounded task)

- **Current item:** **CLOSED — all 13 open issues triaged against current main and closed.**
  Open set reduced to 0. Every close carries a one-line-verified justification comment.
- **Last merged:** n/a (triage thread — no code PRs; all work was issue closeout via `gh`).
- **Blockers:** none.
- **Claims:** none (touched no product code; only this status file).
- **Inbox:** (other threads append requests here; owner triages into backlog)

## Disposition table (all verified vs current main before closing)

| #   | Title (short)                           | Disposition        | Reason (verified)                                                                                        |
| --- | --------------------------------------- | ------------------ | -------------------------------------------------------------------------------------------------------- |
| 1   | Roadmap / wtf is this                   | closed-answered    | Pointed to README + docs/roadmap.md + feature-coverage.md; project is real, 0.2.0 live.                  |
| 19  | Kitty R6 slice-1 follow-ups             | closed-done        | R6 COMPLETE (5 slices #7/#64/#89/#96/#106); divide-guard #171; async-safety latent (guarded, Sync-only). |
| 22  | copy-on-select → {false,true,clipboard} | closed→app-tails   | Still `bool` at config.rs:40; enum widening is app-tails config work.                                    |
| 23  | wire `key-remap`                        | closed→app-tails   | No `key-remap` config field; RemapSet ported-but-unwired; feature-coverage:214 `[ ]`; app-tails.         |
| 24  | T3 tracking (keybinds+config+import)    | closed-done/track  | T3 shipped keybind system + config-core + import; remainder is #22/#23/#34 (app-tails).                  |
| 25  | T5 tracking (VT completeness)           | closed-done/track  | All sub-issues #26–#37 shipped; only #178 remains (vt-tails); tmux stays Josh-gated.                     |
| 29  | selection tracked-pin anchor            | closed→app-tails   | gesture.rs still `anchor: ScreenPoint` (:141); documented never-unsound deviation; app-tails polish.     |
| 30  | wire selection-gesture keys             | closed-done        | All 3 keys wired (config.rs + app.rs:1785/3524); feature-coverage:239-242 `[x]`; T4 confirms.            |
| 34  | set_tab_title action + title templates  | closed→app-tails   | `SetTabTitle` parsed in input crate (action.rs:826) but NO app dispatch; feature-coverage:138 `[ ]`.     |
| 40  | T7 tracking (Linux)                     | closed→linux/track | P1+P2 shipped (#135/#172/#187/#209); P3/P4 (GTK/OpenGL) owned by linux thread + feature-coverage.        |
| 41  | software GpuBackend + `Engine<B>`       | closed-done        | Shipped #135/#172/#187/#209; `Engine<Software>` on Linux CI; feature-coverage:307-318 `[x]`.             |
| 42  | un-gate acceptance tests on Linux       | closed→linux       | Tests still `#![cfg(target_os="macos")]` on main; seam ready (software_headless.rs); linux thread owns.  |
| 178 | DECCOLM clears scrolled content         | closed→vt-tails    | Real open bug, still budgeted in afl_corpus.rs:135-138; sole remaining VT divergence; vt-tails owns.     |

**Routing summary:** closed-done — #1 (answered), #19, #30, #41, and tracking #24/#25 (work
shipped). Closed-in-favor-of-thread (live work, redundant GitHub issue): #22/#23/#29/#34 →
**app-tails**; #40/#42 → **linux**; #178 → **vt-tails**. Open issue count 13 → 0.

**No Josh-decision issues left open.** The maintainer authorized aggressive closeout; every
close is reversible and carries a verified justification comment naming the owning thread. The
two product-flavored calls (tmux control mode, P4 GTK greenlight) are already Josh-gated inside
the now-closed tracking issues #25/#40 and need no standalone open issue. Live work is tracked by
the active app-tails / vt-tails / linux threads' feature-coverage backlogs, not redundant issues.

## Log

- 2026-07-14: session start — created workspace `work/issues`, read AGENTS.md, threads/README.md,
  feature-coverage.md; skimmed t1–t8 status (all CLOSED). Verified all 13 open issues against
  current main (grep code + `gh pr view` + feature-coverage) before disposition.
- 2026-07-14: closed all 13 — #30/#41 done; #1 answered; #24/#25/#19/#40 tracking closed-in-favor;
  #178→vt-tails, #42→linux, #22/#23/#34/#29→app-tails. Open issue count 13 → 0. Touched no product
  code (no territory risk). Task complete — no respawn needed.
