// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;
use mister_magik_fb::framebuffer::ownership::{FramebufferRouteAction, FramebufferRouteGuard};
use std::io;

pub(in crate::ui_runner) trait LauncherDisplayHardware {
    fn enable_launcher_route(
        &mut self,
        route: LauncherFramebufferRoute,
        fb_width: usize,
        fb_height: usize,
    ) -> io::Result<u16>;
}

impl LauncherDisplayHardware for Fpga {
    fn enable_launcher_route(
        &mut self,
        route: LauncherFramebufferRoute,
        fb_width: usize,
        fb_height: usize,
    ) -> io::Result<u16> {
        Fpga::enable_launcher_framebuffer_route(self, route, fb_width, fb_height)
    }
}

pub struct LauncherDisplaySession {
    route: LauncherFramebufferRoute,
    fb_width: usize,
    fb_height: usize,
    route_guard: FramebufferRouteGuard,
    reassert_count: u64,
    last_reassert_frame: u64,
    last_reassert_ok: bool,
    last_reassert_error: String,
}

impl LauncherDisplaySession {
    pub fn new(ui: &UiDisplay) -> Self {
        Self::with_guard(ui, FramebufferRouteGuard::from_env())
    }

    pub(in crate::ui_runner) fn with_guard(
        ui: &UiDisplay,
        route_guard: FramebufferRouteGuard,
    ) -> Self {
        Self {
            route: LauncherFramebufferRoute::for_scan(ui.scan_w(), ui.scan_h(), ui.direct_video()),
            fb_width: ui.fb_w(),
            fb_height: ui.fb_h(),
            route_guard,
            reassert_count: 0,
            last_reassert_frame: 0,
            last_reassert_ok: false,
            last_reassert_error: String::new(),
        }
    }

    pub fn route(&self) -> LauncherFramebufferRoute {
        self.route
    }

    pub fn enable_initial(&mut self, hardware: &mut Fpga) -> io::Result<u16> {
        self.enable_route(hardware)
    }

    pub fn enable_boot_settle(&mut self, hardware: &mut Fpga) -> io::Result<u16> {
        self.enable_route(hardware)
    }

    fn enable_route(&self, hardware: &mut impl LauncherDisplayHardware) -> io::Result<u16> {
        hardware.enable_launcher_route(self.route, self.fb_width, self.fb_height)
    }

    pub(super) fn begin_frame(
        &mut self,
        frame: u64,
        launching: bool,
        hardware: &mut Fpga,
    ) -> FramebufferRouteAction {
        if launching {
            return FramebufferRouteAction {
                reassert_route: false,
                force_full_present: false,
            };
        }
        self.begin_frame_with_hardware(frame, hardware)
    }

    fn begin_frame_with_hardware(
        &mut self,
        frame: u64,
        hardware: &mut impl LauncherDisplayHardware,
    ) -> FramebufferRouteAction {
        let mut action = self.route_guard.tick(frame);
        if !action.reassert_route {
            return action;
        }

        self.reassert_count = self.reassert_count.saturating_add(1);
        self.last_reassert_frame = frame;
        match self.enable_route(hardware) {
            Ok(flag) => {
                self.last_reassert_ok = true;
                self.last_reassert_error.clear();
                boot_analytics::event(
                    "launcher_fb_route_reasserted",
                    format!("frame={frame} support_flag={flag}"),
                );
            }
            Err(e) => {
                crate::ui_errln!("failed to reassert Slint framebuffer route: {e}");
                action.force_full_present = false;
                self.last_reassert_ok = false;
                self.last_reassert_error = e.to_string();
                boot_analytics::event(
                    "launcher_fb_route_reassert_failed",
                    format!("frame={frame} error={e}"),
                );
            }
        }
        action
    }

    pub(super) fn should_present_full_frame(
        &self,
        launching: bool,
        action: FramebufferRouteAction,
    ) -> bool {
        launching || action.force_full_present
    }

    pub(in crate::ui_runner) fn activate_fb0_route_with_hardware(
        &mut self,
        hardware: &mut impl LauncherDisplayHardware,
    ) -> io::Result<u16> {
        self.enable_route(hardware)
    }

    pub(super) fn recover_after_launch_failure(
        &mut self,
        frame: u64,
        hardware: &mut Fpga,
    ) -> io::Result<u16> {
        self.recover_after_launch_failure_with_hardware(frame, hardware)
    }

    fn recover_after_launch_failure_with_hardware(
        &mut self,
        frame: u64,
        hardware: &mut impl LauncherDisplayHardware,
    ) -> io::Result<u16> {
        self.reassert_count = self.reassert_count.saturating_add(1);
        self.last_reassert_frame = frame;
        match self.enable_route(hardware) {
            Ok(flag) => {
                self.last_reassert_ok = true;
                self.last_reassert_error.clear();
                boot_analytics::event(
                    "launcher_fb_route_recovered",
                    format!("frame={frame} support_flag={flag}"),
                );
                Ok(flag)
            }
            Err(e) => {
                self.last_reassert_ok = false;
                self.last_reassert_error = e.to_string();
                boot_analytics::event(
                    "launcher_fb_route_recovery_failed",
                    format!("frame={frame} error={e}"),
                );
                Err(e)
            }
        }
    }

    pub(super) fn route_ok(&self) -> bool {
        self.last_reassert_error.is_empty()
    }

    pub(super) fn reassert_count(&self) -> u64 {
        self.reassert_count
    }

    pub(super) fn last_reassert_frame(&self) -> u64 {
        self.last_reassert_frame
    }

    pub(super) fn last_reassert_ok(&self) -> bool {
        self.last_reassert_ok
    }

    pub(super) fn last_reassert_error(&self) -> &str {
        &self.last_reassert_error
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeHardware {
        enable_results: Vec<io::Result<u16>>,
        enable_calls: usize,
        last_enable_args: Option<(u16, u16, bool, usize, usize)>,
    }

    impl LauncherDisplayHardware for FakeHardware {
        fn enable_launcher_route(
            &mut self,
            route: LauncherFramebufferRoute,
            fb_width: usize,
            fb_height: usize,
        ) -> io::Result<u16> {
            self.enable_calls += 1;
            self.last_enable_args = Some((
                route.mode().hact,
                route.mode().vact,
                route.direct_video(),
                fb_width,
                fb_height,
            ));
            if self.enable_results.is_empty() {
                Ok(1)
            } else {
                self.enable_results.remove(0)
            }
        }
    }

    fn session_with_direct_video(
        interval_frames: u64,
        direct_video: bool,
    ) -> LauncherDisplaySession {
        let ini = format!(
            "[Menu]\nvideo_mode=8\n[MiSTer]\ndirect_video={}\n",
            u8::from(direct_video)
        );
        let plan = UiDisplayPlan::from_mister_ini_text(&ini).expect("display plan");
        let ui = UiDisplay::for_plan(plan);
        LauncherDisplaySession::with_guard(&ui, FramebufferRouteGuard::new(interval_frames))
    }

    fn session(interval_frames: u64) -> LauncherDisplaySession {
        session_with_direct_video(interval_frames, true)
    }

    #[test]
    fn successful_reassert_forces_one_full_present_and_records_status() {
        let mut session = session(60);
        let mut hardware = FakeHardware::default();

        let action = session.begin_frame_with_hardware(0, &mut hardware);

        assert!(action.reassert_route);
        assert!(action.force_full_present);
        assert_eq!(hardware.enable_calls, 1);
        assert_eq!(session.reassert_count(), 1);
        assert_eq!(session.last_reassert_frame(), 0);
        assert!(session.last_reassert_ok());
        assert!(session.route_ok());
    }

    #[test]
    fn failed_reassert_suppresses_full_present_and_records_route_loss() {
        let mut session = session(60);
        let mut hardware = FakeHardware {
            enable_results: vec![Err(io::Error::other("route failed"))],
            ..FakeHardware::default()
        };

        let action = session.begin_frame_with_hardware(0, &mut hardware);

        assert!(action.reassert_route);
        assert!(!action.force_full_present);
        assert!(!session.last_reassert_ok());
        assert!(!session.route_ok());
        assert_eq!(session.last_reassert_error(), "route failed");
    }

    #[test]
    fn fb0_fallback_reenables_only_the_framebuffer_geometry() {
        let mut session = session(0);
        let mut hardware = FakeHardware::default();

        let support = session
            .activate_fb0_route_with_hardware(&mut hardware)
            .unwrap();

        assert_eq!(support, 1);
        assert_eq!(hardware.enable_calls, 1);
        assert_eq!(
            hardware.last_enable_args,
            Some((
                session.route.mode().hact,
                session.route.mode().vact,
                session.route.direct_video(),
                session.fb_width,
                session.fb_height,
            ))
        );
    }

    #[test]
    fn launch_failure_recovery_reasserts_only_framebuffer_geometry() {
        let mut session = session(0);
        let mut hardware = FakeHardware::default();

        let flag = session
            .recover_after_launch_failure_with_hardware(42, &mut hardware)
            .unwrap();

        assert_eq!(flag, 1);
        assert_eq!(hardware.enable_calls, 1);
        assert_eq!(session.reassert_count(), 1);
        assert_eq!(session.last_reassert_frame(), 42);
        assert!(session.route_ok());
    }

    #[test]
    fn route_enable_uses_session_scan_and_framebuffer_geometry() {
        let session = session(0);
        let mut hardware = FakeHardware::default();

        session.enable_route(&mut hardware).unwrap();

        assert_eq!(
            hardware.last_enable_args,
            Some((
                session.route.mode().hact,
                session.route.mode().vact,
                session.route.direct_video(),
                session.fb_width,
                session.fb_height,
            ))
        );
    }

    #[test]
    fn non_direct_video_route_retains_its_geometry_semantics() {
        let session = session_with_direct_video(0, false);
        let mut hardware = FakeHardware::default();

        session.enable_route(&mut hardware).unwrap();

        assert_eq!(
            hardware.last_enable_args,
            Some((
                session.route.mode().hact,
                session.route.mode().vact,
                false,
                session.fb_width,
                session.fb_height,
            ))
        );
    }
}
