//! Replacing a file whole: the new contents go to a temporary file beside the
//! file, synced, then renamed over it. A save cut off part way (jc killed, the
//! disk full) leaves the old file whole, and a reader (the other app, Jay's
//! editor) sees the old file or the new one, never an empty or half one.
//!
//! A symlink is written through: the file it names is replaced, and the link
//! stays (a TODO.md or `settings.local.json` linked into a dot-files repo stays
//! linked). The file keeps its permissions.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

/// The file `path` names once every symlink is followed, whether or not that
/// file exists yet (a dangling link names the file a write would create).
pub fn resolve(path: &Path) -> io::Result<PathBuf> {
  let mut path = path.to_path_buf();
  for _ in 0..40 {
    match fs::symlink_metadata(&path) {
      Ok(meta) if meta.file_type().is_symlink() => {
        let target = fs::read_link(&path)?;
        path = match path.parent() {
          Some(dir) if target.is_relative() => dir.join(target),
          _ => target,
        };
      }
      _ => return Ok(path),
    }
  }
  Err(io::Error::other(format!("{}: too many levels of symlinks", path.display())))
}

/// Replace the file `path` names (through any symlink) with `bytes`.
pub fn write_replacing(path: &Path, bytes: &[u8]) -> io::Result<()> {
  write_replacing_with(path, bytes, |file, bytes| file.write_all(bytes), || Ok(true)).map(|_| ())
}

/// [`write_replacing`], with `write` (a seam for tests) filling the temporary
/// file, and `proceed` asked once it is written and synced, just before the
/// rename: `false` removes the temporary file and replaces nothing (the
/// caller saw the file change meanwhile). Returns whether it replaced.
///
/// The temporary name is per process, so each file must have one writer
/// thread per process.
pub fn write_replacing_with(
  path: &Path,
  bytes: &[u8],
  write: impl FnOnce(&mut fs::File, &[u8]) -> io::Result<()>,
  proceed: impl FnOnce() -> io::Result<bool>,
) -> io::Result<bool> {
  let target = resolve(path)?;
  let dir = target.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
  let mut name = std::ffi::OsString::from(".");
  name.push(target.file_name().unwrap_or_default());
  name.push(format!(".tmp-{}", std::process::id()));
  let temp = dir.join(name);
  let permissions = fs::metadata(&target).ok().map(|m| m.permissions());
  let replaced = (|| -> io::Result<bool> {
    let mut file = fs::File::create(&temp)?;
    if let Some(permissions) = permissions {
      file.set_permissions(permissions)?;
    }
    write(&mut file, bytes)?;
    file.sync_all()?;
    if !proceed()? {
      return Ok(false);
    }
    fs::rename(&temp, &target)?;
    Ok(true)
  })();
  if !matches!(replaced, Ok(true)) {
    let _ = fs::remove_file(&temp);
  }
  if replaced? {
    // The rename is durable once the directory is synced; the file is replaced
    // either way, so a filesystem that cannot sync a directory fails no save.
    let _ = fs::File::open(dir).and_then(|d| d.sync_all());
    return Ok(true);
  }
  Ok(false)
}

#[cfg(test)]
mod tests {
  use super::*;
  use std::os::unix::fs::PermissionsExt;

  fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("jc-replace-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
  }

  #[test]
  fn a_save_cut_off_part_way_leaves_the_old_file_whole() {
    let dir = scratch("cut");
    let path = dir.join("sessions.toml");
    fs::write(&path, "old").unwrap();

    let cut = write_replacing_with(
      &path,
      b"new contents",
      |file, bytes| {
        file.write_all(&bytes[..3])?;
        Err(io::Error::other("killed mid-write"))
      },
      || Ok(true),
    );
    assert!(cut.is_err());
    assert_eq!(fs::read_to_string(&path).unwrap(), "old", "a cut-off save leaves the old file");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1, "and no temporary file behind");

    write_replacing(&path, b"new contents").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new contents");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    fs::remove_dir_all(&dir).unwrap();
  }

  #[test]
  fn a_symlinked_file_is_written_through_and_keeps_its_permissions() {
    let dir = scratch("link");
    fs::create_dir_all(dir.join("dot")).unwrap();
    fs::create_dir_all(dir.join("project")).unwrap();
    let target = dir.join("dot/todo.md");
    fs::write(&target, "old").unwrap();
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.join("project/TODO.md");
    std::os::unix::fs::symlink("../dot/todo.md", &link).unwrap();

    write_replacing(&link, b"new").unwrap();
    assert!(fs::symlink_metadata(&link).unwrap().file_type().is_symlink(), "the link stays");
    assert_eq!(fs::read_to_string(&target).unwrap(), "new", "its target has the new text");
    assert_eq!(fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
    assert_eq!(fs::read_dir(dir.join("dot")).unwrap().count(), 1, "no temporary file behind");
    assert_eq!(fs::read_dir(dir.join("project")).unwrap().count(), 1);

    // A dangling link names the file a write creates.
    let dangling = dir.join("project/NEW.md");
    std::os::unix::fs::symlink("../dot/new.md", &dangling).unwrap();
    write_replacing(&dangling, b"made").unwrap();
    assert_eq!(fs::read_to_string(dir.join("dot/new.md")).unwrap(), "made");
    assert!(fs::symlink_metadata(&dangling).unwrap().file_type().is_symlink());
    fs::remove_dir_all(&dir).unwrap();
  }

  #[test]
  fn a_refused_replace_leaves_the_file_and_no_temporary() {
    let dir = scratch("refused");
    let path = dir.join("TODO.md");
    fs::write(&path, "theirs").unwrap();
    let replaced =
      write_replacing_with(&path, b"ours", |f, b| f.write_all(b), || Ok(false)).unwrap();
    assert!(!replaced);
    assert_eq!(fs::read_to_string(&path).unwrap(), "theirs");
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    fs::remove_dir_all(&dir).unwrap();
  }
}
