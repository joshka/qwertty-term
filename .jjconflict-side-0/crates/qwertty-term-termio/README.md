# qwertty-term-termio

Terminal I/O foundations for [qwertty-term](https://github.com/joshka/qwertty-term):
the PTY primitive, the child-process spawn/exec path, and the two-stage read
pipeline that drives bytes into a `qwertty-term-vt` terminal. Ported from
Ghostty's termio subsystem (commit `2da015cd6`).

## What it does

- **`pty`**: the POSIX PTY primitive (via [`rustix`](https://crates.io/crates/rustix)) —
  openpty semantics, IUTF8 setup, `TIOCSWINSZ` resize.
- **`exec`**: spawn `$SHELL` (or a command override) with the child wired to the
  pty slave, inheriting env / cwd.
- **Read pipeline**: upstream's two-stage reader (a read thread feeding a
  processing stage) — **no async runtime** (see `docs/adr/002`); plain threads.

## Platform

Unix (macOS + Linux). The pty layer is POSIX; there is no Windows ConPTY backend.

## Design note

This crate keeps the OS-facing I/O concerns (pty, fork/exec, blocking reads)
separate from the pure terminal state machine in `qwertty-term-vt`, so the
engine stays runtime-free and embeddable while the app composes them. See
`docs/analysis/termio-foundations.md` and `docs/analysis/termio-exec.md`.

## License

MIT OR Apache-2.0
