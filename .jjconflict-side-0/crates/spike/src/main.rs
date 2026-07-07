use std::{
    io::{self, IsTerminal, Read, Write},
    time::Duration,
};

use crossterm::{
    cursor::{self, SetCursorStyle},
    event::{self, Event, KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{
        Attribute, Color as CrosstermColor, Print, ResetColor, SetAttribute, SetBackgroundColor,
        SetForegroundColor,
    },
    terminal::{self as term, Clear, ClearType, EnterAlternateScreen, LeaveAlternateScreen},
};
use ghostty_spike::{CellStyle, CursorStyle, Engine, SnapshotColor, SnapshotUnderline};

mod config;
mod pty;
mod window;

use pty::{PtyResult, PtySession};

fn main() -> PtyResult<()> {
    if std::env::args().nth(1).is_some_and(|arg| arg == "--window") {
        return window::run_window();
    }

    if std::env::args()
        .nth(1)
        .is_some_and(|arg| arg == "--font-report")
    {
        for line in window::font_report_lines() {
            println!("{line}");
        }
        return Ok(());
    }

    if std::env::args()
        .nth(1)
        .is_some_and(|arg| arg == "--render-probe")
    {
        for line in window::render_probe_lines() {
            println!("{line}");
        }
        return Ok(());
    }

    if let Some(command) = std::env::args()
        .nth(1)
        .filter(|arg| arg == "--smoke-command")
    {
        drop(command);
        let Some(command) = std::env::args().nth(2) else {
            return Err("missing command after --smoke-command".into());
        };
        return run_smoke_command(&command);
    }

    if !io::stdin().is_terminal() {
        return run_replay();
    }

    run_interactive()
}

fn run_replay() -> PtyResult<()> {
    let mut bytes = Vec::new();
    io::stdin().read_to_end(&mut bytes)?;

    let (cols, rows) = term::size().unwrap_or((80, 24));
    let mut engine = Engine::new(cols as usize, rows.max(1) as usize);
    engine.write(&bytes);

    println!("{}", engine.screen_dump());
    Ok(())
}

fn run_smoke_command(command: &str) -> PtyResult<()> {
    let mut engine = Engine::new(80, 24);
    let mut pty = PtySession::spawn(80, 24)?;
    pty.write_all(command.as_bytes())?;
    pty.write_all(b"\nexit\n")?;

    loop {
        while let Some(bytes) = pty.try_read() {
            engine.write(&bytes);
            let response = engine.take_output();
            if !response.is_empty() {
                pty.write_all(&response)?;
            }
        }
        if pty.child_exited()? {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    while let Some(bytes) = pty.try_read() {
        engine.write(&bytes);
    }

    println!("{}", engine.screen_dump());
    Ok(())
}

fn run_interactive() -> PtyResult<()> {
    let mut stdout = io::stdout();
    let (cols, rows) = term::size().unwrap_or((80, 24));
    let terminal_rows = rows.max(1);
    let mut engine = Engine::new(cols as usize, terminal_rows as usize);
    let mut pty = PtySession::spawn(cols, terminal_rows)?;

    term::enable_raw_mode()?;
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;

    let result = interactive_loop(&mut engine, &mut pty, &mut stdout);

    execute!(
        stdout,
        ResetColor,
        SetAttribute(Attribute::Reset),
        cursor::Show,
        LeaveAlternateScreen
    )?;
    term::disable_raw_mode()?;

    result
}

fn interactive_loop(
    engine: &mut Engine,
    pty: &mut PtySession,
    stdout: &mut io::Stdout,
) -> PtyResult<()> {
    render(engine, stdout)?;

    loop {
        let mut dirty = false;
        while let Some(bytes) = pty.try_read() {
            engine.write(&bytes);
            let response = engine.take_output();
            if !response.is_empty() {
                pty.write_all(&response)?;
            }
            dirty = true;
        }

        if pty.child_exited()? {
            break;
        }

        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if should_quit(key) => break,
                Event::Key(key) => {
                    if let Some(bytes) = encode_key(key, engine.application_cursor_keys()) {
                        pty.write_all(&bytes)?;
                    }
                }
                Event::Paste(text) => {
                    if engine.bracketed_paste() {
                        pty.write_all(b"\x1b[200~")?;
                        pty.write_all(text.as_bytes())?;
                        pty.write_all(b"\x1b[201~")?;
                    } else {
                        pty.write_all(text.as_bytes())?;
                    }
                }
                Event::Resize(cols, rows) => {
                    let terminal_rows = rows.max(1);
                    engine.resize(cols as usize, terminal_rows as usize);
                    pty.resize(cols, terminal_rows)?;
                    dirty = true;
                }
                _ => {}
            }
        }

        if dirty {
            render(engine, stdout)?;
        }
    }

    Ok(())
}

fn should_quit(key: KeyEvent) -> bool {
    matches!(key.code, KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL))
}

fn encode_key(key: KeyEvent, application_cursor_keys: bool) -> Option<Vec<u8>> {
    let bytes = match key.code {
        KeyCode::Char(ch) => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                encode_control_char(ch)?
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                let mut bytes = vec![0x1b];
                let mut buf = [0; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                bytes
            } else {
                let mut buf = [0; 4];
                ch.encode_utf8(&mut buf).as_bytes().to_vec()
            }
        }
        KeyCode::Enter => b"\r".to_vec(),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => b"\t".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Left => cursor_key(b'D', application_cursor_keys),
        KeyCode::Right => cursor_key(b'C', application_cursor_keys),
        KeyCode::Up => cursor_key(b'A', application_cursor_keys),
        KeyCode::Down => cursor_key(b'B', application_cursor_keys),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        KeyCode::PageUp => b"\x1b[5~".to_vec(),
        KeyCode::PageDown => b"\x1b[6~".to_vec(),
        _ => return None,
    };
    Some(bytes)
}

fn cursor_key(final_byte: u8, application_cursor_keys: bool) -> Vec<u8> {
    if application_cursor_keys {
        vec![0x1b, b'O', final_byte]
    } else {
        vec![0x1b, b'[', final_byte]
    }
}

fn encode_control_char(ch: char) -> Option<Vec<u8>> {
    let upper = ch.to_ascii_uppercase();
    if upper == ' ' {
        return Some(vec![0]);
    }
    if upper == '?' {
        return Some(vec![0x7f]);
    }
    if upper.is_ascii_uppercase() {
        return Some(vec![upper as u8 - b'@']);
    }
    None
}

fn render(engine: &Engine, stdout: &mut io::Stdout) -> PtyResult<()> {
    queue!(
        stdout,
        cursor::MoveTo(0, 0),
        Clear(ClearType::All),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;

    let snapshot = engine.snapshot();
    let window = snapshot.visible_window(0);
    for (row, snapshot_row) in window.iter().enumerate() {
        queue!(stdout, cursor::MoveTo(0, row as u16))?;
        let mut last_style: Option<CellStyle> = None;
        for cell in &snapshot_row.cells {
            if cell.is_spacer() {
                continue;
            }
            if Some(cell.style) != last_style {
                apply_style(stdout, &cell.style)?;
                last_style = Some(cell.style);
            }
            queue!(stdout, Print(cell.ch))?;
        }
    }

    let cursor = snapshot.cursor;
    queue!(
        stdout,
        ResetColor,
        SetAttribute(Attribute::Reset),
        to_crossterm_cursor_style(cursor.style),
        cursor::MoveTo(cursor.col as u16, cursor.row as u16)
    )?;
    if cursor.visible {
        queue!(stdout, cursor::Show)?;
    } else {
        queue!(stdout, cursor::Hide)?;
    }
    stdout.flush()?;
    Ok(())
}

fn to_crossterm_cursor_style(style: CursorStyle) -> SetCursorStyle {
    match style {
        CursorStyle::Block | CursorStyle::BlockHollow => SetCursorStyle::SteadyBlock,
        CursorStyle::Underline => SetCursorStyle::SteadyUnderScore,
        CursorStyle::Bar => SetCursorStyle::SteadyBar,
    }
}

fn apply_style(stdout: &mut io::Stdout, style: &CellStyle) -> PtyResult<()> {
    queue!(
        stdout,
        ResetColor,
        SetAttribute(Attribute::Reset),
        SetAttribute(if style.bold {
            Attribute::Bold
        } else if style.faint {
            Attribute::Dim
        } else {
            Attribute::NormalIntensity
        }),
        SetAttribute(if style.italic {
            Attribute::Italic
        } else {
            Attribute::NoItalic
        }),
        SetAttribute(if style.underline != SnapshotUnderline::None {
            Attribute::Underlined
        } else {
            Attribute::NoUnderline
        }),
        SetAttribute(if style.blink {
            Attribute::SlowBlink
        } else {
            Attribute::NoBlink
        }),
        SetAttribute(if style.inverse {
            Attribute::Reverse
        } else {
            Attribute::NoReverse
        }),
        SetAttribute(if style.strikethrough {
            Attribute::CrossedOut
        } else {
            Attribute::NotCrossedOut
        })
    )?;

    if let Some(fg) = to_crossterm_color(style.fg) {
        queue!(stdout, SetForegroundColor(fg))?;
    }
    if let Some(bg) = to_crossterm_color(style.bg) {
        queue!(stdout, SetBackgroundColor(bg))?;
    }

    Ok(())
}

fn to_crossterm_color(color: SnapshotColor) -> Option<CrosstermColor> {
    match color {
        SnapshotColor::Default => None,
        SnapshotColor::Palette(value) => Some(CrosstermColor::AnsiValue(value)),
        SnapshotColor::Rgb { r, g, b } => Some(CrosstermColor::Rgb { r, g, b }),
    }
}
