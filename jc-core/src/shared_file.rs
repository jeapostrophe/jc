//! A text file an editor pane holds while other writers change it on disk:
//! jc saves a project's TODO.md routinely (a send, a new heading, a `/clear`),
//! as jcc does its todo.md (a label from Cira), and Jay edits either in other
//! editors too, or in a second jc on the same project. Every decision
//! about the file lives here, so it is tested without a window; `CodeView`
//! only applies the answers to its editor.
//!
//! The rules, each of which once lost text:
//!
//! - **A save merges first.** It re-reads the file and, when someone else
//!   changed it since this pane's [`SharedFile::base`], three-way merges the
//!   pane's text with theirs; a conflict writes nothing. A write landing after
//!   that read is caught by a second read just before the replace, which then
//!   starts over. Every save by this code also holds an exclusive lock on
//!   the file's directory, so two of them never interleave.
//! - **An own write is recognised by its content.** A disk change whose text
//!   equals the base is this pane's own save (or no change); anything else is
//!   someone's, however soon after a save it lands.
//! - **A conflict stops every save to the file and loses nothing.** The
//!   pane's text goes to `<file>.conflict-<unix secs>` beside it instead, at
//!   the conflict and at every save while it lasts (a send, a schedule), and
//!   the banner names that copy. It ends when [`SharedFile::reload`] takes the
//!   disk's text, or the disk returns to the base or merges cleanly.
//! - **A failed save is visible**: the banner says why until a save works,
//!   and a save waits a bounded time for another's lock.
//! - **A save replaces the file whole** ([`crate::replace`]): through a
//!   symlink, keeping permissions, never truncating it in place.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

pub struct SharedFile {
  path: PathBuf,
  /// What this pane last read from or wrote to the file: the common ancestor
  /// of its edits and anyone else's.
  base: String,
  /// The pane's edits and a disk change could not be merged, and the copy
  /// beside the file that holds the pane's text meanwhile (`None` when even
  /// that could not be written: then `failure` says why).
  conflict: Option<Option<PathBuf>>,
  /// Why the last save failed; `None` once one works.
  failure: Option<String>,
  /// How long a save waits for another save's lock.
  lock_wait: std::time::Duration,
}

/// What a disk change means for the pane ([`SharedFile::disk_changed`]).
#[derive(Debug, PartialEq, Eq)]
pub enum DiskChange {
  /// The disk holds the base: this pane's own write, or nothing changed.
  Unchanged,
  /// Show this text: the disk's (the pane had no edits) or the disk's merged
  /// with the pane's edits.
  Replace(String),
  /// The pane's edits conflict with the disk's change; saving is off.
  Conflict,
}

/// What [`SharedFile::save`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
  /// This text is on disk: the pane's, or the pane's merged with another
  /// writer's change, which the pane should then show.
  Written(String),
  /// Nothing was written: the file is conflicted, or became so now.
  Conflict,
}

/// How many times a save starts over when the file changes between its read
/// and its replace before giving up.
const SAVE_ATTEMPTS: usize = 5;
/// How long a save waits for the lock another save holds.
const LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

impl SharedFile {
  pub fn new(path: PathBuf) -> Self {
    Self { path, base: String::new(), conflict: None, failure: None, lock_wait: LOCK_WAIT }
  }

  pub fn path(&self) -> &Path {
    &self.path
  }

  pub fn base(&self) -> &str {
    &self.base
  }

  pub fn has_conflict(&self) -> bool {
    self.conflict.is_some()
  }

  /// What the pane's banner says, if anything: a conflict and where the
  /// pane's text is kept, or why the last save failed.
  pub fn banner(&self) -> Option<String> {
    let failed = self.failure.as_ref().map(|why| format!("{} not saved: {why}", self.name()));
    match &self.conflict {
      Some(Some(copy)) => Some(format!(
        "Merge conflict \u{2014} your text is kept in {}; nothing is saved to {} until Cmd-R \
         reloads it from disk",
        copy.display(),
        self.name()
      )),
      Some(None) => Some(format!(
        "Merge conflict \u{2014} nothing is saved until Cmd-R reloads from disk. {}",
        failed.unwrap_or_default()
      )),
      None => failed,
    }
  }

  fn name(&self) -> String {
    self.path.file_name().unwrap_or_default().to_string_lossy().into_owned()
  }

  /// Enter (or stay in) a conflict, keeping `ours` beside the file: in the
  /// conflict's copy, made at its start and replaced at each later save.
  fn keep(&mut self, ours: &str) {
    let copy = match &self.conflict {
      Some(Some(copy)) => copy.clone(),
      _ => {
        let secs = std::time::SystemTime::now()
          .duration_since(std::time::UNIX_EPOCH)
          .unwrap_or_default()
          .as_secs();
        let mut name = self.path.clone().into_os_string();
        name.push(format!(".conflict-{secs}"));
        PathBuf::from(name)
      }
    };
    match crate::replace::write_replacing(&copy, ours.as_bytes()) {
      Ok(()) => {
        self.conflict = Some(Some(copy));
        self.failure = None;
      }
      Err(e) => {
        self.conflict = Some(None);
        self.failure = Some(format!("cannot keep your text in {}: {e}", copy.display()));
      }
    }
  }

  /// Take the disk's text as the base, ending any conflict. A missing file
  /// reads empty; one that cannot be read reads as the error, which no save
  /// writes over (a save re-reads it first).
  pub fn reload(&mut self) -> String {
    let text = read(&self.path).unwrap_or_else(|e| format!("Error: {e}"));
    self.base = text.clone();
    self.conflict = None;
    self.failure = None;
    text
  }

  /// The watcher saw the file change while the pane holds `ours`.
  pub fn disk_changed(&mut self, ours: &str) -> DiskChange {
    let Ok(theirs) = read(&self.path) else { return DiskChange::Unchanged };
    if theirs == self.base {
      // Our own write, or the change that conflicted was undone.
      self.conflict = None;
      return DiskChange::Unchanged;
    }
    if ours == self.base {
      self.base = theirs.clone();
      return DiskChange::Replace(theirs);
    }
    match diffy::merge(&self.base, ours, &theirs) {
      Ok(merged) => {
        self.base = theirs;
        self.conflict = None;
        DiskChange::Replace(merged)
      }
      Err(_) => {
        self.keep(ours);
        DiskChange::Conflict
      }
    }
  }

  /// Save `ours`, merging in whatever another writer saved since the base.
  /// A failure is also kept for [`Self::banner`] until a save works.
  pub fn save(&mut self, ours: &str) -> io::Result<Saved> {
    self.save_with(ours, |file, bytes| file.write_all(bytes), || {})
  }

  /// [`Self::save`], with `write` filling the temporary file and
  /// `before_replace` run between the save's read and its check that the file
  /// is still what it read (seams for tests: another writer's write lands
  /// there).
  fn save_with(
    &mut self,
    ours: &str,
    mut write: impl FnMut(&mut fs::File, &[u8]) -> io::Result<()>,
    mut before_replace: impl FnMut(),
  ) -> io::Result<Saved> {
    if self.has_conflict() {
      self.keep(ours);
      return Ok(Saved::Conflict);
    }
    let saved = self.write(ours, &mut write, &mut before_replace);
    match &saved {
      Ok(Saved::Written(_)) => self.failure = None,
      Ok(Saved::Conflict) => {}
      Err(e) => self.failure = Some(e.to_string()),
    }
    saved
  }

  fn write(
    &mut self,
    ours: &str,
    mut write: impl FnMut(&mut fs::File, &[u8]) -> io::Result<()>,
    mut before_replace: impl FnMut(),
  ) -> io::Result<Saved> {
    let target = crate::replace::resolve(&self.path)?;
    let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let _lock = lock(dir, self.lock_wait)?;
    for _ in 0..SAVE_ATTEMPTS {
      let theirs = read(&target)?;
      let text = if theirs == self.base {
        ours.to_string()
      } else {
        match diffy::merge(&self.base, ours, &theirs) {
          Ok(merged) => merged,
          Err(_) => {
            self.keep(ours);
            return Ok(Saved::Conflict);
          }
        }
      };
      if text == theirs {
        self.base = theirs;
        return Ok(Saved::Written(text));
      }
      let replaced =
        crate::replace::write_replacing_with(&target, text.as_bytes(), &mut write, || {
          before_replace();
          Ok(read(&target)? == theirs)
        })?;
      if replaced {
        self.base = text.clone();
        return Ok(Saved::Written(text));
      }
    }
    Err(io::Error::other(format!("{} kept changing while saving", target.display())))
  }
}

/// An exclusive lock on `dir`, held until the result drops, waiting at most
/// `wait` for another save to let go. A filesystem with no locks (some
/// network ones) saves unlocked: the check before the rename still holds.
fn lock(dir: &Path, wait: std::time::Duration) -> io::Result<Option<fs::File>> {
  let file = fs::File::open(dir)?;
  let deadline = std::time::Instant::now() + wait;
  loop {
    match file.try_lock() {
      Ok(()) => return Ok(Some(file)),
      Err(fs::TryLockError::Error(e)) if e.kind() == io::ErrorKind::Unsupported => {
        return Ok(None);
      }
      Err(fs::TryLockError::Error(e)) => return Err(e),
      Err(fs::TryLockError::WouldBlock) if std::time::Instant::now() < deadline => {
        std::thread::sleep(std::time::Duration::from_millis(10));
      }
      Err(fs::TryLockError::WouldBlock) => {
        return Err(io::Error::new(
          io::ErrorKind::TimedOut,
          format!("another save has held the lock on {} for {wait:?}", dir.display()),
        ));
      }
    }
  }
}

/// The file's text; a missing file is empty.
fn read(path: &Path) -> io::Result<String> {
  match fs::read_to_string(path) {
    Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(String::new()),
    read => read,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// A TODO.md jc holds while another writer (Jay's editor, another jc) saves it too.
  const BASE: &str = "# TODO\nnotes\n\n# Claude\n## jc session\n> uuid=u-1\n### WAIT\n\n## other session\n> uuid=u-2\n### WAIT\n";

  /// The other writer's save: a new session heading at the end.
  fn other_adds_a_session(text: &str) -> String {
    format!("{text}## added elsewhere\n> uuid=u-3\n### WAIT\n")
  }

  /// jc's edit: a message sent from its session's WAIT.
  fn jc_sends(text: &str) -> String {
    text.replacen("> uuid=u-1\n### WAIT\n", "> uuid=u-1\n### Message 0\nhi\n### WAIT\n", 1)
  }

  fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jc-shared-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("TODO.md");
    fs::write(&path, BASE).unwrap();
    path
  }

  fn opened(path: &Path) -> SharedFile {
    let mut file = SharedFile::new(path.to_path_buf());
    assert_eq!(file.reload(), BASE);
    file
  }

  fn disk(path: &Path) -> String {
    fs::read_to_string(path).unwrap()
  }

  fn done(path: &Path) {
    fs::remove_dir_all(path.parent().unwrap()).unwrap();
  }

  /// The `.conflict-*` copies beside `path`.
  fn conflict_copies(path: &Path) -> Vec<PathBuf> {
    let mut copies: Vec<PathBuf> = fs::read_dir(path.parent().unwrap())
      .unwrap()
      .flatten()
      .map(|e| e.path())
      .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with("TODO.md.conflict-"))
      .collect();
    copies.sort();
    copies
  }

  #[test]
  fn a_conflict_keeps_jays_text_beside_the_file_and_says_where() {
    let path = scratch("p2-keep");
    let mut jc = opened(&path);
    let theirs = BASE.replace("notes\n", "notes from elsewhere\n");
    fs::write(&path, &theirs).unwrap();
    let ours = BASE.replace("notes\n", "notes from jc\n");
    assert_eq!(jc.save(&ours).unwrap(), Saved::Conflict);
    let copies = conflict_copies(&path);
    assert_eq!(copies.len(), 1, "a conflicting save kept Jay's text nowhere");
    assert_eq!(disk(&copies[0]), ours);
    let banner = jc.banner().expect("the banner says there is a conflict");
    assert!(banner.contains(&copies[0].display().to_string()), "{banner}");
    // An automatic edit while the conflict lasts (a send) is kept too.
    let sent = jc_sends(&ours);
    assert_eq!(jc.save(&sent).unwrap(), Saved::Conflict);
    assert_eq!(conflict_copies(&path), copies, "one copy per conflict");
    assert_eq!(disk(&copies[0]), sent, "the send made during the conflict was lost");
    assert_eq!(disk(&path), theirs);
    done(&path);
  }

  #[test]
  fn a_conflict_seen_by_the_watcher_keeps_jays_text_too() {
    let path = scratch("p2-watch");
    let mut jc = opened(&path);
    fs::write(&path, BASE.replace("notes\n", "notes from elsewhere\n")).unwrap();
    let ours = BASE.replace("notes\n", "notes from jc\n");
    assert_eq!(jc.disk_changed(&ours), DiskChange::Conflict);
    let copies = conflict_copies(&path);
    assert_eq!(copies.len(), 1, "the watcher's conflict kept Jay's text nowhere");
    assert_eq!(disk(&copies[0]), ours);
    done(&path);
  }

  #[test]
  fn a_conflict_ends_when_the_disk_returns_to_the_base() {
    let path = scratch("p2-back");
    let mut jc = opened(&path);
    fs::write(&path, BASE.replace("notes\n", "notes from elsewhere\n")).unwrap();
    let ours = BASE.replace("notes\n", "notes from jc\n");
    assert_eq!(jc.disk_changed(&ours), DiskChange::Conflict);
    // The other writer undoes its edit.
    fs::write(&path, BASE).unwrap();
    assert_eq!(jc.disk_changed(&ours), DiskChange::Unchanged);
    assert!(!jc.has_conflict(), "the conflict outlived the change that caused it");
    assert_eq!(jc.banner(), None);
    assert_eq!(jc.save(&ours).unwrap(), Saved::Written(ours.clone()));
    assert_eq!(disk(&path), ours);
    done(&path);
  }

  #[test]
  fn a_save_waits_a_bounded_time_for_the_lock_and_says_so() {
    let path = scratch("p3-lock");
    let held = fs::File::open(path.parent().unwrap()).unwrap();
    held.lock().unwrap(); // another save, stuck
    let (tx, rx) = std::sync::mpsc::channel();
    let saving = path.clone();
    std::thread::spawn(move || {
      let mut jc = opened(&saving);
      jc.lock_wait = std::time::Duration::from_millis(200);
      let saved = jc.save(&jc_sends(BASE));
      let _ = tx.send((saved.is_err(), jc.banner()));
    });
    let (failed, banner) = rx
      .recv_timeout(std::time::Duration::from_secs(10))
      .expect("a save waited without bound on a lock another save holds");
    assert!(failed);
    let banner = banner.expect("a save that failed says so in the banner");
    assert!(banner.contains("lock"), "{banner}");
    drop(held);
    assert_eq!(disk(&path), BASE);
    done(&path);
  }

  #[test]
  fn a_save_that_cannot_read_the_file_says_so_and_the_next_good_one_clears_it() {
    use std::os::unix::fs::PermissionsExt;
    let path = scratch("p3-read");
    let mut jc = opened(&path);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    assert!(jc.save(&jc_sends(BASE)).is_err());
    let banner = jc.banner().expect("an automatic save that failed was silent");
    assert!(banner.contains("not saved"), "{banner}");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    jc.save(&jc_sends(BASE)).unwrap();
    assert_eq!(jc.banner(), None);
    done(&path);
  }

  #[test]
  fn a_save_keeps_what_another_writer_saved_since_jc_read() {
    let path = scratch("p1-before");
    let mut jc = opened(&path);
    // The other writer saves before jc's watcher has told jc anything.
    fs::write(&path, other_adds_a_session(BASE)).unwrap();
    let saved = jc.save(&jc_sends(BASE)).unwrap();
    let both = other_adds_a_session(&jc_sends(BASE));
    assert_eq!(disk(&path), both, "the other writer's new heading was lost to jc's save");
    assert_eq!(saved, Saved::Written(both.clone()), "jc's buffer takes the merge");
    assert_eq!(jc.base(), both);
    done(&path);
  }

  #[test]
  fn a_write_landing_between_the_read_and_the_replace_is_kept() {
    let path = scratch("p1-between");
    let mut jc = opened(&path);
    let mut once = Some(other_adds_a_session(BASE));
    let saved = jc
      .save_with(
        &jc_sends(BASE),
        |file, bytes| file.write_all(bytes),
        || {
          if let Some(theirs) = once.take() {
            fs::write(&path, theirs).unwrap();
          }
        },
      )
      .unwrap();
    let both = other_adds_a_session(&jc_sends(BASE));
    assert_eq!(disk(&path), both, "the write made during jc's save was lost");
    assert_eq!(saved, Saved::Written(both));
    done(&path);
  }

  #[test]
  fn a_save_that_conflicts_writes_nothing() {
    let path = scratch("p1-conflict");
    let mut jc = opened(&path);
    let theirs = BASE.replace("notes\n", "notes from elsewhere\n");
    fs::write(&path, &theirs).unwrap();
    let saved = jc.save(&BASE.replace("notes\n", "notes from jc\n")).unwrap();
    assert_eq!(saved, Saved::Conflict);
    assert_eq!(
      disk(&path),
      theirs,
      "the other writer's text was overwritten by a conflicting save"
    );
    assert!(jc.has_conflict());
    done(&path);
  }

  #[test]
  fn another_writers_save_right_after_jcs_is_seen() {
    let path = scratch("p2");
    let mut jc = opened(&path);
    let ours = jc_sends(BASE);
    jc.save(&ours).unwrap();
    assert_eq!(jc.disk_changed(&ours), DiskChange::Unchanged, "jc's own write is no change");
    // The other writer saves while jc's save is still recent; the watcher reports it.
    let theirs = other_adds_a_session(&ours);
    fs::write(&path, &theirs).unwrap();
    assert_eq!(
      jc.disk_changed(&ours),
      DiskChange::Replace(theirs.clone()),
      "the watcher hid the other writer's save, leaving jc's base stale"
    );
    assert_eq!(jc.base(), theirs);
    done(&path);
  }

  #[test]
  fn a_conflicted_file_is_not_saved_until_reloaded() {
    let path = scratch("p3");
    let mut jc = opened(&path);
    let ours = BASE.replace("notes\n", "notes from jc\n");
    let theirs = other_adds_a_session(&BASE.replace("notes\n", "notes from elsewhere\n"));
    fs::write(&path, &theirs).unwrap();
    assert_eq!(jc.disk_changed(&ours), DiskChange::Conflict);
    // An automatic save (a send, a label sync) or save-all at quit.
    assert_eq!(jc.save(&jc_sends(&ours)).unwrap(), Saved::Conflict);
    assert_eq!(disk(&path), theirs, "a save while conflicted removed the other writer's heading");
    // Reloading resolves: it takes the disk, and saving works again.
    assert_eq!(jc.reload(), theirs);
    assert!(!jc.has_conflict());
    let ours = jc_sends(&theirs);
    assert_eq!(jc.save(&ours).unwrap(), Saved::Written(ours.clone()));
    assert_eq!(disk(&path), ours);
    done(&path);
  }

  #[test]
  fn a_reader_during_a_save_sees_the_old_file_whole() {
    let path = scratch("p4");
    let mut jc = opened(&path);
    let mut seen = None;
    let cut = jc.save_with(
      &jc_sends(BASE),
      |file, bytes| {
        file.write_all(&bytes[..10])?;
        // Another reader loads TODO.md now (a watcher fired, or jc starts).
        seen = Some(disk(&path));
        Err(io::Error::other("killed mid-write"))
      },
      || {},
    );
    assert!(cut.is_err());
    assert_eq!(seen.as_deref(), Some(BASE), "a reader saw a half-written TODO.md");
    assert_eq!(disk(&path), BASE, "a save cut off part way left TODO.md half written");
    assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 1, "no temporary left");
    done(&path);
  }

  #[test]
  fn a_symlinked_todo_stays_a_link_and_takes_the_merge() {
    let path = scratch("symlink");
    let dir = path.parent().unwrap();
    fs::create_dir_all(dir.join("elsewhere")).unwrap();
    fs::rename(&path, dir.join("elsewhere/project-todo.md")).unwrap();
    std::os::unix::fs::symlink("elsewhere/project-todo.md", &path).unwrap();
    let mut jc = opened(&path);
    fs::write(&path, other_adds_a_session(BASE)).unwrap();
    jc.save(&jc_sends(BASE)).unwrap();
    assert!(fs::symlink_metadata(&path).unwrap().file_type().is_symlink(), "TODO.md stays a link");
    assert_eq!(disk(&dir.join("elsewhere/project-todo.md")), other_adds_a_session(&jc_sends(BASE)));
    done(&path);
  }

  #[test]
  fn a_missing_file_reads_empty_and_a_save_creates_it() {
    let path = scratch("missing");
    fs::remove_file(&path).unwrap();
    let mut jc = SharedFile::new(path.clone());
    assert_eq!(jc.reload(), "");
    assert_eq!(jc.save("# TODO\n").unwrap(), Saved::Written("# TODO\n".into()));
    assert_eq!(disk(&path), "# TODO\n");
    done(&path);
  }
}
