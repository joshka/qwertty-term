//! Shell integration: the RC-injection machinery port of
//! `termio/shell_integration.zig` (`2da015cd6`). This is M2 chunk G from
//! `docs/plans/m2-termio.md` (after chunk D/Exec). Analysis:
//! `docs/analysis/shell-integration.md`.
//!
//! Detects the shell from the command's basename (or an explicit override),
//! then mutates the child environment / command line so the shell
//! auto-sources one of the vendored scripts in `resources/shell-integration/`
//! (copied byte-for-byte from upstream — plan decision 5; only this injection
//! logic is ported, not the shell code itself). Those scripts are what
//! actually emit OSC 133 (semantic prompt marks), OSC 7 (cwd reporting), and
//! (zsh/bash, `cursor` feature, on by default) DECSCUSR bar-cursor-at-prompt.
//!
//! # Per-shell mechanism (see the analysis doc for the full write-up)
//!
//! * **zsh**: `ZDOTDIR` indirection ([`setup_zsh`]) — point `ZDOTDIR` at the
//!   vendored `zsh/` dir (which auto-sources `.zshenv`, restoring the real
//!   `ZDOTDIR` and chaining to the user's own config before running the
//!   integration). Preserves an existing `ZDOTDIR` into `GHOSTTY_ZSH_ZDOTDIR`.
//! * **bash**: `--posix` + `ENV` trickery ([`setup_bash`]) — force POSIX-mode
//!   startup (which sources `$ENV` unconditionally) pointed at the vendored
//!   `bash/ghostty.bash`, which then manually replays bash's normal startup
//!   sequence. Bails (no integration) on `-c`, already-`--posix`, or other
//!   non-interactive-implying flags.
//! * **fish / elvish**: `XDG_DATA_DIRS` ([`setup_xdg_data_dirs`]) — both
//!   auto-load vendor config from data dirs; no argv rewrite needed.
//! * **nushell**: `XDG_DATA_DIRS` (vendor autoload) plus an injected
//!   `--execute 'use ghostty *'` ([`setup_nushell`]).
//!
//! No `GHOSTTY_SHELL_INTEGRATION_NO_*` variables exist at the pinned commit
//! (verified against the full upstream tree) — feature toggling is entirely
//! through the `GHOSTTY_SHELL_FEATURES` comma-list env var
//! ([`setup_features`]) plus the `shell-integration = none` escape hatch
//! (skip [`setup`] entirely). The single real
//! `GHOSTTY_SHELL_INTEGRATION_XDG_DIR` var is unrelated (fish/elvish/nushell
//! use it to strip their entry back out of `XDG_DATA_DIRS` after loading).

use std::path::{Path, PathBuf};

use crate::exec::Command;

/// The shells we can automatically integrate. Port of `Shell`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Shell {
    Bash,
    Elvish,
    Fish,
    Nushell,
    Zsh,
}

/// The result of a successful [`setup`]. Port of `ShellIntegration`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellIntegration {
    /// The shell that was integrated.
    pub shell: Shell,
    /// The (possibly rewritten) command to launch the shell with.
    pub command: Command,
}

/// Shell integration feature flags. Port of `config.ShellIntegrationFeatures`;
/// defaults match upstream's `Config.zig` field defaults exactly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellIntegrationFeatures {
    /// Set the cursor to a bar at the prompt (DECSCUSR, in the vendored
    /// scripts' prompt hooks). Upstream default: **true**. This is the
    /// bar-cursor-at-prompt behavior.
    pub cursor: bool,
    /// Wrap `sudo` to preserve `TERMINFO`. Upstream default: false.
    pub sudo: bool,
    /// Set the window title via OSC 2. Upstream default: true.
    pub title: bool,
    /// SSH env-var forwarding compatibility (`ghostty +ssh`). Upstream
    /// default: false.
    pub ssh_env: bool,
    /// SSH automatic terminfo installation (`ghostty +ssh`). Upstream
    /// default: false.
    pub ssh_terminfo: bool,
    /// Add Ghostty's binary directory to `PATH`. Upstream default: true.
    pub path: bool,
}

impl Default for ShellIntegrationFeatures {
    fn default() -> Self {
        ShellIntegrationFeatures {
            cursor: true,
            sudo: false,
            title: true,
            ssh_env: false,
            ssh_terminfo: false,
            path: true,
        }
    }
}

/// A simple ordered string/string environment the setup functions mutate in
/// place, mirroring the Zig `EnvMap` the upstream functions take by pointer.
/// `Vec`-backed (matches [`crate::exec::Config::env`]'s representation) with
/// map-like `get`/`put`/`remove` helpers; small env sizes make linear scan
/// fine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EnvMap(pub Vec<(String, String)>);

impl EnvMap {
    pub fn new() -> Self {
        EnvMap(Vec::new())
    }

    pub fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        EnvMap(pairs)
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    pub fn put(&mut self, key: &str, value: impl Into<String>) {
        let value = value.into();
        if let Some(slot) = self.0.iter_mut().find(|(k, _)| k == key) {
            slot.1 = value;
        } else {
            self.0.push((key.to_string(), value));
        }
    }

    pub fn remove(&mut self, key: &str) {
        self.0.retain(|(k, _)| k != key);
    }

    pub fn count(&self) -> usize {
        self.0.len()
    }

    pub fn into_pairs(self) -> Vec<(String, String)> {
        self.0
    }
}

/// Set up automatic shell integration for `command` given `resource_dir` (the
/// directory that must contain a `shell-integration/` subtree — the vendored
/// resources), mutating `env` in place and returning the (possibly modified)
/// command to launch. Returns `None` if the shell couldn't be detected (and
/// `force_shell` wasn't given), or if the shell's specific resources
/// (`ghostty.bash`, the `zsh/` dir, ...) aren't found under `resource_dir`.
/// Port of `setup` (`shell_integration.zig:42`).
pub fn setup(
    resource_dir: &str,
    command: &Command,
    env: &mut EnvMap,
    force_shell: Option<Shell>,
) -> Option<ShellIntegration> {
    let shell = force_shell.or_else(|| detect_shell(command))?;

    let new_command = match shell {
        Shell::Bash => setup_bash(command, resource_dir, env)?,
        Shell::Nushell => setup_nushell(command, resource_dir, env)?,
        Shell::Zsh => setup_zsh(command, resource_dir, env)?,
        Shell::Elvish | Shell::Fish => {
            if !setup_xdg_data_dirs(resource_dir, env) {
                return None;
            }
            command.clone()
        }
    };

    Some(ShellIntegration {
        shell,
        command: new_command,
    })
}

/// Split a shell command line into argv-like tokens the way
/// `config.Command.argIterator` does: whitespace-separated, with a single
/// level of `"..."` / `'...'` quoting so a quoted executable path containing
/// spaces (`"/a b/bash"`) survives as one token. This is a pragmatic subset
/// of upstream's real command-line parser (which has its own dedicated
/// tokenizer) sufficient for [`detect_shell`] and the per-shell setup
/// functions, which only ever look at (a) the basename of the first token and
/// (b) a small fixed set of well-known flag tokens.
fn arg_iter(command: &Command) -> Vec<String> {
    match command {
        Command::Direct(argv) => argv.clone(),
        Command::Shell(line) => {
            let mut out = Vec::new();
            let mut chars = line.chars().peekable();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() {
                    chars.next();
                    continue;
                }
                let mut tok = String::new();
                if c == '"' || c == '\'' {
                    let quote = c;
                    chars.next();
                    for c2 in chars.by_ref() {
                        if c2 == quote {
                            break;
                        }
                        tok.push(c2);
                    }
                } else {
                    for c2 in chars.by_ref() {
                        if c2.is_whitespace() {
                            break;
                        }
                        tok.push(c2);
                    }
                }
                out.push(tok);
            }
            out
        }
    }
}

/// Detect the shell from `command`'s first argument's basename. Port of
/// `detectShell` (`shell_integration.zig:136`).
fn detect_shell(command: &Command) -> Option<Shell> {
    let args = arg_iter(command);
    let arg0 = args.first()?;
    let exe = Path::new(arg0)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();

    if exe == "bash" {
        // Apple's patched Bash 3.2 on macOS disables the ENV-based POSIX
        // startup path. `/bin` is non-writable under SIP, so "/bin/bash"
        // reliably means Apple's Bash. Ported exactly (see analysis doc).
        if cfg!(target_os = "macos") && arg0 == "/bin/bash" {
            return None;
        }
        return Some(Shell::Bash);
    }
    match exe.as_str() {
        "elvish" => Some(Shell::Elvish),
        "fish" => Some(Shell::Fish),
        "nu" => Some(Shell::Nushell),
        "zsh" => Some(Shell::Zsh),
        _ => None,
    }
}

/// Set the `GHOSTTY_SHELL_FEATURES` env var from `features`, sorted
/// case-insensitively for deterministic output, with `cursor` carrying a
/// `:blink`/`:steady` suffix from `cursor_blink`. Sets nothing if no feature
/// is enabled. Port of `setupFeatures` (`shell_integration.zig:188`).
pub fn setup_features(env: &mut EnvMap, features: ShellIntegrationFeatures, cursor_blink: bool) {
    // (name, enabled) pairs, pre-sorted to match upstream's comptime
    // case-insensitive field-name sort.
    let mut names: Vec<&str> = vec!["cursor", "path", "ssh-env", "ssh-terminfo", "sudo", "title"];
    names.sort_by_key(|n| n.to_ascii_lowercase());

    let enabled = |name: &str| -> bool {
        match name {
            "cursor" => features.cursor,
            "sudo" => features.sudo,
            "title" => features.title,
            "ssh-env" => features.ssh_env,
            "ssh-terminfo" => features.ssh_terminfo,
            "path" => features.path,
            _ => false,
        }
    };

    let mut out = String::new();
    for name in names {
        if !enabled(name) {
            continue;
        }
        if !out.is_empty() {
            out.push(',');
        }
        out.push_str(name);
        if name == "cursor" {
            out.push_str(if cursor_blink { ":blink" } else { ":steady" });
        }
    }

    if !out.is_empty() {
        env.put("GHOSTTY_SHELL_FEATURES", out);
    }
}

/// Setup the bash automatic shell integration: start bash in POSIX mode and
/// use `ENV` to load the vendored `bash/ghostty.bash`, which then manually
/// replays bash's normal startup sequence. Port of `setupBash`
/// (`shell_integration.zig:298`). See the analysis doc for the subtle parts
/// (`--norc`/`--noprofile`/`--rcfile` interception, `HISTFILE`).
fn setup_bash(command: &Command, resource_dir: &str, env: &mut EnvMap) -> Option<Command> {
    let args = arg_iter(command);
    let mut iter = args.into_iter();

    let mut new_args: Vec<String> = Vec::new();
    new_args.push(iter.next()?);
    new_args.push("--posix".to_string());

    // "1" lets the script tell manual-source apart from auto-inject.
    let mut inject = String::from("1");
    let mut rcfile: Option<String> = None;

    let mut iter = iter.peekable();
    while let Some(arg) = iter.next() {
        if arg == "--posix" {
            return None;
        } else if arg == "--norc" {
            inject.push_str(" --norc");
        } else if arg == "--noprofile" {
            inject.push_str(" --noprofile");
        } else if arg == "--rcfile" || arg == "--init-file" {
            rcfile = iter.next();
        } else if arg.len() > 1 && arg.starts_with('-') && !arg.starts_with("--") {
            // -c always means non-interactive.
            if arg.contains('c') {
                return None;
            }
            new_args.push(arg);
        } else if arg == "-" || arg == "--" {
            new_args.push(arg);
            new_args.extend(iter.by_ref());
            break;
        } else {
            new_args.push(arg);
        }
    }

    // Preserve an existing ENV before we overwrite it.
    if let Some(v) = env.get("ENV") {
        let v = v.to_string();
        env.put("GHOSTTY_BASH_ENV", v);
    }

    let script_path = format!("{resource_dir}/shell-integration/bash/ghostty.bash");
    if !Path::new(&script_path).is_file() {
        env.remove("GHOSTTY_BASH_ENV");
        return None;
    }
    env.put("ENV", script_path);
    env.put("GHOSTTY_BASH_INJECT", inject);
    if let Some(v) = rcfile {
        env.put("GHOSTTY_BASH_RCFILE", v);
    }

    // POSIX mode defaults HISTFILE to ~/.sh_history; restore ~/.bash_history
    // unless the caller already set HISTFILE explicitly.
    if env.get("HISTFILE").is_none()
        && let Some(home) = home_dir()
    {
        let histfile = format!("{}/.bash_history", home.display());
        env.put("HISTFILE", histfile);
        env.put("GHOSTTY_BASH_UNEXPORT_HISTFILE", "1");
    }

    Some(Command::Shell(new_args.join(" ")))
}

/// Set up automatic integration for shells that autoload modules from
/// `XDG_DATA_DIRS` (fish, elvish; also used as the first step of nushell's
/// setup). Prepends the vendored `shell-integration/` dir, defaulting the
/// base value per the XDG basedir spec if unset (rather than clobbering it —
/// upstream issue #2711). Port of `setupXdgDataDirs`
/// (`shell_integration.zig:623`).
fn setup_xdg_data_dirs(resource_dir: &str, env: &mut EnvMap) -> bool {
    let integ_path = format!("{resource_dir}/shell-integration");
    if !Path::new(&integ_path).is_dir() {
        return false;
    }

    env.put("GHOSTTY_SHELL_INTEGRATION_XDG_DIR", integ_path.clone());

    let base = env
        .get("XDG_DATA_DIRS")
        .map(str::to_string)
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    env.put("XDG_DATA_DIRS", format!("{integ_path}:{base}"));

    true
}

/// Set up automatic Nushell shell integration: `XDG_DATA_DIRS` (vendor
/// autoload of `nushell/vendor/autoload/ghostty.nu`) plus an injected
/// `--execute 'use ghostty *'`. Port of `setupNushell`
/// (`shell_integration.zig:755`).
fn setup_nushell(command: &Command, resource_dir: &str, env: &mut EnvMap) -> Option<Command> {
    // Set XDG_DATA_DIRS even if the rest of setup later bails, so the plain
    // vendor-autoload module still loads.
    if !setup_xdg_data_dirs(resource_dir, env) {
        return None;
    }

    let args = arg_iter(command);
    let mut iter = args.into_iter();

    let mut new_args: Vec<String> = Vec::new();
    new_args.push(iter.next()?);
    new_args.push("--execute 'use ghostty *'".to_string());

    let mut iter = iter.peekable();
    while let Some(arg) = iter.next() {
        if arg == "--command" || arg == "--lsp" {
            return None;
        } else if arg.len() > 1 && arg.starts_with('-') && !arg.starts_with("--") {
            if arg.contains('c') {
                return None;
            }
            new_args.push(arg);
        } else if arg == "-" || arg == "--" {
            new_args.push(arg);
            new_args.extend(iter.by_ref());
            break;
        } else {
            new_args.push(arg);
        }
    }

    Some(Command::Shell(new_args.join(" ")))
}

/// Setup the zsh automatic shell integration: point `ZDOTDIR` at the vendored
/// `shell-integration/zsh` dir (verified to exist first), preserving any
/// existing `ZDOTDIR` into `GHOSTTY_ZSH_ZDOTDIR` so the vendored `.zshenv` can
/// restore it before chaining to the user's real config. The command line is
/// returned unmodified -- zsh needs no argv rewriting. Port of `setupZsh`
/// (`shell_integration.zig:895`).
fn setup_zsh(command: &Command, resource_dir: &str, env: &mut EnvMap) -> Option<Command> {
    if let Some(old) = env.get("ZDOTDIR") {
        let old = old.to_string();
        env.put("GHOSTTY_ZSH_ZDOTDIR", old);
    }

    let integ_path = format!("{resource_dir}/shell-integration/zsh");
    if !Path::new(&integ_path).is_dir() {
        return None;
    }
    env.put("ZDOTDIR", integ_path);

    Some(command.clone())
}

/// Resolve the home directory (for bash's `HISTFILE` default). A small local
/// helper rather than a `homedir` crate dependency -- `$HOME` is sufficient
/// for the POSIX targets this crate supports (see `pty.rs`'s POSIX-only
/// scope).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Resolve the Ghostty resources directory used to locate the vendored
/// shell-integration scripts at runtime. Port of the relevant slice of
/// `os/resourcesdir.zig` plus the Rust-specific dev-time fallback (see the
/// analysis doc's "Resources-dir resolution" section for what's deliberately
/// NOT ported: the release-build terminfo-sentinel climb, which needs a real
/// packaged app bundle this repo doesn't produce yet).
///
/// Resolution order:
/// 1. `GHOSTTY_RESOURCES_DIR` env var, if set and non-empty -- always wins.
/// 2. A `CARGO_MANIFEST_DIR`-relative compiled-in fallback pointing at this
///    crate's own `resources/` directory, so `cargo run`/`cargo test` find the
///    vendored scripts straight out of the workspace with no install step.
pub fn resources_dir() -> String {
    if let Ok(dir) = std::env::var("GHOSTTY_RESOURCES_DIR")
        && !dir.is_empty()
    {
        return dir;
    }
    concat!(env!("CARGO_MANIFEST_DIR"), "/resources").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A scratch resources-dir-shaped directory, mirroring the Zig
    /// `TmpResourcesDir` test helper (`shell_integration.zig:983`): creates
    /// `shell-integration/<shell>` under a fresh temp dir (unique per test via
    /// an atomic counter + pid, since these tests may run concurrently and
    /// `std::env::temp_dir` is shared), and for bash also drops an empty
    /// `ghostty.bash` (setup checks the script file actually opens).
    struct TmpResourcesDir {
        dir: PathBuf,
        shell_path: PathBuf,
    }

    impl TmpResourcesDir {
        fn init(shell: Shell) -> TmpResourcesDir {
            static COUNTER: AtomicU64 = AtomicU64::new(0);
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let dir = std::env::temp_dir().join(format!(
                "ghostty-shell-integration-test-{}-{n}",
                std::process::id()
            ));
            let name = match shell {
                Shell::Bash => "bash",
                Shell::Elvish => "elvish",
                Shell::Fish => "fish",
                Shell::Nushell => "nushell",
                Shell::Zsh => "zsh",
            };
            let shell_path = dir.join("shell-integration").join(name);
            std::fs::create_dir_all(&shell_path).unwrap();
            if shell == Shell::Bash {
                std::fs::write(shell_path.join("ghostty.bash"), "").unwrap();
            }
            TmpResourcesDir { dir, shell_path }
        }

        fn path(&self) -> &str {
            self.dir.to_str().unwrap()
        }
    }

    impl Drop for TmpResourcesDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    // ---- "force shell" -----------------------------------------------------
    #[test]
    fn force_shell() {
        for &shell in &[
            Shell::Bash,
            Shell::Elvish,
            Shell::Fish,
            Shell::Nushell,
            Shell::Zsh,
        ] {
            let res = TmpResourcesDir::init(shell);
            let mut env = EnvMap::new();
            let result = setup(
                res.path(),
                &Command::Shell("sh".to_string()),
                &mut env,
                Some(shell),
            );
            assert_eq!(shell, result.unwrap().shell);
        }
    }

    // ---- "shell integration failure" ---------------------------------------
    #[test]
    fn shell_integration_failure() {
        let mut env = EnvMap::new();
        let result = setup(
            "/nonexistent",
            &Command::Shell("sh".to_string()),
            &mut env,
            None,
        );
        assert!(result.is_none());
        assert_eq!(0, env.count());
    }

    // ---- detectShell --------------------------------------------------------
    #[test]
    fn detect_shell_basics() {
        assert_eq!(detect_shell(&Command::Shell("sh".to_string())), None);
        assert_eq!(
            detect_shell(&Command::Shell("bash".to_string())),
            Some(Shell::Bash)
        );
        assert_eq!(
            detect_shell(&Command::Shell("elvish".to_string())),
            Some(Shell::Elvish)
        );
        assert_eq!(
            detect_shell(&Command::Shell("fish".to_string())),
            Some(Shell::Fish)
        );
        assert_eq!(
            detect_shell(&Command::Shell("nu".to_string())),
            Some(Shell::Nushell)
        );
        assert_eq!(
            detect_shell(&Command::Shell("zsh".to_string())),
            Some(Shell::Zsh)
        );

        if cfg!(target_os = "macos") {
            assert_eq!(detect_shell(&Command::Shell("/bin/bash".to_string())), None);
        }

        assert_eq!(
            detect_shell(&Command::Shell("bash -c 'command'".to_string())),
            Some(Shell::Bash)
        );
        assert_eq!(
            detect_shell(&Command::Shell("\"/a b/bash\"".to_string())),
            Some(Shell::Bash)
        );
    }

    // ---- "setup features" ---------------------------------------------------
    #[test]
    fn setup_features_all_enabled() {
        let mut env = EnvMap::new();
        setup_features(
            &mut env,
            ShellIntegrationFeatures {
                cursor: true,
                sudo: true,
                title: true,
                ssh_env: true,
                ssh_terminfo: true,
                path: true,
            },
            true,
        );
        assert_eq!(
            env.get("GHOSTTY_SHELL_FEATURES"),
            Some("cursor:blink,path,ssh-env,ssh-terminfo,sudo,title")
        );
    }

    #[test]
    fn setup_features_all_disabled() {
        let mut env = EnvMap::new();
        setup_features(
            &mut env,
            ShellIntegrationFeatures {
                cursor: false,
                sudo: false,
                title: false,
                ssh_env: false,
                ssh_terminfo: false,
                path: false,
            },
            true,
        );
        assert_eq!(env.get("GHOSTTY_SHELL_FEATURES"), None);
    }

    #[test]
    fn setup_features_mixed() {
        let mut env = EnvMap::new();
        setup_features(
            &mut env,
            ShellIntegrationFeatures {
                cursor: false,
                sudo: true,
                title: false,
                ssh_env: true,
                ssh_terminfo: false,
                path: false,
            },
            true,
        );
        assert_eq!(env.get("GHOSTTY_SHELL_FEATURES"), Some("ssh-env,sudo"));
    }

    #[test]
    fn setup_features_blinking_cursor() {
        let mut env = EnvMap::new();
        setup_features(
            &mut env,
            ShellIntegrationFeatures {
                cursor: true,
                sudo: false,
                title: false,
                ssh_env: false,
                ssh_terminfo: false,
                path: false,
            },
            true,
        );
        assert_eq!(env.get("GHOSTTY_SHELL_FEATURES"), Some("cursor:blink"));
    }

    #[test]
    fn setup_features_steady_cursor() {
        let mut env = EnvMap::new();
        setup_features(
            &mut env,
            ShellIntegrationFeatures {
                cursor: true,
                sudo: false,
                title: false,
                ssh_env: false,
                ssh_terminfo: false,
                path: false,
            },
            false,
        );
        assert_eq!(env.get("GHOSTTY_SHELL_FEATURES"), Some("cursor:steady"));
    }

    // ---- bash -----------------------------------------------------------
    #[test]
    fn bash_basic() {
        let res = TmpResourcesDir::init(Shell::Bash);
        let mut env = EnvMap::new();
        let command =
            setup_bash(&Command::Shell("bash".to_string()), res.path(), &mut env).unwrap();
        assert_eq!(command, Command::Shell("bash --posix".to_string()));
        assert_eq!(env.get("GHOSTTY_BASH_INJECT"), Some("1"));
        assert_eq!(
            env.get("ENV"),
            Some(format!("{}/ghostty.bash", res.shell_path.display()).as_str())
        );
    }

    #[test]
    fn bash_unsupported_options() {
        let res = TmpResourcesDir::init(Shell::Bash);
        let cmdlines = [
            "bash --posix",
            "bash --rcfile script.sh --posix",
            "bash --init-file script.sh --posix",
            "bash -c script.sh",
            "bash -ic script.sh",
        ];
        for cmdline in cmdlines {
            let mut env = EnvMap::new();
            assert!(
                setup_bash(&Command::Shell(cmdline.to_string()), res.path(), &mut env).is_none()
            );
            assert_eq!(0, env.count());
        }
    }

    #[test]
    fn bash_inject_flags() {
        let res = TmpResourcesDir::init(Shell::Bash);

        let mut env = EnvMap::new();
        let command = setup_bash(
            &Command::Shell("bash --norc".to_string()),
            res.path(),
            &mut env,
        )
        .unwrap();
        assert_eq!(command, Command::Shell("bash --posix".to_string()));
        assert_eq!(env.get("GHOSTTY_BASH_INJECT"), Some("1 --norc"));

        let mut env = EnvMap::new();
        let command = setup_bash(
            &Command::Shell("bash --noprofile".to_string()),
            res.path(),
            &mut env,
        )
        .unwrap();
        assert_eq!(command, Command::Shell("bash --posix".to_string()));
        assert_eq!(env.get("GHOSTTY_BASH_INJECT"), Some("1 --noprofile"));
    }

    #[test]
    fn bash_rcfile() {
        let res = TmpResourcesDir::init(Shell::Bash);
        let mut env = EnvMap::new();

        let command = setup_bash(
            &Command::Shell("bash --rcfile profile.sh".to_string()),
            res.path(),
            &mut env,
        )
        .unwrap();
        assert_eq!(command, Command::Shell("bash --posix".to_string()));
        assert_eq!(env.get("GHOSTTY_BASH_RCFILE"), Some("profile.sh"));

        let command = setup_bash(
            &Command::Shell("bash --init-file profile.sh".to_string()),
            res.path(),
            &mut env,
        )
        .unwrap();
        assert_eq!(command, Command::Shell("bash --posix".to_string()));
        assert_eq!(env.get("GHOSTTY_BASH_RCFILE"), Some("profile.sh"));
    }

    #[test]
    fn bash_histfile() {
        let res = TmpResourcesDir::init(Shell::Bash);

        // HISTFILE unset.
        {
            let mut env = EnvMap::new();
            let _ = setup_bash(&Command::Shell("bash".to_string()), res.path(), &mut env);
            assert!(env.get("HISTFILE").unwrap().ends_with(".bash_history"));
            assert_eq!(env.get("GHOSTTY_BASH_UNEXPORT_HISTFILE"), Some("1"));
        }

        // HISTFILE set.
        {
            let mut env = EnvMap::new();
            env.put("HISTFILE", "my_history");
            let _ = setup_bash(&Command::Shell("bash".to_string()), res.path(), &mut env);
            assert_eq!(env.get("HISTFILE"), Some("my_history"));
            assert_eq!(env.get("GHOSTTY_BASH_UNEXPORT_HISTFILE"), None);
        }
    }

    #[test]
    fn bash_env() {
        let res = TmpResourcesDir::init(Shell::Bash);
        let mut env = EnvMap::new();
        env.put("ENV", "env.sh");

        let _ = setup_bash(&Command::Shell("bash".to_string()), res.path(), &mut env);
        assert_eq!(env.get("GHOSTTY_BASH_ENV"), Some("env.sh"));
        assert_eq!(
            env.get("ENV"),
            Some(format!("{}/ghostty.bash", res.shell_path.display()).as_str())
        );
    }

    #[test]
    fn bash_additional_arguments() {
        let res = TmpResourcesDir::init(Shell::Bash);
        let mut env = EnvMap::new();

        let command = setup_bash(
            &Command::Shell("bash - --arg file1 file2".to_string()),
            res.path(),
            &mut env,
        )
        .unwrap();
        assert_eq!(
            command,
            Command::Shell("bash --posix - --arg file1 file2".to_string())
        );

        let command = setup_bash(
            &Command::Shell("bash -- --arg file1 file2".to_string()),
            res.path(),
            &mut env,
        )
        .unwrap();
        assert_eq!(
            command,
            Command::Shell("bash --posix -- --arg file1 file2".to_string())
        );
    }

    #[test]
    fn bash_missing_resources() {
        let dir = std::env::temp_dir().join(format!(
            "ghostty-shell-integration-test-missing-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut env = EnvMap::new();
        assert!(
            setup_bash(
                &Command::Shell("bash".to_string()),
                dir.to_str().unwrap(),
                &mut env
            )
            .is_none()
        );
        assert_eq!(0, env.count());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- xdg --------------------------------------------------------------
    #[test]
    fn xdg_empty_xdg_data_dirs() {
        let res = TmpResourcesDir::init(Shell::Fish);
        let mut env = EnvMap::new();

        assert!(setup_xdg_data_dirs(res.path(), &mut env));
        assert_eq!(
            env.get("GHOSTTY_SHELL_INTEGRATION_XDG_DIR"),
            Some(format!("{}/shell-integration", res.path()).as_str())
        );
        assert_eq!(
            env.get("XDG_DATA_DIRS"),
            Some(
                format!(
                    "{}/shell-integration:/usr/local/share:/usr/share",
                    res.path()
                )
                .as_str()
            )
        );
    }

    #[test]
    fn xdg_existing_xdg_data_dirs() {
        let res = TmpResourcesDir::init(Shell::Fish);
        let mut env = EnvMap::new();
        env.put("XDG_DATA_DIRS", "/opt/share");

        assert!(setup_xdg_data_dirs(res.path(), &mut env));
        assert_eq!(
            env.get("GHOSTTY_SHELL_INTEGRATION_XDG_DIR"),
            Some(format!("{}/shell-integration", res.path()).as_str())
        );
        assert_eq!(
            env.get("XDG_DATA_DIRS"),
            Some(format!("{}/shell-integration:/opt/share", res.path()).as_str())
        );
    }

    #[test]
    fn xdg_missing_resources() {
        let dir = std::env::temp_dir().join(format!(
            "ghostty-shell-integration-test-xdg-missing-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut env = EnvMap::new();
        assert!(!setup_xdg_data_dirs(dir.to_str().unwrap(), &mut env));
        assert_eq!(0, env.count());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- nushell ------------------------------------------------------------
    #[test]
    fn nushell_basic() {
        let res = TmpResourcesDir::init(Shell::Nushell);
        let mut env = EnvMap::new();

        let command =
            setup_nushell(&Command::Shell("nu".to_string()), res.path(), &mut env).unwrap();
        assert_eq!(
            command,
            Command::Shell("nu --execute 'use ghostty *'".to_string())
        );
        assert_eq!(
            env.get("GHOSTTY_SHELL_INTEGRATION_XDG_DIR"),
            Some(format!("{}/shell-integration", res.path()).as_str())
        );
        assert!(
            env.get("XDG_DATA_DIRS")
                .unwrap()
                .starts_with(&format!("{}/shell-integration", res.path()))
        );
    }

    #[test]
    fn nushell_unsupported_options() {
        let res = TmpResourcesDir::init(Shell::Nushell);
        let cmdlines = [
            "nu --command exit",
            "nu --lsp",
            "nu -c script.sh",
            "nu -ic script.sh",
        ];
        for cmdline in cmdlines {
            let mut env = EnvMap::new();
            assert!(
                setup_nushell(&Command::Shell(cmdline.to_string()), res.path(), &mut env).is_none()
            );
            assert!(env.get("XDG_DATA_DIRS").is_some());
            assert!(env.get("GHOSTTY_SHELL_INTEGRATION_XDG_DIR").is_some());
        }
    }

    #[test]
    fn nushell_missing_resources() {
        let dir = std::env::temp_dir().join(format!(
            "ghostty-shell-integration-test-nu-missing-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut env = EnvMap::new();
        assert!(
            setup_nushell(
                &Command::Shell("nu".to_string()),
                dir.to_str().unwrap(),
                &mut env
            )
            .is_none()
        );
        assert_eq!(0, env.count());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ---- zsh --------------------------------------------------------------
    #[test]
    fn zsh_basic() {
        let res = TmpResourcesDir::init(Shell::Zsh);
        let mut env = EnvMap::new();

        let command = setup_zsh(&Command::Shell("zsh".to_string()), res.path(), &mut env).unwrap();
        assert_eq!(command, Command::Shell("zsh".to_string()));
        assert_eq!(env.get("ZDOTDIR"), Some(res.shell_path.to_str().unwrap()));
        assert_eq!(env.get("GHOSTTY_ZSH_ZDOTDIR"), None);
    }

    #[test]
    fn zsh_zdotdir_preserved() {
        let res = TmpResourcesDir::init(Shell::Zsh);
        let mut env = EnvMap::new();
        env.put("ZDOTDIR", "$HOME/.config/zsh");

        let command = setup_zsh(&Command::Shell("zsh".to_string()), res.path(), &mut env).unwrap();
        assert_eq!(command, Command::Shell("zsh".to_string()));
        assert_eq!(env.get("ZDOTDIR"), Some(res.shell_path.to_str().unwrap()));
        assert_eq!(env.get("GHOSTTY_ZSH_ZDOTDIR"), Some("$HOME/.config/zsh"));
    }

    #[test]
    fn zsh_missing_resources() {
        let dir = std::env::temp_dir().join(format!(
            "ghostty-shell-integration-test-zsh-missing-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut env = EnvMap::new();
        assert!(
            setup_zsh(
                &Command::Shell("zsh".to_string()),
                dir.to_str().unwrap(),
                &mut env
            )
            .is_none()
        );
        assert_eq!(0, env.count());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
