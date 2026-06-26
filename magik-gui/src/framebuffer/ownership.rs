use std::time::Duration;

// Keep periodic route checks out of the steady scroll hot path by default.
// Diagnostics can restore the old one-second cadence with
// MISTER_FB_ROUTE_REASSERT_FRAMES=60.
pub const DEFAULT_REASSERT_FRAMES: u64 = 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FramebufferRouteAction {
    pub reassert_route: bool,
    pub force_full_present: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct FramebufferRouteGuard {
    interval_frames: u64,
    next_frame: u64,
}

impl FramebufferRouteGuard {
    pub fn new(interval_frames: u64) -> Self {
        Self {
            interval_frames,
            next_frame: 0,
        }
    }

    pub fn from_env() -> Self {
        Self::new(reassert_interval_frames_from_env())
    }

    pub const fn disabled() -> Self {
        Self {
            interval_frames: 0,
            next_frame: u64::MAX,
        }
    }

    pub fn tick(&mut self, frame: u64) -> FramebufferRouteAction {
        if self.interval_frames == 0 || frame < self.next_frame {
            return FramebufferRouteAction {
                reassert_route: false,
                force_full_present: false,
            };
        }

        self.next_frame = frame.saturating_add(self.interval_frames.max(1));
        FramebufferRouteAction {
            reassert_route: true,
            force_full_present: true,
        }
    }
}

pub fn reassert_interval_frames_from_env() -> u64 {
    std::env::var("MISTER_FB_ROUTE_REASSERT_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(DEFAULT_REASSERT_FRAMES)
}

pub fn reassert_interval_duration(frames: u64, refresh_hz: u64) -> Option<Duration> {
    if frames == 0 || refresh_hz == 0 {
        return None;
    }
    Some(Duration::from_millis(
        frames.saturating_mul(1000) / refresh_hz,
    ))
}

pub fn should_present_full_frame(launching: bool, route_action: FramebufferRouteAction) -> bool {
    launching || route_action.force_full_present
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn guard_reasserts_on_first_frame() {
        let mut guard = FramebufferRouteGuard::new(60);

        assert_eq!(
            guard.tick(0),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
    }

    #[test]
    fn guard_waits_until_interval_elapses() {
        let mut guard = FramebufferRouteGuard::new(3);

        assert!(guard.tick(0).reassert_route);
        assert!(!guard.tick(1).reassert_route);
        assert!(!guard.tick(2).reassert_route);
        assert_eq!(
            guard.tick(3),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
    }

    #[test]
    fn periodic_route_reassertions_force_full_presents() {
        let mut guard = FramebufferRouteGuard::new(2);

        assert_eq!(
            guard.tick(0),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
        assert_eq!(
            guard.tick(2),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
        assert_eq!(
            guard.tick(4),
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        );
    }

    #[test]
    fn disabled_guard_never_reasserts() {
        let mut guard = FramebufferRouteGuard::disabled();

        for frame in [0, 1, 60, u64::MAX - 1] {
            assert_eq!(
                guard.tick(frame),
                FramebufferRouteAction {
                    reassert_route: false,
                    force_full_present: false
                }
            );
        }
    }

    #[test]
    fn interval_duration_handles_disabled_values() {
        assert_eq!(reassert_interval_duration(0, 60), None);
        assert_eq!(reassert_interval_duration(60, 0), None);
        assert_eq!(
            reassert_interval_duration(60, 60),
            Some(Duration::from_secs(1))
        );
    }

    #[test]
    fn full_frame_present_follows_launch_or_explicit_action() {
        assert!(should_present_full_frame(
            false,
            FramebufferRouteAction {
                reassert_route: true,
                force_full_present: true
            }
        ));
        assert!(should_present_full_frame(
            true,
            FramebufferRouteAction {
                reassert_route: false,
                force_full_present: false
            }
        ));
        assert!(!should_present_full_frame(
            false,
            FramebufferRouteAction {
                reassert_route: false,
                force_full_present: false
            }
        ));
    }
}
