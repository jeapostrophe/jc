use crate::hooks::HOOK_PATH_PREFIX;
use anyhow::Result;
use serde_json::Value;
use std::path::Path;

/// Install jc hook entries into a project's `.claude/settings.local.json`.
///
/// Removes any stale jc hooks first (sentinel: URL containing `/jc-hook/`),
/// then appends fresh entries pointing at the given port.
pub fn install_hooks(project_path: &Path, port: u16) -> Result<()> {
  let settings_path = project_path.join(".claude/settings.local.json");
  let mut settings = load_settings(&settings_path);

  remove_jc_hooks(&mut settings);

  let hooks = settings
    .as_object_mut()
    .expect("settings is an object")
    .entry("hooks")
    .or_insert_with(|| Value::Object(serde_json::Map::default()));

  let hooks_obj = hooks.as_object_mut().unwrap_or_else(|| panic!("hooks should be an object"));

  let base = format!("http://127.0.0.1:{port}{HOOK_PATH_PREFIX}");

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

  write_settings(&settings_path, &settings)
}

/// Remove jc hook entries from a project's `.claude/settings.local.json`.
pub fn uninstall_hooks(project_path: &Path) -> Result<()> {
  let settings_path = project_path.join(".claude/settings.local.json");
  if !settings_path.exists() {
    return Ok(());
  }
  let mut settings = load_settings(&settings_path);
  remove_jc_hooks(&mut settings);
  write_settings(&settings_path, &settings)
}

/// Remove all hook matcher groups that contain a URL with `/jc-hook/`.
/// Cleans up empty event arrays and the hooks object itself if empty.
fn remove_jc_hooks(settings: &mut Value) {
  let Some(hooks) = settings.get_mut("hooks").and_then(|v| v.as_object_mut()) else {
    return;
  };

  let mut empty_keys = Vec::new();

  for (key, value) in hooks.iter_mut() {
    let Some(arr) = value.as_array_mut() else {
      continue;
    };
    arr.retain(|matcher_group| !contains_jc_hook_url(matcher_group));
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

/// Check if a matcher group object contains any hook URL with `/jc-hook/`.
fn contains_jc_hook_url(value: &Value) -> bool {
  match value {
    Value::String(s) => s.contains(HOOK_PATH_PREFIX),
    Value::Object(obj) => obj.values().any(contains_jc_hook_url),
    Value::Array(arr) => arr.iter().any(contains_jc_hook_url),
    _ => false,
  }
}

fn load_settings(path: &Path) -> Value {
  std::fs::read_to_string(path)
    .ok()
    .and_then(|s| serde_json::from_str(&s).ok())
    .unwrap_or_else(|| Value::Object(serde_json::Map::default()))
}

fn write_settings(path: &Path, settings: &Value) -> Result<()> {
  if let Some(parent) = path.parent() {
    std::fs::create_dir_all(parent)?;
  }
  let json = serde_json::to_string_pretty(settings)?;
  std::fs::write(path, json)?;
  Ok(())
}
