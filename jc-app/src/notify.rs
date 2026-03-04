use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};

/// Bounce the dock icon to get the user's attention.
///
/// `critical`: if true, bounces repeatedly until the user focuses the app.
/// If false, bounces once.
pub fn bounce_dock_icon(critical: bool) {
  let attention_type = if critical {
    NSRequestUserAttentionType::CriticalRequest
  } else {
    NSRequestUserAttentionType::InformationalRequest
  };
  if let Some(mtm) = MainThreadMarker::new() {
    let app = NSApplication::sharedApplication(mtm);
    app.requestUserAttention(attention_type);
  }
}

/// Notify the user via dock bounce.
///
/// Notification banners (`osascript` / `UNUserNotificationCenter`) require
/// an `.app` bundle with a bundle ID — they silently fail for unbundled
/// binaries.  Dock bounce works without bundling.
pub fn notify(title: &str, message: &str, critical: bool) {
  eprintln!("notify: {title} — {message}");
  bounce_dock_icon(critical);
}
