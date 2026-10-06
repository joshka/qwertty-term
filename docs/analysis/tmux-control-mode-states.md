# tmux control mode — state & transition model

A model of the native-app ↔ tmux-`CC` integration (ADR 006), so lifecycle
transitions are handled systematically rather than bug-by-bug. Written while
shaking out live-testing edge cases; the "Gaps" section is the actionable
backlog.

## Entities

| Entity                    | What                                                    | Backing                                     |
| ------------------------- | ------------------------------------------------------- | ------------------------------------------- |
| **Control surface** `S_c` | the surface running `tmux -CC`; owns the `TmuxSession`  | native, **pty-backed**                      |
| tmux **window** `W_i`     | ↔ one native **tab** `T_i` (1:1)                        | —                                           |
| tmux **pane** `P_j`       | ↔ one **display-only surface** `S_j` inside `T_i` (1:1) | **no pty**; fed by `%output` via the Viewer |

## Modes

- **Normal** — no active tmux session. `S_c` is just a shell; app tabs behave natively.
- **TmuxActive** — `S_c` is running `tmux -CC`. Its control tab is **hidden**; tmux
  windows are shown as native tabs; tmux owns all window/pane structure.

## Invariants (must always hold — every current bug is a violation of one)

- **I1 — Never-empty window.** The app window always contains ≥1 *visible, focusable*
  surface. When the last tmux pane/tab is removed, `S_c` must be **restored (shown +
  focused) before** the window would hit zero surfaces (a zero-surface window closes).
- **I2 — Focus ≡ tmux active pane.** The focused surface and tmux's active pane are kept
  in sync both directions. *(done)*
- **I3 — tmux owns the layout.** The app never creates/destroys tmux tabs/panes directly;
  it issues a tmux command and then mirrors the resulting reconcile. Native `SplitTree`
  mutation is blocked inside a tmux tab.
- **I4 — Control surface is exclusive.** While windows are shown, `S_c` is hidden and
  never holds keyboard focus; it is the reconcile driver, not a user surface.

## Transitions

### User actions (in TmuxActive)

| Trigger                                                | Should map to                                             | Result                                                                                   | Status                                                                                       |
| ------------------------------------------------------ | --------------------------------------------------------- | ---------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| Cmd-D / split                                          | `split-window -t %p` (`-h` or `-v`)                       | reconcile adds the pane                                                                  | DONE                                                                                         |
| Cmd-T / new tab                                        | `new-window`                                              | `%window-add` → new tab                                                                  | DONE                                                                                         |
| Cmd-W on a **pane**                                    | `kill-pane -t %p`                                         | reconcile removes pane; if it was the window's last pane → `%window-close` → tab removed | DONE (pane); collapse-to-tab-close UNVERIFIED                                                |
| Ctrl-D (EOF) in a pane                                 | (goes to the pane shell) → shell exits → tmux `kill-pane` | same as kill-pane                                                                        | BUG ("Cmd-T then Ctrl-D bad")                                                                |
| Close a **tab** (Cmd-W on tab / close button)          | **`kill-window -t @w`**                                   | `%window-close` → tab removed                                                            | BUG — not mapped, closes the native tab directly (I3 violation, "closing the tab works bad") |
| Close the **last** pane / **last** window              | → tmux → `%exit`                                          | teardown + **restore `S_c`** (I1)                                                        | BUG — window closes (I1 violation)                                                           |
| Click / directional-nav focus                          | `select-pane -t %p`                                       | focus synced                                                                             | DONE                                                                                         |
| Close the **app window** (red button) while TmuxActive | end the session (detach, or kill) → `%exit`               | clean teardown                                                                           | UNVERIFIED                                                                                   |

### tmux → app events

| Event                        | App action                                                 | Status     |
| ---------------------------- | ---------------------------------------------------------- | ---------- |
| `%window-add @w`             | create tab `T_w` + pane surface(s)                         | DONE       |
| `%layout-change @w …`        | add / remove / resize panes in `T_w`                       | DONE       |
| `%window-pane-changed`       | move keyboard focus (I2)                                   | DONE       |
| `%window-close @w`           | remove tab `T_w`; if 0 windows remain, expect `%exit` next | UNVERIFIED |
| `%exit` (control mode ended) | tear down every tmux tab; **restore + focus `S_c`** (I1)   | PARTIAL    |

## Gaps (the actionable backlog these transitions surface)

1. **Close-window → `kill-window`.** Closing a *tab* (Cmd-W on a tmux tab, tab close
   button) must issue `kill-window -t @w` (I3), not close the native tab directly. Direct
   close desyncs (tmux still thinks the window exists) — the "closing the tab works bad".
2. **Never-empty (I1).** On the last `%window-close` / `%exit`, restore `S_c` **before** the
   window empties. Sequence when the last pane of the last window closes must end in
   `S_c` restored, never a zero-surface (self-closing) window. Covers both "close last pane
   → window closed" and part of "Cmd-T then Ctrl-D bad".
3. **Ctrl-D on the last pane of a (non-last) window.** Shell EOF → tmux closes that pane →
   window has 0 panes → tmux closes the window → `%window-close` → remove that tab and move
   focus to a surviving tab. Must not close the app. (This is the "Cmd-T then Ctrl-D" path:
   Cmd-T makes a 1-pane window; Ctrl-D closes its only pane.)
4. **App-window close (red ●) while TmuxActive.** Should cleanly end the `tmux -CC` client
   (detach or kill-session per config) — not orphan the tmux server or leave a zombie.
5. **Exit-flash (known).** On restore, `S_c` briefly shows raw `%…` protocol text — the
   control surface's terminal shouldn't display the consumed control stream (engine-side,
   `qwertty-term-vt/src/tmux`).

## Design rule of thumb

Every native window/tab/pane operation, while a tab is tmux-managed, is either **(a)
translated to the equivalent tmux command** (then the reconcile does the real work), or
**(b) refused** — never applied to the native tree directly (I3). And any path that could
remove the last visible surface must first restore `S_c` (I1).
