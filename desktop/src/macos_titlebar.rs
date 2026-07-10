use slint::winit_030::winit;
use winit::platform::macos::WindowAttributesExtMacOS;
use winit::window::WindowAttributes;

pub fn apply_unified_titlebar(attributes: WindowAttributes) -> WindowAttributes {
    attributes
        .with_fullsize_content_view(true)
        .with_titlebar_transparent(true)
        .with_title_hidden(true)
}

pub async fn setup_window(window: &slint::Window) -> Option<()> {
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{
        NSToolbar, NSView, NSWindowStyleMask, NSWindowTitleVisibility, NSWindowToolbarStyle,
    };
    use objc2_foundation::NSString;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use slint::winit_030::WinitWindowAccessor;

    let winit_window = window.winit_window().await.ok()?;
    let RawWindowHandle::AppKit(handle) = winit_window.window_handle().ok()?.as_raw() else {
        return None;
    };

    // SAFETY: The raw AppKit handle comes from winit for this live window and
    // remains valid while the winit window is alive on the UI thread.
    let ns_view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    let ns_window = ns_view.window()?;

    let mask =
        NSWindowStyleMask(ns_window.styleMask().0 | NSWindowStyleMask::FullSizeContentView.0);
    ns_window.setStyleMask(mask);
    ns_window.setTitlebarAppearsTransparent(true);
    ns_window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
    ns_window.setMovable(false);

    let mtm = MainThreadMarker::new()?;
    let identifier = NSString::from_str("MisterMagikDesktopToolbar");
    let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &identifier);
    ns_window.setToolbarStyle(NSWindowToolbarStyle::Unified);
    ns_window.setToolbar(Some(&toolbar));

    Some(())
}

#[cfg_attr(not(feature = "compiled-ui"), allow(dead_code))]
pub fn activate_benchmark_window(window: &winit::window::Window) -> Option<()> {
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSApplication, NSView};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };

    // SAFETY: The AppKit view is owned by this live winit window. This helper
    // runs from Slint's UI event loop on the macOS main thread.
    let ns_view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    let ns_window = ns_view.window()?;
    let mtm = MainThreadMarker::new()?;
    let application = NSApplication::sharedApplication(mtm);
    #[allow(deprecated)]
    application.activateIgnoringOtherApps(true);
    ns_window.makeKeyAndOrderFront(None);
    ns_window.orderFrontRegardless();
    Some(())
}
