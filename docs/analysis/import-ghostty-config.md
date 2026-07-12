# `+import-ghostty-config`: converter design & mapping table (T3 analysis)

Surveyed against ghostty commit `2da015cd6` (verify with
`git -C ~/local/ghostty rev-parse HEAD`). This note specifies the
`+import-ghostty-config` CLI converter: it reads a real Ghostty config (both macOS
default locations, plus `config-file` includes and referenced themes) and emits our
**TOML** config, passing through every key we support and emitting a commented,
explanatory line for every key we don't (yet) or can't (platform-N/A).

The **acceptance test** is round-tripping Josh's real config (§5). Load-order and
merge semantics come from [config-core.md](config-core.md); keybind grammar from
[keybinds.md](keybinds.md). The upstream option inventory (204 keys) is the source
of the mapping table (§4). House rule: **warn-and-skip, never fail** — an
unconvertible key becomes a comment, never an error.

---

## 1. What the converter does

```text
qwertty-term +import-ghostty-config [--path <file>] [--output <file>] [--dry-run]
```

1. **Locate inputs.** Default: the two macOS locations in load order (XDG then App
   Support; [config-core.md](config-core.md) §2), following `config-file` includes
   (deferred queue, cycle-detected) and any referenced `theme` files. `--path`
   overrides with a single explicit file.
2. **Parse** each file with Ghostty's line syntax (`key = value`, `#` comments,
   empty-value-resets, quoted values), preserving source order and origin
   (file:line) for diagnostics.
3. **Merge** across locations with upstream's last-wins/append rule so the emitted
   TOML reflects the *effective* config, not a naive concatenation.
4. **Convert** each key via the §4 mapping: supported keys become TOML; unsupported/
   platform-N/A keys become `# <reason>` comment lines that preserve the original
   value so nothing is silently lost.
5. **Emit** TOML (to `--output` or stdout with `--dry-run`), grouped by section with
   a header noting provenance and a summary of skipped keys.

---

## 2. Syntax conversion (Ghostty `key = value` → TOML)

| Ghostty form                         | TOML form                                 | Notes                                                |
| ------------------------------------ | ----------------------------------------- | ---------------------------------------------------- |
| `font-family = Iosevka`              | `font-family = "Iosevka"`                 | scalar string quoted                                 |
| `font-size = 16`                     | `font-size = 16`                          | int/float bare                                       |
| `mouse-hide-while-typing = true`     | `mouse-hide-while-typing = true`          | bool bare                                            |
| `background = #1e1e2e`               | `background = "#1e1e2e"`                  | color as string (our `Rgb::parse` accepts `#rrggbb`) |
| repeated `font-family = A` / `= B`   | `font-family = ["A", "B"]`                | repeatable → TOML array                              |
| `keybind = shift+enter=text:\x1b\r`  | `keybind = ["shift+enter=text:\\x1b\\r"]` | repeatable; **inner grammar unchanged** (see §3)     |
| `key =` (empty, reset)               | (drop / reset to default)                 | empty-value reset; emit nothing                      |
| `resize-overlay-duration = 4s 200ms` | `resize-overlay-duration = "4s 200ms"`    | duration kept as string, parsed our side             |
| `theme = light:A,dark:B`             | `theme = "light:A,dark:B"`                | pair kept as string                                  |

Rules: our keys keep upstream's **kebab-case names verbatim** (so a Ghostty user's
muscle memory transfers). Scalars that aren't plainly numeric/bool are quoted.
Repeatable keys accumulate into a TOML array (our `keybind` already uses this shape;
extend the same convention to other repeatables as they're wired). `#` comments and
blank lines are dropped except the auto-generated template header (skipped silently).

---

## 3. Value-format conversions worth calling out

- **keybind** — the string after `keybind =` is passed through **byte-identical**;
  the trigger/action grammar is the same in both tools (that is the whole point of
  the [keybinds.md](keybinds.md) port). Only TOML-quoting/escaping of the surrounding
  string changes (backslashes doubled inside a basic string, or use a literal
  string). Josh's `shift+enter=text:\x1b\r` must survive exactly.
- **colors** — Ghostty accepts `#rrggbb`, `rrggbb`, and named X11 colors; emit as a
  quoted `#rrggbb` (normalize named/bare forms to hex via our `Rgb::parse`).
- **palette** — `palette = N=COLOR` (repeatable, 256 slots) → TOML array of
  `"N=#rrggbb"` strings (or a dedicated table when the palette key is wired).
- **enums** — pass the value string through; our parser validates against the same
  variant names. Unknown variant → keep as comment with a "valid values: …"
  diagnostic rather than emitting an invalid TOML value.
- **durations** (`5s`, `750ms`, `4s 200ms`) — keep as string.
- **theme** — if the theme names a **custom** file in the user themes dir, note it
  in a comment (the converter can optionally also copy/convert the theme file);
  built-in theme names pass through.

---

## 4. Mapping table (all 204 upstream keys → import status)

Status legend:

- **now** — supported today (the 8 wired keys); pass straight through.
- **soon** — in the T3 option-surface wiring backlog; the converter should already
  emit it as a live TOML key (it round-trips even before the effect is wired) and
  rely on our warn-and-skip loader until the effect lands. Marked so we don't emit
  it as a comment.
- **key** — handled by the keybind system ([keybinds.md](keybinds.md)).
- **comment** — recognized, no equivalent yet → emit `# unsupported (not yet): <key> = <value>`.
- **n/a** — platform-N/A on our macOS target (Linux/GTK/X11) → emit
  `# platform-specific (linux/gtk), ignored: <key> = <value>`.

Grouped by upstream category (source order within each). Where a whole family shares
a status it's collapsed to one row.

### Fonts & text rendering

| Keys                                                                                                                                                               | Status                                        |
| ------------------------------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------- |
| `font-family`, `font-size`                                                                                                                                         | **now**                                       |
| `font-family-bold/-italic/-bold-italic`, `font-style*`, `font-synthetic-style`, `font-feature`, `font-variation*`, `font-codepoint-map`, `font-shaping-break`      | **soon** (T3 font-wiring batch)               |
| `font-thicken`, `font-thicken-strength`                                                                                                                            | **soon** (macOS)                              |
| `clipboard-codepoint-map`                                                                                                                                          | **soon**                                      |
| `adjust-*` (14 metric keys: cell-width/height, font-baseline, underline/strikethrough/overline pos+thickness, cursor-thickness/height, box-thickness, icon-height) | **soon** (T3 `adjust-*` batch → font Metrics) |
| `grapheme-width-method`                                                                                                                                            | **soon** (plumb to vt — file-claim)           |
| `alpha-blending`                                                                                                                                                   | **soon** (renderer)                           |
| `freetype-load-flags`                                                                                                                                              | **n/a** (linux)                               |

### Colors, cursor, selection

| Keys                                                                                                          | Status                                                          |
| ------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------- |
| `theme`                                                                                                       | **now**                                                         |
| `background`, `foreground`                                                                                    | **soon** (color group)                                          |
| `background-image*` (opacity/position/fit/repeat)                                                             | **comment** (no bg-image support)                               |
| `selection-foreground/-background`, `selection-clear-on-typing/-on-copy`, `selection-word-chars`              | **soon** (T4 selection built; T3 adds keys)                     |
| `minimum-contrast`                                                                                            | **now-ish** (feature-coverage notes already-wired)              |
| `palette`, `palette-generate`, `palette-harmonious`                                                           | **soon** (color group)                                          |
| `cursor-color`, `cursor-opacity`, `cursor-style`, `cursor-style-blink`, `cursor-text`, `cursor-click-to-move` | **soon** (T3 cursor group)                                      |
| `bold-color`, `faint-opacity`                                                                                 | **soon** (color group)                                          |
| `split-divider-color`, `unfocused-split-opacity`, `unfocused-split-fill`                                      | `unfocused-split-*` = **now**; `split-divider-color` = **soon** |
| `search-foreground/-background/-selected-*`                                                                   | **soon** (search colors)                                        |

### Mouse, clipboard, scroll

| Keys                                                                                        | Status                               |
| ------------------------------------------------------------------------------------------- | ------------------------------------ |
| `mouse-scroll-multiplier`                                                                   | **now**                              |
| `copy-on-select`                                                                            | **now**                              |
| `mouse-hide-while-typing`, `mouse-shift-capture`, `mouse-reporting`                         | **soon** (mouse gates; T4 behaviors) |
| `right-click-action`, `middle-click-action`, `click-repeat-interval`, `focus-follows-mouse` | **soon**                             |
| `clipboard-read/-write/-trim-trailing-spaces/-paste-protection/-paste-bracketed-safe`       | **soon** (clipboard gates)           |
| `scroll-to-bottom`                                                                          | **soon**                             |
| `scrollback-limit`                                                                          | **soon** (plumb to vt — file-claim)  |
| `image-storage-limit`                                                                       | **soon** (plumb to vt — file-claim)  |
| `scrollbar`                                                                                 | **comment** (no scrollbar UI yet)    |

### Window, tabs, splits

| Keys                                                                                                                               | Status                                                                                   |
| ---------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------- |
| `window-padding-x/-y`, `window-padding-balance`, `window-padding-color`                                                            | **soon** (window group)                                                                  |
| `window-title-font-family`, `title`, `title-report`                                                                                | **soon**                                                                                 |
| `maximize`, `fullscreen`                                                                                                           | **soon**                                                                                 |
| `window-decoration`, `window-theme`, `window-colorspace`, `window-vsync`                                                           | `window-theme`/`-colorspace`/`-vsync` = **soon** (macOS); `window-decoration` = **soon** |
| `window-height`/`-width`/`-position-x`/`-position-y`, `window-save-state`, `window-step-resize`, `window-new-tab-position`         | **soon** (macOS window group)                                                            |
| `window-inherit-working-directory`, `tab-inherit-working-directory`, `split-inherit-working-directory`, `window-inherit-font-size` | **soon**                                                                                 |
| `split-preserve-zoom`                                                                                                              | **soon**                                                                                 |
| `resize-overlay`, `resize-overlay-position`, `resize-overlay-duration`                                                             | **comment** (no resize overlay yet)                                                      |
| `class`, `x11-instance-name`, `window-subtitle`, `window-show-tab-bar`, `window-titlebar-background/-foreground`, `gtk-titlebar*`  | **n/a** (linux/gtk)                                                                      |

### Shell, command, environment

| Keys                                                  | Status                                         |
| ----------------------------------------------------- | ---------------------------------------------- |
| `command`, `initial-command`                          | **soon**                                       |
| `env`, `input`, `working-directory`                   | **soon**                                       |
| `wait-after-command`, `abnormal-command-exit-runtime` | **soon**                                       |
| `shell-integration`                                   | **soon**                                       |
| `shell-integration-features`                          | **soon** (granular; T3 backlog)                |
| `notify-on-command-finish*` (3 keys)                  | **comment** (no desktop notifications yet)     |
| `term`, `enquiry-response`                            | **soon** (VT config; T5-adjacent — file-claim) |
| `vt-kam-allowed`                                      | **soon** (VT toggle — file-claim)              |

### Keybinds & input

| Keys                  | Status                                                                   |
| --------------------- | ------------------------------------------------------------------------ |
| `keybind`             | **key** (full [keybinds.md](keybinds.md) port; `text:` subset works now) |
| `key-remap`           | **soon** (`RemapSet` already ported in `qwertty_term_input`, unwired)    |
| `macos-option-as-alt` | **soon** (macOS; `Mods::translation` supports it)                        |

### Links, bell, notifications, misc

| Keys                                                                                                                              | Status                                                       |
| --------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ |
| `link`, `link-url`, `link-previews`                                                                                               | **comment** (link matching not yet wired)                    |
| `bell-features`, `bell-audio-path`, `bell-audio-volume`                                                                           | **comment** (no bell UI); audio = also platform-limited      |
| `desktop-notifications`, `progress-style`, `app-notifications`                                                                    | **comment** / `app-notifications` **n/a** (gtk)              |
| `command-palette-entry`                                                                                                           | **comment** (no command palette yet)                         |
| `osc-color-report-format`                                                                                                         | **soon** (VT — file-claim)                                   |
| `custom-shader`, `custom-shader-animation`                                                                                        | **comment** (no shader pipeline)                             |
| `confirm-close-surface`, `quit-after-last-window-closed`, `quit-after-last-window-closed-delay`, `initial-window`, `undo-timeout` | **soon** (macOS-relevant subset); `-delay` = **n/a** (linux) |

### Quick terminal

| Keys                                                                                  | Status                                                         |
| ------------------------------------------------------------------------------------- | -------------------------------------------------------------- |
| `quick-terminal-position/-size/-screen/-animation-duration/-autohide/-space-behavior` | **comment** (no quick terminal yet; macOS-relevant when built) |
| `gtk-quick-terminal-layer/-namespace`, `quick-terminal-keyboard-interactivity`        | **n/a** (gtk/wayland)                                          |

### macOS integration

| Keys                                                                                                                                                                          | Status                                          |
| ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| `macos-non-native-fullscreen`, `macos-window-buttons`, `macos-titlebar-style`, `macos-titlebar-proxy-icon`, `macos-window-shadow`, `macos-hidden`, `macos-dock-drop-behavior` | **soon** (macOS window/chrome batch)            |
| `macos-auto-secure-input`, `macos-secure-input-indication`, `macos-applescript`, `macos-shortcuts`                                                                            | **soon** (macOS integration)                    |
| `macos-icon`, `macos-custom-icon`, `macos-icon-frame`, `macos-icon-ghost-color`, `macos-icon-screen-color`                                                                    | **comment** (custom app-icon system not ported) |
| `auto-update`, `auto-update-channel`                                                                                                                                          | **comment** (no updater)                        |

### Linux/GTK-only (all **n/a** on macOS)

`language`, `linux-cgroup*` (4 keys), `gtk-opengl-debug`, `gtk-single-instance`,
`gtk-tabs-location`, `gtk-toolbar-style`, `gtk-wide-tabs`,
`gtk-horizontal-tab-scroll`, `gtk-custom-css`, `async-backend`. Emitted as
`# platform-specific (linux/gtk), ignored`.

**Coverage check:** every one of the 204 keys in the C2 inventory falls into exactly
one row above. When a batch lands and flips its keys from **soon** to wired, no
converter change is needed (the key was already emitted live) — only this table's
status column and `docs/feature-coverage.md` update.

---

## 5. Acceptance test — Josh's real config

Captured 2026-07-11. Both macOS locations are populated, exercising the two-location
merge:

**`~/.config/ghostty/config` (XDG):**

```text
theme = Aardvark Ink
```

**`~/Library/Application Support/com.mitchellh.ghostty/config` (App Support):**
(template header comments, then)

```text
font-family = FiraCode Nerd Font Mono
theme = Aardvark Ink
copy-on-select = clipboard
font-size = 16
keybind = shift+enter=text:\x1b\r
```

**Expected emitted TOML** (App Support wins the duplicate `theme`; both agree here):

```toml
# imported from Ghostty config (2 files merged: XDG + App Support)
theme = "Aardvark Ink"          # custom theme file: ~/.config/ghostty/themes/Aardvark Ink
font-family = "FiraCode Nerd Font Mono"
copy-on-select = "clipboard"
font-size = 16
keybind = ["shift+enter=text:\\x1b\\r"]
```

Every key here is **now**-status, so the round-trip is zero-warning — the primary
acceptance bar in the T3 spec ("Josh's real config imports cleanly with zero
warnings he cares about"). Notes:

- `copy-on-select = clipboard` — upstream's `CopyOnSelect` enum has `{false, true,
  clipboard}`; our current field is a plain `bool`. **Gap to close**: widen our
  `copy-on-select` to accept `clipboard` (matches upstream) so the value survives
  rather than being coerced. Tracked as a one-key fix in the mouse/clipboard batch.
- `theme = Aardvark Ink` references a **custom** file in `~/.config/ghostty/themes/`
  (Josh also has `Betamax Brownout`). The converter emits a provenance comment; theme
  **file** conversion (Ghostty theme file → our theme format) is a follow-up once the
  theme-search path from [config-core.md](config-core.md) §5 is wired.
- The template-header comment block in the App Support file is dropped silently.

---

## 6. Slice plan

1. **Reader + merge (M).** Ghostty line-syntax parser (reuse the config-core loader),
   two-location + include discovery, effective-merge. Output: an ordered list of
   `(key, value, origin)`.
2. **Mapping + emit (M).** The §4 table as a lookup; TOML emission with §2/§3 value
   conversions; commented pass-through for `comment`/`n/a`; provenance header +
   skipped-key summary.
3. **Acceptance (M).** Josh's config as a fixture test (`--dry-run` output asserted
   byte-for-byte); the `copy-on-select` widening; a synthetic fixture exercising
   includes, a `light:X,dark:Y` theme, a palette, and a multi-action keybind.
4. **Theme-file conversion (follow-up).** Gated on config-core §5 theme search.
