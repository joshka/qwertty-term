# T3 — Config & keybinds thread

**Model:** Opus (Binding.zig port + config architecture); spawn Sonnet sub-agents for
per-key wiring batches · **Wave:** 2 (starts when T4 drains — you inherit the app crate) ·
**Workspace:** `work/t3` · **Status:** `status/t3.md`
**Territory:** config module, keybind system, and option WIRING across crates (wiring
edits outside the app get file-claims). Rules: `docs/threads/README.md`.

## Mission

Grow the deliberately-minimal TOML config into full expressive power: the complete
keybind system (Binding.zig, 4.9k Zig — the largest single unported subsystem that users
feel) and the config option surface, wired for real. TOML stays (ADR'd deviation);
upstream's SEMANTICS per option are the spec. End state: a Ghostty power-user's config
translates mechanically.

## Order of attack

1. **Keybind system port** (XL — the centerpiece; analysis doc first:
   `docs/analysis/keybinds.md`). Port `src/input/Binding.zig` semantics: trigger parse
   (chords, `physical:`, key names), action enum (~60 actions — enumerate ALL from
   upstream, table them in the analysis doc with wired/stub status), leader/`chain`
   sequences, key tables (`activate_key_table`, one-shot), `global:`/`all:`/`performable:`
   prefixes, unconsumed triggers, config syntax `keybind = trigger=action` (TOML array).
   The existing `tabkeys/splitkeys/searchkeys/keybind.rs text:` tables all COLLAPSE into
   this system as default bindings — deleting those bespoke tables is the proof the port
   is real. Slices: (a) trigger+action model & parse + defaults table generated from
   upstream's default set, 1:1 tests; (b) dispatch integration replacing bespoke tables
   (every existing smoke must stay green — they are the regression net); (c) leader/
   tables/global; (d) the action long tail (`jump_to_prompt`, `scroll_page_*`,
   `write_*_file`, `adjust_selection`, font-size set, `crash` debug action, etc. — each
   action lands with its behavior, or an explicit stub note in the analysis table).
2. **Config core** (M): `config-file` includes + `config-default-files`, load order
   (matching upstream's two-location merge on macOS), **`config-reload`** (keybind action
   + live re-apply: fonts/theme/keybinds swap safely at runtime — design note needed for
   what's reload-safe, mirror upstream's surface reload), CLI `--flag=value` overrides.
3. **Option surface wiring** (M each batch, Sonnet sub-agents): batches by group, each
   batch = parse + plumb + effect + test + feature-coverage flip:
   `adjust-*` metrics (~18 keys, plumb to font Metrics), font-feature/-variation/-style/
   -synthetic-style/-codepoint-map, cursor group (color/opacity/blink timer/style),
   color groups (bold-color, cell-fg/bg, faint-opacity, minimum-contrast already),
   window group (padding-balance/color, title templates), mouse/clipboard gates
   (T4 built behaviors — you add the keys), scrollback-limit / image-storage-limit
   (plumb to vt — file-claim), shell-integration-features granular.
4. **`+import-ghostty-config` converter** (M): read real Ghostty config (+themes refs),
   emit our TOML with comments for unsupported keys; round-trip Josh's actual config as
   the acceptance test; document mapping table.

## Method rules

Upstream defaults are LAW — every key's default verified at `2da015cd6` and cited.
Unknown/unsupported input: warn-and-skip, never fail startup (house rule). Every batch
updates `docs/feature-coverage.md`. Live-reload changes need a windowed smoke (reload
with font-size change → cell metrics change asserted). The keybind dispatch sits in the
hot key path — no per-event allocation; bench the typing smoke before/after.

## Definition of done

Bespoke key tables deleted in favor of the real system; upstream's full default keymap
active; config sections of feature-coverage.md `[x]` or explicitly `[—]`; Josh's real
config imports cleanly with zero warnings he cares about.
