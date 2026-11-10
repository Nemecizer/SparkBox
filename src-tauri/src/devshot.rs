//! Debug builds only: pictures of the window for Help, taken by WebKit itself. A window capture
//! needs the window painted on screen, so it comes out blank on a locked Mac or a covered window.
//! `tools/agents/docs-shots.sh` asks for a picture by writing the output path to
//! "$SPARKBOX_DEV_SCRIPT.shot". Nothing here is in a release build.
#![cfg(all(debug_assertions, target_os = "macos"))]

use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, Sel};
use objc2::{class, msg_send, sel};
use objc2_foundation::NSString;
use tauri::WebviewWindow;

/// A locked screen counts as covered, and WebKit stops drawing a covered page. This keeps it going,
/// so that the page's layout, timers and charts run as they do on screen.
pub fn keep_painting(window: &WebviewWindow) {
    let _ = window.with_webview(|view| unsafe {
        let web = view.inner() as *mut NSObject;
        let off: Sel = sel!(_setWindowOcclusionDetectionEnabled:);
        let known: Bool = msg_send![web, respondsToSelector: off];
        if known.as_bool() {
            let _: () = msg_send![web, _setWindowOcclusionDetectionEnabled: Bool::NO];
            // WebKit reads that only when the view's state changes, so change it.
            let _: () = msg_send![web, setHidden: Bool::YES];
            let _: () = msg_send![web, setHidden: Bool::NO];
        }
    });
}

/// Writes the page as `out` (a TIFF), then `out` + ".done". On failure, ".done" holds the reason.
pub fn snapshot(window: &WebviewWindow, out: String) {
    let _ = window.with_webview(move |view| unsafe {
        let web = view.inner() as *mut AnyObject;
        let config: Retained<AnyObject> = msg_send![class!(WKSnapshotConfiguration), new];
        let _: () = msg_send![&*config, setAfterScreenUpdates: Bool::NO];
        let path = out.clone();
        let done = RcBlock::new(move |image: *mut AnyObject, _error: *mut AnyObject| {
            let finished = format!("{path}.done");
            if image.is_null() {
                let _ = std::fs::write(&finished, "WebKit returned no picture");
                return;
            }
            let tiff: *mut AnyObject = msg_send![image, TIFFRepresentation];
            let target = NSString::from_str(&path);
            let saved: Bool = msg_send![tiff, writeToFile: &*target, atomically: Bool::YES];
            let _ = std::fs::write(&finished, if saved.as_bool() { "" } else { "couldn't write the picture" });
        });
        let _: () = msg_send![web, takeSnapshotWithConfiguration: &*config, completionHandler: &*done];
    });
}
