use std::{
    env,
    error::Error,
    io::{Read, Write},
    sync::mpsc::{self, Receiver},
    thread,
};

use portable_pty::{Child, CommandBuilder, ExitStatus, MasterPty, PtySize, native_pty_system};

pub(crate) type PtyResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

pub(crate) struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Receiver<Vec<u8>>,
}

impl PtySession {
    pub(crate) fn spawn(cols: u16, rows: u16) -> PtyResult<Self> {
        let pty_system = native_pty_system();
        let pair = pty_system.openpty(size(cols, rows))?;

        let shell = env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".to_string());
        let mut command = CommandBuilder::new(shell);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");

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

    pub(crate) fn write_all(&mut self, bytes: &[u8]) -> PtyResult<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    pub(crate) fn resize(&mut self, cols: u16, rows: u16) -> PtyResult<()> {
        self.master.resize(size(cols, rows))?;
        Ok(())
    }

    pub(crate) fn try_read(&self) -> Option<Vec<u8>> {
        self.output.try_recv().ok()
    }

    pub(crate) fn child_exited(&mut self) -> PtyResult<bool> {
        Ok(self.child.try_wait()?.is_some())
    }

    pub(crate) fn child_status(&mut self) -> PtyResult<Option<ExitStatus>> {
        Ok(self.child.try_wait()?)
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
