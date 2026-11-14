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

impl PtyHandle {
  /// Make the child send its size again although it has not changed: one
  /// row less, then, after [`Self::NUDGE`] on a thread of its own, its size
  /// back (unless it was resized meanwhile). A resize to the same size raises
  /// no SIGWINCH, and two at once may reach the child as one.
  pub fn reassert_size(self: &Arc<Self>) -> Result<()> {
    let size = self.master.lock().get_size()?;
    let nudged = PtySize { rows: if size.rows > 1 { size.rows - 1 } else { 2 }, ..size };
    self.master.lock().resize(nudged)?;
    let this = self.clone();
    std::thread::spawn(move || {
      std::thread::sleep(Self::NUDGE);
      let master = this.master.lock();
      if master.get_size().is_ok_and(|now| now == nudged) {
        let _ = master.resize(size);
      }
    });
    Ok(())
  }

  /// How long the child is shown the other size: long enough to be read as
  /// its own SIGWINCH.
  const NUDGE: std::time::Duration = std::time::Duration::from_millis(100);
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
      .expect("the Terminal pane's tests attach to a real tmux: brew install tmux")
  }

  /// Read `child`'s output until it contains `want`, or panic with what it
  /// printed. Returns what it read.
  fn read_until(child: &PtyChild, want: &str) -> String {
    let mut out = Vec::new();
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while !String::from_utf8_lossy(&out).contains(want) {
      let left = deadline.saturating_duration_since(std::time::Instant::now());
      match child.bytes.recv_timeout(left) {
        Ok(chunk) => out.extend(chunk),
        Err(_) => panic!("no {want:?} in: {}", String::from_utf8_lossy(&out)),
      }
    }
    String::from_utf8_lossy(&out).into_owned()
  }

  /// A tmux server set up as Cira's (its `tmux.conf`), on a private socket in
  /// a directory whose name a shell would split and expand; ended on drop.
  struct CiraTmux {
    tmux: std::path::PathBuf,
    dir: std::path::PathBuf,
  }

  impl CiraTmux {
    /// With one shell, `s-<id>`, running `command` with `CIRA_SESSION=<id>`.
    fn new(name: &str, id: &str, command: &str) -> Self {
      let dir = std::env::temp_dir().join(format!("jc {name} $HOME {}", std::process::id()));
      let _ = std::fs::remove_dir_all(&dir);
      std::fs::create_dir_all(&dir).unwrap();
      let t = Self { tmux: tmux(), dir };
      let conf = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/cira-tmux.conf");
      let session = format!("s-{id}");
      let env = format!("CIRA_SESSION={id}");
      let made = t.run(&[
        "-f",
        conf,
        "new-session",
        "-d",
        "-s",
        &session,
        "-x",
        "80",
        "-y",
        "24",
        "-e",
        &env,
        command,
      ]);
      assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
      t
    }

    fn sock(&self) -> String {
      self.dir.join("tmux.sock").to_str().unwrap().to_string()
    }

    fn run(&self, args: &[&str]) -> std::process::Output {
      std::process::Command::new(&self.tmux).arg("-S").arg(self.sock()).args(args).output().unwrap()
    }

    /// A fake `cira` that answers `session shell` for `id` as Cira does: its
    /// answer is the captured one (jc-core's fixture) with this server's paths.
    fn fake_cira(&self, id: &str) -> std::path::PathBuf {
      let raw = include_bytes!("../../jc-core/tests/fixtures/cira/session-shell.json");
      let mut answer: serde_json::Value = serde_json::from_slice(raw).unwrap();
      let argv = answer["argv"].as_array_mut().unwrap();
      argv[0] = self.tmux.to_str().unwrap().into();
      argv[3] = self.sock().into();
      argv[6] = format!("=s-{id}").into();
      let answer_file = self.dir.join("answer.json");
      std::fs::write(&answer_file, answer.to_string()).unwrap();
      let fake = self.dir.join("cira");
      std::fs::write(&fake, format!("#!/bin/sh\nexec cat '{}'\n", answer_file.display())).unwrap();
      use std::os::unix::fs::PermissionsExt;
      std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
      fake
    }

    /// What jc runs for `id`: asked of the fake `cira` through jc's client.
    fn attach_argv(&self, id: &str) -> Vec<String> {
      use jc_core::cira::{At, Cira};
      use jc_core::lifecycle::{ShellChild, shell_child};
      let cira = Cira::new(self.fake_cira(id), Duration::from_secs(10));
      let at = At { project: Some("p".into()), dir: self.dir.clone() };
      match shell_child(cira.session_shell(&at, id), false) {
        ShellChild::Shared(argv) => argv,
        other => panic!("Cira named a shell, jc made {other:?} of it"),
      }
    }
  }

  impl Drop for CiraTmux {
    fn drop(&mut self) {
      self.run(&["kill-server"]);
      let _ = std::fs::remove_dir_all(&self.dir);
    }
  }

  #[test]
  fn the_terminal_pane_attaches_to_the_shell_cira_names() {
    // Cira PLAN TMUX-02. jc runs `argv` as an argument list: the socket's
    // directory has a space and a `$` (a shell would split and expand it),
    // and `=s-…` is a path lookup in zsh.
    let server = CiraTmux::new("shell", "c-1", "/bin/sh");
    let argv = server.attach_argv("c-1");
    // JC's own environment says otherwise: what answers is the shared shell.
    let env = [("CIRA_SESSION".into(), "c-jc".into())];
    let child = PtyChild::spawn(&ChildSpec::Command(argv), &env, 80, 24, None).unwrap();
    child.handle.write_all(b"echo X=$CIRA_SESSION=X\r").unwrap();
    read_until(&child, "X=c-1=X");
    // Jay's `exit` ends the shell and so the client: the pane asks again.
    child.handle.write_all(b"exit\r").unwrap();
    let code = child.exited.recv_timeout(Duration::from_secs(10)).expect("the client exits");
    assert_eq!(code, Some(0), "an exit is not a failure (lifecycle::ShellPane::exited)");
  }

  #[test]
  fn a_drag_in_ciras_tmux_reaches_the_terminal_as_a_clipboard_store() {
    // Cira's tmux.conf turns the mouse on, so a drag selects in tmux, which
    // sends the copy back as OSC 52 (measured, tmux 3.8: `ESC ] 52 ; ; …`).
    let server =
      CiraTmux::new("clip", "c-2", "/bin/sh -c 'printf \"clip-me-text\\n\"; exec sleep 60'");
    let argv = server.attach_argv("c-2");
    let child = PtyChild::spawn(&ChildSpec::Command(argv), &[], 80, 24, None).unwrap();
    let (tx, events) = flume::unbounded();
    let state = crate::terminal::TerminalState::new(80, 24, tx);
    let mut processor = alacritty_terminal::vte::ansi::Processor::<
      alacritty_terminal::vte::ansi::StdSyncHandler,
    >::default();
    let mut feed = |until: std::time::Instant| {
      while let Ok(bytes) =
        child.bytes.recv_timeout(until.saturating_duration_since(std::time::Instant::now()))
      {
        state.with_term_mut(|term| processor.advance(term, &bytes));
      }
    };
    let soon = || std::time::Instant::now() + Duration::from_millis(800);
    feed(soon());
    // Press on column 1 of row 1, drag to column 8, release.
    for event in ["\x1b[<0;1;1M", "\x1b[<32;8;1M", "\x1b[<0;8;1m"] {
      child.handle.write_all(event.as_bytes()).unwrap();
      feed(std::time::Instant::now() + Duration::from_millis(200));
    }
    feed(soon());
    let stored: Vec<String> = events
      .try_iter()
      .filter_map(|e| match e {
        crate::terminal::TerminalEvent::ClipboardStore(text) => Some(text),
        _ => None,
      })
      .collect();
    assert_eq!(stored, ["clip-me-"], "the drag's text, as the clipboard store");
  }

  /// TMUX-08: `claude attach` sends the job its size only when the size it
  /// reads at a SIGWINCH differs from the last it sent (measured: a SIGWINCH
  /// at an unchanged size leaves the job at another client's size), and the
  /// job draws at the last size any client sent. Re-asserting must therefore
  /// show the child a different size, then its own.
  #[test]
  fn reasserting_the_size_shows_the_child_a_change_and_ends_at_its_size() {
    let script =
      r#"trap 'echo "size $(stty size)"' WINCH; echo ready; while :; do sleep 0.05; done"#;
    let child = PtyChild::spawn(&sh(script), &[], 80, 24, None).unwrap();
    read_until(&child, "ready");
    child.handle.reassert_size().unwrap();
    let out = read_until(&child, "size 24 80");
    let nudged = out.find("size 23 80").unwrap_or_else(|| panic!("no other size first: {out}"));
    assert!(nudged < out.find("size 24 80").unwrap(), "{out}");
    let size = child.handle.master.lock().get_size().unwrap();
    assert_eq!((size.cols, size.rows), (80, 24), "it ends at its own size");
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
