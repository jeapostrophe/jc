use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, AnyProtocol, Bool, ClassBuilder, Sel};
use objc2::{MainThreadMarker, msg_send, sel};
use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
use objc2_foundation::NSString;

static ACTION_TX: OnceLock<flume::Sender<String>> = OnceLock::new();
static AUTHORIZED: AtomicBool = AtomicBool::new(false);

const SWITCH_ACTION_ID: &str = "SWITCH_SESSION";
const CATEGORY_ID: &str = "CLAUDE_EVENT";

/// Returns a receiver for notification action slugs (session the user wants to switch to).
/// Call once at startup before `init()`.
pub fn action_receiver() -> flume::Receiver<String> {
  let (tx, rx) = flume::unbounded();
  let _ = ACTION_TX.set(tx);
  rx
}

/// Initialize the notification system: request authorization, register categories, set delegate.
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

    // Create "Switch to Session" action (foreground activation).
    let action_id = NSString::from_str(SWITCH_ACTION_ID);
    let action_title = NSString::from_str("Switch to Session");
    let action: Retained<AnyObject> = msg_send![
      AnyClass::get(c"UNNotificationAction").unwrap(),
      actionWithIdentifier: &*action_id,
      title: &*action_title,
      options: 1usize // UNNotificationActionOptionForeground
    ];

    // Create category containing the action.
    let category_id = NSString::from_str(CATEGORY_ID);
    let actions: Retained<AnyObject> =
      msg_send![AnyClass::get(c"NSArray").unwrap(), arrayWithObject: &*action];
    let empty: Retained<AnyObject> = msg_send![AnyClass::get(c"NSArray").unwrap(), array];
    let category: Retained<AnyObject> = msg_send![
      AnyClass::get(c"UNNotificationCategory").unwrap(),
      categoryWithIdentifier: &*category_id,
      actions: &*actions,
      intentIdentifiers: &*empty,
      options: 0usize
    ];
    let categories: Retained<AnyObject> =
      msg_send![AnyClass::get(c"NSSet").unwrap(), setWithObject: &*category];
    let () = msg_send![&*center, setNotificationCategories: &*categories];

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
  }
}

/// Post a notification. Falls back to dock bounce if banners aren't authorized.
pub fn notify(title: &str, message: &str, critical: bool, slug: Option<&str>) {
  eprintln!("notify: {title} — {message}");
  bounce_dock_icon(critical);

  if !AUTHORIZED.load(Ordering::Relaxed) {
    return;
  }

  let title = title.to_string();
  let message = message.to_string();
  let slug = slug.map(str::to_string);

  // Post from a background thread to avoid blocking the UI.
  std::thread::spawn(move || {
    // SAFETY: All ObjC calls here create new objects; no shared mutable state.
    unsafe { post_notification(&title, &message, slug.as_deref()) };
  });
}

unsafe fn post_notification(title: &str, message: &str, slug: Option<&str>) {
  unsafe {
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

    // Category enables the "Switch to Session" action button.
    let cat_id = NSString::from_str(CATEGORY_ID);
    let () = msg_send![&*content, setCategoryIdentifier: &*cat_id];

    // Thread identifier groups notifications by slug.
    if let Some(slug) = slug {
      let thread_id = NSString::from_str(slug);
      let () = msg_send![&*content, setThreadIdentifier: &*thread_id];

      // Store slug in userInfo so the action handler can route to the session.
      let slug_key = NSString::from_str("slug");
      let slug_val = NSString::from_str(slug);
      let user_info: Retained<AnyObject> = msg_send![
        AnyClass::get(c"NSDictionary").unwrap(),
        dictionaryWithObject: &*slug_val,
        forKey: &*slug_key
      ];
      let () = msg_send![&*content, setUserInfo: &*user_info];
    }

    // Unique request identifier.
    let ts = std::time::SystemTime::now()
      .duration_since(std::time::UNIX_EPOCH)
      .unwrap_or_default()
      .as_nanos();
    let request_id = NSString::from_str(&format!("jc-{ts}"));
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
}

fn bounce_dock_icon(critical: bool) {
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

// --- Notification delegate ---

/// Delegate method: userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:
///
/// Extracts the slug from the notification's userInfo and sends it through
/// the action channel so the workspace can switch sessions.
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
    let slug_key = NSString::from_str("slug");
    let slug_val: Option<Retained<NSString>> = msg_send![&*user_info, objectForKey: &*slug_key];

    if let (Some(slug), Some(tx)) = (slug_val, ACTION_TX.get()) {
      let _ = tx.send(slug.to_string());
    }

    // Call the completion handler block: ((void)(^)(void))
    if !handler.is_null() {
      let invoke: unsafe extern "C" fn(*mut AnyObject) =
        std::mem::transmute((*handler.cast::<BlockLayout>()).invoke);
      invoke(handler);
    }
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
