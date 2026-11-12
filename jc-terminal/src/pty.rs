use anyhow::Result;
use parking_lot::Mutex;
use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::path::Path;
use std::sync::Arc;

/// What runs in a PTY.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum ChildSpec {
  /// The user's login shell.
  #[default]
  Shell,
  /// An argument vector: program, then its arguments, each passed as one
  /// argument (never split on whitespace).
  Command(Vec<String>),
}

/// Handle to the master side of a PTY. Dropping the last clone hangs up the
/// child (SIGHUP) — for `claude attach` that is a detach, not an end.
pub struct PtyHandle {
  master: Mutex<Box<dyn MasterPty + Send>>,
  writer: Arc<Mutex<Box<dyn Write + Send>>>,
  killer: Mutex<Box<dyn ChildKiller + Send + Sync>>,
}

/// A child running in a PTY: the handle, the bytes it prints, and its exit.
pub struct PtyChild {
  pub handle: Arc<PtyHandle>,
  /// Output chunks, until the child closes the PTY.
  pub bytes: flume::Receiver<Vec<u8>>,
  /// The exit code, sent once the child has been reaped (`None` when waiting
  /// failed). A child is always reaped, so an exited one never lingers as a
  /// zombie.
  pub exited: flume::Receiver<Option<u32>>,
  #[cfg(test)]
  pub pid: Option<u32>,
}

impl PtyChild {
  /// Spawn `spec` in a new PTY with `env` added to the inherited environment.
  pub fn spawn(
    spec: &ChildSpec,
    env: &[(String, String)],
    cols: u16,
    rows: u16,
    working_dir: Option<&Path>,
  ) -> Result<Self> {
    let mut cmd = match spec {
      ChildSpec::Shell => CommandBuilder::new_default_prog(),
      ChildSpec::Command(argv) => {
        let (program, args) = argv.split_first().ok_or_else(|| anyhow::anyhow!("empty argv"))?;
        let mut cmd = CommandBuilder::new(program);
        cmd.args(args);
        cmd
      }
    };
    cmd.env("TERM", "xterm-256color");
    cmd.env("COLORTERM", "truecolor");
    for (k, v) in env {
      cmd.env(k, v);
    }
    if let Some(dir) = working_dir {
      cmd.cwd(dir);
    }

    let pair =
      native_pty_system().openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })?;
    let mut child = pair.slave.spawn_command(cmd)?;
    drop(pair.slave);
    #[cfg(test)]
    let pid = child.process_id();
    let killer = child.clone_killer();

    let mut reader = pair.master.try_clone_reader()?;
    let writer = pair.master.take_writer()?;
    let handle = Arc::new(PtyHandle {
      master: Mutex::new(pair.master),
      writer: Arc::new(Mutex::new(writer)),
      killer: Mutex::new(killer),
    });

    let (bytes_tx, bytes) = flume::unbounded::<Vec<u8>>();
    let (exit_tx, exited) = flume::bounded(1);
    // Blocking reads until the child closes the PTY, then reap it.
    std::thread::spawn(move || {
      let mut buf = [0u8; 4096];
      loop {
        match reader.read(&mut buf) {
          Ok(0) | Err(_) => break,
          Ok(n) => {
            if bytes_tx.send(buf[..n].to_vec()).is_err() {
              break;
            }
          }
        }
      }
      drop(bytes_tx);
      let code = child.wait().ok().map(|s| s.exit_code());
      let _ = exit_tx.send(code);
    });

    Ok(Self {
      handle,
      bytes,
      exited,
      #[cfg(test)]
      pid,
    })
  }
}

impl PtyHandle {
  /// Write bytes to the PTY (sends input to the child).
  pub fn write_all(&self, bytes: &[u8]) -> Result<()> {
    let mut writer = self.writer.lock();
    writer.write_all(bytes)?;
    writer.flush()?;
    Ok(())
  }

  /// Resize the PTY.
  pub fn resize(&self, cols: u16, rows: u16, pixel_width: u16, pixel_height: u16) -> Result<()> {
    self.master.lock().resize(PtySize { rows, cols, pixel_width, pixel_height })?;
    Ok(())
  }
}

impl Drop for PtyHandle {
  /// The reader thread holds its own dup of the master, so dropping the master
  /// alone does not hang up the child. Signal it explicitly.
  fn drop(&mut self) {
    let _ = self.killer.lock().kill();
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::time::Duration;

  fn sh(script: &str) -> ChildSpec {
    ChildSpec::Command(vec!["/bin/sh".into(), "-c".into(), script.into()])
  }

  /// `ps` state of `pid`, or empty once the process is gone (reaped).
  fn ps_state(pid: u32) -> String {
    let out = std::process::Command::new("ps")
      .args(["-o", "stat=", "-p", &pid.to_string()])
      .output()
      .unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_string()
  }

  #[test]
  fn argv_arguments_are_not_split_and_env_is_passed() {
    let child = PtyChild::spawn(
      &ChildSpec::Command(vec![
        "/bin/sh".into(),
        "-c".into(),
        "printf '[%s]' \"$1\" \"$CIRA_SESSION\"".into(),
        "sh".into(),
        "New Session".into(),
      ]),
      &[("CIRA_SESSION".into(), "c-1".into())],
      80,
      24,
      None,
    )
    .unwrap();
    let mut out = Vec::new();
    while let Ok(chunk) = child.bytes.recv_timeout(Duration::from_secs(10)) {
      out.extend(chunk);
    }
    assert_eq!(String::from_utf8_lossy(&out), "[New Session][c-1]");
  }

  #[test]
  fn the_login_shell_gets_the_env_too() {
    // The shell pane exports CIRA_SESSION (Cira TMUX-02).
    let child = PtyChild::spawn(
      &ChildSpec::Shell,
      &[("CIRA_SESSION".into(), "c-shell-9".into())],
      80,
      24,
      None,
    )
    .unwrap();
    child.handle.write_all(b"echo X=$CIRA_SESSION=X; exit\r").unwrap();
    let mut out = Vec::new();
    while let Ok(chunk) = child.bytes.recv_timeout(Duration::from_secs(30)) {
      out.extend(chunk);
    }
    let out = String::from_utf8_lossy(&out);
    assert!(out.contains("X=c-shell-9=X"), "shell output: {out}");
  }

  #[test]
  fn an_exited_child_is_reaped_and_reports_its_code() {
    let child = PtyChild::spawn(&sh("exit 3"), &[], 80, 24, None).unwrap();
    let pid = child.pid.unwrap();
    let code = child.exited.recv_timeout(Duration::from_secs(10)).expect("exit reported");
    assert_eq!(code, Some(3));
    assert_eq!(ps_state(pid), "", "the child must not linger as a zombie");
  }

  #[test]
  fn dropping_the_handle_hangs_up_the_child() {
    let child = PtyChild::spawn(&sh("sleep 30"), &[], 80, 24, None).unwrap();
    let pid = child.pid.unwrap();
    let exited = child.exited.clone();
    drop(child);
    exited.recv_timeout(Duration::from_secs(10)).expect("hung-up child exits");
    assert_eq!(ps_state(pid), "");
  }
}
