# Config core: load order, includes, and reload-safety (T3 analysis)

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse HEAD`). This note specifies the config
**machinery** — load order, the two-location macOS merge, `config-file` includes,
`config-default-files`, theme resolution, conditional (light/dark) config, CLI
overrides, and — the crux — **what re-applies live on reload vs what needs a
restart**. It is the design spec for the T3 "config core" slice.

TOML stays as our surface syntax (the ADR'd deviation); upstream's **semantics per
mechanism** are the spec. Where our current implementation diverges, §7 lists the
gap. Line cites are `Config.zig:NNNN` etc. at the pinned commit; current-code cites
are against `work/t3` at branch time.

---

## 1. Load order, end to end

`Config.load()` is the canonical sequence (Config.zig:3850-3865):

1. **`default(alloc)`** — struct defaults + the default keymap (see
   [keybinds.md](keybinds.md) §7) + default command-palette entries + the default
   URL link matcher.
2. **`loadDefaultFiles`** — the two-location chain (§2).
3. **`loadCliArgs`** — `--key=value` overrides (§4).
4. **`loadRecursiveFiles`** — processes every `config-file` include accumulated so
   far, **after** CLI args (§3).
5. **`finalize`** — theme loads *here*, plus defaulting/clamping (§5).

Every parsed arg is recorded as a **replay step** (`_replay_steps`,
Config.zig:3832; `Replay.Step` = arg / expand / conditional_arg / diagnostic / `-e`).
This is the load-bearing architectural choice: **reload, theme layering, and
light/dark switching are all implemented by replaying recorded steps into a fresh
`Config`, never by re-reading files** (Config.zig:5121-5237). A port that wants
upstream's reload guarantees should adopt the same replay model rather than
re-parsing on every reload.

---

## 2. The two macOS locations and their merge rule

`loadDefaultFiles` loads, **in order** (Config.zig:4040-4110, file_load.zig):

1. legacy XDG `$XDG_CONFIG_HOME/ghostty/config`
2. XDG `$XDG_CONFIG_HOME/ghostty/config.ghostty`
3. macOS only: legacy `~/Library/Application Support/<bundle-id>/config`
4. macOS only: `~/Library/Application Support/<bundle-id>/config.ghostty`
   (bundle id `com.mitchellh.ghostty`; skipped if identical to the preferred path)

**Merge rule: there is no "which wins" selection — every file that exists is loaded,
in the order above, and later assignments override earlier ones** (last-wins for
scalars, append for repeatables). So on macOS the App Support file effectively
overrides the XDG file, and `config.ghostty` overrides legacy `config` in each
location. If both legacy and new exist in one location, Ghostty logs "loading them
both in that order" (Config.zig:4049-4090).

All default files load via `loadOptionalFile`: **missing is fine; any other error
is logged and the file skipped** (Config.zig:3996-4014). Empty files are treated as
`FileIsEmpty`. Non-files (dirs/sockets) are warned and skipped. A UTF-8 BOM is
skipped. If **no** default file exists anywhere, a commented template is written
(App Support on macOS, XDG elsewhere).

After each file loads, `expandPaths(dirname(file))` converts every `Path`/
`RepeatablePath` field to absolute relative to that file's directory (and does `~/`
expansion), recording an `expand` replay step (Config.zig:3945, 4389-4420).

> **Note for our import converter.** Josh's real setup populates **both** locations
> (`~/.config/ghostty/config` = `theme = Aardvark Ink`; App Support config = font +
> theme + copy-on-select + font-size + a `keybind`). Correct import must merge both
> in this order, App Support winning on the duplicate `theme`. See
> [import-ghostty-config.md](import-ghostty-config.md).

---

## 3. `config-file` includes and `config-default-files`

**`config-file` (`RepeatablePath`, Config.zig:2477):**

- `?path` prefix marks an include optional (no error if missing); `"..."` quoting
  allows a literal leading `?`.
- Relative paths resolve against the **containing file's directory** (or CWD for a
  CLI `--config-file`).
- **Deferred processing** — includes are NOT loaded at the directive site.
  `loadRecursiveFiles` runs once, **after CLI args**, walking the accumulated list
  in order and appending newly-discovered includes to the end (breadth-first
  queue). Upstream documents the consequence: an include "does not take effect
  until after the entire configuration is loaded", so it can override keys set
  *after* the directive in the parent file (Config.zig:4217-4323, doc 2453-2476).
- **Cycle prevention is the only depth limit**: a set of already-loaded absolute
  paths; a repeat emits a "cycle detected" diagnostic and skips (also dedupes
  diamond includes). No numeric recursion cap.
- Open/stat failures are diagnostics, not fatal; these diagnostics are recorded as
  replay steps because include-loading is not re-run on replay.

**`config-default-files` (bool, default true, Config.zig:2480-2490):** documented as
**CLI-only** — setting it in a file is silently ineffective. It is force-reset to
true at the start of `loadCliArgs`; if the CLI set it false, the config is rebuilt
by replaying only the CLI-portion of the steps into an empty config, discarding
everything default files contributed.

---

## 4. CLI overrides

- Args are `--key=value` (value optional). Anything not `--`-prefixed is an
  "invalid field" diagnostic. `+action` args are consumed by the subcommand
  dispatcher, not the config parser.
- One parser (`cli.args.parse`, via `Config.loadIter`) drives CLI, files, and
  replays alike — files are just synthetic `--key=value` lines (each line gets `--`
  prepended; `#` comments and blanks skipped; a fully double-quoted value is
  unquoted; max line 4096; args.zig:1390-1510).
- **Repeated keys**: scalars last-wins; repeatable types append; an **empty value
  resets the list**. Special case: `font-family*` from the CLI *overwrites* file
  values (not append), via `overwrite_next` set only during CLI parsing.
- **`-e` command**: consumes all remaining argv as the command, records an `-e`
  replay marker, and implies `gtk-single-instance=false`,
  `quit-after-last-window-closed=true`, delay `null`, `shell-integration=detect`.
- **Errors accumulate, never abort** (§6).

---

## 5. Theme system and conditional (light/dark) config

**Theme (`?Theme`, Config.zig:592).** A plain value sets both light and dark; a
value containing `,`/`:`/`=` triggers pair parsing `light:NAME,dark:NAME` (both
halves required). Resolution (`themepkg.open`, theme.zig:110-240): an absolute path
loads directly; otherwise the name must contain no path separators and is searched
in priority order — **user dir** `$XDG_CONFIG_HOME/ghostty/themes/<name>` first,
then **resources** (`Ghostty.app/Contents/Resources/ghostty/themes` on macOS). A
theme file is an ordinary config file; `theme` and `config-file` inside it are
silently ignored.

**When themes load** — at the *start of `finalize`*, i.e. after defaults, default
files, CLI, and all includes (Config.zig:4520). `loadTheme` parses the theme into a
**fresh empty config**, marks the theme's replay steps conditional on
`theme==light|dark`, then **replays all prior user steps on top** — so any user-set
key beats the theme regardless of where it appeared.

**Conditional config (conditional.zig).** A *static typed state*
`{ theme: light|dark, os }`, not general key-values. Conditionals attach to replay
steps (currently only produced by theme loading). Re-evaluation
(`changeConditionalState`, Config.zig:4330-4385) returns early if no *used*
conditional key changed; otherwise it clones an empty config, sets the new state,
and replays all steps. **No files are re-read** — so a deleted config file cannot
break a light/dark switch. Trigger: an OS appearance change →
`Surface.colorSchemeCallback` → apprt `reload_config{ soft = true }`.

> **Scope call for T3.** Our config has no conditional/light-dark system today. The
> minimal faithful port is: support `theme = "light:X,dark:Y"` and drive it off the
> macOS `NSApp` effective-appearance change. Recommend landing plain `theme` +
> single-theme reload first, and the light/dark split as a follow-up slice (it needs
> the replay-step machinery to be worth doing properly).

---

## 6. Error philosophy — warn and continue

Everywhere except OOM, config problems are **diagnostics, not failures**
(cli/args.zig:40-179, Config.zig throughout):

- Parser: unknown fields, missing values, invalid enum values become `Diagnostic`
  entries with a location (CLI arg index or file:line); parsing continues.
- Default files: any error but not-found is logged and the file skipped.
- Includes: open/stat/cycle problems are diagnostics (and recorded as replay steps).
- Themes: every failure mode appends diagnostics and returns null; load proceeds
  without the theme.
- Runtime reload: a failed conditional-state or DerivedConfig build logs and keeps
  the previous good config.

Surfacing diagnostics to the user is the apprt's job; the core accumulates
`_diagnostics` and logs. This **matches our existing house rule** (warn-and-skip,
never fail startup) — the port keeps it; the upgrade is *structured* diagnostics
with file:line locations instead of ad-hoc `eprintln!`.

---

## 7. `config-reload`: what re-applies live vs needs a restart

This is the heart of the reload slice. Upstream reload flow: apprt reloads (or
reuses) the app Config, then each surface gets a `.change_config` mailbox message →
`Surface.updateConfig(original)` (Surface.zig:1711-1816):

1. Apply the surface's conditional state; on failure log and keep original.
2. Rebuild `DerivedConfig`; on failure **keep the old config** (1732-1741).
3. Un-hide mouse if `mouse-hide-while-typing` turned off; **drop any in-progress key
   sequence and deactivate key tables** (they hold pointers into the old config).
4. Rebuild and push a **new font grid** unconditionally, but font *size* follows the
   new config **only if the user hasn't manually adjusted it** on that surface
   (`font_size_adjusted`, 1764-1769).
5. Send the renderer `initChangeConfig` and termio a new `DerivedConfig`, then wake
   the renderer.
6. Re-apply configured `title`; emit apprt `config_change`.

**Re-applies live** (everything in `Surface.DerivedConfig`, Surface.zig:290-423):

- **keybinds + key tables + `key-remap`** — swapped wholesale (in-progress
  sequences dropped first).
- **fonts** — family/size/features/variations; the grid is rebuilt and cell size
  follows (subject to the manual-adjust exception).
- **clipboard** read/write/paste-protection, copy-on-select, right/middle-click
  actions, confirm-close-surface.
- **colors / background / cursor styling** — via the renderer change-config message.
- **mouse** interval/hide/reporting/scroll-multiplier/shift-capture,
  selection behavior + word chars, window padding, title + title-report, link
  matchers, scroll-to-bottom + command-finish notifications.
- shell-integration/scrollback go to termio.

**Startup-only / limited-effect** (per field doc comments):

| Field                                                                 | Reload behavior                         |
| --------------------------------------------------------------------- | --------------------------------------- |
| `language`                                                            | full restart                            |
| `font-size`                                                           | skips surfaces with a manual adjustment |
| `font-codepoint-map`, `grapheme-width-method`                         | new terminals only                      |
| `scrollback-limit`                                                    | new terminals only                      |
| `window-padding-x/-y`, `window-vsync`                                 | new terminals/windows only              |
| `background-opacity` (macOS)                                          | full restart                            |
| `macos-non-native-fullscreen`                                         | next fullscreen only                    |
| `macos-window-buttons`, `macos-titlebar-style`                        | new windows only                        |
| `quick-terminal-position` (macOS)                                     | restart                                 |
| `window-height`/`-width`/`-position`                                  | initial-size only by nature             |
| `config-default-files`, `initial-command`/`-e`, `gtk-single-instance` | startup/CLI-time                        |
| `async-backend`, `auto-update-channel`                                | full restart                            |

**Port guidance (reload-safety design).**

- Model reload as **build-a-new-Config-then-diff-apply**, keeping the old config on
  any failure (upstream's "never leave the surface worse than before" rule).
- Group config into a `DerivedConfig`-equivalent split by consumer: **renderer**
  (colors/cursor/opacity), **font grid** (family/size/metrics), **termio**
  (scrollback/shell-integration), **input** (keybinds/key-remap/mouse), **window
  chrome** (padding/title/decorations). Reload re-derives each group; the
  startup-only fields above are simply *not* in any group's live-apply path.
- **Always drop in-progress key sequences / key tables on reload** — they reference
  the old keymap.
- **Font size**: preserve a per-surface manual adjustment across reload.
- The natural first `config-reload` slice: the `reload_config` keybind action
  (already in [keybinds.md](keybinds.md) §6, #61) re-reads the file(s), rebuilds the
  Config, and re-applies the renderer + font + input groups. A windowed smoke should
  reload with a changed `font-size` and assert cell metrics change (per the T3
  spec's live-reload smoke requirement).

---

## 8. Current Rust state (what this replaces)

(Cites against `work/t3` at branch time; see [keybinds.md](keybinds.md) §9 and the
R1 research note for detail.)

- **8 TOML keys** (`theme`, `copy-on-select`, `font-size`, `font-family`,
  `mouse-scroll-multiplier`, `keybind`, `unfocused-split-opacity`,
  `unfocused-split-fill`) vs upstream's 204.
- **Single fixed path**: `$QWERTTY_TERM_CONFIG_DIR/config.toml` or
  `~/.config/qwertty-term/config.toml`. No two-location merge, no XDG chain, no
  includes, no `config-default-files`.
- **No reload** — `load()` runs once at `Controller::new`; no file watch, no
  `config-reload` action.
- **Error handling already warn-and-continue** — missing file writes a template and
  returns defaults; malformed TOML logs and returns defaults; unknown keys ignored
  (`#[serde(default)]`, no `deny_unknown_fields`); per-field bad values fall back to
  that field's default. This is the right philosophy; the port keeps it and adds
  structured file:line diagnostics.

---

## 9. Ordered slices for the config-core work

1. **Load order + two-location merge (M).** Add the XDG + macOS App Support chain
   with last-wins/append merge; keep TOML syntax. `config.toml` in each location;
   preserve the warn-and-skip semantics. (No app-crate behavior change beyond where
   the file is read — mostly `config.rs` territory.)
2. **`config-file` includes + `config-default-files` (M).** Deferred include queue
   with cycle detection; relative-to-including-file resolution; `?optional` prefix;
   CLI `--config-file` and `--config-default-files=false`.
3. **CLI `--flag=value` overrides (M).** Map TOML keys to `--key=value`; last-wins/
   append/empty-reset; feed the same parser as the file loader.
4. **`config-reload` (M, gated on T4 for the app-crate apply path).** Replay-model
   Config rebuild; `DerivedConfig`-style grouped live-apply; drop key sequences;
   preserve manual font size; windowed reload smoke (font-size → cell metrics).
5. **`theme` resolution + light/dark (M, follow-up).** User-dir + resources theme
   search; `light:X,dark:Y` pair driven by `NSApp` appearance change; needs the
   replay-step machinery from slice 4.
