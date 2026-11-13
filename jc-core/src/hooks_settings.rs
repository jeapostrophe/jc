use crate::flavor::Flavor;
use anyhow::{Context, Result};
use serde_json::Value;
use std::path::Path;

/// Install `flavor`'s hook entries into a project's `.claude/settings.local.json`.
///
/// Removes any stale entries of that flavor first (sentinel: a URL containing
/// its [`Flavor::hook_prefix`]), then appends fresh entries pointing at the
/// given port. The other flavor's entries are left alone, so `jc` and `jcc`
/// with the same project open keep each other's hooks.
pub fn install_hooks(flavor: Flavor, project_path: &Path, port: u16) -> Result<()> {
  edit_settings(project_path, true, |settings| add_hooks(settings, flavor.hook_prefix(), port))
}

/// Replace `prefix`'s hook entries in `settings` with fresh ones on `port`.
fn add_hooks(settings: &mut Value, prefix: &str, port: u16) {
  remove_jc_hooks(settings, prefix);

  let hooks = settings
    .as_object_mut()
    .expect("settings is an object")
    .entry("hooks")
    .or_insert_with(|| Value::Object(serde_json::Map::default()));

  let hooks_obj = hooks.as_object_mut().expect("hooks should be an object");

  let base = format!("http://127.0.0.1:{port}{prefix}");

  // UserPromptSubmit hook (fires when user submits a prompt — Claude about to start)
  let prompt_submit_entry = serde_json::json!({
      "hooks": [{ "type": "http", "url": format!("{base}prompt-submit") }]
  });
  hooks_obj
    .entry("UserPromptSubmit")
    .or_insert_with(|| Value::Array(Vec::new()))
    .as_array_mut()
    .expect("UserPromptSubmit should be an array")
    .push(prompt_submit_entry);

  // Stop hook
  let stop_entry = serde_json::json!({
      "hooks": [{ "type": "http", "url": format!("{base}stop") }]
  });
  hooks_obj
    .entry("Stop")
    .or_insert_with(|| Value::Array(Vec::new()))
    .as_array_mut()
    .expect("Stop should be an array")
    .push(stop_entry);

  // StopFailure hook (fires when turn ends due to API error)
  let stop_failure_entry = serde_json::json!({
      "hooks": [{ "type": "http", "url": format!("{base}stop-failure") }]
  });
  hooks_obj
    .entry("StopFailure")
    .or_insert_with(|| Value::Array(Vec::new()))
    .as_array_mut()
    .expect("StopFailure should be an array")
    .push(stop_failure_entry);

  // Notification hooks (idle_prompt = Claude finished & waiting, permission_prompt = needs approval)
  let notification_entry = serde_json::json!({
      "matcher": ".*",
      "hooks": [{ "type": "http", "url": format!("{base}notification") }]
  });
  hooks_obj
    .entry("Notification")
    .or_insert_with(|| Value::Array(Vec::new()))
    .as_array_mut()
    .expect("Notification should be an array")
    .push(notification_entry);

  // PermissionRequest hook (fires when a tool permission dialog appears)
  let permission_entry = serde_json::json!({
      "hooks": [{ "type": "http", "url": format!("{base}permission") }]
  });
  hooks_obj
    .entry("PermissionRequest")
    .or_insert_with(|| Value::Array(Vec::new()))
    .as_array_mut()
    .expect("PermissionRequest should be an array")
    .push(permission_entry);

  // SessionEnd hook (detect /clear) — only "command" type is supported.
  // The hook receives JSON on stdin with session_id, reason, cwd, etc.
  let session_end_entry = serde_json::json!({
      "hooks": [{
        "type": "command",
        "command": format!(
          "curl -sf -X POST {base}session-end -H 'Content-Type: application/json' -d @-"
        ),
        "timeout": 5
      }]
  });
  hooks_obj
    .entry("SessionEnd")
    .or_insert_with(|| Value::Array(Vec::new()))
    .as_array_mut()
    .expect("SessionEnd should be an array")
    .push(session_end_entry);

  // SessionStart hook (a /clear, and every start or resume: the session is live) — only "command" type is supported.
  let session_start_entry = serde_json::json!({
      "hooks": [{
        "type": "command",
        "command": format!(
          "curl -sf -X POST {base}session-start -H 'Content-Type: application/json' -d @-"
        ),
        "timeout": 5
      }]
  });
  hooks_obj
    .entry("SessionStart")
    .or_insert_with(|| Value::Array(Vec::new()))
    .as_array_mut()
    .expect("SessionStart should be an array")
    .push(session_start_entry);
}

/// Remove `flavor`'s hook entries from a project's `.claude/settings.local.json`.
pub fn uninstall_hooks(flavor: Flavor, project_path: &Path) -> Result<()> {
  edit_settings(project_path, false, |settings| remove_jc_hooks(settings, flavor.hook_prefix()))
}

/// Read, `edit` and write back a project's `.claude/settings.local.json`,
/// which jc, jcc and Jay all edit: under an exclusive lock on the `.claude`
/// directory, so jc and jcc changing their hooks at once never lose each
/// other's change, and replaced whole ([`crate::replace`]: a reader never sees
/// half a file, and a symlinked file stays a link). A file that does not parse is left alone
/// (an error), never replaced by one holding only hooks. With `create` false
/// a missing file stays missing.
fn edit_settings(project_path: &Path, create: bool, edit: impl FnOnce(&mut Value)) -> Result<()> {
  let dir = project_path.join(".claude");
  let path = dir.join("settings.local.json");
  if !create && !path.exists() {
    return Ok(());
  }
  std::fs::create_dir_all(&dir)?;
  let lock = std::fs::File::open(&dir)?;
  lock.lock().with_context(|| format!("failed to lock {}", dir.display()))?;
  let mut settings = match std::fs::read_to_string(&path) {
    Ok(text) => serde_json::from_str(&text)
      .with_context(|| format!("{} does not parse; leaving it alone", path.display()))?,
    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
      if !create {
        return Ok(());
      }
      Value::Object(serde_json::Map::default())
    }
    Err(e) => return Err(e.into()),
  };
  if !settings.is_object() {
    anyhow::bail!("{} is not a JSON object; leaving it alone", path.display());
  }
  edit(&mut settings);
  crate::replace::write_replacing(&path, serde_json::to_string_pretty(&settings)?.as_bytes())?;
  Ok(())
}

/// Remove all hook matcher groups that contain a URL with `prefix`.
/// Cleans up empty event arrays and the hooks object itself if empty.
fn remove_jc_hooks(settings: &mut Value, prefix: &str) {
  let Some(hooks) = settings.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
    return;
  };

  let mut empty_keys = Vec::new();

  for (key, value) in hooks.iter_mut() {
    let Some(arr) = value.as_array_mut() else {
      continue;
    };
    arr.retain(|matcher_group| !contains_hook_url(matcher_group, prefix));
    if arr.is_empty() {
      empty_keys.push(key.clone());
    }
  }

  for key in empty_keys {
    hooks.remove(&key);
  }

  // Remove empty hooks object
  if hooks.is_empty() {
    settings.as_object_mut().unwrap().remove("hooks");
  }
}

/// Check if a matcher group object contains any hook URL with `prefix`.
fn contains_hook_url(value: &Value, prefix: &str) -> bool {
  match value {
    Value::String(s) => s.contains(prefix),
    Value::Object(obj) => obj.values().any(|v| contains_hook_url(v, prefix)),
    Value::Array(arr) => arr.iter().any(|v| contains_hook_url(v, prefix)),
    _ => false,
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  /// The hook URLs of `flavor` in the project's settings file.
  fn urls(project: &Path, flavor: Flavor) -> usize {
    let text = std::fs::read_to_string(project.join(".claude/settings.local.json")).unwrap();
    text.matches(&format!("127.0.0.1:{}{}", port_of(flavor), flavor.hook_prefix())).count()
  }

  fn port_of(flavor: Flavor) -> u16 {
    match flavor {
      Flavor::Jc => 1111,
      Flavor::Jcc => 2222,
    }
  }

  #[test]
  fn jc_and_jcc_on_one_project_keep_each_others_hooks() {
    let project = std::env::temp_dir().join(format!("jc-hooks-both-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let (jc, jcc) = (Flavor::Jc, Flavor::Jcc);

    install_hooks(jc, &project, port_of(jc)).unwrap();
    let each = urls(&project, jc);
    assert!(each > 0);
    install_hooks(jcc, &project, port_of(jcc)).unwrap();
    assert_eq!(urls(&project, jc), each, "installing jcc's hooks keeps jc's");
    assert_eq!(urls(&project, jcc), each);
    // A restart reinstalls: still one set each.
    install_hooks(jc, &project, port_of(jc)).unwrap();
    assert_eq!((urls(&project, jc), urls(&project, jcc)), (each, each));

    uninstall_hooks(jcc, &project).unwrap();
    assert_eq!(urls(&project, jc), each, "jcc quitting leaves jc's hooks");
    assert_eq!(urls(&project, jcc), 0);
    install_hooks(jcc, &project, port_of(jcc)).unwrap();
    uninstall_hooks(jc, &project).unwrap();
    assert_eq!((urls(&project, jc), urls(&project, jcc)), (0, each), "jc quitting leaves jcc's");
    std::fs::remove_dir_all(&project).unwrap();
  }

  #[test]
  fn jc_and_jcc_changing_their_hooks_at_once_lose_neither() {
    let project = std::env::temp_dir().join(format!("jc-hooks-race-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(&project).unwrap();
    let both: Vec<_> = [Flavor::Jc, Flavor::Jcc]
      .into_iter()
      .map(|flavor| {
        let project = project.clone();
        std::thread::spawn(move || {
          for _ in 0..40 {
            uninstall_hooks(flavor, &project).unwrap();
            install_hooks(flavor, &project, port_of(flavor)).unwrap();
          }
        })
      })
      .collect();
    for t in both {
      t.join().unwrap();
    }
    assert!(urls(&project, Flavor::Jc) > 0, "jc's last install survives jcc's");
    assert!(urls(&project, Flavor::Jcc) > 0, "jcc's last install survives jc's");
    std::fs::remove_dir_all(&project).unwrap();
  }

  #[test]
  fn a_symlinked_settings_file_stays_a_link_and_keeps_its_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let root = std::env::temp_dir().join(format!("jc-hooks-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("project");
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    std::fs::create_dir_all(root.join("elsewhere")).unwrap();
    let target = root.join("elsewhere/settings.local.json");
    std::fs::write(&target, "{\"permissions\": {\"allow\": [\"Bash(ls)\"]}}").unwrap();
    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = project.join(".claude/settings.local.json");
    std::os::unix::fs::symlink(&target, &link).unwrap();

    install_hooks(Flavor::Jc, &project, 1111).unwrap();
    assert!(
      std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink(),
      "installing hooks replaced the symlinked settings.local.json with a file"
    );
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.contains("/jc-hook/") && text.contains("Bash(ls)"), "{text}");
    assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o600);
    std::fs::remove_dir_all(&root).unwrap();
  }

  #[test]
  fn a_settings_file_that_does_not_parse_is_left_alone() {
    let project = std::env::temp_dir().join(format!("jc-hooks-bad-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&project);
    std::fs::create_dir_all(project.join(".claude")).unwrap();
    let path = project.join(".claude/settings.local.json");
    let bad = "{\"permissions\": {\"allow\": [\"Bash(ls)\"]}, oops";
    std::fs::write(&path, bad).unwrap();
    assert!(install_hooks(Flavor::Jcc, &project, 2222).is_err());
    assert!(uninstall_hooks(Flavor::Jc, &project).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), bad, "Jay's file is not replaced");
    std::fs::remove_dir_all(&project).unwrap();
  }
}
