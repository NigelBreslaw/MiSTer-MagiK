// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Deliberately small consumer application for Tooling 2.0.

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let probe = Probe::new()?;
    let weak = probe.as_weak();
    probe.on_increment(move || {
        if let Some(probe) = weak.upgrade() {
            probe.set_counter(probe.get_counter() + 1);
        }
    });
    let weak = probe.as_weak();
    probe.on_reset(move || {
        if let Some(probe) = weak.upgrade() {
            probe.set_counter(0);
        }
    });
    let weak = probe.as_weak();
    probe.on_toggle_details(move || {
        if let Some(probe) = weak.upgrade() {
            probe.set_details_open(!probe.get_details_open());
        }
    });
    probe.run()
}
