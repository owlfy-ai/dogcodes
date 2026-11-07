//! `UNUserNotificationCenter`, for a real app bundle. A click reaches the delegate from the
//! system with the notification's identifier, so it needs no waiting thread and still works
//! after a restart. Touching this center from an unbundled binary aborts the process.
use block2::RcBlock;
use objc2::{
    AnyThread, define_class, msg_send,
    rc::Retained,
    runtime::{Bool, ProtocolObject},
};
use objc2_foundation::{NSArray, NSBundle, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNMutableNotificationContent, UNNotification,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::{cell::Cell, path::Path, sync::OnceLock};

const IDENTIFIER_PREFIX: &str = "conversation-";
const DISMISS_ACTION: &str = "com.apple.UNNotificationDismissActionIdentifier";

static ON_CLICK: OnceLock<fn(String)> = OnceLock::new();
static DELEGATE: OnceLock<Retained<AttentionDelegate>> = OnceLock::new();

/// One per conversation: a newer notification replaces the one before it.
fn identifier(task_id: &str) -> Retained<NSString> {
    NSString::from_str(&format!("{IDENTIFIER_PREFIX}{task_id}"))
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "DogCodesAttentionDelegate"]
    struct AttentionDelegate;

    unsafe impl NSObjectProtocol for AttentionDelegate {}

    unsafe impl UNUserNotificationCenterDelegate for AttentionDelegate {
        // Without this the system drops the banner while this app is frontmost, which is
        // when someone is looking at a different conversation.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            completion
                .call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            completion: &block2::DynBlock<dyn Fn()>,
        ) {
            let identifier = response.notification().request().identifier().to_string();
            if response.actionIdentifier().to_string() != DISMISS_ACTION
                && let Some(task_id) = identifier.strip_prefix(IDENTIFIER_PREFIX)
                && let Some(on_click) = ON_CLICK.get()
            {
                on_click(task_id.to_owned());
            }
            completion.call(());
        }
    }
);

/// Whether this process may use the center at all. By path: a dev binary has an embedded
/// Info.plist, so it reports a bundle identifier without being in a bundle.
pub fn available() -> bool {
    Path::new(&NSBundle::mainBundle().bundlePath().to_string())
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
}

/// Call while the app is starting, so a click that launched it is delivered too.
pub fn install(on_click: fn(String)) {
    let _ = ON_CLICK.set(on_click);
    DELEGATE.get_or_init(|| {
        // SAFETY: `init` is `NSObject`'s designated initializer and this class adds no state
        // of its own that it would need to set up.
        let delegate: Retained<AttentionDelegate> = unsafe {
            let this = AttentionDelegate::alloc().set_ivars(());
            msg_send![super(this), init]
        };
        UNUserNotificationCenter::currentNotificationCenter()
            .setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        delegate
    });
}

/// Asks for permission the first time, then shows it. Someone declining is their answer and
/// shows nothing; `refused` runs only if the system won't serve this app at all, as it may
/// for one without a proper signature.
pub fn send(task_id: &str, title: &str, body: &str, refused: impl FnOnce() + Send + 'static) {
    let identifier = identifier(task_id);
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(body));
    let refused = Cell::new(Some(refused));
    UNUserNotificationCenter::currentNotificationCenter()
        .requestAuthorizationWithOptions_completionHandler(
            UNAuthorizationOptions::Alert,
            &RcBlock::new(move |granted: Bool, error: *mut NSError| {
                if !error.is_null() {
                    if let Some(refused) = refused.take() {
                        refused();
                    }
                    return;
                }
                if granted.as_bool() {
                    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
                        &identifier,
                        &content,
                        None,
                    );
                    UNUserNotificationCenter::currentNotificationCenter()
                        .addNotificationRequest_withCompletionHandler(&request, None);
                }
            }),
        );
}

/// Takes back what's no longer news, once the conversation has been seen.
pub fn withdraw(task_id: &str) {
    UNUserNotificationCenter::currentNotificationCenter()
        .removeDeliveredNotificationsWithIdentifiers(&NSArray::from_retained_slice(&[identifier(
            task_id,
        )]));
}
