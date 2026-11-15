//! jcc's Scratch panel (Cira PLAN TMUX-11): the Cira project's scratch pad,
//! read and saved through Cira's daemon, which alone writes its store (Cira
//! DESIGN 4: `projects/<project>/scratch.md`). The daemon's API is HTTP/1.1
//! with JSON bodies on the Unix socket `<home>/cira.sock` (Cira's
//! `cira_proto`); the pad is `GET /v1/scratch?project=` and `POST /v1/scratch`
//! `{project, text, base}`, where `base` is the `version` (the SHA-256 of the
//! text) the edit started from: a save on an older copy is refused
//! (`conflict`), so a stale window never overwrites newer notes. The `cira`
//! CLI has no command for it, so jcc speaks to the socket itself, one request
//! per connection. A save by any client is told on the daemon's change feed
//! (`GET /v1/events`, a `scratch` event; Cira PLAN TMUX-15), which [`feed`]
//! follows so a note typed on Cira's page shows here at once.
//!
//! [`Pad`] holds every decision, so it is tested without a window; the panel
//! only applies its answers. The rules follow the page's pad (Cira DESIGN 9)
//! and JC's own safe save ([`crate::shared_file`]):
//!
//! - Until a read has succeeded it takes no typing: a save then could only
//!   overwrite notes it never saw.
//! - A read while there are unsaved edits merges them with the newer copy
//!   (three-way, from the copy they started from); when they cannot merge,
//!   nothing is saved until Jay takes the saved copy or keeps his.
//! - A save refused as made on an older copy reads again, which merges.

use serde::Deserialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The daemon's socket under the store's home (Cira's `cira_proto::SOCKET`).
pub fn socket(home: &Path) -> PathBuf {
  home.join("cira.sock")
}

/// The protocol version jcc speaks (Cira's `cira_proto::PROTOCOL`).
const PROTOCOL: &str = "2";
/// How long each read or write of a request's socket may wait.
const TIMEOUT: Duration = Duration::from_secs(5);

/// A project's pad as the daemon keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Scratch {
  pub project: String,
  pub text: String,
  /// Names this text: the SHA-256 of its bytes, in hex.
  pub version: String,
}

/// Why a request got no pad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Failure {
  /// Nothing listens on the socket: the daemon is not running.
  NoDaemon(String),
  /// The daemon refused it; `code` is Cira's error code (`conflict`,
  /// `read_only`, `not_found`, …).
  Refused { code: String, message: String },
  /// Anything else: no answer, or not one a Cira daemon gives.
  Failed(String),
}

impl std::fmt::Display for Failure {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match self {
      Self::NoDaemon(why) => write!(f, "Cira's daemon is not running ({why})"),
      Self::Refused { message, .. } => f.write_str(message),
      Self::Failed(why) => f.write_str(why),
    }
  }
}

/// `GET /v1/scratch`: `project`'s pad.
pub fn read(socket: &Path, project: &str) -> Result<Scratch, Failure> {
  let path = format!("/v1/scratch?project={}", query_value(project));
  parse(&request(socket, "GET", &path, None)?)
}

/// `POST /v1/scratch`: replace `project`'s pad with `text`, if `base` is the
/// version kept.
pub fn write(socket: &Path, project: &str, text: &str, base: &str) -> Result<Scratch, Failure> {
  let body = serde_json::json!({ "project": project, "text": text, "base": base });
  parse(&request(socket, "POST", "/v1/scratch", Some(body.to_string().as_bytes()))?)
}

fn parse(body: &[u8]) -> Result<Scratch, Failure> {
  serde_json::from_slice(body).map_err(|e| Failure::Failed(format!("not a scratch pad: {e}")))
}

/// A query value, percent-encoded but for the unreserved characters.
fn query_value(value: &str) -> String {
  value
    .bytes()
    .map(|b| match b {
      b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
        (b as char).to_string()
      }
      _ => format!("%{b:02X}"),
    })
    .collect()
}

fn connect(socket: &Path) -> Result<UnixStream, Failure> {
  UnixStream::connect(socket).map_err(|e| match e.kind() {
    std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
      Failure::NoDaemon(format!("{}: {e}", socket.display()))
    }
    _ => Failure::Failed(format!("cannot connect to {}: {e}", socket.display())),
  })
}

/// One request on a connection of its own; the body of a success.
fn request(
  socket: &Path,
  method: &str,
  path: &str,
  body: Option<&[u8]>,
) -> Result<Vec<u8>, Failure> {
  let mut stream = connect(socket)?;
  let failed = |e: std::io::Error| Failure::Failed(format!("no answer from Cira: {e}"));
  stream.set_read_timeout(Some(TIMEOUT)).map_err(failed)?;
  stream.set_write_timeout(Some(TIMEOUT)).map_err(failed)?;
  let body = body.unwrap_or_default();
  let mut head = format!(
    "{method} {path} HTTP/1.1\r\nHost: cira\r\ncira-protocol: {PROTOCOL}\r\nConnection: close\r\n\
     Content-Length: {}\r\n",
    body.len()
  );
  if !body.is_empty() {
    head.push_str("Content-Type: application/json\r\n");
  }
  head.push_str("\r\n");
  stream.write_all(head.as_bytes()).and_then(|()| stream.write_all(body)).map_err(failed)?;
  let mut answer = Vec::new();
  stream.read_to_end(&mut answer).map_err(failed)?;
  let (status, body) = response(&answer)?;
  if (200..300).contains(&status) {
    return Ok(body);
  }
  #[derive(Deserialize)]
  struct ErrorBody {
    code: String,
    message: String,
  }
  match serde_json::from_slice::<ErrorBody>(&body) {
    Ok(e) => Err(Failure::Refused { code: e.code, message: e.message }),
    Err(_) => Err(Failure::Failed(format!(
      "Cira answered {status} with {:?}",
      String::from_utf8_lossy(&body)
    ))),
  }
}

/// A whole HTTP/1.1 response read to its end: its status and body (a chunked
/// one put together). Only a Cira daemon's (it names its build) is taken.
fn response(raw: &[u8]) -> Result<(u16, Vec<u8>), Failure> {
  let bad = |why: &str| Failure::Failed(format!("not an answer from Cira: {why}"));
  let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or_else(|| bad("no end of head"))?;
  let head = std::str::from_utf8(&raw[..end]).map_err(|_| bad("head is not text"))?;
  let mut lines = head.split("\r\n");
  let status = lines
    .next()
    .and_then(|l| l.split(' ').nth(1))
    .and_then(|s| s.parse().ok())
    .ok_or_else(|| bad("no status"))?;
  let mut chunked = false;
  let mut cira = false;
  for line in lines {
    let Some((name, value)) = line.split_once(':') else { continue };
    let name = name.trim().to_ascii_lowercase();
    chunked |= name == "transfer-encoding" && value.trim().eq_ignore_ascii_case("chunked");
    cira |= name == "cira-build";
  }
  if !cira {
    return Err(bad("no cira-build header"));
  }
  let body = &raw[end + 4..];
  if !chunked {
    return Ok((status, body.to_vec()));
  }
  let mut out = Vec::new();
  let mut rest = body;
  loop {
    let eol = rest.windows(2).position(|w| w == b"\r\n").ok_or_else(|| bad("bad chunk"))?;
    let size = std::str::from_utf8(&rest[..eol])
      .ok()
      .and_then(|s| usize::from_str_radix(s.split(';').next()?.trim(), 16).ok())
      .ok_or_else(|| bad("bad chunk size"))?;
    rest = &rest[eol + 2..];
    if size == 0 {
      return Ok((status, out));
    }
    let chunk = rest.get(..size).ok_or_else(|| bad("short chunk"))?;
    out.extend_from_slice(chunk);
    rest = rest.get(size + 2..).ok_or_else(|| bad("short chunk"))?;
  }
}

/// What the daemon's change feed (`GET /v1/events`, server-sent events)
/// says of the pads (Cira PLAN TMUX-15).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notice {
  /// `project`'s pad was saved as `version`, by any client (a `scratch`
  /// event): a panel holding another version reads it again.
  Saved { project: String, version: String },
  /// The feed started over (`resync`, the first event of every connection,
  /// and whenever the daemon could not replay): a save may have gone
  /// untold, so every pad is read again.
  Missed,
}

/// How long the feed may say nothing before it counts as dead: the daemon
/// sends a keep-alive every 15 s.
const FEED_SILENCE: Duration = Duration::from_secs(60);
/// The pause before following the feed again after it ended or failed.
const FEED_RETRY: Duration = Duration::from_secs(2);

/// Follow the feed on a thread of its own, from now on, reconnecting after
/// [`FEED_RETRY`] whenever it ends (the daemon restarted or is down). The
/// thread ends at the first notice, or reconnect, after the receiver is
/// dropped.
pub fn feed(socket: PathBuf) -> flume::Receiver<Notice> {
  let (tx, rx) = flume::unbounded();
  std::thread::Builder::new()
    .name("jcc-scratch-feed".into())
    .spawn(move || {
      while !tx.is_disconnected() {
        follow(&socket, |n| tx.send(n).is_ok());
        std::thread::sleep(FEED_RETRY);
      }
    })
    .expect("spawning the scratch feed thread");
  rx
}

/// Follow the feed on `socket`, giving `on` each notice, until the stream
/// ends or fails, or `on` returns false; why it ended. Blocks.
pub fn follow(socket: &Path, mut on: impl FnMut(Notice) -> bool) -> Failure {
  let mut stream = match connect(socket) {
    Ok(s) => s,
    Err(e) => return e,
  };
  let failed = |e: std::io::Error| Failure::Failed(format!("the feed broke: {e}"));
  let head = format!(
    "GET /v1/events HTTP/1.1\r\nHost: cira\r\ncira-protocol: {PROTOCOL}\r\naccept: text/event-stream\r\n\r\n"
  );
  if let Err(e) = stream
    .set_read_timeout(Some(FEED_SILENCE))
    .and_then(|()| stream.set_write_timeout(Some(TIMEOUT)))
    .and_then(|()| stream.write_all(head.as_bytes()))
  {
    return failed(e);
  }
  let mut reader = std::io::BufReader::new(stream);
  if let Err(e) = feed_head(&mut reader) {
    return e;
  }
  let mut events = Events::default();
  loop {
    let bytes = match next_chunk(&mut reader) {
      Ok(Some(b)) => b,
      Ok(None) => return Failure::Failed("the feed ended".into()),
      Err(e) => return failed(e),
    };
    for (name, data) in events.push(&bytes) {
      let notice = match name.as_str() {
        "resync" => Notice::Missed,
        "scratch" => match serde_json::from_str::<Changed>(&data) {
          Ok(Changed { project, version }) => Notice::Saved { project, version },
          Err(e) => return Failure::Failed(format!("not a scratch notice: {e}")),
        },
        _ => continue,
      };
      if !on(notice) {
        return Failure::Failed("no longer followed".into());
      }
    }
  }
}

/// A `scratch` event's data (Cira's `cira_proto::ScratchChanged`).
#[derive(Deserialize)]
struct Changed {
  project: String,
  version: String,
}

/// The feed answer's head, read through. Only a Cira daemon's 200 is taken,
/// and its stream comes chunked (it has no length).
fn feed_head(reader: &mut impl std::io::BufRead) -> Result<(), Failure> {
  let bad = |why: String| Failure::Failed(format!("not Cira's feed: {why}"));
  let (mut status, mut chunked, mut cira) = (None, false, false);
  loop {
    let mut line = String::new();
    match reader.read_line(&mut line) {
      Ok(0) => return Err(bad("no end of head".into())),
      Ok(_) => {}
      Err(e) => return Err(bad(e.to_string())),
    }
    let line = line.trim_end();
    if line.is_empty() {
      break;
    }
    if status.is_none() {
      status = Some(line.split(' ').nth(1).and_then(|s| s.parse::<u16>().ok()).unwrap_or(0));
      continue;
    }
    let Some((name, value)) = line.split_once(':') else { continue };
    let name = name.trim().to_ascii_lowercase();
    chunked |= name == "transfer-encoding" && value.trim().eq_ignore_ascii_case("chunked");
    cira |= name == "cira-build";
  }
  match status {
    _ if !cira => Err(bad("no cira-build header".into())),
    Some(200) if chunked => Ok(()),
    Some(200) => Err(bad("not chunked".into())),
    other => Err(bad(format!("status {}", other.unwrap_or(0)))),
  }
}

/// The next chunk of a chunked body; `None` at its end.
fn next_chunk(reader: &mut impl std::io::BufRead) -> std::io::Result<Option<Vec<u8>>> {
  let mut line = String::new();
  if reader.read_line(&mut line)? == 0 {
    return Ok(None);
  }
  let size = usize::from_str_radix(line.trim().split(';').next().unwrap_or_default(), 16)
    .map_err(|_| std::io::Error::other(format!("bad chunk size {line:?}")))?;
  if size == 0 {
    return Ok(None);
  }
  let mut chunk = vec![0; size + 2]; // and its CRLF
  reader.read_exact(&mut chunk)?;
  chunk.truncate(size);
  Ok(Some(chunk))
}

/// Server-sent events put together from the body's bytes as they come: each
/// event's name and data, once the blank line ending it has come.
#[derive(Default)]
struct Events {
  partial: Vec<u8>,
  name: String,
  data: String,
}

impl Events {
  fn push(&mut self, bytes: &[u8]) -> Vec<(String, String)> {
    self.partial.extend_from_slice(bytes);
    let mut out = Vec::new();
    while let Some(eol) = self.partial.iter().position(|&b| b == b'\n') {
      let line: Vec<u8> = self.partial.drain(..=eol).collect();
      let line = String::from_utf8_lossy(&line);
      let line = line.trim_end_matches(['\n', '\r']);
      if line.is_empty() {
        if !self.name.is_empty() || !self.data.is_empty() {
          out.push((std::mem::take(&mut self.name), std::mem::take(&mut self.data)));
        }
      } else if let Some(v) = line.strip_prefix("event:") {
        self.name = v.trim_start().to_string();
      } else if let Some(v) = line.strip_prefix("data:") {
        if !self.data.is_empty() {
          self.data.push('\n');
        }
        self.data.push_str(v.strip_prefix(' ').unwrap_or(v));
      }
      // `id:` and `:` comments (keep-alives) are not the pad's.
    }
    out
  }
}

/// The panel's pad: what the daemon last agreed it keeps, and what is owed.
/// One request is in flight at a time, as on Cira's page: a read asked while
/// a save is out waits for its answer, so no answer is applied out of order.
#[derive(Debug, Default)]
pub struct Pad {
  /// The copy the panel's text was edited from, as last read or saved.
  kept: Option<Scratch>,
  /// A newer copy the unsaved edits could not merge with; nothing is saved
  /// until [`Pad::take_theirs`] or [`Pad::keep_mine`].
  theirs: Option<Scratch>,
  /// Why the last read or save failed; `None` once one works.
  failure: Option<String>,
  /// A request is in flight.
  busy: bool,
  /// A read was asked for while one was in flight: sent at its answer.
  read_owed: bool,
  /// Which pad this is ([`Pad::retarget`]): an answer about an earlier one
  /// is dropped.
  epoch: u64,
}

/// Names the request an answer is for ([`Pad::ask_read`], [`Pad::to_save`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket(u64);

/// What the panel does after a save's answer ([`Pad::saved`]).
#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
  /// The daemon keeps it: send what is owed next ([`Pad::read_owed`], else
  /// a save of what was typed meanwhile).
  Kept,
  /// Made on an older copy: a read is owed, which merges.
  Conflict,
  /// Not saved; the banner says why. Nothing is tried until the next edit,
  /// focus or Cmd-S, so a refusal does not repeat in a loop.
  Failed,
  /// About a pad this panel no longer shows: ignored.
  Stale,
}

impl Pad {
  /// A read has succeeded: the panel takes typing.
  pub fn ready(&self) -> bool {
    self.kept.is_some()
  }

  /// A notice that the pad was saved as `version` ([`Notice::Saved`]) is
  /// news to this panel: it holds another, or none yet. Its own save, told
  /// back, is not.
  pub fn stale(&self, version: &str) -> bool {
    self.kept.as_ref().is_none_or(|k| k.version != version)
  }

  /// `ours` holds edits the daemon does not keep.
  pub fn dirty(&self, ours: &str) -> bool {
    self.kept.as_ref().is_some_and(|k| k.text != ours)
  }

  /// Start over on another project's pad. Answers about this one are
  /// dropped from now on.
  pub fn retarget(&mut self) {
    *self = Self { epoch: self.epoch + 1, ..Self::default() };
  }

  /// A read to send now, or `None` while a request is in flight: it is then
  /// owed, and sent at that request's answer.
  pub fn ask_read(&mut self) -> Option<Ticket> {
    if self.busy {
      self.read_owed = true;
      return None;
    }
    self.read_owed = false;
    self.busy = true;
    Some(Ticket(self.epoch))
  }

  /// A read is owed ([`Pad::ask_read`]).
  pub fn read_owed(&self) -> bool {
    self.read_owed
  }

  /// A read answered while the panel holds `ours`: the text to show instead,
  /// if any (the newer copy, or it merged with the unsaved edits).
  pub fn read(
    &mut self,
    ticket: Ticket,
    ours: &str,
    got: Result<Scratch, Failure>,
  ) -> Option<String> {
    if ticket.0 != self.epoch {
      return None;
    }
    self.busy = false;
    let theirs = match got {
      Ok(theirs) => theirs,
      Err(e) => {
        self.failure = Some(format!("The scratch pad could not be read: {e}"));
        return None;
      }
    };
    self.failure = None;
    let Some(kept) = &self.kept else {
      let text = theirs.text.clone();
      self.kept = Some(theirs);
      return Some(text);
    };
    if theirs.version == kept.version {
      return None;
    }
    if ours == kept.text {
      let text = theirs.text.clone();
      self.kept = Some(theirs);
      self.theirs = None;
      return (text != ours).then_some(text);
    }
    match diffy::merge(&kept.text, ours, &theirs.text) {
      Ok(merged) => {
        self.kept = Some(theirs);
        self.theirs = None;
        Some(merged)
      }
      Err(_) => {
        self.theirs = Some(theirs);
        None
      }
    }
  }

  /// The save owed for `ours` (its text and the version it was edited from),
  /// if any: none before a read, while a request is in flight, while edits
  /// and a newer copy wait to be reconciled, or with nothing unsaved.
  pub fn to_save(&mut self, ours: &str) -> Option<(String, String, Ticket)> {
    if self.busy || self.theirs.is_some() || !self.dirty(ours) {
      return None;
    }
    let base = self.kept.as_ref()?.version.clone();
    self.busy = true;
    Some((ours.to_string(), base, Ticket(self.epoch)))
  }

  /// A save's answer.
  pub fn saved(&mut self, ticket: Ticket, got: Result<Scratch, Failure>) -> Saved {
    if ticket.0 != self.epoch {
      return Saved::Stale;
    }
    self.busy = false;
    match got {
      Ok(kept) => {
        self.kept = Some(kept);
        self.failure = None;
        Saved::Kept
      }
      Err(Failure::Refused { code, .. }) if code == "conflict" => {
        self.read_owed = true;
        Saved::Conflict
      }
      Err(e) => {
        self.failure = Some(format!("The scratch pad is not saved: {e}"));
        Saved::Failed
      }
    }
  }

  /// Drop the unsaved edits for the newer copy: its text, to show.
  pub fn take_theirs(&mut self) -> Option<String> {
    let theirs = self.theirs.take()?;
    let text = theirs.text.clone();
    self.kept = Some(theirs);
    Some(text)
  }

  /// Keep the unsaved edits over the newer copy: the next save replaces it.
  pub fn keep_mine(&mut self) -> bool {
    match self.theirs.take() {
      Some(theirs) => {
        self.kept = Some(theirs);
        true
      }
      None => false,
    }
  }

  /// What the panel's banner says, if anything.
  pub fn banner(&self) -> Option<String> {
    if self.theirs.is_some() {
      return Some(
        "The scratch pad changed elsewhere and your edits conflict with it: nothing is saved \
         until Cmd-R takes the saved pad or Cmd-S keeps yours"
          .into(),
      );
    }
    self.failure.clone()
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::os::unix::net::UnixListener;
  use std::sync::atomic::{AtomicUsize, Ordering};

  fn pad(text: &str, version: &str) -> Scratch {
    Scratch { project: "p".into(), text: text.into(), version: version.into() }
  }

  fn conflict() -> Failure {
    Failure::Refused { code: "conflict".into(), message: "changed".into() }
  }

  /// A read sent and answered at once.
  fn read(p: &mut Pad, ours: &str, got: Result<Scratch, Failure>) -> Option<String> {
    let ticket = p.ask_read().expect("a request is in flight");
    p.read(ticket, ours, got)
  }

  /// The save owed, without its ticket.
  fn save(p: &mut Pad, ours: &str) -> Option<(String, String)> {
    p.to_save(ours).map(|(text, base, _)| (text, base))
  }

  /// The answer to the save in flight.
  fn saved(p: &mut Pad, got: Result<Scratch, Failure>) -> Saved {
    let ticket = Ticket(p.epoch);
    p.saved(ticket, got)
  }

  #[test]
  fn nothing_is_saved_before_a_read() {
    let mut p = Pad::default();
    assert!(!p.ready());
    assert_eq!(save(&mut p, "typed before the read"), None, "a save could overwrite unseen notes");
    assert_eq!(read(&mut p, "", Ok(pad("notes", "v1"))), Some("notes".into()));
    assert!(p.ready());
  }

  #[test]
  fn an_edit_is_saved_on_the_version_it_was_made_from() {
    let mut p = Pad::default();
    read(&mut p, "", Ok(pad("a\n", "v1")));
    assert_eq!(save(&mut p, "a\n"), None, "nothing unsaved");
    assert_eq!(save(&mut p, "a\nb\n"), Some(("a\nb\n".into(), "v1".into())));
    assert_eq!(save(&mut p, "a\nb\nc\n"), None, "one save at a time");
    assert_eq!(saved(&mut p, Ok(pad("a\nb\n", "v2"))), Saved::Kept);
    assert_eq!(save(&mut p, "a\nb\nc\n"), Some(("a\nb\nc\n".into(), "v2".into())));
  }

  /// Another window saved since this copy: the save is refused, the read
  /// that follows merges the edits with the newer copy, and that is saved
  /// on its version. Neither side's lines are lost.
  #[test]
  fn a_save_on_an_older_copy_merges_with_the_newer() {
    let mut p = Pad::default();
    read(&mut p, "", Ok(pad("one\ntwo\nthree\n", "v1")));
    let ours = "zero\none\ntwo\nthree\n";
    assert!(save(&mut p, ours).is_some());
    assert_eq!(saved(&mut p, Err(conflict())), Saved::Conflict);
    assert!(p.read_owed(), "a refused save reads again");
    let merged = read(&mut p, ours, Ok(pad("one\ntwo\nthree\nfour\n", "v2"))).expect("merged");
    assert_eq!(merged, "zero\none\ntwo\nthree\nfour\n");
    assert_eq!(save(&mut p, &merged), Some((merged.clone(), "v2".into())));
  }

  /// Review P2: a read asked while a save is out (the window regained focus)
  /// waits for the save's answer, so an older copy read before the save
  /// landed never replaces the text just saved.
  #[test]
  fn a_read_asked_during_a_save_waits_for_its_answer() {
    let mut p = Pad::default();
    read(&mut p, "", Ok(pad("a\n", "v1")));
    assert!(save(&mut p, "a\nb\n").is_some());
    assert_eq!(p.ask_read(), None, "a read went out while a save was in flight");
    assert!(p.read_owed());
    assert_eq!(saved(&mut p, Ok(pad("a\nb\n", "v2"))), Saved::Kept);
    assert_eq!(read(&mut p, "a\nb\n", Ok(pad("a\nb\n", "v2"))), None);
    assert!(!p.read_owed() && !p.dirty("a\nb\n"));
  }

  /// Review P1: the panel moved to another project's pad while a read and a
  /// save of the first were out; their answers are not applied to it.
  #[test]
  fn an_answer_about_the_previous_pad_is_dropped() {
    let mut p = Pad::default();
    read(&mut p, "", Ok(pad("A\n", "a1")));
    let (_, _, old_save) = p.to_save("A edited\n").unwrap();
    p.retarget();
    let b = p.ask_read().expect("the new pad is read at once");
    assert_eq!(p.read(b, "", Ok(pad("B\n", "b1"))), Some("B\n".into()));
    assert_eq!(p.saved(old_save, Ok(pad("A edited\n", "a2"))), Saved::Stale);
    assert!(!p.dirty("B\n"), "the old pad's save moved this one's copy");
    assert_eq!(save(&mut p, "B more\n"), Some(("B more\n".into(), "b1".into())));
  }

  #[test]
  fn edits_that_conflict_save_nothing_until_jay_picks() {
    let mut p = Pad::default();
    read(&mut p, "", Ok(pad("idea\n", "v1")));
    let ours = "my idea\n";
    assert_eq!(read(&mut p, ours, Ok(pad("their idea\n", "v2"))), None, "the text stays his");
    assert!(p.banner().is_some_and(|b| b.contains("Cmd-R")));
    assert_eq!(save(&mut p, ours), None, "a conflict saves nothing");
    assert!(p.keep_mine());
    assert_eq!(save(&mut p, ours), Some((ours.into(), "v2".into())), "kept: saved over theirs");
    assert_eq!(p.banner(), None);

    let mut p = Pad::default();
    read(&mut p, "", Ok(pad("idea\n", "v1")));
    read(&mut p, ours, Ok(pad("their idea\n", "v2")));
    assert_eq!(p.take_theirs(), Some("their idea\n".into()));
    assert!(!p.dirty("their idea\n"));
  }

  #[test]
  fn an_unedited_pad_follows_the_newer_copy() {
    let mut p = Pad::default();
    read(&mut p, "", Ok(pad("a", "v1")));
    assert_eq!(read(&mut p, "a", Ok(pad("a", "v1"))), None);
    assert_eq!(read(&mut p, "a", Ok(pad("b", "v2"))), Some("b".into()));
    assert!(!p.dirty("b"));
  }

  #[test]
  fn a_failure_shows_until_one_works() {
    let mut p = Pad::default();
    read(&mut p, "", Ok(pad("a", "v1")));
    assert!(save(&mut p, "ab").is_some());
    let ro = Failure::Refused { code: "read_only".into(), message: "p is read-only".into() };
    assert_eq!(saved(&mut p, Err(ro)), Saved::Failed);
    assert!(p.banner().is_some_and(|b| b.contains("read-only")), "{:?}", p.banner());
    assert!(save(&mut p, "ab").is_some(), "tried again at the next edit or focus");
    saved(&mut p, Ok(pad("ab", "v2")));
    assert_eq!(p.banner(), None);
  }

  /// A one-shot server on a fresh socket: answers `reply` and hands back the
  /// request it got.
  fn serve(reply: &'static [u8]) -> (PathBuf, std::thread::JoinHandle<String>) {
    static N: AtomicUsize = AtomicUsize::new(0);
    let path = PathBuf::from(format!(
      "/tmp/jcc-scratch-{}-{}.sock",
      std::process::id(),
      N.fetch_add(1, Ordering::SeqCst)
    ));
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).unwrap();
    let handle = std::thread::spawn(move || {
      let (mut s, _) = listener.accept().unwrap();
      let mut got = Vec::new();
      let mut buf = [0u8; 4096];
      // Read the head, then the body its Content-Length names.
      loop {
        let n = s.read(&mut buf).unwrap();
        got.extend_from_slice(&buf[..n]);
        let text = String::from_utf8_lossy(&got).to_string();
        if let Some(end) = text.find("\r\n\r\n") {
          let len: usize = text
            .lines()
            .find_map(|l| l.strip_prefix("Content-Length: "))
            .map_or(0, |v| v.trim().parse().unwrap());
          if got.len() >= end + 4 + len {
            break;
          }
        }
        if n == 0 {
          break;
        }
      }
      s.write_all(reply).unwrap();
      String::from_utf8(got).unwrap()
    });
    (path, handle)
  }

  #[test]
  fn reads_and_writes_speak_cira_protocol_on_the_socket() {
    let (sock, got) = serve(
      b"HTTP/1.1 200 OK\r\ncira-build: 0.1.0+x\r\ncontent-type: application/json\r\n\
        transfer-encoding: chunked\r\n\r\nf\r\n{\"project\":\"my \r\n1f\r\nproj\",\"text\":\"n\",\"version\":\"v\"}\r\n0\r\n\r\n",
    );
    let scratch = super::read(&sock, "my proj").unwrap();
    assert_eq!(
      scratch,
      Scratch { project: "my proj".into(), text: "n".into(), version: "v".into() }
    );
    let req = got.join().unwrap();
    assert!(req.starts_with("GET /v1/scratch?project=my%20proj HTTP/1.1\r\n"), "{req}");
    assert!(req.contains("cira-protocol: 2\r\n"), "{req}");

    let (sock, got) = serve(
      b"HTTP/1.1 409 Conflict\r\ncira-build: 0.1.0+x\r\ncontent-length: 57\r\n\r\n\
        {\"code\":\"conflict\",\"message\":\"changed since\",\"at\":\"base\"}",
    );
    let refused = super::write(&sock, "p", "t\n", "v1");
    assert_eq!(
      refused,
      Err(Failure::Refused { code: "conflict".into(), message: "changed since".into() })
    );
    let req = got.join().unwrap();
    let body = req.split("\r\n\r\n").nth(1).unwrap();
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body, serde_json::json!({ "project": "p", "text": "t\n", "base": "v1" }));
    assert!(req.starts_with("POST /v1/scratch HTTP/1.1\r\n"), "{req}");
  }

  /// PLAN TMUX-15: the change feed tells a save made elsewhere (the page's)
  /// as a `scratch` event, and starts with `resync`, after which every pad
  /// is read again; other events are not the pad's. An event may be split
  /// across chunks.
  #[test]
  fn the_feed_tells_saves_and_resyncs_and_nothing_else() {
    let (sock, got) = serve(
      b"HTTP/1.1 200 OK\r\ncira-build: 0.1.0+x\r\ncontent-type: text/event-stream\r\n\
        transfer-encoding: chunked\r\n\r\n\
        32\r\nevent: resync\nid: b:3\ndata: {\"boot\":\"b\",\"seq\":3}\n\n\r\n\
        20\r\nevent: change\nid: b:4\ndata: {}\n\n\r\n\
        3\r\n:\n\n\r\n\
        22\r\nevent: scratch\nid: b:4\ndata: {\"pro\r\n\
        1b\r\nject\":\"p\",\"version\":\"v2\"}\n\n\r\n0\r\n\r\n",
    );
    let mut told = Vec::new();
    let ended = super::follow(&sock, |n| {
      told.push(n);
      true
    });
    assert_eq!(ended, Failure::Failed("the feed ended".into()));
    assert_eq!(
      told,
      [Notice::Missed, Notice::Saved { project: "p".into(), version: "v2".into() }],
      "the feed's notices were not read as the pad's"
    );
    let req = got.join().unwrap();
    assert!(req.starts_with("GET /v1/events HTTP/1.1\r\n"), "{req}");
    assert!(req.contains("cira-protocol: 2\r\n"), "{req}");
  }

  /// A notice of the version the pad holds (its own save, told back) reads
  /// nothing; any other reads it again.
  #[test]
  fn only_a_version_the_pad_does_not_hold_is_stale() {
    let mut p = Pad::default();
    assert!(p.stale("v1"), "a pad never read takes any notice");
    read(&mut p, "", Ok(pad("a", "v1")));
    assert!(!p.stale("v1"), "its own save read it again");
    assert!(p.stale("v2"));
  }

  #[test]
  fn no_daemon_and_a_stranger_are_told_apart() {
    let missing = PathBuf::from("/tmp/jcc-scratch-none.sock");
    assert!(matches!(super::read(&missing, "p"), Err(Failure::NoDaemon(_))));
    let (sock, got) = serve(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\n{}");
    assert!(
      matches!(super::read(&sock, "p"), Err(Failure::Failed(why)) if why.contains("cira-build"))
    );
    got.join().unwrap();
  }
}
