// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    Development,
    Public,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AutomationButton {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    Home,
    X,
    Y,
}

impl AutomationButton {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
            Self::Left => "left",
            Self::Right => "right",
            Self::A => "a",
            Self::B => "b",
            Self::Home => "home",
            Self::X => "x",
            Self::Y => "y",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AutomationAction {
    Tap(AutomationButton),
    Hold {
        button: AutomationButton,
        duration_ms: u64,
    },
    ReleaseAll,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AlphaCandidateHashes {
    pub platform_manifest: String,
    pub main: String,
    pub gui: String,
    pub manager: String,
    pub scanout_module: String,
    pub scanout_metadata: String,
    pub latch_rbf: String,
    pub latch_metadata: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DeviceFailure {
    Busy(String),
    AccessDenied(String),
    Unavailable(String),
    Authentication(String),
    InvalidRequest(String),
    ArtifactMismatch(String),
    Unhealthy(String),
    OperationFailed(String),
    RecoveryRequired(String),
}
