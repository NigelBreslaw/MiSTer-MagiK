// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Domain-level platform seam shared by device and future mobile applications.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputEvent {
    NavigateUp,
    NavigateDown,
    NavigateLeft,
    NavigateRight,
    Accept,
    Back,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DisplayOutcome {
    Presented,
    Skipped,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Settings {
    pub simple_joystick_handling: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchRequest {
    pub launch_ref: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    HandedOff,
    Rejected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlatformError {
    message: String,
}

impl PlatformError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    pub fn message(&self) -> &str {
        &self.message
    }
}

pub trait MagikPlatform {
    fn next_input(&mut self) -> Result<Option<InputEvent>, PlatformError>;
    fn present(&mut self) -> Result<DisplayOutcome, PlatformError>;
    fn load_settings(&self) -> Result<Settings, PlatformError>;
    fn save_settings(&mut self, settings: &Settings) -> Result<(), PlatformError>;
    fn launch(&mut self, request: LaunchRequest) -> Result<LaunchOutcome, PlatformError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    #[derive(Default)]
    struct FakePlatform {
        inputs: VecDeque<InputEvent>,
        settings: Settings,
        presented: usize,
        launches: Vec<LaunchRequest>,
        reject_launch: bool,
        fail_present: bool,
    }

    impl MagikPlatform for FakePlatform {
        fn next_input(&mut self) -> Result<Option<InputEvent>, PlatformError> {
            Ok(self.inputs.pop_front())
        }

        fn present(&mut self) -> Result<DisplayOutcome, PlatformError> {
            if self.fail_present {
                return Err(PlatformError::new("display unavailable"));
            }
            self.presented += 1;
            Ok(DisplayOutcome::Presented)
        }

        fn load_settings(&self) -> Result<Settings, PlatformError> {
            Ok(self.settings.clone())
        }

        fn save_settings(&mut self, settings: &Settings) -> Result<(), PlatformError> {
            self.settings = settings.clone();
            Ok(())
        }

        fn launch(&mut self, request: LaunchRequest) -> Result<LaunchOutcome, PlatformError> {
            self.launches.push(request);
            Ok(if self.reject_launch {
                LaunchOutcome::Rejected
            } else {
                LaunchOutcome::HandedOff
            })
        }
    }

    #[test]
    fn fake_platform_covers_input_settings_present_and_launch() {
        let mut platform = FakePlatform::default();
        platform.inputs.push_back(InputEvent::Accept);
        assert_eq!(platform.next_input().unwrap(), Some(InputEvent::Accept));

        let settings = Settings {
            simple_joystick_handling: true,
        };
        platform.save_settings(&settings).unwrap();
        assert_eq!(platform.load_settings().unwrap(), settings);
        assert_eq!(platform.present().unwrap(), DisplayOutcome::Presented);

        let request = LaunchRequest {
            launch_ref: "/media/fat/game.mgl".into(),
        };
        assert_eq!(
            platform.launch(request.clone()).unwrap(),
            LaunchOutcome::HandedOff
        );
        assert_eq!(platform.launches, vec![request]);
    }

    #[test]
    fn fake_platform_exposes_rejection_and_failure() {
        let mut platform = FakePlatform {
            reject_launch: true,
            fail_present: true,
            ..FakePlatform::default()
        };
        assert_eq!(
            platform
                .launch(LaunchRequest {
                    launch_ref: "unsupported".into(),
                })
                .unwrap(),
            LaunchOutcome::Rejected
        );
        assert_eq!(
            platform.present().unwrap_err().message(),
            "display unavailable"
        );
    }
}
