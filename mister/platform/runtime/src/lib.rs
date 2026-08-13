// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! MiSTer-specific adapters kept outside the application and portable domain.

#[macro_export]
macro_rules! ui_errln {
    ($($arg:tt)*) => {{ eprintln!($($arg)*); }};
}

#[macro_export]
macro_rules! ui_logln {
    ($($arg:tt)*) => {{ println!($($arg)*); }};
}

pub mod boot_analytics;
pub mod display_plan;
pub mod display_resolution;
pub mod fpga;
pub mod framebuffer;
#[cfg(all(feature = "framebuffer-lab", target_os = "linux"))]
pub mod lab_input;
pub mod latch_readiness;
#[cfg(feature = "app-runtime")]
pub mod main_command;
#[cfg(feature = "app-runtime")]
pub mod runtime_status;
#[cfg(feature = "app-runtime")]
pub mod settings;
pub mod vt;

use mister_magik_core::platform::{
    DisplayOutcome, InputEvent, LaunchOutcome, LaunchRequest, MagikPlatform, PlatformError,
    Settings,
};

/// Low-level MiSTer capabilities used by the domain-level platform adapter.
/// File descriptors, ioctls, FPGA addresses, and Main command strings remain
/// private to implementations of this interface.
pub trait MisterRuntimeBackend {
    fn next_input(&mut self) -> Result<Option<InputEvent>, PlatformError>;
    fn present(&mut self) -> Result<DisplayOutcome, PlatformError>;
    fn load_settings(&self) -> Result<Settings, PlatformError>;
    fn save_settings(&mut self, settings: &Settings) -> Result<(), PlatformError>;
    fn handoff_launch(&mut self, request: LaunchRequest) -> Result<LaunchOutcome, PlatformError>;
}

pub struct MisterRuntime<B> {
    backend: B,
}

impl<B> MisterRuntime<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn into_backend(self) -> B {
        self.backend
    }
}

impl<B: MisterRuntimeBackend> MagikPlatform for MisterRuntime<B> {
    fn next_input(&mut self) -> Result<Option<InputEvent>, PlatformError> {
        self.backend.next_input()
    }

    fn present(&mut self) -> Result<DisplayOutcome, PlatformError> {
        self.backend.present()
    }

    fn load_settings(&self) -> Result<Settings, PlatformError> {
        self.backend.load_settings()
    }

    fn save_settings(&mut self, settings: &Settings) -> Result<(), PlatformError> {
        self.backend.save_settings(settings)
    }

    fn launch(&mut self, request: LaunchRequest) -> Result<LaunchOutcome, PlatformError> {
        self.backend.handoff_launch(request)
    }
}
