//! Native window shape: the panel is clipped to a continuous ("squircle") rounded
//! rectangle, so macOS draws its shadow and edge around that shape instead of
//! around the transparent rectangular window.
use objc2::{msg_send, runtime::AnyObject};
use objc2_foundation::NSString;

pub const CORNER_RADIUS: f64 = 18.0;

pub fn round_window(window: &tauri::WebviewWindow) {
    let Ok(ns_window) = window.ns_window() else {
        return;
    };
    let ns_window = ns_window.cast::<AnyObject>();
    if ns_window.is_null() {
        return;
    }
    // SAFETY: called on the main thread with Tauri's live NSWindow; every message
    // is a standard AppKit / Core Animation selector.
    unsafe {
        let view: *mut AnyObject = msg_send![ns_window, contentView];
        if view.is_null() {
            return;
        }
        let () = msg_send![view, setWantsLayer: true];
        let layer: *mut AnyObject = msg_send![view, layer];
        if !layer.is_null() {
            let () = msg_send![layer, setCornerRadius: CORNER_RADIUS];
            let curve = NSString::from_str("continuous");
            let () = msg_send![layer, setCornerCurve: &*curve];
            let () = msg_send![layer, setMasksToBounds: true];
        }
        let () = msg_send![ns_window, setHasShadow: true];
        let () = msg_send![ns_window, invalidateShadow];
    }
}
