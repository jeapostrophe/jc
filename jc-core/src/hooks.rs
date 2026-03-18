use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

pub const HOOK_PATH_PREFIX: &str = "/jc-hook/";

#[derive(Debug, Clone)]
pub struct HookEvent {
  pub session_id: String,
  pub project_path: Option<PathBuf>,
  pub kind: HookEventKind,
}

#[derive(Debug, Clone)]
pub enum HookEventKind {
  Stop,
  IdlePrompt,
  PermissionPrompt,
  /// A `/clear` was detected: the old session ended and a new one started.
  SessionClear { old_session_id: String, new_session_id: String },
}

/// Lightweight HTTP server that receives Claude Code hook POSTs.
///
/// Binds to an OS-assigned port on localhost. Connection failures from
/// Claude Code are non-blocking, so this is safe when jc isn't running.
pub struct HookServer {
  pub port: u16,
  pub rx: flume::Receiver<HookEvent>,
  server: std::sync::Arc<tiny_http::Server>,
}

#[derive(Default, serde::Deserialize)]
struct HookPayload {
  session_id: Option<String>,
  cwd: Option<String>,
  notification_type: Option<String>,
  /// SessionEnd reason: "clear", "logout", "prompt_input_exit", etc.
  reason: Option<String>,
  /// SessionStart source: "clear", "startup", "resume", "compact".
  source: Option<String>,
}

/// Stashed SessionEnd(reason=clear) waiting for matching SessionStart.
struct ClearStash {
  old_session_id: String,
  project_path: Option<PathBuf>,
  timestamp: Instant,
}

impl HookServer {
  pub fn start(project_paths: Vec<PathBuf>) -> anyhow::Result<Self> {
    let server = tiny_http::Server::http("127.0.0.1:0")
      .map_err(|e| anyhow::anyhow!("failed to bind hook server: {e}"))?;
    let port = match server.server_addr() {
      tiny_http::ListenAddr::IP(addr) => addr.port(),
      _ => anyhow::bail!("unexpected server address type"),
    };

    let server = std::sync::Arc::new(server);
    let (tx, rx) = flume::unbounded();

    let listener = server.clone();
    std::thread::spawn(move || {
      accept_loop(&listener, tx, &project_paths);
    });

    eprintln!("hook server listening on port {port}");

    Ok(Self { port, rx, server })
  }

  pub fn shutdown(&self) {
    self.server.unblock();
  }
}

fn accept_loop(
  server: &tiny_http::Server,
  tx: flume::Sender<HookEvent>,
  project_paths: &[PathBuf],
) {
  // Stash for correlating SessionEnd(clear) → SessionStart(clear) pairs.
  // Keyed by project path; entries expire after 10 seconds.
  let clear_stash: Mutex<Vec<ClearStash>> = Mutex::new(Vec::new());

  for mut request in server.incoming_requests() {
    let path = request.url().to_string();

    if !path.starts_with(HOOK_PATH_PREFIX) {
      let _ = request.respond(json_response(404, r#"{"error":"not found"}"#));
      continue;
    }

    let route = &path[HOOK_PATH_PREFIX.len()..];

    // Read body
    let mut body = String::default();
    if request.as_reader().read_to_string(&mut body).is_err() {
      let _ = request.respond(json_response(200, "{}"));
      continue;
    }

    let payload: HookPayload = serde_json::from_str(&body).unwrap_or_default();

    let session_id = payload.session_id.unwrap_or_default();

    // Match project from cwd
    let project_path = payload.cwd.as_deref().and_then(|cwd| {
      let cwd = Path::new(cwd);
      project_paths.iter().find(|p| cwd.starts_with(p)).cloned()
    });

    let kind = match route {
      "stop" => Some(HookEventKind::Stop),
      "notification" => parse_notification_kind(&payload.notification_type),
      "permission" => Some(HookEventKind::PermissionPrompt),

      // SessionEnd with reason=clear: stash the old session_id for pairing.
      "session-end" if payload.reason.as_deref() == Some("clear") => {
        let mut stash = clear_stash.lock().unwrap();
        // Expire stale entries.
        stash.retain(|s| s.timestamp.elapsed().as_secs() < 10);
        stash.push(ClearStash {
          old_session_id: session_id.clone(),
          project_path: project_path.clone(),
          timestamp: Instant::now(),
        });
        eprintln!("hook: session-end clear, stashed old_session_id={session_id}");
        None
      }

      // SessionStart with source=clear: pair with stashed SessionEnd.
      "session-start" if payload.source.as_deref() == Some("clear") => {
        let mut stash = clear_stash.lock().unwrap();
        stash.retain(|s| s.timestamp.elapsed().as_secs() < 10);
        // Find a stash entry matching this project path.
        let idx = stash.iter().position(|s| s.project_path == project_path);
        if let Some(idx) = idx {
          let stashed = stash.remove(idx);
          Some(HookEventKind::SessionClear {
            old_session_id: stashed.old_session_id,
            new_session_id: session_id.clone(),
          })
        } else {
          eprintln!(
            "hook: session-start clear with no matching stash, session_id={session_id}"
          );
          None
        }
      }

      // Ignore other session-start/session-end events.
      "session-end" | "session-start" => None,

      _ => None,
    };

    let _ = request.respond(json_response(200, "{}"));

    if let Some(kind) = kind {
      let event = HookEvent { session_id, project_path, kind };
      if tx.send(event).is_err() {
        break; // receiver dropped
      }
    }
  }
}

fn json_response(status: u16, body: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
  let data = body.as_bytes().to_vec();
  tiny_http::Response::from_data(data)
    .with_status_code(status)
    .with_header("Content-Type: application/json".parse::<tiny_http::Header>().unwrap())
}

fn parse_notification_kind(notification_type: &Option<String>) -> Option<HookEventKind> {
  // notification_type values: "idle_prompt", "permission_prompt", "auth_success", "elicitation_dialog"
  match notification_type.as_deref() {
    Some("idle_prompt") => Some(HookEventKind::IdlePrompt),
    Some("permission_prompt") => Some(HookEventKind::PermissionPrompt),
    // Ignore auth_success, elicitation_dialog, etc.
    _ => None,
  }
}
