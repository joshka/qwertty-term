//! Interim PTY session (chunk M2-A replaces `portable-pty` later).
//!
//! Lifted from the reference `crates/spike/src/pty.rs` pattern: open a PTY,
//! spawn the user's `$SHELL`, pump the master's output on a background thread
//! into an mpsc channel the render loop drains, and expose write/resize. Added
//! for R5: [`PtySession::spawn_in_dir`] so a new tab inherits the current tab's
//! working directory (OSC 7 pwd).

use std::{
    env,
    error::Error,
    io::{Read, Write},
    path::Path,
    sync::mpsc::{self, Receiver},
    thread,
};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

pub type PtyResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// A running PTY + child shell, with its output pumped to a channel.
pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Receiver<Vec<u8>>,
}

impl PtySession {
    /// Spawn a shell at the given grid size in the process's current directory.
    pub fn spawn(cols: u16, rows: u16) -> PtyResult<Self> {
        Self::spawn_in_dir(cols, rows, None)
    }

    /// Spawn a shell at the given grid size, optionally in `cwd` (used for
    /// new-tab working-directory inheritance). If `cwd` is `None` or does not
    /// exist, the child inherits the process's directory.
    pub fn spawn_in_dir(cols: u16, rows: u16, cwd: Option<&Path>) -> PtyResult<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size(cols, rows))?;

        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let mut command = CommandBuilder::new(shell);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        if let Some(dir) = cwd
            && dir.is_dir()
        {
            command.cwd(dir);
        }

        let child = pair.slave.spawn_command(command)?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let (tx, output) = mpsc::channel();

        thread::spawn(move || {
            let mut buf = [0; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            master: pair.master,
            writer,
            child,
            output,
        })
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> PtyResult<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> PtyResult<()> {
        self.master.resize(size(cols, rows))?;
        Ok(())
    }

    /// Drain one chunk of output if available (non-blocking).
    pub fn try_read(&self) -> Option<Vec<u8>> {
        self.output.try_recv().ok()
    }

    /// Whether the child shell has exited.
    pub fn child_exited(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
        }
    }
}

fn size(cols: u16, rows: u16) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}
