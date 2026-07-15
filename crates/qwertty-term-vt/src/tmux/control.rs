//! tmux control-mode parser. Port of `terminal/tmux/control.zig` (Ghostty
//! `2da015cd6`).
//!
//! Takes tmux control-mode output (`tmux -CC`) one byte at a time and yields
//! structured [`Notification`]s. It is fully agnostic to how the data is
//! received and sent — the caller establishes the connection (exec, socket, …)
//! and drives [`ControlParser::put`]. The native viewer that turns these
//! notifications into surfaces lives in the app/termio layer (ADR 004 slice 5).
//!
//! Upstream matches the notification lines with `oniguruma` regexes; we
//! hand-roll equivalent byte scanners (the patterns are simple and anchored),
//! keeping the core VT crate dependency-free. The two multi-field patterns
//! (`%layout-change`, `%client-session-changed`) use greedy leading captures
//! upstream; we replicate that with right-anchored splits so the behaviour
//! matches for every input tmux actually emits (see the field notes below).

use std::fmt;

/// The parser exceeded its byte budget and entered the broken state. Returned
/// exactly once (the first time the limit is hit); subsequent `put` calls
/// return `Ok(None)` as all input is dropped. Port of the `error.OutOfMemory`
/// that `Parser.put` returns on overflow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferOverflow;

impl fmt::Display for BufferOverflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("tmux control-mode buffer exceeded its byte limit")
    }
}

impl std::error::Error for BufferOverflow {}

/// Default maximum in-progress buffer size (1 MiB). Port of `max_bytes`'s
/// default. Exceeding it forces the control-mode session into the broken state.
pub const DEFAULT_MAX_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Outside any active notification. Drops output unless it is `%` at the
    /// start of a line.
    Idle,
    /// Unexpected input; processing cannot continue and all input is dropped.
    Broken,
    /// Inside an active notification (started with `%`).
    Notification,
    /// Inside a `%begin`/`%end` block, accumulating its raw payload.
    Block,
}

/// A tmux control-mode parser. Port of `tmux/control.zig`'s `Parser`.
#[derive(Debug, Clone)]
pub struct ControlParser {
    state: State,
    /// In-progress notification / block payload. Faithful to upstream's
    /// `buffer`: notification data returned from `put` is copied out of it, so
    /// (unlike upstream's borrowed slices) a returned [`Notification`] does not
    /// alias the buffer.
    buffer: Vec<u8>,
    max_bytes: usize,
}

impl Default for ControlParser {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlParser {
    /// A new parser in the idle state with the default 1 MiB byte budget.
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            buffer: Vec::new(),
            max_bytes: DEFAULT_MAX_BYTES,
        }
    }

    /// A new parser with a custom byte budget (`max_bytes`). Mirrors setting
    /// the `max_bytes` field upstream.
    pub fn with_max_bytes(max_bytes: usize) -> Self {
        Self {
            state: State::Idle,
            buffer: Vec::new(),
            max_bytes,
        }
    }

    /// Feed one byte of control-mode input. Returns `Ok(Some(notification))`
    /// when a byte completes one, `Ok(None)` while accumulating, and
    /// `Err(BufferOverflow)` the first time the byte budget is exceeded (after
    /// which the parser is broken and returns `Ok(None)`). Port of `Parser.put`.
    pub fn put(&mut self, byte: u8) -> Result<Option<Notification>, BufferOverflow> {
        // Broken: drop everything. Checked before the buffer since `broken`
        // conceptually releases it.
        if self.state == State::Broken {
            return Ok(None);
        }

        if self.buffer.len() >= self.max_bytes {
            self.state = State::Broken;
            self.buffer = Vec::new();
            return Err(BufferOverflow);
        }

        match self.state {
            State::Broken => return Ok(None),

            // Waiting for a notification: anything but `%` at line start is
            // unexpected — break and report an exit (upstream returns `.exit`).
            State::Idle => {
                if byte != b'%' {
                    self.state = State::Broken;
                    self.buffer = Vec::new();
                    return Ok(Some(Notification::Exit));
                }
                self.buffer.clear();
                self.state = State::Notification;
            }

            // Accumulate the notification line; a newline completes it.
            State::Notification => {
                if byte == b'\n' {
                    // A parse failure is NOT fatal (we may parse later
                    // notifications), so it maps to `Ok(None)`.
                    return Ok(self.parse_notification());
                }
            }

            // Accumulate the block payload; on each newline, check whether the
            // just-finished line is the block's `%end`/`%error` guard.
            State::Block => {
                if byte == b'\n' {
                    let written = &self.buffer;
                    let idx = match written.iter().rposition(|&b| b == b'\n') {
                        Some(v) => v + 1,
                        None => 0,
                    };
                    let line = &written[idx..];

                    if let Some(terminator) = parse_block_terminator(line) {
                        let output = trim_end_crlf(&written[..idx]).to_vec();
                        self.state = State::Idle;
                        return Ok(Some(match terminator {
                            BlockTerminator::End => Notification::BlockEnd(output),
                            BlockTerminator::Err => Notification::BlockErr(output),
                        }));
                    }
                    // Not a terminator: fall through and accumulate the newline.
                }
            }
        }

        self.buffer.push(byte);
        Ok(None)
    }

    /// Parse the accumulated notification line (buffer holds the line without
    /// its terminating newline). Returns `None` for `%begin` (which transitions
    /// to block state), for unknown commands, and for malformed known commands
    /// — all of which return to idle. Port of `Parser.parseNotification`.
    fn parse_notification(&mut self) -> Option<Notification> {
        debug_assert_eq!(self.state, State::Notification);

        let line = trim_end_cr(&self.buffer);
        let cmd_end = line.iter().position(|&b| b == b' ').unwrap_or(line.len());
        let cmd = &line[..cmd_end];

        // `%begin`: start a block; the matching `%end`/`%error` terminates it.
        if cmd == b"%begin" {
            self.state = State::Block;
            self.buffer.clear();
            return None;
        }

        // Try to parse the recognized notification shapes. Each returns `Some`
        // on a match; a malformed known command falls through to the reset.
        let parsed = parse_line(line);
        // Reset to idle regardless (a matched notification owns its copied
        // data, so clearing the buffer here is safe — unlike upstream, which
        // must retain the buffer because its notifications borrow it).
        self.buffer.clear();
        self.state = State::Idle;
        parsed
    }
}

/// The two block-guard terminators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BlockTerminator {
    End,
    Err,
}

/// A line terminates a block only if it exactly matches tmux's `%end`/`%error`
/// guard-line shape: `%end <time> <command-id> <flags>` with all three being
/// base-10 integers and no extra tokens. Port of `parseBlockTerminator`.
fn parse_block_terminator(line_raw: &[u8]) -> Option<BlockTerminator> {
    let line = trim_end_cr(line_raw);

    let mut fields = line.split(|&b| b == b' ').filter(|f| !f.is_empty());
    let cmd = fields.next()?;
    let terminator = if cmd == b"%end" {
        BlockTerminator::End
    } else if cmd == b"%error" {
        BlockTerminator::Err
    } else {
        return None;
    };

    let time = fields.next()?;
    let command_id = fields.next()?;
    let flags = fields.next()?;
    if fields.next().is_some() {
        return None;
    }

    // The three metadata fields must all be base-10 integers (upstream parses
    // them as usize and requires success). We only validate; the values are
    // unused for now (a future improvement is matching them to the `%begin`).
    parse_all_digits(time)?;
    parse_all_digits(command_id)?;
    parse_all_digits(flags)?;

    Some(terminator)
}

/// Decode tmux control-mode `%output` escaping. tmux writes each control /
/// non-printable byte (and backslash) in `%output` data as a backslash followed
/// by exactly three octal digits (`\ooo`, e.g. ESC → `\033`, LF → `\012`,
/// backslash → `\134`). Everything else is passed through verbatim. The pane
/// terminal needs the raw bytes, so `Notification::Output.data` is the decoded
/// form (matching upstream's `// unescaped` intent). A backslash not followed by
/// three octal digits is left as-is (defensive; tmux always emits the escaped
/// form).
fn unescape_output(data: &[u8]) -> Vec<u8> {
    let is_octal = |b: u8| (b'0'..=b'7').contains(&b);
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'\\'
            && i + 3 < data.len()
            && is_octal(data[i + 1])
            && is_octal(data[i + 2])
            && is_octal(data[i + 3])
        {
            // 3 octal digits -> one byte. Compute in u16 to avoid an overflow
            // panic on a malformed `\4xx`+ escape (real tmux only emits 0-255).
            let v = ((data[i + 1] - b'0') as u16) << 6
                | ((data[i + 2] - b'0') as u16) << 3
                | (data[i + 3] - b'0') as u16;
            out.push(v as u8);
            i += 4;
        } else {
            out.push(data[i]);
            i += 1;
        }
    }
    out
}

/// Parse a single notification line (without trailing CR) into a
/// [`Notification`], or `None` if it doesn't match a recognized shape. This is
/// the hand-rolled equivalent of `parseNotification`'s per-command regexes.
fn parse_line(line: &[u8]) -> Option<Notification> {
    // %output %<pane-id> <data>   (data non-empty)
    if let Some(rest) = strip_prefix(line, b"%output %") {
        let (pane_id, rest) = parse_usize_prefix(rest)?;
        let data = strip_prefix(rest, b" ")?;
        if data.is_empty() {
            return None;
        }
        return Some(Notification::Output {
            pane_id,
            data: unescape_output(data),
        });
    }

    // %session-changed $<id> <name>   (name non-empty)
    if let Some(rest) = strip_prefix(line, b"%session-changed $") {
        let (id, rest) = parse_usize_prefix(rest)?;
        let name = strip_prefix(rest, b" ")?;
        if name.is_empty() {
            return None;
        }
        return Some(Notification::SessionChanged {
            id,
            name: name.to_vec(),
        });
    }

    // %sessions-changed   (exact)
    if line == b"%sessions-changed" {
        return Some(Notification::SessionsChanged);
    }

    // %layout-change @<id> <layout> <visible-layout> <raw-flags>
    // The last capture (`raw-flags`) is `.*` (may be empty); the first two are
    // `.+`. Upstream's greedy leading captures resolve to a split on the final
    // two spaces, which we replicate directly.
    if let Some(rest) = strip_prefix(line, b"%layout-change @") {
        let (window_id, rest) = parse_usize_prefix(rest)?;
        let rest = strip_prefix(rest, b" ")?;
        let (layout, visible_layout, raw_flags) = split_last_two_spaces(rest)?;
        return Some(Notification::LayoutChange {
            window_id,
            layout: layout.to_vec(),
            visible_layout: visible_layout.to_vec(),
            raw_flags: raw_flags.to_vec(),
        });
    }

    // %window-add @<id>
    if let Some(rest) = strip_prefix(line, b"%window-add @") {
        let (id, rest) = parse_usize_prefix(rest)?;
        if !rest.is_empty() {
            return None;
        }
        return Some(Notification::WindowAdd { id });
    }

    // %window-close @<id> / %unlinked-window-close @<id>: a window closed. tmux
    // emits `%window-close` for a window linked to the current session and
    // `%unlinked-window-close` for one that isn't (e.g. the only pane of a
    // non-active window gets Ctrl-D'd); both mean the native tab for that window
    // should be dropped. Upstream `control.zig` does not decode these — see
    // `docs/analysis/tmux-control-mode-states.md` gap 3.
    if let Some(rest) = strip_prefix(line, b"%window-close @")
        .or_else(|| strip_prefix(line, b"%unlinked-window-close @"))
    {
        let (id, rest) = parse_usize_prefix(rest)?;
        if !rest.is_empty() {
            return None;
        }
        return Some(Notification::WindowClose { id });
    }

    // %window-renamed @<id> <name>   (name non-empty)
    if let Some(rest) = strip_prefix(line, b"%window-renamed @") {
        let (id, rest) = parse_usize_prefix(rest)?;
        let name = strip_prefix(rest, b" ")?;
        if name.is_empty() {
            return None;
        }
        return Some(Notification::WindowRenamed {
            id,
            name: name.to_vec(),
        });
    }

    // %window-pane-changed @<window-id> %<pane-id>
    if let Some(rest) = strip_prefix(line, b"%window-pane-changed @") {
        let (window_id, rest) = parse_usize_prefix(rest)?;
        let rest = strip_prefix(rest, b" %")?;
        let (pane_id, rest) = parse_usize_prefix(rest)?;
        if !rest.is_empty() {
            return None;
        }
        return Some(Notification::WindowPaneChanged { window_id, pane_id });
    }

    // %client-detached <client>   (client non-empty)
    if let Some(rest) = strip_prefix(line, b"%client-detached ") {
        if rest.is_empty() {
            return None;
        }
        return Some(Notification::ClientDetached {
            client: rest.to_vec(),
        });
    }

    // %client-session-changed <client> $<id> <name>
    // Upstream: `(.+) \$([0-9]+) (.+)` with a greedy leading `client`. We take
    // the right-most ` $<digits> ` boundary that leaves a non-empty client and
    // name — identical for every input tmux emits (clients are tty paths with
    // no ` $`).
    if let Some(rest) = strip_prefix(line, b"%client-session-changed ") {
        if let Some((client, session_id, name)) = split_client_session(rest) {
            return Some(Notification::ClientSessionChanged {
                client: client.to_vec(),
                session_id,
                name: name.to_vec(),
            });
        }
        return None;
    }

    // Unknown command: upstream logs and returns to idle.
    None
}

/// Split `<client> $<id> <name>` with a greedy leading `client`: scan right to
/// left for a ` $<digits> <non-empty-name>` suffix that leaves a non-empty
/// client. Returns `(client, id, name)`.
fn split_client_session(s: &[u8]) -> Option<(&[u8], usize, &[u8])> {
    // Candidate delimiters are occurrences of " $". Prefer the right-most that
    // parses, so `client` is as long as possible (greedy).
    let mut search_end = s.len();
    while search_end >= 2 {
        // Find the last " $" at or before search_end; none left -> no match.
        let pos = s[..search_end].windows(2).rposition(|w| w == b" $")?;
        let client = &s[..pos];
        let after = &s[pos + 2..]; // after " $"
        if !client.is_empty()
            && let Some((id, rest)) = parse_usize_prefix(after)
            && let Some(name) = strip_prefix(rest, b" ")
            && !name.is_empty()
        {
            return Some((client, id, name));
        }
        // This delimiter didn't work; try an earlier one.
        search_end = pos + 1;
    }
    None
}

/// Split `s` on its final two spaces into `(a, b, c)` where `a` and `b` are
/// non-empty and `c` may be empty. Replicates the greedy `(.+) (.+) (.*)`
/// capture: greedy leading groups push the split to the last two spaces.
fn split_last_two_spaces(s: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let last = s.iter().rposition(|&b| b == b' ')?;
    let (head, c) = (&s[..last], &s[last + 1..]);
    let prev = head.iter().rposition(|&b| b == b' ')?;
    let (a, b) = (&head[..prev], &head[prev + 1..]);
    if a.is_empty() || b.is_empty() {
        return None;
    }
    Some((a, b, c))
}

/// If `s` begins with `prefix`, return the remainder; else `None`.
fn strip_prefix<'a>(s: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    s.strip_prefix(prefix)
}

/// Parse one-or-more leading ASCII digits as a `usize`, returning the value and
/// the remainder. `None` if there is no leading digit or the value overflows
/// `usize`. (Upstream `parseInt … catch unreachable` would panic on overflow;
/// we treat overflow as a non-match — no panic on adversarial input, matching
/// the Zig-port zero-capacity/overflow guidance.)
fn parse_usize_prefix(s: &[u8]) -> Option<(usize, &[u8])> {
    let mut i = 0;
    let mut val: usize = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        val = val.checked_mul(10)?.checked_add((s[i] - b'0') as usize)?;
        i += 1;
    }
    if i == 0 {
        return None;
    }
    Some((val, &s[i..]))
}

/// `Some(())` if `s` is non-empty and entirely ASCII digits within `usize`.
fn parse_all_digits(s: &[u8]) -> Option<()> {
    let (_, rest) = parse_usize_prefix(s)?;
    if rest.is_empty() { Some(()) } else { None }
}

/// Trim a single trailing `\r` (upstream strips one CR before parsing a line).
fn trim_end_cr(s: &[u8]) -> &[u8] {
    match s.last() {
        Some(&b'\r') => &s[..s.len() - 1],
        _ => s,
    }
}

/// Trim all trailing `\r`/`\n` (upstream `trimRight(…, "\r\n")` on block output).
fn trim_end_crlf(s: &[u8]) -> &[u8] {
    let mut end = s.len();
    while end > 0 && (s[end - 1] == b'\r' || s[end - 1] == b'\n') {
        end -= 1;
    }
    &s[..end]
}

/// A tmux control-mode notification. Port of `control.zig`'s `Notification`
/// union. Byte-slice fields are owned `Vec<u8>` (tmux data is arbitrary bytes,
/// and copying decouples a notification's lifetime from the parser buffer).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notification {
    /// Entering tmux control mode. Not sent by tmux itself — emitted by the
    /// integration when control mode is detected (the DCS `\ePtmux;` seam).
    Enter,

    /// Exit control mode. (tmux's human-readable "reason" string is dropped,
    /// matching upstream.)
    Exit,

    /// End of a `%begin`/`%end` block, carrying the raw payload.
    BlockEnd(Vec<u8>),
    /// End of a `%begin`/`%error` block, carrying the raw payload.
    BlockErr(Vec<u8>),

    /// Raw output from a pane (`%output %<pane> <data>`). `data` is still
    /// escaped (unescaping is the `output` module's job — ADR 004 slice 3).
    Output { pane_id: usize, data: Vec<u8> },

    /// The client attached to session `id` named `name`
    /// (`%session-changed $<id> <name>`).
    SessionChanged { id: usize, name: Vec<u8> },

    /// A session was created or destroyed (`%sessions-changed`).
    SessionsChanged,

    /// The layout of window `window_id` changed (`%layout-change`).
    LayoutChange {
        window_id: usize,
        layout: Vec<u8>,
        visible_layout: Vec<u8>,
        raw_flags: Vec<u8>,
    },

    /// Window `id` was linked to the current session (`%window-add @<id>`).
    WindowAdd { id: usize },

    /// Window `id` closed (`%window-close @<id>` or `%unlinked-window-close
    /// @<id>`) — its native tab should be dropped.
    WindowClose { id: usize },

    /// Window `id` was renamed to `name` (`%window-renamed @<id> <name>`).
    WindowRenamed { id: usize, name: Vec<u8> },

    /// The active pane in window `window_id` changed to `pane_id`
    /// (`%window-pane-changed @<window> %<pane>`).
    WindowPaneChanged { window_id: usize, pane_id: usize },

    /// The client detached (`%client-detached <client>`).
    ClientDetached { client: Vec<u8> },

    /// The client attached to session `session_id` named `name`
    /// (`%client-session-changed <client> $<id> <name>`).
    ClientSessionChanged {
        client: Vec<u8>,
        session_id: usize,
        name: Vec<u8>,
    },
}

#[cfg(test)]
mod tests;
