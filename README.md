# ghostty-rs

Rust proof-of-concept for a focused slice of Ghostty's VT core and terminal IO.

This is not a full Ghostty port. It keeps the VT state machine, grid, cursor,
style, and screen behavior small enough to inspect while borrowing behavior
from Ghostty's terminal and stream tests.

## Run

There are four useful run modes. The native window is the closest thing to
"run the terminal app"; the terminal-hosted mode is useful when you want to
exercise the PTY and VT core without opening another window.

### Terminal-Hosted Shell

Start a PTY-backed terminal running your `$SHELL` inside the terminal you used
to launch Cargo:

```bash
cargo run
```

The app uses the local terminal as a simple renderer, starts a real PTY, and
feeds PTY output through the Rust VT core. Keyboard input goes to the PTY.
Press `Ctrl-Q` to exit the wrapper. `Esc` and `Ctrl-C` are sent to the shell or
foreground application.

Use this mode for quick terminal-core checks. It is not the native macOS app
experience.

### Native Window

Start the experimental native macOS window frontend:

```bash
cargo run -- --window
```

This opens a separate window backed by the same PTY and VT core. Close the
window, or exit the shell inside it, when you are done. The Cargo process stays
attached to the launching terminal while the window is open.

The window path supports mouse-wheel scrollback and `Shift-PageUp` /
`Shift-PageDown`. The window derives its grid size from the active monospace
font metrics. It loads local Nerd Fonts automatically when they are installed,
preferring mono regular variants, and falls back to egui's bundled monospace
font otherwise. Drag with the primary mouse button to select visible text, then
copy with the platform copy shortcut. When a terminal app enables mouse
reporting, the window sends click, drag, and wheel events to the PTY; hold
`Shift` to select text instead. The window closes after a successful shell exit
and keeps an error banner visible when the child exits unsuccessfully.

Override the native window font with:

```bash
GHOSTTY_RS_FONT_PATH="$HOME/Library/Fonts/JetBrainsMonoNerdFontMono-Regular.ttf" \
  cargo run -- --window
GHOSTTY_RS_FONT_SIZE=16 cargo run -- --window
```

When `GHOSTTY_RS_FONT_PATH` is set, that font is installed first and discovered
Nerd Fonts are kept as bounded fallbacks. This gives explicit family selection
priority while still leaving local symbol fonts available for devicons and
Powerline-style glyphs. The preferences window shows a compact coverage readout
for installed terminal fonts, currently probing Powerline separator glyphs and
a small devicon set.

Print the same font coverage report without opening the UI:

```bash
cargo run -- --font-report
```

Print the deterministic renderer-run probe for the same symbol set:

```bash
cargo run -- --render-probe
```

### App Bundle

Build a local macOS `.app` bundle and open it:

```bash
cargo run -p xtask -- bundle
open target/ghostty-rs.app
```

The bundle is a local development wrapper around `cargo run -- --window`, not a
release-ready signed app. Closing the opened window is enough to stop the app
process. The bundle step generates `Resources/ghostty-rs.icns` locally and
writes `CFBundleIconFile` into `Info.plist`.

The native window has a small first-pass macOS app shell:

- `Command-N` opens another terminal window.
- `Command-,` opens preferences with a font-size control.
- `Command-Q` closes the current window.

Window size and font size are restored through:

```text
~/Library/Application Support/ghostty-rs/preferences
```

### Replay And Smoke Checks

Replay piped VT bytes and print the plain-text screen dump:

```bash
printf 'Hello \033[31mred\033[0m\n' | cargo run --quiet
```

Run a repeatable PTY smoke command:

```bash
cargo run -- --smoke-command 'printf READY'
```

The native renderer maps 256-color indexed SGR values through the xterm color
cube and grayscale ramp. It also plans visible rows into text runs before
painting, which is the current attachment point for future shaping, fallback,
and glyph caching work.

### Which Command Should I Use?

- Use `cargo run -- --window` for the current UI.
- Use `cargo run -p xtask -- bundle && open target/ghostty-rs.app` to try the
  local `.app` wrapper.
- Use `cargo run -- --font-report` to check local Nerd Font symbol coverage.
- Use `cargo run -- --render-probe` to check renderer run planning for the
  Nerd Font probe glyphs.
- Use `cargo run` for the terminal-hosted PTY wrapper.
- Use `cargo run -- --smoke-command 'printf READY'` for a noninteractive check.
- Use piped input when you want a deterministic VT replay screen dump.

## Implemented Core

- `Terminal::new(cols, rows)`
- `Terminal::write(&mut self, bytes: &[u8])`
- grid, cell, cursor, current-style, screen, title, scrollback, and bell
  accessors
- styled scrollback rows for native renderers
- viewport resize with visible cell preservation and cursor clamping
- PTY-backed shell process with read, write, and resize propagation
- terminal-hosted and native-window frontends
- terminal-to-PTY responses for DSR operating status, cursor position reports,
  color-scheme query, and primary/secondary device attributes
- DEC private modes for application cursor keys, cursor visibility,
  wraparound, alternate screen, bracketed paste, focus reporting, and basic
  mouse reporting
- DECSCUSR cursor shape control for block, underline, and bar cursors
- plain-text screen dump
- UTF-8 printable text, including code points split across writes and basic
  wide-character cell handling
- C0 controls: BS, HT, LF, VT, FF, CR, and BEL counting
- ESC: IND, NEL, RI, save/restore cursor, RIS, and DECALN
- CSI: CUU, CUD, CUF, CUB, CUP, HVP, ED, EL, SU, SD, scroll region,
  save/restore cursor, ICH, DCH, ECH, IL, DL, CHA, VPA, CNL, CPL, CHT,
  CBT, REP, TBC, DEC wraparound mode, and alternate screen
- SGR: reset, bold, faint, italic, underline, blink, inverse, strikethrough,
  ANSI colors, bright ANSI colors, 256-color, and RGB foreground/background
  colors
- OSC 0/2 title setting and OSC 52 clipboard writes with BEL and ST
  terminators
- pending wrap and scroll-up at the bottom row

## Module Layout

- `src/lib.rs`: small public facade and re-exports.
- `src/terminal.rs`: terminal state, parser dispatch, high-level mode dispatch,
  and reset behavior.
- `src/terminal/edit.rs`: screen editing, cursor movement, tabs, wrapping,
  scroll regions, alternate screen switching, and DECALN.
- `src/terminal/effects.rs`: OSC-driven host-facing side effects such as title
  and clipboard writes.
- `src/terminal/modes.rs`: terminal-specific mode policy for DEC private modes
  and cursor-shape control.
- `src/terminal/report.rs`: terminal-to-PTY reports such as DSR, cursor
  position, color-scheme query, and device attributes.
- `src/screen.rs`: grid, cursor, scrollback, and plain-text screen helpers.
- `src/mode.rs`: terminal mode state and value types shared by core and
  frontends.
- `src/osc.rs`: pure OSC payload decoding.
- `src/parser.rs`: parser state, CSI sequence shape, and parameter parsing
  helpers.
- `src/color.rs`, `src/style.rs`, `src/cell.rs`: focused value types,
  including SGR style mutation.
- `src/pty.rs`: small PTY session wrapper.
- `src/main.rs`: runnable crossterm terminal, replay CLI, and PTY smoke mode.
- `src/window/`: experimental native egui window frontend, app-shell
  shortcuts, renderer row/run planning, input encoding, font loading, and
  color mapping. Font loading includes glyph coverage probes for expected Nerd
  Font symbols.
- `xtask/`: local development helper for generating a macOS `.app` bundle.
  It builds the launcher, writes bundle metadata, and generates the app icon.

## Reference Files

The Zig checkout at `/Users/joshka/local/ghostty` is used only as reference.
The main files consulted are:

- `src/terminal/Parser.zig`
- `src/terminal/stream.zig`
- `src/terminal/stream_terminal.zig`
- `src/terminal/Terminal.zig`
- `src/terminal/Screen.zig`
- `src/terminal/PageList.zig`
- `src/terminal/page.zig`
- `src/terminal/style.zig`
- `src/terminal/color.zig`

## Roadmap

See `docs/roadmap.md` for the recent progress list, prioritized next work, and
the current gap to Ghostty proper.

Useful continuation docs:

- `docs/handoff.md`: agent pickup packet with current state, validation,
  tradeoffs, lessons, and next chunks.
- `docs/architecture.md`: current architecture, module boundaries, and known
  architecture debt.
- `docs/ghostty-gap.md`: explicit gap between this prototype and Ghostty proper.

## Intentional Omissions

The implementation uses `TODO(port): ...` comments for omitted behavior that
would belong in a fuller port, including:

- full C0/C1 coverage
- charset designation and locking shifts
- most device reports and queries beyond the implemented DSR/DA subset
- complete DEC private mode behavior
- OSC palette, clipboard readback policy, hyperlinks, shell integration, and
  Kitty protocols
- full Unicode grapheme correctness and ambiguous-width policy
- renderer architecture, font shaping, Kitty graphics, tmux, search, C ABI,
  and `build.zig` integration

## Validation

Run:

```bash
cargo fmt --check
cargo test --workspace
markdownlint-cli2 "**/*.md"
```

Replay compatibility fixtures live under `tests/fixtures/replay/*`. Each fixture
has:

- `size.txt`: `cols rows`
- `input.esc`: terminal stream using escaped bytes such as `\e`, `\n`, `\r`,
  and `\xHH`
- `expected.txt`: expected plain-text screen dump
