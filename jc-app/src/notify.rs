use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
use objc2_foundation::{NSArray, NSString};

static ACTION_TX: OnceLock<flume::Sender<String>> = OnceLock::new();
static AUTHORIZED: AtomicBool = AtomicBool::new(false);

/// Play the macOS system alert sound as invalid-action feedback (e.g. a
/// Cmd-Enter that is rejected because a scheduled message is pending).
pub fn beep() {
  objc2_app_kit::NSBeep();
}

/// Show `label` on the app's Dock icon, or no badge for `None`
/// (`NSDockTile.badgeLabel`, which AppKit draws). Main thread only.
pub fn set_badge(label: Option<&str>) {
  let Some(mtm) = MainThreadMarker::new() else { return };
  let label = label.map(NSString::from_str);
  NSApplication::sharedApplication(mtm).dockTile().setBadgeLabel(label.as_deref());
}

/// Returns a receiver for session IDs from notification clicks.
/// Call once at startup before `init()`.
pub fn action_receiver() -> flume::Receiver<String> {
  let (tx, rx) = flume::unbounded();
  let _ = ACTION_TX.set(tx);
  rx
}

/// Initialize the notification system: request authorization and set delegate.
/// Must be called on the main thread.
pub fn init() {
  if MainThreadMarker::new().is_none() {
    return;
  }

  // Ensure the UserNotifications framework is linked by referencing a type from it.
  let _ = std::mem::size_of::<objc2_user_notifications::UNNotificationContent>();

  unsafe {
    let center: Retained<AnyObject> =
      msg_send![AnyClass::get(c"UNUserNotificationCenter").unwrap(), currentNotificationCenter];

    // Build and set the delegate. Leak it to keep it alive for the app lifetime.
    let delegate = build_delegate();
    let () = msg_send![&*center, setDelegate: &*delegate];
    std::mem::forget(delegate);

    // Request authorization (alert + sound + badge).
    let handler = RcBlock::new(|granted: Bool, _error: *mut AnyObject| {
      AUTHORIZED.store(granted.as_bool(), Ordering::Relaxed);
      if !granted.as_bool() {
        eprintln!("notify: notification authorization denied");
      }
    });
    let () = msg_send![
      &*center,
      requestAuthorizationWithOptions: 7usize,
      completionHandler: &*handler
    ];

    // Observe app activation (dock click, Cmd-Tab, etc.) to bring the window forward.
    observe_app_activation();
  }
}

/// Post a notification. Falls back to dock bounce if banners aren't authorized.
///
/// jc only notifies for a session Claude has stopped on (permission prompt, API
/// error), so every notification is a critical attention request, which
/// bounces the Dock icon until jc is focused or [`cancel_attention`] ends it.
/// Returns that request's id, none while jc is active (AppKit makes none) or
/// off the main thread, and the notification's identifier, none when banners
/// aren't authorized ([`remove_notifications`] takes it).
pub fn notify(title: &str, message: &str, session_id: &str) -> (Option<isize>, Option<String>) {
  eprintln!("notify: {title} — {message}");
  let request = bounce_dock_icon();

  if !AUTHORIZED.load(Ordering::Relaxed) {
    return (request, None);
  }

  let title = title.to_string();
  let message = message.to_string();
  let session_id = session_id.to_string();
  let ts = std::time::SystemTime::now()
    .duration_since(std::time::UNIX_EPOCH)
    .unwrap_or_default()
    .as_nanos();
  let id = format!("jc-{ts}");
  let notice = id.clone();

  // Post from a background thread to avoid blocking the UI.
  std::thread::spawn(move || {
    // SAFETY: All ObjC calls here create new objects; no shared mutable state.
    unsafe { post_notification(&id, &title, &message, &session_id) };
  });
  (request, Some(notice))
}

/// Remove the notifications `ids` (`notify`'s identifiers) from Notification
/// Center, and any not delivered yet.
pub fn remove_notifications(ids: &[String]) {
  if ids.is_empty() {
    return;
  }
  let ids: Vec<Retained<NSString>> = ids.iter().map(|id| NSString::from_str(id)).collect();
  let ids = NSArray::from_retained_slice(&ids);
  // SAFETY: both methods take an NSArray of NSString; the center is
  // thread-safe, as `post_notification` relies on.
  unsafe {
    let center: Retained<AnyObject> =
      msg_send![AnyClass::get(c"UNUserNotificationCenter").unwrap(), currentNotificationCenter];
    let () = msg_send![&*center, removeDeliveredNotificationsWithIdentifiers: &*ids];
    let () = msg_send![&*center, removePendingNotificationRequestsWithIdentifiers: &*ids];
  }
}

/// End the Dock bounce of attention request `request` (`notify`'s id); one
/// already over is ignored. Main thread only.
pub fn cancel_attention(request: isize) {
  if let Some(mtm) = MainThreadMarker::new() {
    NSApplication::sharedApplication(mtm).cancelUserAttentionRequest(request);
  }
}

unsafe fn post_notification(id: &str, title: &str, message: &str, session_id: &str) {
  let content: Retained<AnyObject> =
    msg_send![AnyClass::get(c"UNMutableNotificationContent").unwrap(), new];

  let ns_title = NSString::from_str(title);
  let ns_body = NSString::from_str(message);
  let () = msg_send![&*content, setTitle: &*ns_title];
  let () = msg_send![&*content, setBody: &*ns_body];

  // Default notification sound.
  let sound: Retained<AnyObject> =
    msg_send![AnyClass::get(c"UNNotificationSound").unwrap(), defaultSound];
  let () = msg_send![&*content, setSound: &*sound];

  // Thread identifier groups notifications by session.
  let thread_id = NSString::from_str(session_id);
  let () = msg_send![&*content, setThreadIdentifier: &*thread_id];

  // Store session_id in userInfo so the click handler can route to the session.
  let key = NSString::from_str("session_id");
  let val = NSString::from_str(session_id);
  let user_info: Retained<AnyObject> = msg_send![
    AnyClass::get(c"NSDictionary").unwrap(),
    dictionaryWithObject: &*val,
    forKey: &*key
  ];
  let () = msg_send![&*content, setUserInfo: &*user_info];

  let request_id = NSString::from_str(id);
  let null: *const AnyObject = std::ptr::null();
  let request: Retained<AnyObject> = msg_send![
    AnyClass::get(c"UNNotificationRequest").unwrap(),
    requestWithIdentifier: &*request_id,
    content: &*content,
    trigger: null
  ];

  let center: Retained<AnyObject> =
    msg_send![AnyClass::get(c"UNUserNotificationCenter").unwrap(), currentNotificationCenter];
  let handler = RcBlock::new(|_error: *mut AnyObject| {});
  let () = msg_send![
    &*center,
    addNotificationRequest: &*request,
    withCompletionHandler: &*handler
  ];
}

fn bounce_dock_icon() -> Option<isize> {
  let mtm = MainThreadMarker::new()?;
  let app = NSApplication::sharedApplication(mtm);
  let request = app.requestUserAttention(NSRequestUserAttentionType::CriticalRequest);
  (request != 0).then_some(request)
}

// --- Notification delegate ---

/// Delegate method: userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:
///
/// Extracts the session_id from the notification's userInfo and sends it through
/// the action channel so the workspace can switch to the session.
unsafe extern "C" fn did_receive_response(
  _this: *mut AnyObject,
  _sel: Sel,
  _center: *mut AnyObject,
  response: *mut AnyObject,
  handler: *mut AnyObject, // block; raw pointer avoids MethodImplementation lifetime issues
) {
  unsafe {
    let notification: Retained<AnyObject> = msg_send![&*response, notification];
    let request: Retained<AnyObject> = msg_send![&*notification, request];
    let content: Retained<AnyObject> = msg_send![&*request, content];
    let user_info: Retained<AnyObject> = msg_send![&*content, userInfo];
    let key = NSString::from_str("session_id");
    let val: Option<Retained<NSString>> = msg_send![&*user_info, objectForKey: &*key];

    if let (Some(session_id), Some(tx)) = (val, ACTION_TX.get()) {
      let _ = tx.send(session_id.to_string());
    }

    // Move the key window to the active Space and bring it forward.
    bring_window_to_front();

    // Call the completion handler block: ((void)(^)(void))
    if !handler.is_null() {
      let invoke: unsafe extern "C" fn(*mut AnyObject) =
        std::mem::transmute((*handler.cast::<BlockLayout>()).invoke);
      invoke(handler);
    }
  }
}

/// Register an observer for NSApplicationDidBecomeActiveNotification so that
/// any activation (dock click, Cmd-Tab, etc.) switches to the window's Space.
unsafe fn observe_app_activation() {
  let nc: Retained<AnyObject> =
    msg_send![AnyClass::get(c"NSNotificationCenter").unwrap(), defaultCenter];
  let name = NSString::from_str("NSApplicationDidBecomeActiveNotification");
  let block = RcBlock::new(|_notif: *mut AnyObject| {
    unsafe { bring_window_to_front() };
  });
  // The method returns the observer token (type `@`); declaring `()` fails
  // objc2's message-encoding check in debug builds.
  let observer: Retained<AnyObject> = msg_send![
    &*nc,
    addObserverForName: &*name,
    object: std::ptr::null::<AnyObject>(),
    queue: std::ptr::null::<AnyObject>(),
    usingBlock: &*block
  ];
  // Leak the token and the block so the observer lives for the app's lifetime.
  std::mem::forget(observer);
  std::mem::forget(block);
}

/// Switch to the Space containing the app's window and bring it forward.
/// Must be called on the main thread.
unsafe fn bring_window_to_front() {
  unsafe {
    let app: Retained<AnyObject> =
      msg_send![AnyClass::get(c"NSApplication").unwrap(), sharedApplication];

    // Find the window to focus.
    let mut window: *mut AnyObject = msg_send![&*app, keyWindow];
    if window.is_null() {
      let windows: Retained<AnyObject> = msg_send![&*app, windows];
      let count: usize = msg_send![&*windows, count];
      if count == 0 {
        return;
      }
      window = msg_send![&*windows, objectAtIndex: 0usize];
      if window.is_null() {
        return;
      }
    }

    // orderFrontRegardless makes macOS switch to the Space the window lives on
    // (when "When switching to an application, switch to a Space with open
    // windows" is enabled in System Settings, which is the default).
    let () = msg_send![window, orderFrontRegardless];
    let () = msg_send![&*app, activateIgnoringOtherApps: true];
  }
}

/// Minimal block layout for calling the invoke pointer directly.
#[repr(C)]
struct BlockLayout {
  _isa: *const std::ffi::c_void,
  _flags: i32,
  _reserved: i32,
  invoke: *const std::ffi::c_void,
}

unsafe fn build_delegate() -> Retained<AnyObject> {
  static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();

  let cls = CLASS.get_or_init(|| {
    let superclass = AnyClass::get(c"NSObject").unwrap();
    let mut builder = ClassBuilder::new(c"JCNotificationDelegate", superclass).unwrap();

    if let Some(proto) = AnyProtocol::get(c"UNUserNotificationCenterDelegate") {
      builder.add_protocol(proto);
    }

    // SAFETY: Function signature matches the ObjC method ABI.
    unsafe {
      builder.add_method(
        sel!(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:),
        did_receive_response
          as unsafe extern "C" fn(
            *mut AnyObject,
            Sel,
            *mut AnyObject,
            *mut AnyObject,
            *mut AnyObject,
          ),
      );
    }

    builder.register()
  });

  unsafe { msg_send![*cls, new] }
}
