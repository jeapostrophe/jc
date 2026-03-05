use crate::protocol::{ClientMessage, MobileStateSnapshot, ServerMessage};
use crate::tls;
use anyhow::Result;
use rand::Rng;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use tungstenite::WebSocket;
use tungstenite::protocol::Message;

/// Maximum simultaneous mobile client connections.
const MAX_CLIENTS: usize = 8;

/// In-process WebSocket server for the mobile companion app.
///
/// Architecture follows the same pattern as `HookServer` in `jc-core/src/hooks.rs`:
/// spawns a background thread that accepts connections, with clean shutdown
/// via a flag.
pub struct MobileServer {
  pub port: u16,
  pub token: String,
  pub fingerprint: String,
  state: Arc<Mutex<MobileStateSnapshot>>,
  shutdown: Arc<AtomicBool>,
  _listener_thread: thread::JoinHandle<()>,
}

impl MobileServer {
  pub fn start(port: u16) -> Result<Self> {
    // Ensure a crypto provider is installed. gpui pulls in rustls with both
    // aws-lc-rs and ring, so neither is auto-selected as the default.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let (tls_config, fingerprint) = tls::generate_self_signed()?;
    let tls_config = Arc::new(tls_config);

    let token = generate_token();
    let state = Arc::new(Mutex::new(MobileStateSnapshot::default()));
    let shutdown = Arc::new(AtomicBool::new(false));
    let client_count = Arc::new(AtomicUsize::new(0));

    let listener = TcpListener::bind(format!("0.0.0.0:{port}"))
      .map_err(|e| anyhow::anyhow!("failed to bind mobile server on port {port}: {e}"))?;

    // Use the actual bound port (in case port was 0).
    let actual_port = listener.local_addr()?.port();

    let accept_token = token.clone();
    let accept_state = state.clone();
    let accept_shutdown = shutdown.clone();
    let accept_tls = tls_config;
    let accept_count = client_count.clone();

    let listener_thread = thread::spawn(move || {
      accept_loop(listener, accept_tls, &accept_token, accept_state, accept_shutdown, accept_count);
    });

    eprintln!("mobile server listening on port {actual_port}");

    Ok(Self {
      port: actual_port,
      token,
      fingerprint,
      state,
      shutdown,
      _listener_thread: listener_thread,
    })
  }

  /// Push a new state snapshot to all connected clients.
  /// The snapshot is stored and will be sent to newly connecting clients too.
  pub fn push_state(&self, snapshot: MobileStateSnapshot) {
    *self.state.lock().unwrap() = snapshot;
    // Connected client threads will pick up the new state on their next poll cycle.
  }

  pub fn shutdown(&self) {
    self.shutdown.store(true, Ordering::Relaxed);
    // Connect to ourselves to unblock the accept call.
    let _ = TcpStream::connect(format!("127.0.0.1:{}", self.port));
  }
}

fn accept_loop(
  listener: TcpListener,
  tls_config: Arc<rustls::ServerConfig>,
  token: &str,
  state: Arc<Mutex<MobileStateSnapshot>>,
  shutdown: Arc<AtomicBool>,
  client_count: Arc<AtomicUsize>,
) {
  for stream in listener.incoming() {
    if shutdown.load(Ordering::Relaxed) {
      break;
    }
    let stream = match stream {
      Ok(s) => s,
      Err(e) => {
        if !shutdown.load(Ordering::Relaxed) {
          eprintln!("mobile server accept error: {e}");
        }
        continue;
      }
    };

    if client_count.load(Ordering::Relaxed) >= MAX_CLIENTS {
      eprintln!("mobile server: rejecting connection (max {MAX_CLIENTS} clients)");
      drop(stream);
      continue;
    }

    let tls_config = tls_config.clone();
    let client_token = token.to_string();
    let client_state = state.clone();
    let client_shutdown = shutdown.clone();
    let client_counter = client_count.clone();

    client_count.fetch_add(1, Ordering::Relaxed);
    thread::spawn(move || {
      if let Err(e) =
        handle_client(stream, tls_config, &client_token, client_state, client_shutdown)
      {
        eprintln!("mobile client error: {e}");
      }
      client_counter.fetch_sub(1, Ordering::Relaxed);
    });
  }
}

fn handle_client(
  stream: TcpStream,
  tls_config: Arc<rustls::ServerConfig>,
  token: &str,
  state: Arc<Mutex<MobileStateSnapshot>>,
  shutdown: Arc<AtomicBool>,
) -> Result<()> {
  // Set a read timeout so the broadcast loop can check for shutdown.
  stream.set_read_timeout(Some(std::time::Duration::from_millis(500)))?;

  // Wrap in TLS.
  let tls_conn = rustls::ServerConnection::new(tls_config)?;
  let tls_stream = rustls::StreamOwned::new(tls_conn, stream);

  // Upgrade to WebSocket.
  let mut ws = tungstenite::accept(tls_stream)?;

  // Auth handshake: send challenge, expect correct token back.
  let challenge = ServerMessage::AuthChallenge { token: token.to_string() };
  send_message(&mut ws, &challenge)?;

  // Wait for auth response.
  let auth_msg = ws.read()?;
  let authenticated = match auth_msg {
    Message::Text(text) => {
      if let Ok(ClientMessage::Auth { token: client_token }) = serde_json::from_str(&text) {
        client_token == token
      } else {
        false
      }
    }
    _ => false,
  };

  let result = ServerMessage::AuthResult { success: authenticated };
  send_message(&mut ws, &result)?;

  if !authenticated {
    let _ = ws.close(None);
    return Ok(());
  }

  // Send initial state snapshot.
  let mut last_snapshot = state.lock().unwrap().clone();
  send_message(&mut ws, &ServerMessage::StateSnapshot(last_snapshot.clone()))?;

  // Broadcast loop: check for state changes after each read timeout.
  while !shutdown.load(Ordering::Relaxed) {
    // Read with timeout — handles pings and detects disconnects.
    match ws.read() {
      Ok(Message::Close(_)) => break,
      Ok(Message::Ping(data)) => {
        let _ = ws.send(Message::Pong(data));
      }
      Err(tungstenite::Error::Io(ref e))
        if e.kind() == std::io::ErrorKind::WouldBlock
          || e.kind() == std::io::ErrorKind::TimedOut =>
      {
        // Timeout — expected, just continue to check for new state.
      }
      Err(_) => break, // Real error or disconnect
      _ => {}
    }

    let current = state.lock().unwrap().clone();
    if current != last_snapshot {
      if send_message(&mut ws, &ServerMessage::StateSnapshot(current.clone())).is_err() {
        break;
      }
      last_snapshot = current;
    }
  }

  let _ = ws.close(None);
  Ok(())
}

fn send_message<S: Read + Write>(ws: &mut WebSocket<S>, msg: &ServerMessage) -> Result<()> {
  let json = serde_json::to_string(msg)?;
  ws.send(Message::Text(json.into()))?;
  ws.flush()?;
  Ok(())
}

fn generate_token() -> String {
  let mut rng = rand::thread_rng();
  let bytes: Vec<u8> = (0..32).map(|_| rng.r#gen::<u8>()).collect();
  hex::encode(bytes)
}
