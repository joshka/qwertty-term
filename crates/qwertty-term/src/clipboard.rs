//! System clipboard access via `NSPasteboard` (macOS).
//!
//! Read is used by the Paste menu action; write is available for OSC 52 /
//! copy-on-select (both deferred behaviors, but the plumbing is here). Kept tiny
//! and platform-gated so the non-macOS build (if ever) and the pure-logic tests
//! don't pull AppKit.

#![cfg(target_os = "macos")]

use objc2_app_kit::{NSPasteboard, NSPasteboardTypeString};
use objc2_foundation::NSString;

/// Read the general pasteboard's string contents, if any.
pub fn read() -> Option<String> {
    let pb = NSPasteboard::generalPasteboard();
    let s = unsafe { pb.stringForType(NSPasteboardTypeString) }?;
    Some(s.to_string())
}

/// Write `text` to the general pasteboard, replacing its contents. Returns
/// whether the write succeeded.
pub fn write(text: &str) -> bool {
    let pb = NSPasteboard::generalPasteboard();
    unsafe {
        pb.clearContents();
        pb.setString_forType(&NSString::from_str(text), NSPasteboardTypeString)
    }
}
