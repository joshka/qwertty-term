# Keybind system: porting `src/input/Binding.zig` (T3 analysis)

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse HEAD`). Upstream `input/Binding.zig` is **4882
lines** — the largest single unported subsystem, and the one power users feel
most directly. This document is the port spec: it captures the trigger grammar,
the full 85-variant action enum, leader/chain/key-table semantics, the verbatim
default keymap, and a Rust port design that **collapses our four bespoke tables**
(`tabkeys` / `splitkeys` / `searchkeys` / `keybind.rs`'s `text:` subset) into one
`Binding.Set`-equivalent.

Source line cites are `Binding.zig:NNNN` / `key.zig:NNNN` / `key_mods.zig:NNNN`
/ `Config.zig:NNNN` / `Surface.zig:NNNN` at the pinned commit unless noted.

Our current state (what this replaces) is surveyed in §9; the durable current-code
citations there are against `work/t3` at branch-time.

---

## 1. What exists today, and the proof-of-port

The Rust app currently dispatches keys through **four independent, exact-match
tables**, only one of which is user-configurable:

| Table                             | Config-driven? | Actions                                                                                | Consulted at                                             |
| --------------------------------- | -------------- | -------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| `crate::tabkeys`                  | no (hardcoded) | `NextTab`, `PreviousTab`, `GotoTab(n)`, `LastTab`                                      | `view.rs` `performKeyEquivalent:` → `try_handle_tab_key` |
| `crate::splitkeys`                | no (hardcoded) | `NewSplit`, `GotoSplit`, `GotoAdjacent`, `ToggleZoom`, `ResizeSplit`, `EqualizeSplits` | same path, checked before tab chords                     |
| `crate::searchkeys`               | no (hardcoded) | `Start`, `End`, `Next`, `Previous`                                                     | same path, checked first                                 |
| `crate::keybind` (`text:` subset) | **yes**        | `text:` byte-send only                                                                 | `keyDown:` → `try_handle_text_keybind`                   |

That is ~26 concretely-implemented actions vs upstream's 85-variant enum, one
user-configurable action (`text:`) vs the whole surface, single-chord exact match
only (no leaders, no key tables), and a bespoke 4-bool `TabMods` that duplicates a
strict subset of the already-ported `qwertty_term_input::key_mods::Mods`.

**The definition of done is deletion.** When the port is real, all four tables
above collapse into one `Set` populated by upstream's default keymap (§7) plus the
user's `keybind` config, and `TabMods` is retired in favour of `Mods::binding()`.
Every existing smoke (tab-chord, split, search, `text:` round-trip) must stay green
across the swap — they are the regression net.

---

## 2. Trigger model

A `Binding` is `{ trigger: Trigger, action: Action, flags: Flags }`
(Binding.zig:16-23). Parse errors are `error{InvalidFormat, InvalidAction}`
(Binding.zig:25-28).

`Trigger` (Binding.zig:1660-1665) is `{ key: Trigger.Key, mods: Mods }`. The key
default is `.{ .physical = .unidentified }`, and `isKeyUnset()` is true exactly
when the key is `physical == .unidentified` (Binding.zig:1927-1932).

`Trigger.Key` is a **3-variant union** (Binding.zig:1667-1682):

- `physical: key.Key` — a layout-independent **W3C physical code** (e.g.
  `key_a`), matches the physical location regardless of layout.
- `unicode: u21` — matches whichever key produces this Unicode codepoint.
- `catch_all` — matches any otherwise-unbound key press.

There is **no "translated" variant** — translated keys were removed in Ghostty
1.2. A bare letter `a` parses to `unicode`, never `physical = key_a`
(Binding.zig:3075-3079). This is the single most load-bearing grammar fact
(see §3).

### Mods and `.binding()` normalization

`Mods` is the packed struct in `key_mods.zig` (also already ported to
`qwertty_term_input::key_mods::Mods`): `shift`, `ctrl`, `alt`, `super_`,
`caps_lock`, `num_lock`, plus per-modifier `sides` (left/right). The canonical
config mod names are exactly the four bool field names `ctrl`/`shift`/`alt`/`super`
(Binding.zig:1716-1726), with aliases from `key_mods.alias`: `cmd`/`command`→super,
`opt`/`option`→alt, `control`→ctrl (Binding.zig:1728-1737; tests 3264-3304).

**Lookup and hashing normalize via `mods.binding()`**, which strips locks and
sides down to the four bindable bits (`key_mods.rs:160-170` on the Rust side).
Trigger hashing hashes `mods.binding()` (Binding.zig:1952) and `getEvent` builds
its probe trigger from `event.mods.binding()` (Binding.zig:2659). But plain
`Trigger.equal`/`foldedEqual` compare `mods` **directly** (Binding.zig:1976,1989)
— so a parsed trigger is expected to already be in binding-normalized form. The
Rust port must apply `binding()` at parse time as well as at lookup so the two
equalities agree.

`KeyEvent::binding_hash()` already exists in `qwertty_term_input`
(`key.rs`, port of key.zig:60-77): it hashes `key`, `unshifted_codepoint`, and
`mods.binding()`, deliberately **excluding the action** (press vs release). This
is exactly the hash a `Set` wants — reuse it, do not reinvent it.

---

## 3. Trigger parse grammar

`Trigger.parse(input)` handles exactly one trigger (sequences are split on `>`
earlier). Empty input → `InvalidFormat` (Binding.zig:1706-1803). It splits on `+`
and tries each part **in this exact order** — the order is the spec:

1. **Mods field name** — `ctrl`/`shift`/`alt`/`super`. Duplicate → `InvalidFormat`.
2. **Mod alias** — `cmd`/`command`/`opt`/`option`/`control`. Duplicate (incl.
   alias-vs-canonical like `ctrl+control`) → `InvalidFormat`.
3. Not a mod ⇒ it's the key; **two keys → `InvalidFormat`** (`a+b` fails).
4. **Empty part = literal `+`** → `unicode '+'`. So `+=ignore` and `ctrl++=ignore`
   work; `++=ignore` fails (double key).
5. **Ghostty key enum name** (snake_case: `key_a`, `arrow_up`, `quote`), excluding
   `unidentified` → `physical`.
6. **Single Unicode codepoint** — decode part as UTF-8; exactly one codepoint →
   `unicode`. **This is why bare `a` is `unicode`, tried before rule 7.**
7. **W3C key name** — `Key.fromW3C(part)` (e.g. `KeyA`) → `physical`. Case
   sensitive: `KeyA` works, `Keya` fails (test 2949-2963).
8. **`catch_all`** literal.
9. **Backward-compat table** (Ghostty ≤1.1.x): `zero`…`nine`→unicode digits,
   `plus`→`+`, `up`/`down`/`left`/`right`→`arrow_*`, `kp_*`→`numpad_*`,
   `left_shift`…`right_super`→side-specific physical keys, **plus `physical:`-prefixed
   variants** (Binding.zig:1808-1924). e.g. `physical:zero`→`physical digit_0`
   while `zero`→`unicode '0'` (test 3083-3097).
10. else → `InvalidFormat`.

Mods may appear after the key (`a+shift` is valid). `physical:` is **not** a
general prefix any more — it survives only as those literal compat-table strings.

### Full-binding parse (`Parser`, Binding.zig:74-214)

- **Flag prefixes** are stripped first (`parseFlags`, Binding.zig:148-187):
  `all:`, `global:`, `unconsumed:`, `performable:`, each at most once (duplicate →
  `InvalidFormat`); an unknown prefix stops flag parsing and falls through to
  trigger parsing. Flags combine in any order.
- The `=` separating trigger(s) from action is found by a scan that **skips `=`
  followed by `+` or `=`** (Binding.zig:98-126). So `=` can be a key: `==ignore`,
  `ctrl+==text:=hello`, `=+ctrl=...` all parse.
- **String action params are the raw remainder after the first `:` — no
  unescaping and no copy at parse time** (Binding.zig:1273-1277). The string
  aliases into the input buffer; the caller must `clone` (arena) to own it.
- `chain` is detected when a trigger part literally equals `"chain"`
  (Binding.zig:129); chains may not carry flag prefixes.
- **Global/all bindings cannot be sequences**: if more triggers follow and
  `flags.global or flags.all`, `Parser.next` returns `InvalidFormat`
  (Binding.zig:194-197).

---

## 4. Sequences (leader keys) and chains

**Sequences** — triggers separated by `>` (`ctrl+a>ctrl+b=action`). Split on `>`;
each segment goes through `Trigger.parse`; empty segment (`>a`, `a>`, ``) →
`InvalidFormat`. **No max length.** Storage: each non-final trigger becomes a
`Value.leader` holding a heap child `*Set`; the final trigger is a `leaf` in the
innermost set (Binding.zig:2098-2105, 2376-2466). Runtime matching (leader stack,
timeout, flush-on-invalid) lives in Surface, except `end_key_sequence` (§5).

**Chains** — `chain=action` appends an additional action to the **most recently
added leaf**, converting `leaf` → `leaf_chained` (an `ArrayList(Action)`). Attached
via a pointer-based `chain_parent` that is invalidated by `getOrPut`/remove/unbind/
clone (Binding.zig:2085-2095, 2586-2638). `chain=unbind` and flag-prefixed or
sequenced chains are errors. Chained actions are excluded from the reverse map.

---

## 5. Binding flags / prefixes

`Flags` packed struct (Binding.zig:31-70), C bit layout consumed=1, all=2,
global=4, performable=8:

| Prefix         | Field                | Meaning                                                                                                                 |
| -------------- | -------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| (default)      | `consumed = true`    | when the action fires, the key event is consumed and **not** encoded to the pty                                         |
| `unconsumed:`  | `consumed = false`   | action fires **and** the key is still encoded/forwarded to the pty                                                      |
| `all:`         | `all = true`         | forwarded to **all** active surfaces, not just the focused one                                                          |
| `global:`      | `global = true`      | system-wide binding, fires even when the app is unfocused ("may not work on all platforms")                             |
| `performable:` | `performable = true` | binding fires **only if the action can be performed**; otherwise the key falls through to normal encoding as if unbound |

Legality: any combination is legal (each at most once); `global:`/`all:` cannot be
sequences; no flags on `chain=`.

**`performable:` is the subtlest semantic.** `Surface.performBindingAction` returns
`!bool`; `false` means "did nothing / not supported". When a binding's flags have
`performable=true` and the chosen action(s) all return `false`, Ghostty treats the
whole key event **as if no binding existed** and flushes through to normal key
encoding (Surface.zig:3032-3041). Concretely: `performable:` on `cmd+c` lets
Ctrl/Cmd+C still send its raw byte when there is no selection to copy. Many
defaults rely on this (§7). Additionally, **performable bindings are excluded from
the reverse map** so GUI toolkits don't register them as menu accelerators
(Binding.zig:2072-2077).

---

## 6. The action enum — all 85 variants, with port status

`Action` is a tagged union (Binding.zig:303-973). Config spelling = Zig tag name.
The **Wired?** column is this port's proposed initial status against what the Rust
app can already do (§9) and what T4 built:

- **wired** — an equivalent action already exists in the app; the port routes the
  binding to it.
- **new** — behavior must be implemented in this thread (or is a thin forward to a
  T4-built apprt behavior we add the key for).
- **app** — app-scoped, forwarded to the app layer (`App.zig` upstream); some are
  macOS-only OS integrations.
- **stub** — parsed and accepted, but no-op with a logged note initially (explicit
  stub row in feature-coverage).

Dispatch column: **S** = handled inside `Surface.performBindingAction`; **A** =
app-scoped, forwarded to `App.zig::performAction`.

| #   | Action                       | Param                                                       | Summary                                                              | Disp | Wired?                             |
| --- | ---------------------------- | ----------------------------------------------------------- | -------------------------------------------------------------------- | ---- | ---------------------------------- |
| 1   | `ignore`                     | —                                                           | swallow key, not forwarded to child                                  | A    | new                                |
| 2   | `unbind`                     | —                                                           | pseudo-action: remove a binding, never stored                        | —    | new                                |
| 3   | `csi`                        | string                                                      | send CSI seq (no `ESC [` header)                                     | S    | new                                |
| 4   | `esc`                        | string                                                      | send ESC seq                                                         | S    | new                                |
| 5   | `text`                       | string                                                      | send text (Zig string-literal syntax)                                | S    | **wired** (existing `text:` table) |
| 6   | `cursor_key`                 | struct                                                      | send per DECCKM mode; **not settable from config** (`InvalidAction`) | S    | n/a                                |
| 7   | `reset`                      | —                                                           | full terminal reset                                                  | S    | new                                |
| 8   | `copy_to_clipboard`          | enum{plain,vt,html,mixed}=mixed                             | copy selection                                                       | S    | **wired** (menu Copy)              |
| 9   | `paste_from_clipboard`       | —                                                           | paste default clipboard                                              | S    | **wired** (menu Paste)             |
| 10  | `paste_from_selection`       | —                                                           | paste selection clipboard                                            | S    | new                                |
| 11  | `copy_url_to_clipboard`      | —                                                           | copy URL under cursor                                                | S    | new                                |
| 12  | `copy_title_to_clipboard`    | —                                                           | copy terminal title                                                  | A    | new                                |
| 13  | `increase_font_size`         | f32                                                         | +N points                                                            | S    | **wired** (FontSizeUp)             |
| 14  | `decrease_font_size`         | f32                                                         | −N points                                                            | S    | **wired** (FontSizeDown)           |
| 15  | `reset_font_size`            | —                                                           | reset to configured size                                             | S    | **wired** (FontSizeReset)          |
| 16  | `set_font_size`              | f32                                                         | set to N points                                                      | S    | new                                |
| 17  | `search`                     | string                                                      | start search for text                                                | S    | **wired** (search subsystem)       |
| 18  | `search_selection`           | —                                                           | search current selection                                             | S    | new                                |
| 19  | `navigate_search`            | enum{previous,next}                                         | move through results                                                 | S    | **wired** (Next/Previous)          |
| 20  | `start_search`               | —                                                           | open search UI                                                       | S    | **wired** (Start)                  |
| 21  | `end_search`                 | —                                                           | close search UI                                                      | S    | **wired** (End)                    |
| 22  | `clear_screen`               | —                                                           | clear screen + scrollback                                            | S    | new                                |
| 23  | `select_all`                 | —                                                           | select whole screen                                                  | S    | new (T4 selection)                 |
| 24  | `scroll_to_top`              | —                                                           | scroll to top                                                        | S    | new                                |
| 25  | `scroll_to_bottom`           | —                                                           | scroll to bottom                                                     | S    | new                                |
| 26  | `scroll_to_selection`        | —                                                           | scroll to selection                                                  | S    | new                                |
| 27  | `scroll_to_row`              | usize                                                       | scroll to absolute row                                               | S    | new                                |
| 28  | `scroll_page_up`             | —                                                           | up one page                                                          | S    | new                                |
| 29  | `scroll_page_down`           | —                                                           | down one page                                                        | S    | new                                |
| 30  | `scroll_page_fractional`     | f32                                                         | scroll by fraction of a page                                         | S    | new                                |
| 31  | `scroll_page_lines`          | i16                                                         | scroll by N lines                                                    | S    | new                                |
| 32  | `adjust_selection`           | enum(10 dirs)                                               | extend selection in direction                                        | S    | new (T4 selection)                 |
| 33  | `jump_to_prompt`             | i16                                                         | jump N prompts (shell integration)                                   | S    | new                                |
| 34  | `write_scrollback_file`      | WriteScreen                                                 | dump scrollback → temp file, copy/paste/open                         | S    | new                                |
| 35  | `write_screen_file`          | WriteScreen                                                 | dump screen → temp file                                              | S    | new                                |
| 36  | `write_selection_file`       | WriteScreen                                                 | dump selection → temp file                                           | S    | new                                |
| 37  | `new_window`                 | —                                                           | open new window                                                      | A    | **wired** (NewWindow)              |
| 38  | `new_tab`                    | —                                                           | open new tab                                                         | A    | **wired** (NewTab)                 |
| 39  | `previous_tab`               | —                                                           | previous tab                                                         | A    | **wired** (PreviousTab)            |
| 40  | `next_tab`                   | —                                                           | next tab                                                             | A    | **wired** (NextTab)                |
| 41  | `last_tab`                   | —                                                           | last tab                                                             | A    | **wired** (LastTab)                |
| 42  | `goto_tab`                   | usize                                                       | go to tab N (1-based, clamps)                                        | A    | **wired** (GotoTab)                |
| 43  | `move_tab`                   | isize                                                       | move tab by offset (wraps)                                           | A    | new                                |
| 44  | `toggle_tab_overview`        | —                                                           | tab overview (Linux/libadwaita)                                      | A    | stub (linux)                       |
| 45  | `prompt_surface_title`       | —                                                           | popup to set surface title                                           | A    | new                                |
| 46  | `prompt_tab_title`           | —                                                           | popup to set tab title                                               | A    | new                                |
| 47  | `set_surface_title`          | string                                                      | set surface title (empty resets)                                     | A    | new                                |
| 48  | `set_tab_title`              | string                                                      | set tab title (empty clears)                                         | A    | new                                |
| 49  | `new_split`                  | enum{right,down,left,up,auto}=auto                          | create split                                                         | A    | **wired** (NewSplit)               |
| 50  | `goto_split`                 | enum{previous,next,up,left,down,right}(+top/bottom aliases) | focus split                                                          | A    | **wired** (GotoSplit/GotoAdjacent) |
| 51  | `goto_window`                | enum{previous,next}                                         | focus window                                                         | A    | new                                |
| 52  | `toggle_split_zoom`          | —                                                           | zoom/unzoom split                                                    | A    | **wired** (ToggleZoom)             |
| 53  | `toggle_readonly`            | —                                                           | toggle read-only surface                                             | S    | new                                |
| 54  | `resize_split`               | (dir, u16)                                                  | resize split by px                                                   | A    | **wired** (ResizeSplit)            |
| 55  | `equalize_splits`            | —                                                           | equalize split sizes                                                 | A    | **wired** (EqualizeSplits)         |
| 56  | `reset_window_size`          | —                                                           | reset to default size (macOS)                                        | A    | new                                |
| 57  | `inspector`                  | enum{toggle,show,hide}                                      | terminal inspector                                                   | A    | stub (no inspector yet)            |
| 58  | `show_gtk_inspector`         | —                                                           | GTK inspector (no-op macOS)                                          | A    | stub (linux)                       |
| 59  | `show_on_screen_keyboard`    | —                                                           | OSK (Linux/GTK)                                                      | A    | stub (linux)                       |
| 60  | `open_config`                | —                                                           | open config in editor                                                | A    | new                                |
| 61  | `reload_config`              | —                                                           | reload configuration                                                 | A    | new (§ config-core)                |
| 62  | `close_surface`              | —                                                           | close surface                                                        | S    | **wired** (CloseTab/close_surface) |
| 63  | `close_tab`                  | enum{this,other,right}=this                                 | close tab(s)                                                         | A    | **wired** (partial: `this`)        |
| 64  | `close_window`               | —                                                           | close window + all tabs                                              | A    | new                                |
| 65  | `close_all_windows`          | —                                                           | **DEPRECATED** no-op; use `all:close_window`                         | A    | stub (deprecated)                  |
| 66  | `toggle_maximize`            | —                                                           | maximize/unmaximize (no-op macOS)                                    | A    | stub (linux)                       |
| 67  | `toggle_fullscreen`          | —                                                           | fullscreen/unfullscreen                                              | A    | new                                |
| 68  | `toggle_window_decorations`  | —                                                           | toggle decorations (Linux)                                           | A    | stub (linux)                       |
| 69  | `toggle_window_float_on_top` | —                                                           | always-on-top (macOS)                                                | A    | new                                |
| 70  | `toggle_secure_input`        | —                                                           | secure keyboard input (macOS)                                        | A    | new                                |
| 71  | `toggle_mouse_reporting`     | —                                                           | toggle mouse reporting                                               | S    | new                                |
| 72  | `toggle_command_palette`     | —                                                           | command palette (Linux libadwaita)                                   | A    | stub (no palette yet)              |
| 73  | `toggle_quick_terminal`      | —                                                           | quake drop-down terminal                                             | A    | stub (no quick-term yet)           |
| 74  | `toggle_visibility`          | —                                                           | show/hide all windows (macOS)                                        | A    | new                                |
| 75  | `toggle_background_opacity`  | —                                                           | toggle transparency (macOS)                                          | A    | new                                |
| 76  | `check_for_updates`          | —                                                           | check for updates (macOS)                                            | A    | stub (no updater)                  |
| 77  | `undo`                       | —                                                           | undo window/tab/split op (macOS)                                     | A    | stub (no undo stack yet)           |
| 78  | `redo`                       | —                                                           | redo (macOS)                                                         | A    | stub                               |
| 79  | `end_key_sequence`           | —                                                           | end active sequence, flush prior keys (not this one)                 | S    | new (with §4)                      |
| 80  | `activate_key_table`         | string                                                      | push named key table                                                 | S    | new (with §4)                      |
| 81  | `activate_key_table_once`    | string                                                      | push table, pop after first valid bind                               | S    | new                                |
| 82  | `deactivate_key_table`       | —                                                           | pop current key table                                                | S    | new                                |
| 83  | `deactivate_all_key_tables`  | —                                                           | pop all key tables                                                   | S    | new                                |
| 84  | `quit`                       | —                                                           | quit the app                                                         | A    | **wired** (Quit)                   |
| 85  | `crash`                      | enum{main,io,render}                                        | deliberately panic, for crash-report testing                         | S    | new (debug)                        |

### Action parameter parsing (Binding.zig:1253-1317)

Split on the first `:`; the name is matched against union field names. Rules:

- `void` fields: any `:param` → `InvalidFormat`.
- `[]const u8` fields: `:` **required**; param is the raw remainder, **no
  unescaping at parse time** (the `text:` unescaping happens later at send).
- Custom `parse` hooks: `SplitFocusDirection` (top/bottom aliases), `WriteScreen`
  (optional `,format`).
- enum → `stringToEnum`; int → base-10 `parseInt`; float → `parseFloat`
  (`+0.5` accepted); tuple struct → split on `,`, each element typed (arity errors).
- Missing `:` but the field type declares a `default` decl → use it
  (`copy_to_clipboard`→mixed, `new_split`→auto, `close_tab`→this).
- Unknown action name → `InvalidAction`.
- **Action hashing** bit-casts floats (NaN/±0 caveat) and deep-hashes string
  *contents* (not pointers) (Binding.zig:1613-1631) — the Rust port's
  `Hash`/`Eq` on the action enum must match (contents, not `Rc` identity).

---

## 7. The default keymap (verbatim, upstream `Keybinds.init()`)

`Config.zig:6389-7158` installs **130 raw `.put`/`.putFlags` call sites**.
Effective active bindings: **93 on macOS**, 80 (79 distinct) on Linux/Windows. The
`ctrlOrSuper(mods)` helper (key.zig:862-871) sets `super` on Darwin, `ctrl`
elsewhere — this is why one call site yields a different trigger per OS.

Since this port targets macOS first, the **macOS-effective keymap is the law to
replicate**. Two integrity facts to carry over exactly:

- **`ctrl+shift+w` is registered twice on non-Darwin** (Config.zig:6575 then 6595);
  the first (`close_surface`) is **dead** — `Set.put` overwrites it with
  `close_tab(this)` before `init()` returns. The port's `Set::put` must have the
  same last-wins overwrite so the shipped default matches.
- **goto-tab digits register two triggers each** (physical `digit_N` **and**
  unicode `N`) so non-US layouts (AZERTY) work, and set `performable = !isDarwin`
  purely so the macOS tab-bar menu-shortcut label lookup via the reverse map still
  resolves (Config.zig:6799-6835). Replicate both the dual-trigger install and the
  platform-conditional performable flag.

### macOS-active default keymap

Reconstructed triggers as installed (mac spelling). `P` = `performable:true`.

#### Config / clipboard / font

| Trigger              | Action                    | P   | Cite |
| -------------------- | ------------------------- | --- | ---- |
| `cmd+shift+,`        | `reload_config`           |     | 6399 |
| `cmd+,`              | `open_config`             |     | 6404 |
| `Copy` (media key)   | `copy_to_clipboard:mixed` |     | 6411 |
| `Paste` (media key)  | `paste_from_clipboard`    |     | 6416 |
| `cmd+c`              | `copy_to_clipboard:mixed` | P   | 6448 |
| `cmd+v`              | `paste_from_clipboard`    | P   | 6454 |
| `cmd+=`              | `increase_font_size:1`    |     | 6466 |
| `cmd++`              | `increase_font_size:1`    |     | 6471 |
| `cmd+-`              | `decrease_font_size:1`    |     | 6477 |
| `cmd+0`              | `reset_font_size`         |     | 6482 |
| `shift+ctrl+super+j` | `write_screen_file:copy`  |     | 6488 |
| `cmd+shift+j`        | `write_screen_file:paste` |     | 6494 |
| `cmd+shift+alt+j`    | `write_screen_file:open`  |     | 6500 |

**Expand selection** (all `performable:true`, lines 6507-6549)

`shift+Left/Right/Up/Down` → `adjust_selection:left/right/up/down`;
`shift+PageUp/PageDown` → `adjust_selection:page_up/page_down`;
`shift+Home/End` → `adjust_selection:home/end`.

#### Tabs (all platforms)

| Trigger          | Action         | Cite |
| ---------------- | -------------- | ---- |
| `ctrl+shift+Tab` | `previous_tab` | 6557 |
| `ctrl+Tab`       | `next_tab`     | 6562 |

#### Fullscreen / zoom / palette (all platforms)

| Trigger           | Action                   | Cite |
| ----------------- | ------------------------ | ---- |
| `cmd+Enter`       | `toggle_fullscreen`      | 6850 |
| `cmd+shift+Enter` | `toggle_split_zoom`      | 6857 |
| `cmd+shift+p`     | `toggle_command_palette` | 6864 |

**goto-tab digits** — `cmd+1`…`cmd+8` → `goto_tab:1`…`8`, `cmd+9` → `last_tab`
(each digit installs physical + unicode triggers; `performable=false` on mac by
design), lines 6780-6846.

**macOS-only block (Config.zig:6871-7157).** `P` = `performable:true`.

| Trigger                       | Action                          | P   |
| ----------------------------- | ------------------------------- | --- |
| `cmd+q`                       | `quit`                          |     |
| `cmd+k`                       | `clear_screen`                  | P   |
| `cmd+a`                       | `select_all`                    |     |
| `cmd+shift+t`                 | `undo`                          | P   |
| `cmd+z`                       | `undo`                          | P   |
| `cmd+shift+z`                 | `redo`                          | P   |
| `cmd+Home`                    | `scroll_to_top`                 |     |
| `cmd+End`                     | `scroll_to_bottom`              |     |
| `cmd+PageUp`                  | `scroll_page_up`                |     |
| `cmd+PageDown`                | `scroll_page_down`              |     |
| `cmd+j`                       | `scroll_to_selection`           | P   |
| `cmd+shift+Up`                | `jump_to_prompt:-1`             |     |
| `cmd+shift+Down`              | `jump_to_prompt:1`              |     |
| `cmd+n`                       | `new_window`                    |     |
| `cmd+w`                       | `close_surface`                 |     |
| `cmd+alt+w`                   | `close_tab:this`                |     |
| `cmd+shift+w`                 | `close_window`                  |     |
| `cmd+shift+alt+w`             | `close_all_windows`             |     |
| `cmd+t`                       | `new_tab`                       |     |
| `cmd+shift+[`                 | `previous_tab`                  |     |
| `cmd+shift+]`                 | `next_tab`                      |     |
| `cmd+d`                       | `new_split:right`               |     |
| `cmd+shift+d`                 | `new_split:down`                |     |
| `cmd+[`                       | `goto_split:previous`           |     |
| `cmd+]`                       | `goto_split:next`               |     |
| `cmd+alt+Up/Down/Left/Right`  | `goto_split:up/down/left/right` |     |
| `cmd+ctrl+Up/Down/Left/Right` | `resize_split:*,10`             |     |
| `cmd+ctrl+=`                  | `equalize_splits`               |     |
| `cmd+Up`                      | `jump_to_prompt:-1`             |     |
| `cmd+Down`                    | `jump_to_prompt:1`              |     |
| `cmd+f`                       | `start_search`                  | P   |
| `cmd+e`                       | `search_selection`              | P   |
| `cmd+shift+f`                 | `end_search`                    | P   |
| `Escape`                      | `end_search`                    | P   |
| `cmd+g`                       | `navigate_search:next`          | P   |
| `cmd+shift+g`                 | `navigate_search:previous`      | P   |
| `alt+cmd+i`                   | `inspector:toggle`              |     |
| `cmd+ctrl+f`                  | `toggle_fullscreen`             |     |
| `cmd+shift+v`                 | `paste_from_selection`          |     |
| `cmd+Right`                   | `text:\x05` (EOL)               |     |
| `cmd+Left`                    | `text:\x01` (BOL)               |     |
| `cmd+Backspace`               | `text:\x15` (kill line)         |     |
| `alt+Left`                    | `esc:b` (word back)             |     |
| `alt+Right`                   | `esc:f` (word fwd)              |     |

The Linux/Windows-active block (non-Darwin, Config.zig:6569-6779) differs mainly
in mod spelling (`ctrl+shift+*` for window/tab/split ops, `alt+N` for goto-tab) and
is captured verbatim in the research note; it is deferred behind the macOS set for
the first slices but the default generator should install both under a
`cfg(target_os)` split so Linux (T7) inherits it for free.

---

## 8. Set storage & lookup semantics (the runtime contract)

`Set` (Binding.zig:2045-2843) holds a forward map `Trigger → Value` and a reverse
map `Action → Trigger` (for menu accelerators). Key facts a Rust port must honor:

- **Forward-map equality is folded**: `bindingSetEqual` = `foldedEqual`, so
  `ctrl+A` and `ctrl+a` collide (ASCII tolower fast path, else full Unicode case
  folding; multi-codepoint folds fall back to identity) (Binding.zig:1942-2005).
  The hash uses the folded codepoint + `mods.binding()`.
- **`getEvent` probe order** (Binding.zig:2657-2695): (1) physical key + binding
  mods; (2) unicode from `event.utf8` **iff exactly one codepoint**; (3) unicode
  from `unshifted_codepoint` if > 0; (4) `catch_all` **with** mods; (5) `catch_all`
  with **empty** mods. Physical triggers never match unicode events and vice
  versa.
- **Consumed/unconsumed is not part of lookup** — it is read from the matched
  leaf's flags by the caller and decides whether to also encode the key.
- **Rebinding overwrites**: `put` over an existing trigger deinits the old value
  (leader child set destroyed, reverse entry scrubbed) and installs a plain leaf.
- **Reverse map** holds only the most-recently-added, **non-sequenced,
  non-performable, non-chained** binding per action; removal repoints it to "any
  other" binding with the same action via a hash scan (Binding.zig:2060-2078,
  2740-2780).
- **`unbind` on a sequence** recursively prunes empty leader sets and restores the
  prior value if a re-bind errors; `parseAndPut` fully validates before mutating so
  a parse error never corrupts the set.
- **Strings are not owned at parse time** — `clone` with an arena is how config
  makes action strings durable. The Rust port sidesteps this: parse directly into
  owned `String`/`Box<[u8]>`.

Tests worth porting 1:1 (Binding.zig bottom, listed in the research note): plus/
equals-sign parsing, backward-compat table round-trip, global/all sequence
rejection, chain lifecycle, `getEvent` physical-vs-codepoint separation and
case-folding both directions, catch_all mods-then-no-mods fallback, and the
reverse-map maintenance/performable-exclusion cases.

---

## 9. Rust current state — what the port displaces

(Current-code cites are against `work/t3` at branch time.)

- **Key event flow.** Two AppKit entry points on `TerminalView`
  (`crates/qwertty-term/src/view.rs`): `performKeyEquivalent:` runs first and tries
  search → split → tab chords (each an exact `(Key, TabMods)` match); a hit returns
  `true` and the event never reaches `keyDown:`. `keyDown:` then tries the user
  `text:` table, then IME (`interpretKeyEvents`), then the real encoder
  (`qwertty_term_input::key_encode::encode` via `Controller::encode_key_to_surface`).
  **Every `resolve()` is `None`-on-no-match**, so the code already falls through to
  pty encoding — exactly the consumed/unconsumed discipline `Set` needs.
- **The seam.** Each `resolve()` becomes one `Set::get_event(&KeyEvent) ->
  Option<&Leaf>` call over a much larger trigger/action space, still returning
  `None` to fall through. `performKeyEquivalent:` remains the place to catch chords
  AppKit would otherwise eat (`ctrl+Tab`), but consults the unified `Set`.
- **Reuse, don't reinvent.** `qwertty_term_input` already ports `Key` (W3C physical
  codes, `from_w3c`/`w3c`), the full `Mods` with `binding()`/`translation()`/`ALIAS`,
  `KeyEvent::binding_hash()`, and even a `RemapSet` (`key-remap`, built but unwired).
  The port should retire `crate::tabkeys::TabMods` in favour of `Mods::binding()`
  and use `binding_hash()` as the `Set` key.
- **Gaps to build.** Leader keys / key tables, `global:`/`all:`/`performable:`
  prefixes, the `physical:`/unicode trigger disambiguation (current `keybind.rs`
  infers mod-vs-key positionally), and ~60 missing actions.

---

## 10. Port design & ordered slices

**Crate placement.** The trigger/action model and `Set` are UI-agnostic and belong
in `qwertty_term_input` (next to `Key`/`Mods`/`key_encode`), so the app crate and a
future Linux apprt share them. Dispatch glue (mapping an action to a `Controller`
method / apprt call) stays in the app crate. This keeps the wave-gate surface
small: the `Set`/parser/defaults land in the input crate (T3 territory-adjacent,
file-claim), and only the dispatch swap touches the app crate (gated on T4).

**Data model.**

```rust
enum TriggerKey { Physical(Key), Unicode(u32), CatchAll }
struct Trigger { key: TriggerKey, mods: Mods }   // mods already .binding()-normalized
struct Flags { consumed: bool, all: bool, global: bool, performable: bool }
enum Action { Ignore, Csi(Box<str>), Text(Box<[u8]>), CopyToClipboard(CopyFmt), ... } // 85 - cursor_key
enum Value { Leader(Box<Set>), Leaf { action: Action, flags: Flags }, LeafChained { actions: Vec<Action>, flags: Flags } }
struct Set { forward: IndexMap<Trigger, Value>, reverse: HashMap<Action, Trigger> }
```

Forward-map hashing/eq uses the **folded** codepoint + `mods.binding()` (custom
`Hash`/`Eq` or a pre-computed `binding_hash`), matching §8.

**Slices (ordered; each flips `docs/feature-coverage.md` boxes):**

- **(a) Model + parse + defaults, pure.** Trigger/action/flags types, `Trigger::parse`
  with the exact 10-rule order (§3), action param parsing (§6), the `Set` with
  put/overwrite/reverse/unbind, and a **generated default keymap** from §7 under a
  `cfg(target_os)` split. Port the Binding.zig test suite 1:1 (§8). No app-crate
  code — lands in the input crate. **Gate-safe under the wave gate.**
- **(b) Dispatch integration** (needs T4 drained): replace the four bespoke tables
  with one `Set::get_event` call at the `performKeyEquivalent:`/`keyDown:` seam;
  wire the ~25 already-implemented actions to their existing `Controller` methods;
  honor `consumed`/`performable` fallthrough. Retire `TabMods`. **Every existing
  smoke stays green** — that is the acceptance test for the swap. `text:` keeps
  working (now just one action among many).
- **(c) Leader keys, key tables, `global:`/`all:`.** Sequence matching with the
  leader stack + timeout + flush-on-invalid; `activate_key_table*` /
  `deactivate*` / `end_key_sequence`; `global:` via a system hotkey registration
  (macOS `CGEventTap`/`NSEvent` global monitor — likely its own mini-ADR).
- **(d) The action long tail.** `jump_to_prompt`, `scroll_page_*`, `write_*_file`,
  `adjust_selection`, `set_font_size`, `csi`/`esc`/`reset`, `crash`, etc. — each
  lands with its behavior or an explicit stub note in the §6 table. macOS OS
  integrations (`toggle_visibility`, `toggle_secure_input`, `check_for_updates`,
  `undo`/`redo`) land as their AppKit calls exist; Linux/GTK-only actions stay
  stubbed with logged no-ops.

**Method rules.** Defaults are LAW — every default trigger cited to Config.zig
(§7). Dispatch sits in the hot key path: **no per-event allocation** in
`get_event` (the probe triggers are stack values; the folded hash is cheap) — bench
the typing smoke before/after. Unknown/unsupported `keybind` entries warn-and-skip,
never fail startup (house rule), matching today's `keybind.rs` behavior.

---

## 11. Open questions (mini-ADR candidates)

1. **`global:` on macOS** — global hotkeys need a `CGEventTap` or `NSEvent`
   `addGlobalMonitorForEvents`, plus Accessibility permission. Scope: is `global:`
   in the first-class set or deferred with a stub-warn? (Leaning: parse-and-accept
   in (a), implement in (c) behind an ADR on the permission story.)
2. **`text:`/`csi`/`esc` escape syntax** — upstream stores the raw string and
   unescapes at send using Zig string-literal rules; our `keybind.rs` already
   implements a compatible subset (`\n \r \t \\ \" \' \0 \e \xNN \u{NNNN}`). Confirm
   we keep exactly that subset so Josh's `shift+enter=text:\x1b\r` is unchanged.
3. **Config surface in TOML** — upstream's `keybind` is a repeatable scalar; we
   already accept a TOML **array** of `"trigger=action"` strings. Keep the array
   form (it is the natural TOML shape) and document it as the ADR'd deviation; the
   grammar inside each string is byte-identical to upstream.
