use anyhow::Result;
use parking_lot::Mutex;
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

/// Handle to the master side of a PTY.
pub struct PtyHandle {
  master: Mutex<Box<dyn MasterPty + Send>>,
  writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

impl PtyHandle {
  /// Spawn a shell in a new PTY.
  ///
  /// Returns the handle (for writing/resizing) and a reader (moved to a background thread).
  pub fn spawn_shell(
    cols: u16,
    rows: u16,
    working_dir: Option<&Path>,
  ) -> Result<(Self, Box<dyn Read + Send>)> {
    let pty_system = native_pty_system();
    let pair = pty_system.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;

    let mut cmd = CommandBuilder::new_default_prog();
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    if let Some(dir) = working_dir {
      cmd.cwd(dir);
    }

    let _child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);

    let reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;

    Ok((Self { master: Mutex::new(pair.master), writer: Arc::new(Mutex::new(writer)) }, reader))
  }

  /// Write bytes to the PTY (sends input to the shell).
  pub fn write_all(&self, bytes: &[u8]) -> Result<()> {
    let mut writer = self.writer.lock();
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(())
  }

  /// Resize the PTY.
  pub fn resize(&self, cols: u16, rows: u16) -> Result<()> {
    self.master.lock().resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
    Ok(())
  }
}
