// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::rc::Rc;

use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType, Rgb565Pixel};
use slint::platform::{Platform, WindowAdapter};
use slint::{ComponentHandle, ModelRc, PhysicalSize, SharedString, VecModel};

struct SnapshotPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for SnapshotPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.0.clone())
    }
}

fn main() {
    let output = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "licenses.ppm".into());
    let state = std::env::args().nth(2).unwrap_or_else(|| "list".into());
    let expanded = state == "expanded";
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(SnapshotPlatform(window.clone())))
        .expect("set snapshot platform");
    window.set_size(PhysicalSize::new(960, 540));

    let app = mister_magik_ui::launcher::Launcher::new().expect("create launcher");
    use mister_magik_ui::launcher::{
        InformationView, LauncherScreen, NavigationView, SettingsView,
    };
    let navigation = app.global::<NavigationView>();
    navigation.set_screen(match state.as_str() {
        "catalog-tiles" => LauncherScreen::Home,
        "settings" => LauncherScreen::Settings,
        "about" => LauncherScreen::About,
        "info" => LauncherScreen::Info,
        _ => LauncherScreen::Licenses,
    });
    navigation.set_build_label("Build 42 | 2026-07-12 12:00 UTC".into());
    navigation.set_present_mode_label("RGB565 /dev/fb0".into());
    let information = app.global::<InformationView>();
    information.set_build_label("Build 42 | 2026-07-12 12:00 UTC".into());
    information.set_present_mode_label("RGB565 /dev/fb0".into());
    information.set_database_build("1,284 ms (scan 1,107 ms, save 177 ms)".into());
    information.set_kernel_version("Kernel version detected at launcher startup".into());
    let settings = app.global::<SettingsView>();
    settings.set_selected_license_index(1);
    settings.set_license_expanded(expanded);
    if state == "catalog-tiles" {
        use mister_magik_ui::launcher::{
            MenuItem, MenuItemKind, MenuItemPresentation, MenuItemStatus,
        };
        navigation.set_menu_title("MiSTer MagiK".into());
        navigation.set_menu_breadcrumb("Systems".into());
        navigation.set_home_selected_index(0);
        let items = vec![
            MenuItem {
                id: "arcade".into(),
                label: "Arcade".into(),
                subtitle: "2,184 games".into(),
                available: true,
                node_kind: MenuItemKind::Collection,
                status: MenuItemStatus::Ready,
            },
            MenuItem {
                id: "snes".into(),
                label: "SNES".into(),
                subtitle: "Scanning…".into(),
                available: true,
                node_kind: MenuItemKind::Collection,
                status: MenuItemStatus::Scanning,
            },
            MenuItem {
                id: "c64".into(),
                label: "Commodore 64".into(),
                subtitle: "Scanning…".into(),
                available: true,
                node_kind: MenuItemKind::Collection,
                status: MenuItemStatus::Scanning,
            },
            MenuItem {
                id: "megadrive".into(),
                label: "Mega Drive".into(),
                subtitle: "Scan failed".into(),
                available: true,
                node_kind: MenuItemKind::Collection,
                status: MenuItemStatus::Failed,
            },
        ];
        navigation.set_menu_item_presentation(ModelRc::new(VecModel::from(
            items
                .iter()
                .enumerate()
                .map(|(index, _item)| MenuItemPresentation {
                    selected: index == 0,
                    acknowledged: false,
                })
                .collect::<Vec<_>>(),
        )));
        navigation.set_menu_items(ModelRc::new(VecModel::from(items)));
    }
    settings.set_license_lines(ModelRc::new(VecModel::from(
        [
            "FFmpeg 8.1.2",
            "",
            "MiSTer MagiK statically links selected FFmpeg libraries.",
            "",
            "GNU LESSER GENERAL PUBLIC LICENSE",
            "Version 2.1, February 1999",
            "",
            "This is representative legal text used to verify the virtualized line layout.",
        ]
        .map(SharedString::from)
        .to_vec(),
    )));
    app.show().expect("show launcher");
    window.request_redraw();

    let mut pixels = vec![Rgb565Pixel::default(); 960 * 540];
    window.draw_if_needed(|renderer| {
        renderer.render(&mut pixels, 960);
    });
    let mut ppm = format!("P6\n960 540\n255\n").into_bytes();
    for pixel in pixels {
        let value = pixel.0;
        let red = ((value >> 11) & 0x1f) as u8;
        let green = ((value >> 5) & 0x3f) as u8;
        let blue = (value & 0x1f) as u8;
        ppm.extend_from_slice(&[
            (red << 3) | (red >> 2),
            (green << 2) | (green >> 4),
            (blue << 3) | (blue >> 2),
        ]);
    }
    std::fs::write(output, ppm).expect("write PPM");
}
