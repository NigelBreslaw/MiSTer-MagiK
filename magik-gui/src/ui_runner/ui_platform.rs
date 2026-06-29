use super::*;

pub(crate) struct MisterPlatform {
    pub(crate) window: Rc<MinimalSoftwareWindow>,
    pub(crate) start: Instant,
    pub(crate) fixed_time: Option<Rc<Cell<Duration>>>,
}

#[derive(Clone)]
pub(crate) struct AnimationClock {
    fixed_time: Option<Rc<Cell<Duration>>>,
    fixed_step: Duration,
}

impl AnimationClock {
    pub(crate) fn from_env() -> Self {
        match std::env::var("MISTER_ANIMATION_CLOCK")
            .ok()
            .map(|s| s.to_ascii_lowercase().replace('_', "-"))
            .as_deref()
        {
            None | Some("") | Some("fixed60") | Some("fixed-60") | Some("frame")
            | Some("frame-clock") => Self {
                fixed_time: Some(Rc::new(Cell::new(Duration::ZERO))),
                fixed_step: Duration::from_nanos(16_666_667),
            },
            Some("wall") | Some("wall-clock") => Self {
                fixed_time: None,
                fixed_step: Duration::from_nanos(16_666_667),
            },
            other => {
                eprintln!("ui: unknown MISTER_ANIMATION_CLOCK={other:?}; use wall|fixed60");
                Self {
                    fixed_time: None,
                    fixed_step: Duration::from_nanos(16_666_667),
                }
            }
        }
    }

    pub(crate) fn platform_time(&self) -> Option<Rc<Cell<Duration>>> {
        self.fixed_time.clone()
    }

    #[cfg(any(mister_bench_scenes, mister_video_scene))]
    pub(super) fn label(&self) -> &'static str {
        if self.fixed_time.is_some() {
            "fixed60"
        } else {
            "wall"
        }
    }

    pub(super) fn advance(&self) {
        if let Some(t) = &self.fixed_time {
            t.set(t.get() + self.fixed_step);
        }
    }
}

pub(crate) fn update_slint_animations(animation_clock: &AnimationClock) {
    animation_clock.advance();
    slint::platform::update_timers_and_animations();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg(any(mister_bench_scenes, mister_video_scene))]
pub(super) enum FrameOrder {
    RenderThenVsync,
    VsyncThenRender,
}

#[cfg(any(mister_bench_scenes, mister_video_scene))]
impl FrameOrder {
    pub(super) fn from_env() -> Self {
        match std::env::var("MISTER_FRAME_ORDER")
            .ok()
            .map(|s| s.to_ascii_lowercase().replace('_', "-"))
            .as_deref()
        {
            None | Some("") | Some("render-then-vsync") | Some("render") => Self::RenderThenVsync,
            Some("vsync-then-render") | Some("vsync-first") | Some("vsync") => {
                Self::VsyncThenRender
            }
            other => {
                eprintln!(
                    "ui: unknown MISTER_FRAME_ORDER={other:?}; use render-then-vsync|vsync-first"
                );
                Self::RenderThenVsync
            }
        }
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::RenderThenVsync => "render-then-vsync",
            Self::VsyncThenRender => "vsync-first",
        }
    }
}

impl Platform for MisterPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, slint::PlatformError> {
        Ok(self.window.clone())
    }
    fn duration_since_start(&self) -> core::time::Duration {
        self.fixed_time
            .as_ref()
            .map(|t| t.get())
            .unwrap_or_else(|| self.start.elapsed())
    }
}
