// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[cfg(not(mister_ui_scope_launcher))]
pub(crate) use mister_magik_fb::visual_platform::FrameOrder;
pub(crate) use mister_magik_fb::visual_platform::{
    AnimationClock, MisterPlatform, MisterSoftwareWindow, PresentTiming, update_slint_animations,
};
