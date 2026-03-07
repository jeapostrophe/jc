use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use std::{fs, io, thread};

#[derive(Serialize, Deserialize)]
struct IpcMessage {
  command: String,
  path: String,
}

#[derive(Serialize, Deserialize)]
struct IpcResponse {
  ok: bool,
}

fn socket_path() -> PathBuf {
  let home = std::env::var("HOME").expect("HOME environment variable not set");
  PathBuf::from(home).join(".config/jc/jc.sock")
}

/// Try to send an `open_project` command to an already-running instance.
/// Returns `true` if the message was delivered successfully.
pub fn try_send_to_running(path: &Path) -> bool {
  let sock = socket_path();

  let stream = match UnixStream::connect(&sock) {
    Ok(s) => s,
    Err(_) => {
      // Connection failed — remove stale socket if present.
      let _ = fs::remove_file(&sock);
      return false;
    }
  };

  if stream.set_read_timeout(Some(Duration::from_secs(2))).is_err()
    || stream.set_write_timeout(Some(Duration::from_secs(2))).is_err()
  {
    return false;
  }

  let msg =
    IpcMessage { command: "open_project".into(), path: path.to_string_lossy().into_owned() };

  let mut writer = io::BufWriter::new(&stream);
  let mut line = match serde_json::to_string(&msg) {
    Ok(s) => s,
    Err(_) => return false,
  };
  line.push('\n');

  if writer.write_all(line.as_bytes()).is_err() || writer.flush().is_err() {
    return false;
  }

  // Read response.
  let mut reader = BufReader::new(&stream);
  let mut resp_line = String::default();
  if reader.read_line(&mut resp_line).is_err() {
    return false;
  }

  serde_json::from_str::<IpcResponse>(&resp_line).map(|r| r.ok).unwrap_or(false)
}

/// Remove the socket file if it exists. Safe to call from a signal handler context.
pub fn cleanup_socket() {
  let _ = fs::remove_file(socket_path());
}

pub struct SocketServer {
  shutdown: Arc<AtomicBool>,
  thread: Option<thread::JoinHandle<()>>,
  path: PathBuf,
}

impl SocketServer {
  pub fn bind(callback: impl Fn(PathBuf) + Send + 'static) -> io::Result<SocketServer> {
    let path = socket_path();

    // Remove stale socket if present.
    let _ = fs::remove_file(&path);

    let listener = UnixListener::bind(&path)?;

    // Use a short accept timeout so the thread can check the shutdown flag.
    listener.set_nonblocking(true)?;

    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_flag = Arc::clone(&shutdown);

    let thread = thread::spawn(move || {
      Self::accept_loop(&listener, &shutdown_flag, &callback);
    });

    Ok(SocketServer { shutdown, thread: Some(thread), path })
  }

  fn accept_loop(
    listener: &UnixListener,
    shutdown: &AtomicBool,
    callback: &(impl Fn(PathBuf) + Send + 'static),
  ) {
    while !shutdown.load(Ordering::Relaxed) {
      match listener.accept() {
        Ok((stream, _)) => {
          Self::handle_connection(stream, callback);
        }
        Err(ref e) if e.kind() == io::ErrorKind::WouldBlock => {
          // No pending connection — sleep briefly and retry.
          thread::sleep(Duration::from_millis(100));
        }
        Err(_) => {
          // Unexpected error — brief backoff.
          thread::sleep(Duration::from_millis(250));
        }
      }
    }
  }

  fn handle_connection(stream: UnixStream, callback: &impl Fn(PathBuf)) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));

    let mut reader = BufReader::new(&stream);
    let mut line = String::default();
    if reader.read_line(&mut line).is_err() {
      return;
    }

    let msg: IpcMessage = match serde_json::from_str(&line) {
      Ok(m) => m,
      Err(_) => return,
    };

    if msg.command == "open_project" {
      let path = PathBuf::from(msg.path);
      callback(path);
    }

    let resp = IpcResponse { ok: true };
    if let Ok(mut resp_json) = serde_json::to_string(&resp) {
      resp_json.push('\n');
      let mut writer = io::BufWriter::new(&stream);
      let _ = writer.write_all(resp_json.as_bytes());
      let _ = writer.flush();
    }
  }
}

impl Drop for SocketServer {
  fn drop(&mut self) {
    self.shutdown.store(true, Ordering::Relaxed);

    // Connect briefly to unblock the non-blocking accept loop's sleep cycle.
    // Do this before removing the socket so the connect can reach the listener.
    let _ = UnixStream::connect(&self.path);

    if let Some(handle) = self.thread.take() {
      let _ = handle.join();
    }

    let _ = fs::remove_file(&self.path);
  }
}
