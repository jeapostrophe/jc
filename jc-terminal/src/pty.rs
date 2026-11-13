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
  /// Set by the reader thread at EOF, under this lock, before it reaps the
  /// child. Once set the pid may be freed and reused, so `Drop` must not
  /// signal it; until then an exited child is an unreaped zombie and keeps
  /// its pid.
  closed: Arc<Mutex<bool>>,
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
    let closed = Arc::new(Mutex::new(false));
    let handle = Arc::new(PtyHandle {
      master: Mutex::new(pair.master),
      writer: Arc::new(Mutex::new(writer)),
      killer: Mutex::new(killer),
      closed: closed.clone(),
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
      *closed.lock() = true;
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
    let closed = self.closed.lock();
    if !*closed {
      let _ = self.killer.lock().kill();
    }
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

  /// tmux's full path, as Cira names it in `argv`.
  fn tmux() -> std::path::PathBuf {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
      .chain(["/opt/homebrew/bin".into(), "/usr/local/bin".into()])
      .map(|dir| dir.join("tmux"))
      .find(|t| t.is_file())
      .expect("the Terminal pane's test attaches to a real tmux: brew install tmux")
  }

  /// Read `child`'s output until it contains `want`, or panic with what it printed.
  fn read_until(child: &PtyChild, want: &str) {
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !String::from_utf8_lossy(&out).contains(want) {
      let left = deadline.saturating_duration_since(std::time::Instant::now());
      match child.bytes.recv_timeout(left) {
        Ok(chunk) => out.extend(chunk),
        Err(_) => panic!("no {want:?} in: {}", String::from_utf8_lossy(&out)),
      }
    }
  }

  #[test]
  fn the_terminal_pane_attaches_to_the_shell_cira_names() {
    // Cira PLAN TMUX-02: a fake `cira` answers `session shell` as Cira does,
    // for a shell in a real tmux server on a private socket. jc runs `argv`
    // as an argument list; through zsh, `=s-c-1` would be a path lookup.
    use jc_core::cira::{At, Cira};
    use jc_core::lifecycle::{ShellChild, shell_child};
    let dir = std::env::temp_dir().join(format!("jc-shell-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let tmux = tmux();
    let sock = dir.join("tmux.sock");
    let server = |args: &[&str]| {
      std::process::Command::new(&tmux).arg("-S").arg(&sock).args(args).output().unwrap()
    };
    // Ends the server and its directory however the test ends.
    struct Server<'a>(&'a dyn Fn(&[&str]) -> std::process::Output, &'a Path);
    impl Drop for Server<'_> {
      fn drop(&mut self) {
        (self.0)(&["kill-server"]);
        let _ = std::fs::remove_dir_all(self.1);
      }
    }
    let _server = Server(&server, &dir);
    let made = server(&[
      "-f",
      "/dev/null",
      "new-session",
      "-d",
      "-s",
      "s-c-1",
      "-x",
      "80",
      "-y",
      "24",
      "-e",
      "CIRA_SESSION=c-real",
      "/bin/sh",
    ]);
    assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
    let argv = serde_json::json!([
      tmux.to_str().unwrap(),
      "-u",
      "-S",
      sock.to_str().unwrap(),
      "attach-session",
      "-t",
      "=s-c-1"
    ]);
    let answer = serde_json::json!({
      "project": "p", "session": "c-1", "name": "s-c-1", "socket": sock, "argv": argv,
      "created": false
    });
    let fake = dir.join("cira");
    std::fs::write(&fake, format!("#!/bin/sh\necho '{answer}'\n")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

    let cira = Cira::new(&fake, Duration::from_secs(10));
    let at = At { project: Some("p".into()), dir: dir.clone() };
    let ShellChild::Shared(argv) = shell_child(cira.session_shell(&at, "c-1")) else {
      panic!("Cira named a shell")
    };
    // JC's own environment says otherwise: what answers is the shared shell.
    let child = PtyChild::spawn(
      &ChildSpec::Command(argv),
      &[("CIRA_SESSION".into(), "c-jc".into())],
      80,
      24,
      None,
    )
    .unwrap();
    child.handle.write_all(b"echo X=$CIRA_SESSION=X\r").unwrap();
    read_until(&child, "X=c-real=X");
    // Jay's `exit` ends the shell and so the client: the pane asks again.
    child.handle.write_all(b"exit\r").unwrap();
    let exited = child.exited.recv_timeout(Duration::from_secs(10));
    assert!(exited.is_ok(), "the attach client exits with the shell");
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
  fn a_reaped_child_is_not_signalled_on_drop() {
    // Its pid may belong to another process by then.
    let child = PtyChild::spawn(&sh("exit 0"), &[], 80, 24, None).unwrap();
    child.exited.recv_timeout(Duration::from_secs(10)).expect("exit reported");
    assert!(*child.handle.closed.lock(), "drop must see the child as gone");
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
