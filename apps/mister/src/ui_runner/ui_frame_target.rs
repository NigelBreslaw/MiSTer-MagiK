// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

pub(super) fn frame_target_geometry(ui: &UiDisplay) -> FramebufferTargetGeometry {
    FramebufferTargetGeometry::new(ui.render_w(), ui.render_h())
}

pub(super) fn launcher_dirty_opt_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        !matches!(
            std::env::var("MISTER_LAUNCHER_DIRTY_OPT").as_deref(),
            Ok("0") | Ok("off") | Ok("false") | Ok("no")
        )
    })
}

pub(super) fn preview_run_label() -> String {
    std::env::var("MISTER_PREVIEW_RUN_LABEL").unwrap_or_default()
}

pub(super) fn preview_direct_present_enabled() -> bool {
    static VALUE: OnceLock<bool> = OnceLock::new();
    *VALUE.get_or_init(|| {
        preview_direct_present_enabled_value(
            std::env::var("MISTER_PREVIEW_DIRECT_PRESENT")
                .ok()
                .as_deref(),
        )
    })
}

fn preview_direct_present_enabled_value(value: Option<&str>) -> bool {
    !matches!(
        value,
        Some("0" | "off" | "false" | "no" | "legacy" | "cached")
    )
}

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct PresentCopyStats {
    pub(super) rows: u32,
    pub(super) bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum CatalogRefreshPolicy {
    Default,
    Force,
    Off,
}

impl CatalogRefreshPolicy {
    pub(super) fn force_requested(self) -> bool {
        self == Self::Force
    }

    pub(super) fn worker_enabled(self) -> bool {
        self != Self::Off
    }

    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Force => "force",
            Self::Off => "off",
        }
    }
}

pub(super) fn catalog_refresh_policy() -> CatalogRefreshPolicy {
    static VALUE: OnceLock<CatalogRefreshPolicy> = OnceLock::new();
    *VALUE.get_or_init(|| {
        catalog_refresh_policy_from_value(std::env::var("MISTER_CATALOG_REFRESH").ok().as_deref())
    })
}

fn catalog_refresh_policy_from_value(value: Option<&str>) -> CatalogRefreshPolicy {
    match value {
        Some("1") | Some("on") | Some("true") | Some("yes") | Some("force") => {
            CatalogRefreshPolicy::Force
        }
        Some("0") | Some("off") | Some("false") | Some("no") | Some("load-only") => {
            CatalogRefreshPolicy::Off
        }
        _ => CatalogRefreshPolicy::Default,
    }
}

pub(super) fn forced_arcade_selected_index() -> Option<usize> {
    std::env::var("MISTER_ARCADE_SELECTED_INDEX")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
}

pub(super) fn apply_forced_arcade_selected(nav: &mut LauncherNav, catalog: &ArcadeCatalog) {
    apply_forced_arcade_selected_index(nav, catalog, forced_arcade_selected_index());
}

pub(super) fn apply_forced_arcade_selected_index(
    nav: &mut LauncherNav,
    catalog: &ArcadeCatalog,
    index: Option<usize>,
) {
    let Some(index) = index else {
        return;
    };
    let count = active_system_game_view(catalog, nav).len();
    if count == 0 {
        return;
    }
    nav.screen = Screen::Arcade;
    nav.arcade.selected = index.min(count - 1);
    nav.arcade.snap_to_selected();
    keep_bench_arcade_visible(&mut nav.arcade.scroll_y, nav.arcade.selected, count);
}

pub(super) trait UiFrameTargetPreviewExt {
    fn blit_raw_preview(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawFrame<'_>,
        clear_screen: bool,
    ) -> Option<DirtyRect>;

    fn blit_raw_preview_transition(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawTransitionFrame<'_>,
        effect: PreviewTransitionEffect,
        progress: f32,
    ) -> (DirtyRect, PreviewFadeTrace);

    fn blit_raw_preview_direct(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawFrame<'_>,
        clear_screen: bool,
    ) -> Option<DirtyRect>;

    fn blit_raw_preview_transition_direct(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawTransitionFrame<'_>,
        effect: PreviewTransitionEffect,
        progress: f32,
    ) -> (DirtyRect, PreviewFadeTrace);
}

impl UiFrameTargetPreviewExt for UiFrameTarget {
    fn blit_raw_preview(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawFrame<'_>,
        clear_screen: bool,
    ) -> Option<DirtyRect> {
        Raw565PreviewRenderer::compose_frame(self.cached_565_mut(), ui, frame, clear_screen)
    }

    fn blit_raw_preview_transition(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawTransitionFrame<'_>,
        effect: PreviewTransitionEffect,
        progress: f32,
    ) -> (DirtyRect, PreviewFadeTrace) {
        Raw565PreviewRenderer::compose_transition(
            self.cached_565_mut(),
            ui,
            frame,
            effect,
            progress,
        )
    }

    fn blit_raw_preview_direct(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawFrame<'_>,
        clear_screen: bool,
    ) -> Option<DirtyRect> {
        let screen = preview_screen_rect(ui);
        let (direct_preview, stride) = self.direct_preview_565_rect_mut(screen);
        Raw565PreviewRenderer::compose_frame_strided(
            direct_preview,
            ui,
            frame,
            clear_screen,
            PreviewSurface {
                x0: screen.x0,
                y0: screen.y0,
                stride,
            },
        )
    }

    fn blit_raw_preview_transition_direct(
        &mut self,
        ui: &UiDisplay,
        frame: &PreviewRawTransitionFrame<'_>,
        effect: PreviewTransitionEffect,
        progress: f32,
    ) -> (DirtyRect, PreviewFadeTrace) {
        let screen = preview_screen_rect(ui);
        let (direct_preview, stride) = self.direct_preview_565_rect_mut(screen);
        Raw565PreviewRenderer::compose_transition_strided(
            direct_preview,
            ui,
            frame,
            effect,
            progress,
            PreviewSurface {
                x0: screen.x0,
                y0: screen.y0,
                stride,
            },
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum RawPreviewPresent {
    Cached(DirtyRect),
    Direct(DirtyRect),
}

impl RawPreviewPresent {
    pub(super) fn cached_rect(self) -> Option<DirtyRect> {
        match self {
            Self::Cached(rect) => Some(rect),
            Self::Direct(_) => None,
        }
    }

    pub(super) fn direct_rect(self) -> Option<DirtyRect> {
        match self {
            Self::Cached(_) => None,
            Self::Direct(rect) => Some(rect),
        }
    }
}

pub(super) fn blit_raw_preview_if_needed(
    target: &mut UiFrameTarget,
    ui: &UiDisplay,
    preview: &mut PreviewState,
    transition: &mut PreviewTransitionDemo,
    elapsed: Duration,
    slint_dirty: Option<DirtyRect>,
    full_frame_present: bool,
    allow_direct: bool,
) -> (Option<RawPreviewPresent>, PreviewTransitionTrace) {
    let raw_dirty = preview.take_raw_dirty();
    // Full-frame Slint presents overwrite direct preview pixels, so they count
    // as preview damage even when the preview frame itself is otherwise idle.
    let preview_dirty = preview_dirty_for_present(ui, slint_dirty, full_frame_present);
    let slint_touched_preview = preview_dirty
        .and_then(|rect| rect.intersection(preview_screen_rect(ui)))
        .is_some();
    let transition_frame = preview.raw_transition_frame();
    let mut trace = transition.update(transition_frame.as_ref(), elapsed);
    if !raw_dirty
        && !slint_touched_preview
        && !trace.active
        && !preview.presentation_requires_present()
    {
        return (None, trace);
    }
    let Some(transition_frame) = transition_frame else {
        return (None, trace);
    };
    let direct_present = allow_direct && preview_direct_present_enabled();
    let raw_rect = if trace.active {
        let (raw_rect, fade) = if direct_present {
            target.blit_raw_preview_transition_direct(
                ui,
                &transition_frame,
                trace.effect,
                trace.progress,
            )
        } else {
            target.blit_raw_preview_transition(ui, &transition_frame, trace.effect, trace.progress)
        };
        trace.fade = fade;
        raw_rect
    } else {
        let raw_rect = if direct_present {
            target.blit_raw_preview_direct(ui, &transition_frame.current, raw_dirty)
        } else {
            target.blit_raw_preview(ui, &transition_frame.current, raw_dirty)
        };
        let Some(raw_rect) = raw_rect else {
            return (None, trace);
        };
        raw_rect
    };
    if direct_present {
        (Some(RawPreviewPresent::Direct(raw_rect)), trace)
    } else if preview_dirty.is_some_and(|rect| rect.contains(raw_rect)) {
        (None, trace)
    } else {
        (Some(RawPreviewPresent::Cached(raw_rect)), trace)
    }
}

fn preview_dirty_for_present(
    ui: &UiDisplay,
    slint_dirty: Option<DirtyRect>,
    full_frame_present: bool,
) -> Option<DirtyRect> {
    if full_frame_present {
        Some(DirtyRect {
            x0: 0,
            y0: 0,
            x1: ui.render_w(),
            y1: ui.render_h(),
        })
    } else {
        slint_dirty
    }
}

pub(super) fn copy_arcade_list_update(
    disp: &mut MappedRgb565Framebuffer,
    renderer: &mut ArcadeListRenderer,
    update: ArcadeListUpdate,
) -> PresentCopyStats {
    match update {
        ArcadeListUpdate::Full(rect) => {
            renderer.copy_layer_to_fb0(disp, true);
            PresentCopyStats {
                rows: rect.rows(),
                bytes: renderer.present_pixels(&update, true)
                    * mister_magik_fb::framebuffer::format::RGB565_BYTES_PER_PIXEL,
            }
        }
        ArcadeListUpdate::Scroll { rect, .. } => {
            // `Scroll` means the renderer reused its cached RAM surface. A
            // prior live-framebuffer scroll-present path was visually correct
            // but roughly doubled present cost because `/dev/fb0` reads are
            // expensive on the MiSTer write-combined framebuffer.
            renderer.copy_layer_to_fb0(disp, false);
            PresentCopyStats {
                rows: rect.rows(),
                bytes: renderer.present_pixels(&update, false)
                    * mister_magik_fb::framebuffer::format::RGB565_BYTES_PER_PIXEL,
            }
        }
    }
}

pub(super) fn compose_arcade_list_update(
    target: &mut UiFrameTarget,
    renderer: &mut ArcadeListRenderer,
    update: ArcadeListUpdate,
) -> PresentCopyStats {
    match update {
        ArcadeListUpdate::Full(rect) => {
            renderer.compose_layer_to_cached(target, true);
            PresentCopyStats {
                rows: rect.rows(),
                bytes: renderer.present_pixels(&update, true)
                    * mister_magik_fb::framebuffer::format::RGB565_BYTES_PER_PIXEL,
            }
        }
        ArcadeListUpdate::Scroll { rect, .. } => {
            renderer.compose_layer_to_cached(target, false);
            PresentCopyStats {
                rows: rect.rows(),
                bytes: renderer.present_pixels(&update, false)
                    * mister_magik_fb::framebuffer::format::RGB565_BYTES_PER_PIXEL,
            }
        }
    }
}

pub(super) fn copy_arcade_list_update_to_hidden(
    hidden: &mut ScanoutSlotsRgb565Framebuffer,
    renderer: &mut ArcadeListRenderer,
    update: ArcadeListUpdate,
) -> PresentCopyStats {
    match update {
        ArcadeListUpdate::Full(rect) => {
            renderer.copy_layer_to_hidden(hidden, true);
            PresentCopyStats {
                rows: rect.rows(),
                bytes: renderer.present_pixels(&update, true)
                    * mister_magik_fb::framebuffer::format::RGB565_BYTES_PER_PIXEL,
            }
        }
        ArcadeListUpdate::Scroll { rect, .. } => {
            renderer.copy_layer_to_hidden(hidden, false);
            PresentCopyStats {
                rows: rect.rows(),
                bytes: renderer.present_pixels(&update, false)
                    * mister_magik_fb::framebuffer::format::RGB565_BYTES_PER_PIXEL,
            }
        }
    }
}

pub(super) fn arcade_update_dirty_rect(update: &ArcadeListUpdate) -> DirtyRect {
    match update {
        ArcadeListUpdate::Full(rect) => *rect,
        ArcadeListUpdate::Scroll { rect, .. } => *rect,
    }
}

pub(super) fn arcade_list_needs_forced_redraw(
    renderer: &ArcadeListRenderer,
    slint_dirty: Option<DirtyRect>,
    full_frame_present: bool,
) -> bool {
    full_frame_present
        || slint_dirty.is_some_and(|rect| rect.intersection(renderer.dirty_rect()).is_some())
}

pub(super) fn frame_rect(rect: DirtyRect) -> FrameRect {
    FrameRect {
        x0: rect.x0 as u32,
        y0: rect.y0 as u32,
        x1: rect.x1 as u32,
        y1: rect.y1 as u32,
    }
}

pub(super) fn configure_window(ui: &UiDisplay, window: &Rc<MisterSoftwareWindow>) {
    configure_window_layout(
        &UiLayoutGeometry::for_display(ui, ScreenOrientation::Normal),
        window,
    );
}

pub(super) fn configure_window_layout(
    layout: &UiLayoutGeometry,
    window: &Rc<MisterSoftwareWindow>,
) {
    window.set_size(PhysicalSize::new(
        layout.logical_w() as u32,
        layout.logical_h() as u32,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui_display::{UI_FB_H, UI_FB_W};

    fn rect(x0: usize, y0: usize, x1: usize, y1: usize) -> DirtyRect {
        DirtyRect { x0, y0, x1, y1 }
    }

    #[test]
    fn catalog_refresh_policy_parses_force_off_and_default() {
        assert_eq!(
            catalog_refresh_policy_from_value(None),
            CatalogRefreshPolicy::Default
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("on")),
            CatalogRefreshPolicy::Force
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("force")),
            CatalogRefreshPolicy::Force
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("off")),
            CatalogRefreshPolicy::Off
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("load-only")),
            CatalogRefreshPolicy::Off
        );
        assert_eq!(
            catalog_refresh_policy_from_value(Some("later")),
            CatalogRefreshPolicy::Default
        );
    }

    #[test]
    fn preview_direct_present_defaults_on_with_escape_hatch() {
        assert!(preview_direct_present_enabled_value(None));
        assert!(preview_direct_present_enabled_value(Some("1")));
        assert!(preview_direct_present_enabled_value(Some("on")));
        assert!(!preview_direct_present_enabled_value(Some("0")));
        assert!(!preview_direct_present_enabled_value(Some("off")));
        assert!(!preview_direct_present_enabled_value(Some("cached")));
    }

    #[test]
    fn arcade_list_overlay_redraws_when_full_frame_present_overwrites_stationary_text() {
        let renderer = ArcadeListRenderer::new();
        assert!(arcade_list_needs_forced_redraw(&renderer, None, true));
    }

    #[test]
    fn arcade_list_overlay_redraws_when_slint_dirty_touches_list() {
        let renderer = ArcadeListRenderer::new();
        let rect = renderer.dirty_rect();

        assert!(arcade_list_needs_forced_redraw(
            &renderer,
            Some(rect),
            false
        ));
    }

    #[test]
    fn arcade_list_overlay_stays_idle_for_unrelated_slint_dirty_rect() {
        let rect = DirtyRect {
            x0: ARCADE_LIST_X + ARCADE_LIST_W + 1,
            y0: ARCADE_LIST_Y,
            x1: ARCADE_LIST_X + ARCADE_LIST_W + 20,
            y1: ARCADE_LIST_Y + 20,
        };

        let renderer = ArcadeListRenderer::new();
        assert!(!arcade_list_needs_forced_redraw(
            &renderer,
            Some(rect),
            false
        ));
    }

    #[test]
    fn empty_raw_preview_blit_clears_preview_screen() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let mut target = UiFrameTarget::cached(frame_target_geometry(&ui));
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Empty,
            source_w: 1,
            source_h: 1,
            display_w: ARCADE_PREVIEW_BOX_W,
            display_h: ARCADE_PREVIEW_BOX_H,
        };

        target
            .cached_565_mut()
            .fill(<Rgb565Pixel as TargetPixel>::from_rgb(0, 255, 0));

        let rect = target
            .blit_raw_preview(&ui, &frame, true)
            .expect("empty preview rect");
        let screen = preview_screen_rect(&ui);

        assert_eq!(rect.x0, screen.x0);
        assert_eq!(rect.y0, screen.y0);
        assert_eq!(rect.x1, screen.x1);
        assert_eq!(rect.y1, screen.y1);
        let cached = target.into_cached_565();
        let center = ((screen.y0 + screen.y1) / 2) * ui.render_w() + (screen.x0 + screen.x1) / 2;
        assert_eq!(cached[center].0, 0);
    }

    #[test]
    fn direct_preview_uses_preview_rect_backing() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let mut target = UiFrameTarget::cached(frame_target_geometry(&ui));
        let screen = preview_screen_rect(&ui);
        let pixel = <Rgb565Pixel as TargetPixel>::from_rgb(255, 0, 0);
        let pixels = vec![pixel; 4];
        let frame = PreviewRawFrame {
            pixels: PreviewRawPixels::Rgb565 {
                pixels: &pixels,
                stride_pixels: 2,
            },
            source_w: 2,
            source_h: 2,
            display_w: 2,
            display_h: 2,
        };

        let dirty = target
            .blit_raw_preview_direct(&ui, &frame, true)
            .expect("direct preview dirty");
        let (backing, stride) = target.direct_preview_565_rect_mut(screen);

        assert_eq!(stride, screen.width());
        assert_eq!(backing.len(), screen.width() * (screen.y1 - screen.y0));
        assert!(screen.contains(dirty));
    }

    #[test]
    fn direct_preview_repaints_after_full_frame_present() {
        let ui = UiDisplay::for_framebuffer(UI_FB_W, UI_FB_H);
        let screen = preview_screen_rect(&ui);
        let dirty = preview_dirty_for_present(&ui, None, true).expect("full frame dirty");

        assert!(dirty.contains(screen));
        assert!(preview_dirty_for_present(&ui, None, false).is_none());
    }
}
