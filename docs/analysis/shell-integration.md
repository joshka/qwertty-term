# Shell integration (`termio/shell_integration.zig` + `src/shell-integration/`)

Surveyed and ported against ghostty commit `2da015cd6`
(`2da015cd6ac06cedc89e09756e895d2c1715205d`; verify with
`git -C ~/local/ghostty rev-parse 2da015cd6`). The Rust port lives in
`crates/qwertty-term-termio/src/shell_integration.rs`; the vendored scripts live in
`crates/qwertty-term-termio/resources/shell-integration/`. This covers M2 chunk G
from `docs/plans/m2-termio.md` (after chunk D/Exec).

Zig references:

| file                               | LoC   | inline tests | Rust module                               |
| ---------------------------------- | ----- | ------------ | ----------------------------------------- |
| `src/termio/shell_integration.zig` | 1,032 | 20\*         | `qwertty-term-termio/src/shell_integration.rs` |

\* 21 `test` blocks exist in the file; `test "force shell"` loops over all 5
`Shell` variants in one block rather than being 5 separate tests, so upstream's
own count lands on 20 "logical" tests depending how you count the loop. The
Rust port keeps a 1:1 `#[test]` per Zig `test` block (21 functions), covering
the same 20 behaviors the plan's LoC/test survey counted.

Scripts copied verbatim (plan decision 5 — these are shell code, not Zig; only
the injection logic above is ported):

| upstream path                                                             | bytes        | vendored at                                                                     |
| ------------------------------------------------------------------------- | ------------ | ------------------------------------------------------------------------------- |
| `src/shell-integration/bash/qwertty-term.bash`                                 | see manifest | `resources/shell-integration/bash/qwertty-term.bash`                                 |
| `src/shell-integration/bash/bash-preexec.sh`                              | see manifest | `resources/shell-integration/bash/bash-preexec.sh`                              |
| `src/shell-integration/zsh/.zshenv`                                       | see manifest | `resources/shell-integration/zsh/.zshenv`                                       |
| `src/shell-integration/zsh/qwertty-term-integration`                           | see manifest | `resources/shell-integration/zsh/qwertty-term-integration`                           |
| `src/shell-integration/fish/vendor_conf.d/qwertty-term-shell-integration.fish` | see manifest | `resources/shell-integration/fish/vendor_conf.d/qwertty-term-shell-integration.fish` |
| `src/shell-integration/elvish/lib/qwertty-term-integration.elv`                | see manifest | `resources/shell-integration/elvish/lib/qwertty-term-integration.elv`                |
| `src/shell-integration/nushell/vendor/autoload/qwertty-term.nu`                | see manifest | `resources/shell-integration/nushell/vendor/autoload/qwertty-term.nu`                |
| `src/shell-integration/README.md`                                         | see manifest | `resources/shell-integration/README.md`                                         |

Exact byte counts + sha256 live in the checked-in manifest
(`crates/qwertty-term-termio/resources/shell-integration-manifest.txt`); a test
(`tests/shell_integration_scripts.rs`) hashes the vendored tree against it so
any accidental hand-edit or upstream drift is a loud, immediate test failure.

## Shell detection

`detectShell` (Zig `shell_integration.zig:136`) looks only at the **basename**
of the command's first argument — it does not inspect `$SHELL` or run the
binary:

- `bash` → `.bash`, EXCEPT on Darwin when the full path is literally
  `/bin/bash`: Apple's patched Bash 3.2 disables the ENV-based POSIX startup
  path, so integration silently declines (returns `None`) rather than
  breaking the shell. This is a real, load-bearing platform special case, not
  an oversight — ported exactly.
- `elvish` → `.elvish`, `fish` → `.fish`, `nu` → `.nushell`, `zsh` → `.zsh`.
- Anything else → `None` (no integration; the user can still `source` the
  script manually).

`force_shell` (config `shell-integration = bash|zsh|...` instead of `detect`)
bypasses detection entirely and is honored even for a command whose basename
wouldn't otherwise match — this is exercised by the "force shell" test, which
loops all 5 `Shell` variants through `setup()` with an explicit `force_shell`
and a fake `sh` command line.

## Per-shell injection mechanism

All paths share two upstream env vars set unconditionally before dispatch:

- `QWERTTY_TERM_SHELL_FEATURES` — from `setupFeatures`, a sorted comma list of
  enabled features (`cursor[:blink|:steady]`, `path`, `ssh-env`,
  `ssh-terminfo`, `sudo`, `title`), built at a fixed-size stack buffer since
  the field set is comptime-known. Both automatic and manual (user-sourced)
  integrations read this var, so it's set even when shell detection fails.
- The command construction below (or `None`, degrading to launching the
  unmodified default shell command with only `QWERTTY_TERM_SHELL_FEATURES` set).

### zsh — `ZDOTDIR` indirection (`setupZsh`, line 895)

The simplest and most robust mechanism, because zsh has first-class support
for redirecting its dotfile directory:

1. If the caller already has `ZDOTDIR` set, stash it in `QWERTTY_TERM_ZSH_ZDOTDIR`
   (so it can be restored later — see below). This is the **user-zdotdir
   preservation** the task called out.
2. Point `ZDOTDIR` at `<resource_dir>/shell-integration/zsh` — verified to
   exist first (`open` + `close`; a missing dir means integration silently
   fails, returning `null` and leaving `env` otherwise untouched apart from
   the features var already set by the caller).
3. Return the **command unmodified** — zsh needs no argv rewriting; `ZDOTDIR`
   alone redirects its dotfile search.

The restoration side lives entirely in the vendored `.zshenv` (sourced
automatically by zsh because `ZDOTDIR` points there): it immediately restores
the real `ZDOTDIR` (from `QWERTTY_TERM_ZSH_ZDOTDIR` if present, else unsets it
entirely — zsh treats unset `ZDOTDIR` as `$HOME`), *then* sources the user's
real `.zshenv` from that restored location, and *only after* that (in an
`always` block, so it runs even if the user's `.zshenv` errors) autoloads and
invokes `qwertty-term-integration` from the ghostty resource dir by absolute path
(computed from `${(%):-%x}:A:h`, i.e. "the directory containing the currently
executing file", NOT `ZDOTDIR` — which by that point has already been
restored to the user's value). This ordering — restore, then user config,
then integration — is exactly what lets integration run standalone alongside
whatever the user's own zsh config does, without the user's config ever
observing the redirected `ZDOTDIR`.

`qwertty-term-integration` itself is invoked unconditionally for every zsh startup
under this ZDOTDIR redirect, but internally checks `[[ -o interactive ]]` and
an idempotency guard (`_ghostty_state` already set) before doing anything, and
defers its real setup to a `precmd_functions` hook (`_ghostty_deferred_init`)
so other zsh init files (loaded between ZDOTDIR restore and the precmd firing)
get a chance to configure things first.

### bash — `--posix` + `ENV` trickery (`setupBash`, line 298)

The subtle one, ported exactly per the task's warning:

1. Rewrite argv: keep the original `exe` (bash) as argv[0], then insert
   `--posix` right after it. `--posix` disables bash's normal startup-file
   search (`.bashrc`/`.bash_profile`/etc.) so `ENV`-based POSIX-mode startup
   becomes the only thing that runs.
2. Walk the *rest* of the original arguments, bailing (`return null`, no
   integration) on anything that would make bash non-interactive or already
   POSIX (`-c ...`, `--posix` already present) — because integration only
   makes sense for an interactive login/rc-loading shell. `--norc`/
   `--noprofile` are intercepted (not passed through to the rewritten argv
   directly) and instead folded into a `QWERTTY_TERM_BASH_INJECT` env var
   (`"1[ --norc][ --noprofile]"` — the leading `"1"` lets the script tell
   "manually sourced" apart from "auto-injected") so the vendored
   `qwertty-term.bash` can replay them precisely during its manual startup-file
   walk. `--rcfile FILE`/`--init-file FILE` are captured into
   `QWERTTY_TERM_BASH_RCFILE` (consuming the following arg) rather than passed
   through, for the same reason. A bare `-` or `--` stops all further option
   parsing and passes everything remaining straight through as positional
   args (script file + its args).
3. Preserve any existing `ENV` into `QWERTTY_TERM_BASH_ENV` (about to be
   overwritten), then verify
   `<resource_dir>/shell-integration/bash/qwertty-term.bash` actually opens (else:
   fail integration, `QWERTTY_TERM_BASH_ENV`/etc. rolled back / never set) and
   point `ENV` at it. POSIX-mode bash sources `$ENV` unconditionally on
   startup even for non-login shells — that's the whole trick.
4. `HISTFILE`: in POSIX mode bash defaults `HISTFILE` to `~/.sh_history`
   instead of `~/.bash_history`. If the caller hadn't set `HISTFILE`
   explicitly, set it to `~/.bash_history` and mark
   `QWERTTY_TERM_BASH_UNEXPORT_HISTFILE=1` so the vendored script un-exports it
   again after startup (matching what bash would have done on its own in
   non-POSIX mode).

The vendored `qwertty-term.bash` (loaded via `$ENV` while still in POSIX mode)
detects `QWERTTY_TERM_BASH_INJECT`, unsets the trickery vars, `set +o posix`,
manually replays exactly the startup-file sequence bash itself would have run
(`/etc/profile` + first of `~/.bash_profile`/`~/.bash_login`/`~/.profile` for
a login shell; the distro-specific system bashrc + `$QWERTTY_TERM_BASH_RCFILE`
(default `~/.bashrc`) otherwise, respecting the captured `--norc`/
`--noprofile`), then installs the real integration (`PROMPT_COMMAND`/
`PS0`-based OSC 133/7 hooks, described below).

### fish / elvish — `XDG_DATA_DIRS` (`setupXdgDataDirs`, line 623)

Both ride the same helper, since both shells auto-load vendor config/modules
from directories listed in `XDG_DATA_DIRS`:

1. Verify `<resource_dir>/shell-integration` exists.
2. Stash that path in `QWERTTY_TERM_SHELL_INTEGRATION_XDG_DIR` so the shell's own
   vendored config (fish: `vendor_conf.d/qwertty-term-shell-integration.fish`;
   elvish: `lib/qwertty-term-integration.elv`) can strip it back out of
   `XDG_DATA_DIRS` after loading, so subsequently `exec`'d shells / nested
   shells don't keep re-discovering it via a stale `XDG_DATA_DIRS` prefix.
3. Prepend it to `XDG_DATA_DIRS`, defaulting the base value to the
   XDG-spec-mandated `/usr/local/share:/usr/share` if unset (rather than
   clobbering it — see upstream issue #2711 referenced in the comment).
4. Command argv is untouched — fish/elvish discover the integration purely
   via the data-dir scan, no argv rewriting needed.

### nushell — `XDG_DATA_DIRS` + `--execute` (`setupNushell`, line 755)

Nushell also uses `XDG_DATA_DIRS` (for `nushell/vendor/autoload/qwertty-term.nu`,
loaded automatically) but additionally needs an explicit `use` to bring the
exported commands into scope, so `setupNushell`:

1. Calls `setupXdgDataDirs` first (so the XDG env vars are set even if the
   rest of setup later bails — the plain vendor-autoload module still loads).
2. Rewrites argv: keeps `exe`, inserts
   `--execute 'use ghostty *'` right after it.
3. Walks remaining args, bailing (no integration, XDG vars stay set) on
   `--command`/`--lsp`/any short option containing `c` (all imply
   non-interactive or an alternate mode). `-`/`--` stops parsing and passes
   the rest through, same as bash.

## `QWERTTY_TERM_SHELL_INTEGRATION_*` env vars actually used at this commit

The task description mentioned `QWERTTY_TERM_SHELL_INTEGRATION_NO_*` toggle
variables; **no such variables exist in `shell_integration.zig` or any
vendored script at `2da015cd6`** (grepped the full pinned tree). The actual
`QWERTTY_TERM_SHELL_INTEGRATION_*` variable is singular:
`QWERTTY_TERM_SHELL_INTEGRATION_XDG_DIR` (fish/elvish/nushell only, described
above — a "which of my dirs did ghostty prepend" pointer, not a feature
toggle). Feature toggling is entirely through `QWERTTY_TERM_SHELL_FEATURES`
(comma list) plus the config-level `shell-integration = none` escape hatch
(skips `setup()` entirely, only `QWERTTY_TERM_SHELL_FEATURES` gets set). This
doc corrects the assumption for whoever wrote the task coordination notes;
the Rust port does not invent `_NO_*` vars that don't exist upstream.

## What the scripts DO: OSC 133, OSC 7, and the bar-cursor-at-prompt

Confirmed directly from the vendored zsh and bash scripts (same shape in both,
minor syntax differences; zsh is quoted below as it's the default-enabled
integration — see "app wiring" below).

**OSC 133 (semantic prompt marking).** zsh's `_ghostty_precmd` (fired via
`precmd_functions`) and `_ghostty_preexec` (via `preexec_functions`):

- `preexec`: emits `OSC 133 ; C ST` (mark C — "end of input, start of command
  output") right before the command runs, and sets `_ghostty_state = 1`.
- `precmd`: if the previous command's C mark wasn't closed yet
  (`_ghostty_state == 1`), emits `OSC 133 ; D ; <exit_code> ST` (mark D — "end
  of command output, with exit status") first. Then patches `PS1`/`PS2` (when
  `prompt_percent` is set, the common case) to wrap the *rendered* prompt in
  `OSC 133 ; P ; k=i ST ... OSC 133 ; B ST` (mark `P` = "prompt-continuation
  hint", mark B = "end of prompt, start of input") so the marks travel through
  zsh's own prompt-redraw machinery (resize, `^L`, `SIGWINCH`) rather than
  being emitted only once. A fresh `OSC 133 ; A` (mark A — "start of prompt",
  with `redraw=last` cursor positioning semantics carried in bash's version;
  zsh folds A into the `mark1` constant embedded in `PS1` instead) is what
  actually starts a new prompt each render.
- `zle-line-init`: emits `OSC 133;P;k=i` + `OSC 133;B` defensively if `PS1`
  doesn't already contain a mark (covers prompt plugins that regenerate `PS1`
  after `_ghostty_precmd` ran, e.g. oh-my-posh/zinit async prompts).
- `_ghostty_report_pwd` (OSC 7, see below) is also spliced into
  `_ghostty_precmd` and into `chpwd_functions`.

Bash's `__ghostty_precmd`/`__ghostty_preexec` (via `PROMPT_COMMAND`/`PS0`) emit
the same four marks (`A` with `redraw=last;cl=line;aid=$BASHPID`, `B` via
`PS1` append, `C` in preexec, `D;<exit>;aid=$BASHPID` in precmd) with the
`aid=` (async id / `BASHPID`) field bash adds that zsh's doesn't.

**OSC 7 (cwd reporting).** Both scripts report
`OSC 7 ; kitty-shell-cwd://<host><pwd> ST` — zsh via `_ghostty_report_pwd`
wired to both `chpwd_functions` (covers plain `cd`) and appended into
`_ghostty_precmd` (covers `cmd && cd x` where a child process changes cwd
without a `chpwd` hook firing, since only the next prompt would notice);
bash's `__ghostty_precmd` does the equivalent inline (bash has no `chpwd`
hook at all, so precmd-time comparison against a cached
`_ghostty_last_reported_cwd` is bash's *only* mechanism, with the same known
gap the comment documents: `cd /test && cat` won't be seen until the next
prompt because `PS0` — bash's nearest analogue to a preexec hook — runs
*before* the command, so a cwd change mid-pipeline is invisible until
precmd). This OSC 7 payload format
(`kitty-shell-cwd://host/path`) is what `docs/analysis` / the app's
`pwd_path_from_osc7` parser (already wired, per `crates/qwertty-term-app/src/engine.rs`)
expects.

**Bar-cursor-at-prompt (DECSCUSR, the maintainer's question).** Confirmed:
gated entirely behind the `cursor` feature flag
(`QWERTTY_TERM_SHELL_FEATURES == *cursor*`), which defaults to **enabled**
(`ShellIntegrationFeatures.cursor: bool = true` in `Config.zig:8631`, and
`cursor-style` config doc explicitly says *"shell integration will
automatically set the cursor to a bar at a prompt, regardless of
[`cursor-style`] configuration"* — i.e. this is intentional, on-by-default
UX, not an accident):

- zsh hooks `zle-keymap-select`/`zle-line-init`/`zle-line-finish` (all three
  point at the same `_ghostty_zle_*` function): on every keymap change it
  emits `CSI <n> SP q` where `n` is `1`/`2` (block) for `vicmd`/`visual`
  keymap (vi command mode) or `5`/`6` (**bar**) for every other keymap
  (emacs mode, vi insert mode — i.e. "normal typing at the prompt"), with the
  even/odd choice (`5` vs `6`, `1` vs `2`) picking blinking vs steady from
  `QWERTTY_TERM_SHELL_FEATURES == *cursor:steady*` (itself derived from the
  config's `cursor-style-blink`, defaulting to blinking). `_ghostty_preexec`
  additionally emits `CSI 0 SP q` (reset to the terminal's configured
  default shape) right before the external command runs, so a program that
  takes over the terminal doesn't inherit the bar.
  - Net effect for the default config: sitting at an interactive zsh prompt
    (emacs or vi-insert mode) → cursor is a **blinking bar** (`CSI 5 SP q`).
    Switch to vi command mode (`ESC` in vi-mode zsh) → cursor becomes a
    **blinking block** (`CSI 1 SP q`). Running a program → cursor resets to
    whatever `cursor-style` configures (default: block, steady).
  - bash's `__ghostty_precmd` does the simpler version: appends
    `CSI 5|6 SP q` (bar) directly into `PS1` (no vi-mode distinction — bash's
    readline vi-mode isn't hooked the same way) and `CSI 0 SP q` into `PS0`
    (reset before the command runs), guarded by a "not already present"
    string check so it isn't appended twice across `precmd` calls.

This confirms the specific behavior the maintainer asked about: yes, the
prompt gets a bar cursor by default, it's `zsh`/`bash` integration doing it
via plain DECSCUSR (`CSI Ps SP q`) emitted from prompt-render/keymap hooks,
not a terminal-side "am I at a prompt" heuristic — and it is strictly
downstream of OSC 133 mark presence being wired at all (no shell
integration → no DECSCUSR emission → cursor stays whatever `cursor-style`
says).

**sudo / title (secondary features, both scripts, same shape):**

- `sudo`: wraps the `sudo` builtin/function to add
  `--preserve-env=TERMINFO` (so a re-exec under `sudo` doesn't lose Ghostty's
  terminfo entry) *unless* the invocation looks like `sudoedit`
  (`-e`/`--edit`), which doesn't need terminfo preserved. Gated on
  `QWERTTY_TERM_SHELL_FEATURES == *sudo*` AND a non-empty `$TERMINFO`. Defaults to
  **disabled** (`sudo: bool = false`).
- `title`: sets the OSC 2 window title from the abbreviated pwd (`precmd`,
  zsh only — bash sets title via a plain `PS1` append too) and from the
  about-to-run command (`preexec`, both). Defaults to **enabled**.
- `path`/`ssh-env`/`ssh-terminfo`: PATH augmentation and an `ssh` wrapper
  translating feature flags into `ghostty +ssh` CLI flags; `path` defaults
  enabled, both `ssh-*` default disabled. These are lower priority for chunk
  G (no `+ssh` subcommand exists yet in this Rust tree) and are ported as
  inert (the env plumbing / feature-flag string is correct; the `+ssh`
  wrapper functions in the vendored scripts will simply never fire without a
  `ghostty +ssh` binary to call, same as upstream would behave if that binary
  were missing from PATH).

## Resources-dir resolution

Upstream (`os/resourcesdir.zig:40`, not itself part of this chunk's 1,032 LoC
but the dependency shell_integration.setup's `resource_dir` parameter needs):
in release builds, `QWERTTY_TERM_RESOURCES_DIR` env var wins outright if set and
non-empty; otherwise (and always in debug builds, deliberately, so a debug
Ghostty launched from an old installed Ghostty doesn't inherit stale
resources) it climbs from the running binary's `selfExePath` looking for a
sentinel (`terminfo/78/xterm-ghostty` on macOS) to locate the app bundle's
`Resources` dir.

The Rust port's shape (`resources_dir()` in `shell_integration.rs`):

1. `QWERTTY_TERM_RESOURCES_DIR` env var, if set and non-empty — wins unconditionally
   (we don't have upstream's release/debug distinction machinery, and always
   honoring an explicit override is the least surprising choice for an
   embedder/dev-time tool).
2. Else, a `CARGO_MANIFEST_DIR`-relative fallback
   (`concat!(env!("CARGO_MANIFEST_DIR"), "/resources/shell-integration")`,
   compiled in) — this is how the vendored resources are found **at dev time**
   running `cargo run`/`cargo test` straight out of the workspace, without any
   install step. This is a Rust-specific addition with no direct upstream
   analogue (upstream always has a real installed/bundled resources dir by
   the time shell integration runs); it only matters for this repo's
   dev-loop and would be replaced by a real packaged-resources path in an
   eventual release build (out of scope for chunk G — flagged as a
   deferral).

## App wiring (chunk G's one permitted app touch)

`crates/qwertty-term-app/src/termio.rs`'s `TabIo::spawn` constructs
`qwertty_term_termio::exec::Config` directly. Chunk G adds a
`qwertty_term_termio::shell_integration::setup(...)` call between building the
base env (`std::env::vars().collect()`, from the M2-E env-inherit fix) and
handing `Config` to `Termio::build_exec`, mutating the config's `env` /
`command` in place exactly the way `Exec.Subprocess.init`'s `shell:` block
does upstream. Enabled by default with `zsh` forced
(`docs/plans/m2-termio.md` decision + `ShellIntegrationFeatures::default()`
all-upstream-defaults: `cursor`/`title`/`path` on, `sudo`/`ssh-env`/
`ssh-terminfo` off) — matching upstream's own `shell-integration = detect`
default, simplified to "detect via `$SHELL`'s basename, but we specifically
know we want zsh's integration exercised since that's the default macOS
shell and the one the integration test drives."
