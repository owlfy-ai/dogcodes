//! OS consent belongs to the desktop application, never an arbitrary helper.
#[cfg(target_os = "macos")]
pub fn permissions(prompt: bool) -> serde_json::Value {
    use core_foundation::{
        base::TCFType, boolean::CFBoolean, dictionary::CFDictionary, string::CFString,
    };
    use std::ffi::c_void;
    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn AXIsProcessTrusted() -> bool;
        fn AXIsProcessTrustedWithOptions(options: *const c_void) -> bool;
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }
    let options = CFDictionary::from_CFType_pairs(&[(
        CFString::new("AXTrustedCheckOptionPrompt"),
        CFBoolean::true_value(),
    )]);
    // SAFETY: platform APIs have no borrowed outputs. The live CF dictionary
    // holds the documented boolean prompt option for the synchronous call.
    let (accessibility, screen) = unsafe {
        (
            if prompt {
                AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef().cast())
            } else {
                AXIsProcessTrusted()
            },
            if prompt {
                CGRequestScreenCaptureAccess()
            } else {
                CGPreflightScreenCaptureAccess()
            },
        )
    };
    serde_json::json!({"required":true,"accessibility":accessibility,"screenRecording":screen})
}

#[cfg(not(target_os = "macos"))]
pub fn permissions(_: bool) -> serde_json::Value {
    serde_json::json!({"required":false,"accessibility":true,"screenRecording":true})
}
