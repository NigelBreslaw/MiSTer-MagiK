// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::platform_lifecycle::{TitlebarAdapter, TitlebarController};
use objc2::{MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSToolbar, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
    NSWindowToolbarStyle,
};
use objc2_foundation::NSString;
use slint::winit_030::winit;
use winit::platform::macos::WindowAttributesExtMacOS;
use winit::window::WindowAttributes;

struct AppKitTitlebarAdapter<'a> {
    window: &'a NSWindow,
}

impl TitlebarAdapter for AppKitTitlebarAdapter<'_> {
    fn setup(&mut self) -> bool {
        let mask =
            NSWindowStyleMask(self.window.styleMask().0 | NSWindowStyleMask::FullSizeContentView.0);
        self.window.setStyleMask(mask);
        self.window.setTitlebarAppearsTransparent(true);
        self.window
            .setTitleVisibility(NSWindowTitleVisibility::Hidden);
        self.window.setMovable(false);

        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let identifier = NSString::from_str("MisterMagikDesktopToolbar");
        let toolbar = NSToolbar::initWithIdentifier(NSToolbar::alloc(mtm), &identifier);
        self.window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        self.window.setToolbar(Some(&toolbar));
        true
    }

    fn activate_benchmark(&mut self) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let application = NSApplication::sharedApplication(mtm);
        #[allow(deprecated)]
        application.activateIgnoringOtherApps(true);
        self.window.makeKeyAndOrderFront(None);
        self.window.orderFrontRegardless();
        true
    }
}

pub fn apply_unified_titlebar(attributes: WindowAttributes) -> WindowAttributes {
    attributes
        .with_fullsize_content_view(true)
        .with_titlebar_transparent(true)
        .with_title_hidden(true)
}

pub async fn setup_window(window: &slint::Window) -> Option<()> {
    use objc2_app_kit::NSView;
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

    let mut controller = TitlebarController::default();
    let mut adapter = AppKitTitlebarAdapter { window: &ns_window };
    controller.setup_once(&mut adapter).then_some(())
}

#[cfg_attr(not(feature = "compiled-ui"), allow(dead_code))]
pub fn activate_benchmark_window(window: &winit::window::Window) -> Option<()> {
    use objc2_app_kit::NSView;
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };

    // SAFETY: The AppKit view is owned by this live winit window. This helper
    // runs from Slint's UI event loop on the macOS main thread.
    let ns_view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
    let ns_window = ns_view.window()?;
    let mut controller = TitlebarController::default();
    let mut adapter = AppKitTitlebarAdapter { window: &ns_window };
    controller.activate_benchmark(&mut adapter).then_some(())
}
