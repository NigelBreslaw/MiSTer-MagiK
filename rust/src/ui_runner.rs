//! Shared vsync render loop and Slint bench scene dispatch.

use crate::fb::{Display, Pixel};
use crate::fpga::{Fpga, MODE_1080P60};
use crate::vt::VtGraphicsGuard;
use slint::platform::software_renderer::{MinimalSoftwareWindow, RepaintBufferType};
use slint::platform::{Platform, WindowAdapter};
use slint::{
    ComponentHandle, Image, ModelRc, PhysicalSize, Rgb8Pixel, SharedPixelBuffer, SharedString,
    VecModel,
};
use std::rc::Rc;
use std::time::{Duration, Instant};

mod slint_ui {
    #![allow(clippy::all, unused_imports)]
    pub mod app {
        include!(concat!(env!("OUT_DIR"), "/app.rs"));
    }
    pub mod full_motion {
        include!(concat!(env!("OUT_DIR"), "/full_motion.rs"));
    }
    pub mod static_ui {
        include!(concat!(env!("OUT_DIR"), "/static_ui.rs"));
    }
    pub mod local_motion {
        include!(concat!(env!("OUT_DIR"), "/local_motion.rs"));
    }
    pub mod text_heavy {
        include!(concat!(env!("OUT_DIR"), "/text_heavy.rs"));
    }
    pub mod solid_fill {
        include!(concat!(env!("OUT_DIR"), "/solid_fill.rs"));
    }
    pub mod list_scroll {
        include!(concat!(env!("OUT_DIR"), "/list_scroll.rs"));
    }
    pub mod console_scroll {
        include!(concat!(env!("OUT_DIR"), "/console_scroll.rs"));
    }
    pub mod dirty_band {
        include!(concat!(env!("OUT_DIR"), "/dirty_band.rs"));
    }
    #[cfg(feature = "video")]
    pub mod video_playback {
        include!(concat!(env!("OUT_DIR"), "/video_playback.rs"));
    }
    pub mod controller {
        include!(concat!(env!("OUT_DIR"), "/controller_test.rs"));
    }
    pub mod launcher {
        include!(concat!(env!("OUT_DIR"), "/launcher.rs"));
    }
}

use crate::arcade_catalog::{self, ArcadeCatalog, ArcadeGameEntry};
use crate::controller_db::ControllerDb;
use crate::cpu_profile;
use crate::frame_profile::{FrameProfiler, FrameSample};
use crate::input::{PadInfo, PadPool};
use crate::library_bench;
use crate::launcher::{self, LauncherAction, LauncherNav, Screen};
use crate::preview_worker::PreviewWorker;
use crate::setup_nav::{SetupAction, SetupNav, SetupPhase};
use crate::ui_display::{dirty_band_pct_from_env, UiDisplay, FB_H, FB_W, SLINT_UI_SCALE};
use slint::platform::software_renderer::PhysicalRegion;
use slint_ui::launcher::PreviewStatus;
use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc;

pub const UI_SCENES: &[&str] = &[
    "launcher",
    "demo",
    "controller_test",
    "full_motion",
    "static_ui",
    "local_motion",
    "text_heavy",
    "solid_fill",
    "list_scroll",
    "console_scroll",
    "dirty_band",
    #[cfg(feature = "video")]
    "video_playback",
];

struct MisterPlatform {
    window: Rc<MinimalSoftwareWindow>,
    start: Instant,
    fixed_time: Option<Rc<Cell<Duration>>>,
}

#[derive(Clone)]
struct AnimationClock {
    fixed_time: Option<Rc<Cell<Duration>>>,
    fixed_step: Duration,
}

impl AnimationClock {
    fn from_env() -> Self {
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

    fn platform_time(&self) -> Option<Rc<Cell<Duration>>> {
        self.fixed_time.clone()
    }

    fn label(&self) -> &'static str {
        if self.fixed_time.is_some() {
            "fixed60"
        } else {
            "wall"
        }
    }

    fn advance(&self) {
        if let Some(t) = &self.fixed_time {
            t.set(t.get() + self.fixed_step);
        }
    }
}

fn update_slint_animations(animation_clock: &AnimationClock) {
    animation_clock.advance();
    slint::platform::update_timers_and_animations();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrameOrder {
    RenderThenVsync,
    VsyncThenRender,
}

impl FrameOrder {
    fn from_env() -> Self {
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

    fn label(self) -> &'static str {
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

/// `ui [scene] [secs]` — scene defaults to `launcher`; secs defaults to 0 (infinite).
pub fn parse_ui_args() -> (String, u64) {
    let a2 = std::env::args().nth(2);
    let a3 = std::env::args().nth(3);
    match (a2.as_deref(), a3.as_deref()) {
        (Some(s), Some(t)) if t.parse::<u64>().is_ok() => (normalize_scene(s), t.parse().unwrap()),
        (Some(s), None) if s.parse::<u64>().is_ok() => ("launcher".into(), s.parse().unwrap()),
        (Some(s), Some(t)) => (normalize_scene(s), t.parse::<u64>().unwrap_or(0)),
        (Some(s), None) => (normalize_scene(s), 0),
        _ => ("launcher".into(), 0),
    }
}

fn normalize_scene(s: &str) -> String {
    if UI_SCENES.contains(&s) {
        s.to_string()
    } else {
        eprintln!("unknown scene '{s}' (use: {})", UI_SCENES.join(" | "));
        std::process::exit(2);
    }
}

pub fn print_scenes() {
    let ui = UiDisplay::from_env();
    println!(
        "Slint UI scenes (render {}x{}, fb {}x{}, ui-scale {}):",
        ui.render_w(),
        ui.render_h(),
        FB_W,
        FB_H,
        SLINT_UI_SCALE
    );
    for s in UI_SCENES {
        println!("  {s}");
    }
}

#[derive(Clone, Copy, Debug)]
struct DirtyRect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

impl DirtyRect {
    fn rows(self) -> u32 {
        (self.y1 - self.y0) as u32
    }

    fn is_broad(self, render_w: usize) -> bool {
        (self.x1 - self.x0) >= render_w * 3 / 4
    }
}

fn dirty_rect(region: &PhysicalRegion, render_w: usize, render_h: usize) -> Option<DirtyRect> {
    let o = region.bounding_box_origin();
    let s = region.bounding_box_size();
    if s.width == 0 || s.height == 0 {
        return None;
    }
    let x0 = o.x.max(0) as usize;
    let x1 = ((o.x + s.width as i32) as usize).min(render_w);
    let y0 = o.y.max(0) as usize;
    let y1 = ((o.y + s.height as i32) as usize).min(render_h);
    if x1 > x0 && y1 > y0 {
        Some(DirtyRect { x0, y0, x1, y1 })
    } else {
        None
    }
}

fn copy_cached_rows(disp: &mut Display, ui: &UiDisplay, cached: &[Pixel], y0: usize, y1: usize) {
    disp.copy_rows_scaled(ui.fb_scale(), cached, ui.render_w(), y0, y1);
}

fn copy_cached_rect(disp: &mut Display, ui: &UiDisplay, cached: &[Pixel], rect: DirtyRect) {
    if rect.is_broad(ui.render_w()) {
        copy_cached_rows(disp, ui, cached, rect.y0, rect.y1);
        return;
    }
    disp.copy_rect_scaled(
        ui.fb_scale(),
        cached,
        ui.render_w(),
        rect.x0,
        rect.y0,
        rect.x1,
        rect.y1,
    );
}

fn configure_window(ui: &UiDisplay, window: &Rc<MinimalSoftwareWindow>) {
    window.set_size(PhysicalSize::new(
        ui.render_w() as u32,
        ui.render_h() as u32,
    ));
}

pub fn run_ui(f: &mut Fpga) {
    let (scene, secs) = parse_ui_args();
    let ui = UiDisplay::from_env();
    println!("ui scene={scene} secs={secs}");
    println!("{}", ui.log_line());

    let _vt = VtGraphicsGuard::enter_or_warn();

    let mut disp = match Display::open_boot(FB_W, FB_H) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("failed to open display (/dev/fb0): {e}");
            std::process::exit(1);
        }
    };
    let flag = f.fb_enable_direct(0, FB_W as u16, FB_H as u16, MODE_1080P60, Some(0), Some(0));
    println!("fb routed (support_flag={flag}); Slint software renderer (vsync, dirty-row copy)");

    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    let animation_clock = AnimationClock::from_env();
    slint::platform::set_platform(Box::new(MisterPlatform {
        window: window.clone(),
        start: Instant::now(),
        fixed_time: animation_clock.platform_time(),
    }))
    .expect("set_platform");

    match scene.as_str() {
        "demo" => {
            let app = slint_ui::app::AppWindow::new().expect("AppWindow::new");
            app.global::<slint_ui::app::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            app.show().expect("show");
            run_frame_loop(secs, &ui, &mut disp, &window, &animation_clock);
        }
        "full_motion" => {
            let app = slint_ui::full_motion::FullMotion::new().expect("FullMotion::new");
            app.global::<slint_ui::full_motion::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            app.show().expect("show");
            run_frame_loop(secs, &ui, &mut disp, &window, &animation_clock);
        }
        "static_ui" => {
            let app = slint_ui::static_ui::StaticUi::new().expect("StaticUi::new");
            app.global::<slint_ui::static_ui::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            app.show().expect("show");
            run_frame_loop(secs, &ui, &mut disp, &window, &animation_clock);
        }
        "local_motion" => {
            let app = slint_ui::local_motion::LocalMotion::new().expect("LocalMotion::new");
            app.global::<slint_ui::local_motion::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            app.show().expect("show");
            run_frame_loop(secs, &ui, &mut disp, &window, &animation_clock);
        }
        "text_heavy" => {
            let app = slint_ui::text_heavy::TextHeavy::new().expect("TextHeavy::new");
            app.global::<slint_ui::text_heavy::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            app.show().expect("show");
            run_frame_loop(secs, &ui, &mut disp, &window, &animation_clock);
        }
        "solid_fill" => {
            let app = slint_ui::solid_fill::SolidFill::new().expect("SolidFill::new");
            app.global::<slint_ui::solid_fill::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            app.show().expect("show");
            run_frame_loop(secs, &ui, &mut disp, &window, &animation_clock);
        }
        "list_scroll" => {
            let app = slint_ui::list_scroll::ListScroll::new().expect("ListScroll::new");
            app.global::<slint_ui::list_scroll::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            app.show().expect("show");
            run_frame_loop(secs, &ui, &mut disp, &window, &animation_clock);
        }
        "console_scroll" => {
            let app = slint_ui::console_scroll::ConsoleScroll::new().expect("ConsoleScroll::new");
            app.global::<slint_ui::console_scroll::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            app.show().expect("show");
            run_console_scroll_loop(secs, &ui, &mut disp, &window, app, &animation_clock);
        }
        "dirty_band" => {
            let pct = dirty_band_pct_from_env();
            let app = slint_ui::dirty_band::DirtyBand::new().expect("DirtyBand::new");
            app.global::<slint_ui::dirty_band::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            app.set_band_pct(pct);
            println!("dirty_band band-pct={pct}% (MISTER_DIRTY_BAND_PCT)");
            configure_window(&ui, &window);
            app.show().expect("show");
            run_frame_loop(secs, &ui, &mut disp, &window, &animation_clock);
        }
        #[cfg(feature = "video")]
        "video_playback" => {
            let app =
                slint_ui::video_playback::VideoPlayback::new().expect("VideoPlayback::new");
            app.global::<slint_ui::video_playback::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            app.show().expect("show");
            window.request_redraw();
            run_video_playback_loop(secs, &ui, &mut disp, &window, app, &animation_clock);
        }
        "controller_test" => {
            let pad = open_pads();
            let app = slint_ui::controller::ControllerTest::new().expect("ControllerTest::new");
            app.global::<slint_ui::controller::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            sync_bridge(&app, &pad);
            app.show().expect("show");
            window.request_redraw();
            run_controller_loop(secs, &ui, &mut disp, &window, pad, app, &animation_clock);
        }
        "launcher" => {
            let pad = open_pads();
            let app = slint_ui::launcher::Launcher::new().expect("Launcher::new");
            app.global::<slint_ui::launcher::MisterUi>()
                .set_scale(SLINT_UI_SCALE);
            configure_window(&ui, &window);
            init_launcher_bridge(&app, &pad);
            app.show().expect("show");
            window.request_redraw();
            run_launcher_loop(secs, &ui, &mut disp, f, &window, pad, app, &animation_clock);
        }
        _ => unreachable!(),
    }
}

fn open_pads() -> PadPool {
    for attempt in 0..60 {
        match PadPool::open_all() {
            Ok(p) => {
                if attempt > 0 {
                    println!("gamepad open ok after {attempt} retries");
                }
                return p;
            }
            Err(e) => {
                if attempt == 0 || attempt % 10 == 0 {
                    eprintln!("gamepad open attempt {attempt}: {e}");
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        }
    }
    eprintln!("failed to open gamepad after 30s");
    std::process::exit(1);
}

fn init_launcher_bridge(app: &slint_ui::launcher::Launcher, pad: &PadPool) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_screen_mode(0);
    bridge.set_selected_index(0);
    bridge.set_settings_selected(0);
    bridge.set_confirm_visible(false);
    bridge.set_confirm_title("".into());
    bridge.set_confirm_message("".into());
    bridge.set_confirm_selected(0);
    bridge.set_arcade_selected(0);
    bridge.set_arcade_scroll_y(0);
    bridge.set_arcade_preview_has_image(false);
    bridge.set_arcade_preview_status(PreviewStatus::Empty);
    bridge.set_arcade_preview_title("".into());
    bridge.set_arcade_preview_image(Image::default());
    bridge.set_catalog_scan_visible(false);
    bridge.set_catalog_scan_title("".into());
    bridge.set_catalog_scan_detail("".into());
    bridge.set_setup_visible(false);
    bridge.set_setup_phase(0);
    sync_bridge_pad_launcher(&bridge, pad);
}

fn sync_bridge(app: &slint_ui::controller::ControllerTest, pad: &PadPool) {
    sync_bridge_pad_controller(&app.global::<slint_ui::controller::MisterBridge>(), pad);
}

fn sync_bridge_launcher(
    app: &slint_ui::launcher::Launcher,
    pad: &PadPool,
    nav: &LauncherNav,
    setup: &SetupNav,
    loading_message: &str,
    loading_detail: &str,
    catalog: Option<&ArcadeCatalog>,
    preview: &mut PreviewState,
) {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    sync_bridge_pad_launcher(&bridge, pad);
    bridge.set_screen_mode(match nav.screen {
        Screen::Home => 0,
        Screen::Controller => 1,
        Screen::Arcade => 2,
        Screen::Settings => 3,
    });
    bridge.set_selected_index(nav.selected as i32);
    bridge.set_settings_selected(nav.settings_selected as i32);
    bridge.set_arcade_selected(nav.arcade.selected as i32);
    bridge.set_arcade_scroll_y(nav.arcade.scroll_y);
    bridge.set_confirm_visible(nav.confirm_action.is_some());
    bridge.set_confirm_selected(nav.confirm_selected as i32);
    match nav.confirm_action {
        Some(launcher::ConfirmAction::ResetDatabase) => {
            bridge.set_confirm_title("Reset Database?".into());
            bridge.set_confirm_message("Delete the library database and reboot the MiSTer?".into());
        }
        Some(launcher::ConfirmAction::Restart) => {
            bridge.set_confirm_title("Restart MiSTer?".into());
            bridge.set_confirm_message("Reboot the MiSTer now?".into());
        }
        None => {
            bridge.set_confirm_title("".into());
            bridge.set_confirm_message("".into());
        }
    }
    bridge.set_loading_message(loading_message.into());
    bridge.set_loading_detail(loading_detail.into());
    if nav.screen == Screen::Arcade {
        request_arcade_preview(&bridge, catalog, nav.arcade.selected, preview);
    } else {
        preview.clear(&bridge);
    }
    sync_setup_bridge(&bridge, pad, setup);
}

fn png_to_slint_image(width: u32, height: u32, rgb: Vec<u8>) -> Image {
    let buffer = SharedPixelBuffer::<Rgb8Pixel>::clone_from_slice(&rgb, width, height);
    Image::from_rgb8(buffer)
}

const PREVIEW_IMAGE_CACHE_CAP: usize = 16;

#[derive(Default)]
struct PreviewImageCache {
    entries: VecDeque<(String, Image)>,
}

impl PreviewImageCache {
    fn get(&mut self, path: &str) -> Option<Image> {
        let idx = self.entries.iter().position(|(p, _)| p == path)?;
        let (_, image) = self.entries.remove(idx)?;
        let out = image.clone();
        self.entries.push_back((path.to_string(), image));
        Some(out)
    }

    fn insert(&mut self, path: String, image: Image) {
        if let Some(idx) = self.entries.iter().position(|(p, _)| p == &path) {
            self.entries.remove(idx);
        }
        self.entries.push_back((path, image));
        while self.entries.len() > PREVIEW_IMAGE_CACHE_CAP {
            self.entries.pop_front();
        }
    }
}

struct PreviewState {
    worker: PreviewWorker,
    last_preview_idx: Option<usize>,
    current_generation: u64,
    cache: PreviewImageCache,
    has_visible_preview: bool,
    visible_path: String,
}

impl PreviewState {
    fn new() -> Self {
        Self {
            worker: PreviewWorker::new(),
            last_preview_idx: None,
            current_generation: 0,
            cache: PreviewImageCache::default(),
            has_visible_preview: false,
            visible_path: String::new(),
        }
    }

    fn clear(&mut self, bridge: &slint_ui::launcher::MisterBridge) {
        if self.last_preview_idx.is_some() || self.current_generation != 0 {
            self.last_preview_idx = None;
            self.current_generation = 0;
            self.has_visible_preview = false;
            self.visible_path.clear();
            bridge.set_arcade_preview_has_image(false);
            bridge.set_arcade_preview_status(PreviewStatus::Empty);
            bridge.set_arcade_preview_title("".into());
            bridge.set_arcade_preview_image(Image::default());
        }
    }
}

fn request_arcade_preview(
    bridge: &slint_ui::launcher::MisterBridge,
    catalog: Option<&ArcadeCatalog>,
    selected: usize,
    preview: &mut PreviewState,
) {
    if preview.last_preview_idx == Some(selected) {
        return;
    }
    preview.last_preview_idx = Some(selected);

    let Some(catalog) = catalog else {
        bridge.set_arcade_preview_has_image(false);
        bridge.set_arcade_preview_status(PreviewStatus::Empty);
        bridge.set_arcade_preview_title("".into());
        bridge.set_arcade_preview_image(Image::default());
        return;
    };

    let Some(game) = catalog.games.get(selected) else {
        bridge.set_arcade_preview_has_image(false);
        bridge.set_arcade_preview_status(PreviewStatus::Empty);
        bridge.set_arcade_preview_title("".into());
        bridge.set_arcade_preview_image(Image::default());
        return;
    };

    bridge.set_arcade_preview_title(game.title.clone().into());
    if game.has_image {
        if let Some(image) = preview.cache.get(&game.image_path) {
            preview.current_generation = 0;
            preview.has_visible_preview = true;
            if preview.visible_path != game.image_path {
                preview.visible_path = game.image_path.clone();
                bridge.set_arcade_preview_image(image);
            }
            bridge.set_arcade_preview_has_image(true);
            bridge.set_arcade_preview_status(PreviewStatus::Ready);
            return;
        }
        preview.current_generation =
            preview
                .worker
                .request(selected, game.title.clone(), game.image_path.clone());
        if !preview.has_visible_preview {
            bridge.set_arcade_preview_image(Image::default());
            bridge.set_arcade_preview_has_image(false);
        }
        bridge.set_arcade_preview_status(PreviewStatus::Loading);
        return;
    }
    preview.current_generation = 0;
    preview.has_visible_preview = false;
    preview.visible_path.clear();
    bridge.set_arcade_preview_image(Image::default());
    bridge.set_arcade_preview_has_image(false);
    bridge.set_arcade_preview_status(PreviewStatus::Empty);
}

fn apply_ready_preview(app: &slint_ui::launcher::Launcher, preview: &mut PreviewState) -> bool {
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    let mut dirty = false;
    for result in preview.worker.drain() {
        if result.generation != preview.current_generation {
            continue;
        }
        bridge.set_arcade_preview_title(result.title.into());
        if let Some(image) = result.image {
            let image = png_to_slint_image(image.width, image.height, image.rgb);
            let image_path = result.image_path;
            preview.cache.insert(image_path.clone(), image.clone());
            preview.has_visible_preview = true;
            preview.visible_path = image_path;
            bridge.set_arcade_preview_image(image);
            bridge.set_arcade_preview_has_image(true);
            bridge.set_arcade_preview_status(PreviewStatus::Ready);
        } else {
            preview.has_visible_preview = false;
            preview.visible_path.clear();
            bridge.set_arcade_preview_image(Image::default());
            bridge.set_arcade_preview_has_image(false);
            bridge.set_arcade_preview_status(PreviewStatus::Empty);
        }
        dirty = true;
    }
    dirty
}

fn slint_arcade_games(games: &[ArcadeGameEntry]) -> ModelRc<slint_ui::launcher::ArcadeGame> {
    let rows: Vec<slint_ui::launcher::ArcadeGame> = games
        .iter()
        .map(|g| slint_ui::launcher::ArcadeGame {
            title: g.title.clone().into(),
            mra_path: g.mra_path.clone().into(),
            image_path: g.image_path.clone().into(),
            has_image: g.has_image,
        })
        .collect();
    ModelRc::new(VecModel::from(rows))
}

fn empty_arcade_catalog(root: &str) -> ArcadeCatalog {
    ArcadeCatalog {
        root: PathBuf::from(root),
        games: Vec::new(),
    }
}

fn start_library_catalog_worker(
    root: String,
    cached_catalog_ready: bool,
) -> mpsc::Receiver<CatalogWorkerMessage> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("library-catalog".to_string())
        .spawn(move || {
            lower_background_priority();
            let progress_tx = tx.clone();
            let mut progress = move |title: &str, detail: &str| {
                let _ = progress_tx.send(CatalogWorkerMessage::Progress {
                    title: title.to_string(),
                    detail: detail.to_string(),
                });
            };
            let summary = match library_bench::refresh_default_sqlite_database(Some(&mut progress))
            {
                Ok(summary) => Some(summary),
                Err(e) => {
                    eprintln!("library refresh failed: {e}");
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: "Library scan failed".to_string(),
                        detail: e,
                    });
                    None
                }
            };
            if let Some(summary) = summary.as_ref().filter(|summary| summary.skipped) {
                if cached_catalog_ready {
                    let _ = tx.send(CatalogWorkerMessage::Unchanged {
                        summary: summary.clone(),
                    });
                    return;
                }
            }
            if summary.is_some() {
                let _ = tx.send(CatalogWorkerMessage::Progress {
                    title: "Loading library".to_string(),
                    detail: "Opening SQLite catalog...".to_string(),
                });
            }
            match library_bench::load_arcade_catalog_from_sqlite(&root) {
                Ok(loaded) => {
                    let _ = tx.send(CatalogWorkerMessage::Ready {
                        catalog: loaded.catalog,
                        summary,
                        load_us: loaded.us,
                    });
                }
                Err(e) => {
                    eprintln!("library catalog load failed: {e}");
                    let _ = tx.send(CatalogWorkerMessage::Progress {
                        title: "Library load failed".to_string(),
                        detail: e,
                    });
                }
            }
        })
        .expect("spawn library-catalog");
    rx
}

enum CatalogWorkerMessage {
    Progress {
        title: String,
        detail: String,
    },
    Ready {
        catalog: ArcadeCatalog,
        summary: Option<library_bench::LibraryRefreshSummary>,
        load_us: u64,
    },
    Unchanged {
        summary: library_bench::LibraryRefreshSummary,
    },
}

fn lower_background_priority() {
    #[cfg(target_os = "linux")]
    unsafe {
        let _ = libc::setpriority(libc::PRIO_PROCESS, 0, 10);
    }
}

fn print_startup_event(start: Instant, name: &str, detail: impl std::fmt::Display) {
    println!(
        "startup_timing\t{name}\t{}ms\t{detail}",
        start.elapsed().as_millis()
    );
}

fn setup_pad_info<'a>(pad: &'a PadPool, setup: &SetupNav) -> &'a PadInfo {
    if setup.is_active() {
        pad.info_at(setup.target_pad_idx)
    } else {
        pad.info()
    }
}

fn sync_setup_bridge(bridge: &slint_ui::launcher::MisterBridge, pad: &PadPool, setup: &SetupNav) {
    let info = setup_pad_info(pad, setup);
    let db = pad.db();
    let active = setup.phase != SetupPhase::None;
    bridge.set_setup_visible(active);
    bridge.set_setup_phase(setup.phase as i32);
    if active {
        bridge.set_setup_title(setup.title().into());
        bridge.set_setup_selected(setup.list_index as i32);
        let idx = setup.target_pad_idx;
        let js_path = pad.path_at(idx);

        if setup.phase == SetupPhase::Configure {
            let fields = SetupNav::configure_fields(info, js_path, db);
            let labels: Vec<SharedString> = fields.iter().map(|(k, _)| k.clone().into()).collect();
            let values: Vec<SharedString> = fields.iter().map(|(_, v)| v.clone().into()).collect();
            bridge.set_setup_config_labels(ModelRc::new(VecModel::from(labels)));
            bridge.set_setup_config_values(ModelRc::new(VecModel::from(values)));
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            let live = SetupNav::configure_live_hint(pad.state_at(idx));
            bridge.set_setup_subtitle(live.into());
            bridge.set_setup_name(String::new().into());
            bridge.set_setup_kind_label(String::new().into());
        } else if setup.phase == SetupPhase::NameKind {
            bridge.set_setup_subtitle(setup.subtitle(info, db).into());
            bridge.set_setup_name(setup.draft_label.clone().into());
            bridge.set_setup_kind_label(setup.draft_kind_label().into());
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_labels(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_values(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
        } else if setup.phase == SetupPhase::PickExisting {
            bridge.set_setup_subtitle(setup.subtitle(info, db).into());
            bridge
                .set_setup_config_labels(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_values(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            let rows: Vec<SharedString> = db
                .list_entries()
                .iter()
                .map(|item| {
                    let port = if item.last_usb_port.is_empty() {
                        "unknown port".to_string()
                    } else {
                        format!("was {}", item.last_usb_port)
                    };
                    format!("{} — {}", item.label, port).into()
                })
                .collect();
            bridge.set_setup_list(ModelRc::new(VecModel::from(rows)));
        } else {
            bridge.set_setup_subtitle(setup.subtitle(info, db).into());
            bridge.set_setup_list(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_labels(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge
                .set_setup_config_values(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
            bridge.set_setup_name(String::new().into());
            bridge.set_setup_kind_label(String::new().into());
        }
    }
}

fn sync_bridge_pad_controller(bridge: &slint_ui::controller::MisterBridge, pad: &PadPool) {
    let state = pad.state();
    let info = pad.info();
    bridge.set_dpad_up(state.dpad_up);
    bridge.set_dpad_down(state.dpad_down);
    bridge.set_dpad_left(state.dpad_left);
    bridge.set_dpad_right(state.dpad_right);
    bridge.set_btn_a(state.btn_a);
    bridge.set_btn_b(state.btn_b);
    bridge.set_btn_x(state.btn_x);
    bridge.set_btn_y(state.btn_y);
    bridge.set_btn_l(state.btn_l);
    bridge.set_btn_r(state.btn_r);
    bridge.set_btn_zl(state.btn_zl);
    bridge.set_btn_zr(state.btn_zr);
    bridge.set_btn_select(state.btn_select);
    bridge.set_btn_start(state.btn_start);
    bridge.set_btn_l3(state.btn_l3);
    bridge.set_btn_r3(state.btn_r3);
    bridge.set_btn_home(state.btn_home);
    bridge.set_btn_capture(state.btn_capture);
    bridge.set_capture_available(info.capture_available);
    bridge.set_left_x(state.left_x);
    bridge.set_left_y(state.left_y);
    bridge.set_right_x(state.right_x);
    bridge.set_right_y(state.right_y);
    sync_device_info_controller(bridge, info, pad.db(), pad.path(), pad.len());
    bridge.set_pressed_now(state.pressed_now.clone().into());
    bridge.set_last_event_label(state.last_event_label.clone().into());
    bridge.set_last_raw_event(state.last_raw.clone().into());
}

fn sync_bridge_pad_launcher(bridge: &slint_ui::launcher::MisterBridge, pad: &PadPool) {
    let state = pad.state();
    let info = pad.info();
    bridge.set_dpad_up(state.dpad_up);
    bridge.set_dpad_down(state.dpad_down);
    bridge.set_dpad_left(state.dpad_left);
    bridge.set_dpad_right(state.dpad_right);
    bridge.set_btn_a(state.btn_a);
    bridge.set_btn_b(state.btn_b);
    bridge.set_btn_x(state.btn_x);
    bridge.set_btn_y(state.btn_y);
    bridge.set_btn_l(state.btn_l);
    bridge.set_btn_r(state.btn_r);
    bridge.set_btn_zl(state.btn_zl);
    bridge.set_btn_zr(state.btn_zr);
    bridge.set_btn_select(state.btn_select);
    bridge.set_btn_start(state.btn_start);
    bridge.set_btn_l3(state.btn_l3);
    bridge.set_btn_r3(state.btn_r3);
    bridge.set_btn_home(state.btn_home);
    bridge.set_btn_capture(state.btn_capture);
    bridge.set_capture_available(info.capture_available);
    bridge.set_left_x(state.left_x);
    bridge.set_left_y(state.left_y);
    bridge.set_right_x(state.right_x);
    bridge.set_right_y(state.right_y);
    sync_device_info_launcher(bridge, info, pad.db(), pad.path(), pad.len());
    bridge.set_pressed_now(state.pressed_now.clone().into());
    bridge.set_last_event_label(state.last_event_label.clone().into());
    bridge.set_last_raw_event(state.last_raw.clone().into());
}

fn sync_device_info_controller(
    bridge: &slint_ui::controller::MisterBridge,
    info: &PadInfo,
    db: &ControllerDb,
    js_path: &str,
    pad_count: usize,
) {
    let label = if pad_count > 1 {
        format!("{js_path} ({pad_count} pads)")
    } else {
        js_path.to_string()
    };
    bridge.set_device_label(label.into());
    bridge.set_device_name(db.display_label(info).into());
    bridge.set_usb_port(info.usb_port.clone().into());
    bridge.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    bridge.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    bridge.set_js_counts(
        format!(
            "js API: {} buttons, {} axes · evdev: {} keys, {} abs axes",
            info.js_buttons, info.js_axes, info.evdev_key_count, info.evdev_abs_count
        )
        .into(),
    );
}

fn sync_device_info_launcher(
    bridge: &slint_ui::launcher::MisterBridge,
    info: &PadInfo,
    db: &ControllerDb,
    js_path: &str,
    pad_count: usize,
) {
    let label = if pad_count > 1 {
        format!("{js_path} ({pad_count} pads)")
    } else {
        js_path.to_string()
    };
    bridge.set_device_label(label.into());
    bridge.set_device_name(db.display_label(info).into());
    bridge.set_usb_port(info.usb_port.clone().into());
    bridge.set_usb_id(format!("{}:{}", info.vendor_id, info.product_id).into());
    bridge.set_serial_id(if info.serial.is_empty() {
        "(no serial)".into()
    } else {
        info.serial.clone().into()
    });
    bridge.set_js_counts(
        format!(
            "js API: {} buttons, {} axes · evdev: {} keys, {} abs axes",
            info.js_buttons, info.js_axes, info.evdev_key_count, info.evdev_abs_count
        )
        .into(),
    );
}

fn run_bench_frame(
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    mut cached: &mut [Pixel],
    frame_order: FrameOrder,
    animation_clock: &AnimationClock,
) -> FrameSample {
    let frame_start = Instant::now();
    let t0 = Instant::now();
    let mut this_rect: Option<DirtyRect> = None;

    match frame_order {
        FrameOrder::RenderThenVsync => {
            update_slint_animations(animation_clock);
            let t1 = Instant::now();
            window.draw_if_needed(|renderer| {
                let region = renderer.render(&mut cached, ui.render_w());
                this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
            });
            let t2 = Instant::now();
            disp.wait_vsync();
            let t3 = Instant::now();
            let rows = if let Some(rect) = this_rect {
                copy_cached_rect(disp, ui, cached, rect);
                rect.rows()
            } else {
                0
            };
            let t4 = Instant::now();
            FrameSample {
                anim_us: (t1 - t0).as_micros() as u64,
                render_us: (t2 - t1).as_micros() as u64,
                vsync_us: (t3 - t2).as_micros() as u64,
                copy_us: (t4 - t3).as_micros() as u64,
                rows,
                wall_us: frame_start.elapsed().as_micros() as u64,
            }
        }
        FrameOrder::VsyncThenRender => {
            disp.wait_vsync();
            let t1 = Instant::now();
            update_slint_animations(animation_clock);
            let t2 = Instant::now();
            window.draw_if_needed(|renderer| {
                let region = renderer.render(&mut cached, ui.render_w());
                this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
            });
            let t3 = Instant::now();
            let rows = if let Some(rect) = this_rect {
                copy_cached_rect(disp, ui, cached, rect);
                rect.rows()
            } else {
                0
            };
            let t4 = Instant::now();
            FrameSample {
                anim_us: (t2 - t1).as_micros() as u64,
                render_us: (t3 - t2).as_micros() as u64,
                vsync_us: (t1 - t0).as_micros() as u64,
                copy_us: (t4 - t3).as_micros() as u64,
                rows,
                wall_us: frame_start.elapsed().as_micros() as u64,
            }
        }
    }
}

fn run_frame_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    animation_clock: &AnimationClock,
) {
    let mut cached = vec![Pixel(0); ui.render_w() * ui.render_h()];
    let start = Instant::now();
    let mut frames = 0u64;
    let mut profiler = FrameProfiler::from_env();
    let cpu = cpu_profile::start();
    let profile_on = profiler.enabled();

    // Legacy 1 Hz line (no anim column) when frame profiler is off — keeps bench-toolchain.sh parsing stable.
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut render_us = 0u128;
    let mut vsync_us = 0u128;
    let mut copy_us = 0u128;
    let mut copy_rows_acc = 0u128;
    let frame_order = FrameOrder::from_env();

    println!(
        "bench scene running {secs}s (vsync-locked, dirty-row copy, frame-order={}, animation-clock={})...",
        frame_order.label(),
        animation_clock.label()
    );
    while start.elapsed().as_secs() < secs {
        let sample = run_bench_frame(ui, disp, window, &mut cached, frame_order, animation_clock);
        frames += 1;

        if profiler.enabled() {
            profiler.record(sample);
        } else {
            fps_frames += 1;
            render_us += sample.render_us as u128;
            vsync_us += sample.vsync_us as u128;
            copy_us += sample.copy_us as u128;
            copy_rows_acc += sample.rows as u128;
            if fps_window_start.elapsed().as_millis() >= 1000 {
                let nn = fps_frames.max(1) as u128;
                println!(
                    "  fps ~ {fps_frames}  | render {}us  vsync-wait {}us  copy {}us ({} logical rows avg)",
                    render_us / nn,
                    vsync_us / nn,
                    copy_us / nn,
                    copy_rows_acc / nn
                );
                fps_frames = 0;
                render_us = 0;
                vsync_us = 0;
                copy_us = 0;
                copy_rows_acc = 0;
                fps_window_start = Instant::now();
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if profile_on {
        profiler.finish();
    }
    cpu_profile::finish(cpu);
}

#[cfg(feature = "video")]
fn run_video_playback_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    app: slint_ui::video_playback::VideoPlayback,
    animation_clock: &AnimationClock,
) {
    let path = std::env::var("MISTER_VIDEO_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| crate::video_player::DEFAULT_VIDEO_PATH.to_string());
    let mut player = match crate::video_player::VideoPlayer::open(&path) {
        Ok(player) => player,
        Err(e) => {
            eprintln!("video_playback: {e}");
            std::process::exit(1);
        }
    };
    let mut audio_sink = match crate::mr_audio::MrAudioSink::open_default() {
        Ok(sink) => sink,
        Err(e) => {
            eprintln!("video_playback audio: {e}");
            std::process::exit(1);
        }
    };

    let mut cached = vec![Pixel(0); ui.render_w() * ui.render_h()];
    let start = Instant::now();
    let mut next_video_at = Duration::ZERO;
    let frame_interval = player.frame_interval();
    let mut audio_pacer = AudioPacer::new();
    let mut frames = 0u64;
    let mut profiler = FrameProfiler::from_env();
    let cpu = cpu_profile::start();
    let profile_on = profiler.enabled();
    let frame_order = FrameOrder::from_env();

    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut decode_us = 0u128;
    let mut render_us = 0u128;
    let mut vsync_us = 0u128;
    let mut copy_us = 0u128;
    let mut copy_rows_acc = 0u128;
    let mut audio_stats = AudioWindowStats::default();

    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "video_playback running {label} path={path} frame-order={} animation-clock={}",
        frame_order.label(),
        animation_clock.label()
    );

    while secs == 0 || start.elapsed().as_secs() < secs {
        let frame_start = Instant::now();
        let t0 = Instant::now();
        let mut this_rect: Option<DirtyRect> = None;

        match frame_order {
            FrameOrder::RenderThenVsync => {
                update_slint_animations(animation_clock);
                let now = start.elapsed();
                if now >= next_video_at {
                    let audio_frames = audio_pacer.next_frames(frame_interval);
                    match player.next_frame(audio_frames) {
                        Ok(frame) => {
                            app.set_frame(frame.image);
                            window.request_redraw();
                            let audio_t0 = Instant::now();
                            match audio_sink.write_frames(&frame.audio) {
                                Ok(written) => {
                                    audio_stats.add(
                                        audio_t0.elapsed(),
                                        frame.audio_requested_frames,
                                        written,
                                        frame.loop_count,
                                    );
                                }
                                Err(e) => {
                                    eprintln!("video_playback audio: {e}");
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("video_playback: {e}");
                            break;
                        }
                    }
                    next_video_at += frame_interval;
                    while next_video_at < now {
                        next_video_at += frame_interval;
                    }
                }
                let t1 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t2 = Instant::now();
                disp.wait_vsync();
                let t3 = Instant::now();
                let rows = if let Some(rect) = this_rect {
                    copy_cached_rect(disp, ui, &cached, rect);
                    rect.rows()
                } else {
                    0
                };
                let t4 = Instant::now();
                let sample = FrameSample {
                    anim_us: (t1 - t0).as_micros() as u64,
                    render_us: (t2 - t1).as_micros() as u64,
                    vsync_us: (t3 - t2).as_micros() as u64,
                    copy_us: (t4 - t3).as_micros() as u64,
                    rows,
                    wall_us: frame_start.elapsed().as_micros() as u64,
                };
                record_video_sample(
                    sample,
                    &mut profiler,
                    &mut fps_window_start,
                    &mut fps_frames,
                    &mut decode_us,
                    &mut render_us,
                    &mut vsync_us,
                    &mut copy_us,
                    &mut copy_rows_acc,
                    &mut audio_stats,
                );
            }
            FrameOrder::VsyncThenRender => {
                disp.wait_vsync();
                let t1 = Instant::now();
                update_slint_animations(animation_clock);
                let now = start.elapsed();
                if now >= next_video_at {
                    let audio_frames = audio_pacer.next_frames(frame_interval);
                    match player.next_frame(audio_frames) {
                        Ok(frame) => {
                            app.set_frame(frame.image);
                            window.request_redraw();
                            let audio_t0 = Instant::now();
                            match audio_sink.write_frames(&frame.audio) {
                                Ok(written) => {
                                    audio_stats.add(
                                        audio_t0.elapsed(),
                                        frame.audio_requested_frames,
                                        written,
                                        frame.loop_count,
                                    );
                                }
                                Err(e) => {
                                    eprintln!("video_playback audio: {e}");
                                    break;
                                }
                            }
                        }
                        Err(e) => {
                            eprintln!("video_playback: {e}");
                            break;
                        }
                    }
                    next_video_at += frame_interval;
                    while next_video_at < now {
                        next_video_at += frame_interval;
                    }
                }
                let t2 = Instant::now();
                window.draw_if_needed(|renderer| {
                    let region = renderer.render(&mut cached, ui.render_w());
                    this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
                });
                let t3 = Instant::now();
                let rows = if let Some(rect) = this_rect {
                    copy_cached_rect(disp, ui, &cached, rect);
                    rect.rows()
                } else {
                    0
                };
                let t4 = Instant::now();
                let sample = FrameSample {
                    anim_us: (t2 - t1).as_micros() as u64,
                    render_us: (t3 - t2).as_micros() as u64,
                    vsync_us: (t1 - t0).as_micros() as u64,
                    copy_us: (t4 - t3).as_micros() as u64,
                    rows,
                    wall_us: frame_start.elapsed().as_micros() as u64,
                };
                record_video_sample(
                    sample,
                    &mut profiler,
                    &mut fps_window_start,
                    &mut fps_frames,
                    &mut decode_us,
                    &mut render_us,
                    &mut vsync_us,
                    &mut copy_us,
                    &mut copy_rows_acc,
                    &mut audio_stats,
                );
            }
        }
        frames += 1;
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
    if let Ok(status) = crate::mr_audio::read_status() {
        print!("video_playback audio status: {status}");
    }
    if profile_on {
        profiler.finish();
    }
    cpu_profile::finish(cpu);
}

#[cfg(feature = "video")]
struct AudioPacer {
    nanos_remainder: u128,
}

#[cfg(feature = "video")]
impl AudioPacer {
    fn new() -> Self {
        Self { nanos_remainder: 0 }
    }

    fn next_frames(&mut self, interval: Duration) -> usize {
        let total = crate::mr_audio::SAMPLE_RATE as u128 * interval.as_nanos()
            + self.nanos_remainder;
        let frames = total / 1_000_000_000;
        self.nanos_remainder = total % 1_000_000_000;
        frames as usize
    }
}

#[cfg(feature = "video")]
#[derive(Default)]
struct AudioWindowStats {
    write_us: u128,
    requested_frames: u128,
    written_frames: u128,
    underruns: u64,
    loop_count: u64,
}

#[cfg(feature = "video")]
impl AudioWindowStats {
    fn add(
        &mut self,
        write_elapsed: Duration,
        requested_frames: usize,
        written_frames: usize,
        loop_count: u64,
    ) {
        self.write_us += write_elapsed.as_micros();
        self.requested_frames += requested_frames as u128;
        self.written_frames += written_frames as u128;
        if written_frames < requested_frames {
            self.underruns += 1;
        }
        self.loop_count = self.loop_count.max(loop_count);
    }

    fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(feature = "video")]
#[allow(clippy::too_many_arguments)]
fn record_video_sample(
    sample: FrameSample,
    profiler: &mut FrameProfiler,
    fps_window_start: &mut Instant,
    fps_frames: &mut u64,
    decode_us: &mut u128,
    render_us: &mut u128,
    vsync_us: &mut u128,
    copy_us: &mut u128,
    copy_rows_acc: &mut u128,
    audio_stats: &mut AudioWindowStats,
) {
    if profiler.enabled() {
        profiler.record(sample);
        return;
    }

    *fps_frames += 1;
    *decode_us += sample.anim_us as u128;
    *render_us += sample.render_us as u128;
    *vsync_us += sample.vsync_us as u128;
    *copy_us += sample.copy_us as u128;
    *copy_rows_acc += sample.rows as u128;
    if fps_window_start.elapsed().as_millis() >= 1000 {
        let nn = (*fps_frames).max(1) as u128;
        println!(
            "  fps ~ {}  | decode+anim {}us  render {}us  vsync-wait {}us  copy {}us ({} logical rows avg)  audio-write {}us audio {}/{}f underruns {} loops {}",
            *fps_frames,
            *decode_us / nn,
            *render_us / nn,
            *vsync_us / nn,
            *copy_us / nn,
            *copy_rows_acc / nn,
            audio_stats.write_us / nn,
            audio_stats.written_frames,
            audio_stats.requested_frames,
            audio_stats.underruns,
            audio_stats.loop_count
        );
        *fps_frames = 0;
        *decode_us = 0;
        *render_us = 0;
        *vsync_us = 0;
        *copy_us = 0;
        *copy_rows_acc = 0;
        audio_stats.reset();
        *fps_window_start = Instant::now();
    }
}

const CONSOLE_LIST_X: usize = 40;
const CONSOLE_LIST_Y: usize = 116;
const CONSOLE_LIST_W: usize = 880;
const CONSOLE_LIST_H: usize = 356;
const CONSOLE_ROW_H: usize = 44;

fn run_console_scroll_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    app: slint_ui::console_scroll::ConsoleScroll,
    animation_clock: &AnimationClock,
) {
    let mut cached = vec![Pixel(0); ui.render_w() * ui.render_h()];
    let scale = ui.fb_scale();
    let fb_x = CONSOLE_LIST_X * scale;
    let fb_y = CONSOLE_LIST_Y * scale;
    let scroll_px = 2usize;
    let mut surface = vec![Pixel(0); CONSOLE_LIST_W * CONSOLE_LIST_H];
    let mut font = ConsoleFont::new(12.0);

    window.request_redraw();
    update_slint_animations(animation_clock);
    window.draw_if_needed(|renderer| {
        let _ = renderer.render(&mut cached, ui.render_w());
    });
    disp.wait_vsync();
    copy_cached_rows(disp, ui, &cached, 0, ui.render_h());
    draw_console_virtual_strip(
        &mut surface,
        CONSOLE_LIST_W,
        CONSOLE_LIST_W,
        CONSOLE_LIST_H,
        0,
        0,
        &mut font,
    );
    disp.copy_rect_scaled_at(fb_x, fb_y, scale, &surface, CONSOLE_LIST_W, CONSOLE_LIST_H);

    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!("console_scroll running {label} — fb scroll-copy + exposed-strip redraw");

    let start = Instant::now();
    let mut frames = 0u64;
    let mut virtual_y = 0usize;
    let mut fps_window_start = Instant::now();
    let mut fps_frames = 0u64;
    let mut ram_scroll_us = 0u128;
    let mut strip_us = 0u128;
    let mut fb_copy_us = 0u128;
    let mut label_rect: Option<DirtyRect> = None;

    while secs == 0 || start.elapsed().as_secs() < secs {
        if fps_window_start.elapsed().as_millis() >= 1000 {
            let nn = fps_frames.max(1) as u128;
            let top_row = (virtual_y / CONSOLE_ROW_H) % 1000;
            app.set_fps_label(format!("fps {fps_frames}").into());
            app.set_blit_label(format!("ram scroll {}us", ram_scroll_us / nn).into());
            app.set_strip_label(format!("new strip {}us", strip_us / nn).into());
            app.set_row_label(format!("top row {top_row:03}").into());
            window.request_redraw();
            println!(
                "  fps ~ {fps_frames}  | ram-scroll {}us  exposed-strip {}us  fb-copy {}us  top-row {top_row}",
                ram_scroll_us / nn,
                strip_us / nn,
                fb_copy_us / nn
            );
            fps_frames = 0;
            ram_scroll_us = 0;
            strip_us = 0;
            fb_copy_us = 0;
            fps_window_start = Instant::now();
        }

        update_slint_animations(animation_clock);
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut cached, ui.render_w());
            label_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });

        disp.wait_vsync();
        if let Some(rect) = label_rect.take() {
            copy_cached_rect(disp, ui, &cached, rect);
        }

        let t0 = Instant::now();
        scroll_surface_y(&mut surface, CONSOLE_LIST_W, CONSOLE_LIST_H, scroll_px);
        let t1 = Instant::now();
        virtual_y = virtual_y.wrapping_add(scroll_px);
        draw_console_virtual_strip(
            &mut surface,
            CONSOLE_LIST_W,
            CONSOLE_LIST_W,
            scroll_px,
            CONSOLE_LIST_H - scroll_px,
            virtual_y + CONSOLE_LIST_H - scroll_px,
            &mut font,
        );
        let t2 = Instant::now();
        disp.copy_rect_scaled_at(fb_x, fb_y, scale, &surface, CONSOLE_LIST_W, CONSOLE_LIST_H);
        let t3 = Instant::now();

        frames += 1;
        fps_frames += 1;
        ram_scroll_us += (t1 - t0).as_micros();
        strip_us += (t2 - t1).as_micros();
        fb_copy_us += (t3 - t2).as_micros();
    }

    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}

fn draw_console_virtual_strip(
    dst: &mut [Pixel],
    stride: usize,
    width: usize,
    height: usize,
    dst_y: usize,
    virtual_y_start: usize,
    font: &mut ConsoleFont,
) {
    let row_h = CONSOLE_ROW_H;
    for dy in 0..height {
        let vy = virtual_y_start + dy;
        let row = vy / row_h;
        let row_y = vy % row_h;
        let y = dst_y + dy;
        if y * stride >= dst.len() {
            break;
        }
        for dx in 0..width {
            let pos = y * stride + dx;
            if pos >= dst.len() {
                break;
            }
            dst[pos] = console_pixel(row, dx, row_y);
        }
    }

    let first_row = virtual_y_start / row_h;
    let last_row = (virtual_y_start + height.saturating_sub(1)) / row_h;
    for row in first_row..=last_row {
        let virtual_row_y = row * row_h;
        let row_screen_y = dst_y as isize + virtual_row_y as isize - virtual_y_start as isize;
        font.draw_text_clipped(
            dst,
            stride,
            width,
            dst_y,
            height,
            12,
            row_screen_y + 27,
            &format!("ROW {row:03}  MISTER GAME"),
            if row % 11 == 5 {
                Pixel(0x00fff2a8)
            } else {
                Pixel(0x00dbe7ff)
            },
        );
        font.draw_text_clipped(
            dst,
            stride,
            width,
            dst_y,
            height,
            CONSOLE_LIST_W as isize - 120,
            row_screen_y + 27,
            "COPY",
            Pixel(0x007dd3fc),
        );
    }
}

fn scroll_surface_y(surface: &mut [Pixel], w: usize, h: usize, shift: usize) {
    if shift == 0 || shift >= h {
        return;
    }
    let rows = h - shift;
    surface.copy_within(shift * w..h * w, 0);
    let tail = rows * w;
    for p in &mut surface[tail..h * w] {
        *p = Pixel(0);
    }
}

fn console_pixel(row: usize, x: usize, y: usize) -> Pixel {
    let selected = row % 11 == 5;
    let bg = if selected {
        Pixel(0x003a2750)
    } else if row % 2 == 0 {
        Pixel(0x00101928)
    } else {
        Pixel(0x000b1220)
    };
    if y < 1 || y >= CONSOLE_ROW_H - 1 {
        return if selected {
            Pixel(0x00f5d76e)
        } else {
            Pixel(0x001f2d44)
        };
    }
    if x < 1 || x >= CONSOLE_LIST_W - 1 {
        return Pixel(0x00263752);
    }
    bg
}

struct ConsoleGlyph {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    advance: i32,
    data: Vec<u8>,
}

struct ConsoleFont {
    font: swash::FontRef<'static>,
    scale_context: swash::scale::ScaleContext,
    glyphs: HashMap<char, ConsoleGlyph>,
    pixel_size: f32,
    units_per_em: f32,
}

impl ConsoleFont {
    fn new(pixel_size: f32) -> Self {
        let data = include_bytes!("../ui/fonts/PressStart2P-Regular.ttf");
        let font = swash::FontRef::from_index(data, 0).expect("PressStart2P-Regular.ttf");
        let units_per_em = font.metrics(&[]).units_per_em as f32;
        Self {
            font,
            scale_context: swash::scale::ScaleContext::new(),
            glyphs: HashMap::new(),
            pixel_size,
            units_per_em,
        }
    }

    fn glyph(&mut self, ch: char) -> Option<&ConsoleGlyph> {
        if !self.glyphs.contains_key(&ch) {
            let glyph_id = self.font.charmap().map(ch);
            let advance = if glyph_id == 0 {
                (self.pixel_size * 0.75) as i32
            } else {
                let scale = self.pixel_size / self.units_per_em;
                (self.font.glyph_metrics(&[]).advance_width(glyph_id) * scale) as i32
            };
            let glyph = if glyph_id == 0 || ch == ' ' {
                ConsoleGlyph {
                    left: 0,
                    top: 0,
                    width: 0,
                    height: 0,
                    advance,
                    data: Vec::new(),
                }
            } else {
                let mut scaler = self
                    .scale_context
                    .builder(self.font)
                    .size(self.pixel_size)
                    .build();
                let image = swash::scale::Render::new(&[swash::scale::Source::Outline])
                    .format(swash::zeno::Format::Alpha)
                    .render(&mut scaler, glyph_id)?;
                ConsoleGlyph {
                    left: image.placement.left,
                    top: image.placement.top,
                    width: image.placement.width as usize,
                    height: image.placement.height as usize,
                    advance,
                    data: image.data,
                }
            };
            self.glyphs.insert(ch, glyph);
        }
        self.glyphs.get(&ch)
    }

    fn draw_text_clipped(
        &mut self,
        dst: &mut [Pixel],
        stride: usize,
        clip_w: usize,
        clip_y: usize,
        clip_h: usize,
        x: isize,
        baseline_y: isize,
        text: &str,
        color: Pixel,
    ) {
        let mut pen_x = x;
        for ch in text.chars() {
            let Some(glyph) = self.glyph(ch) else {
                continue;
            };
            let gx0 = pen_x + glyph.left as isize;
            let gy0 = baseline_y - glyph.top as isize;
            for gy in 0..glyph.height {
                let dy = gy0 + gy as isize;
                if dy < clip_y as isize || dy >= (clip_y + clip_h) as isize {
                    continue;
                }
                for gx in 0..glyph.width {
                    let dx = gx0 + gx as isize;
                    if dx < 0 || dx >= clip_w as isize {
                        continue;
                    }
                    let alpha = glyph.data[gy * glyph.width + gx];
                    if alpha >= 128 {
                        dst[dy as usize * stride + dx as usize] = color;
                    }
                }
            }
            pen_x += glyph.advance as isize;
        }
    }
}

fn run_controller_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    window: &Rc<MinimalSoftwareWindow>,
    mut pad: PadPool,
    app: slint_ui::controller::ControllerTest,
    animation_clock: &AnimationClock,
) {
    let mut cached = vec![Pixel(0); ui.render_w() * ui.render_h()];
    let start = Instant::now();
    let mut frames = 0u64;
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "controller_test running {label} — {} pad(s) connected",
        pad.len()
    );
    while secs == 0 || start.elapsed().as_secs() < secs {
        if pad.poll() {
            sync_bridge(&app, &pad);
            window.request_redraw();
        }
        update_slint_animations(animation_clock);
        let mut this_rect: Option<DirtyRect> = None;
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut cached, ui.render_w());
            this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        disp.wait_vsync();
        if let Some(rect) = this_rect {
            copy_cached_rect(disp, ui, &cached, rect);
        }
        frames += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}

fn recover_launcher_ui(f: &mut Fpga, spawned_mister: &mut bool) {
    if *spawned_mister {
        launcher::stop_mister();
        f.fb_enable_direct(0, FB_W as u16, FB_H as u16, MODE_1080P60, Some(0), Some(0));
        *spawned_mister = false;
    }
}

fn run_launcher_loop(
    secs: u64,
    ui: &UiDisplay,
    disp: &mut Display,
    f: &mut Fpga,
    window: &Rc<MinimalSoftwareWindow>,
    mut pad: PadPool,
    app: slint_ui::launcher::Launcher,
    animation_clock: &AnimationClock,
) {
    let start = Instant::now();
    let mut frames = 0u64;
    let mut nav = LauncherNav::new();
    let mut setup = SetupNav::new();
    let mut loading_title = String::new();
    let mut launch_started = Instant::now();
    let mut launch_spawned_mister = false;
    let label = if secs == 0 {
        "forever".to_string()
    } else {
        format!("{secs}s")
    };
    println!(
        "launcher running {label} — {} pad(s), D-pad to move, A to select, Home to go back...",
        pad.len()
    );
    if let Some(idx) = pad.index_needing_setup() {
        let status = pad.db().registry_status(pad.info_at(idx));
        eprintln!("controller setup: pad {idx} needs setup ({status:?}) — showing prompt");
        setup.open_for(status, idx);
    }
    let mut cached = vec![Pixel(0); ui.render_w() * ui.render_h()];
    let mut preview = PreviewState::new();
    let arcade_root = std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string());
    let mut catalog = match library_bench::load_arcade_catalog_from_sqlite(&arcade_root) {
        Ok(loaded) => {
            print_startup_event(
                start,
                "library_db_loaded",
                format!(
                    "games={} rows={} load_us={}",
                    loaded.catalog.len(),
                    loaded.rows,
                    loaded.us
                ),
            );
            loaded.catalog
        }
        Err(e) => {
            print_startup_event(start, "library_db_miss", e);
            empty_arcade_catalog(&arcade_root)
        }
    };
    let mut catalog_ready = !catalog.games.is_empty();
    print_startup_event(start, "library_worker_started", &arcade_root);
    let catalog_rx = start_library_catalog_worker(arcade_root.clone(), catalog_ready);
    let mut catalog_refresh_done = false;
    let bridge = app.global::<slint_ui::launcher::MisterBridge>();
    bridge.set_arcade_games(slint_arcade_games(&catalog.games));
    bridge.set_catalog_scan_visible(!catalog_ready);
    bridge.set_catalog_scan_title(if catalog_ready {
        "Refreshing library".into()
    } else {
        "Indexing library".into()
    });
    bridge.set_catalog_scan_detail(if catalog_ready {
        format!("Using cached {} games", catalog.len()).into()
    } else {
        "Starting scan...".into()
    });
    sync_bridge_launcher(
        &app,
        &pad,
        &nav,
        &setup,
        "",
        "",
        Some(&catalog),
        &mut preview,
    );
    window.request_redraw();
    let mut first_frame_logged = false;
    while secs == 0 || start.elapsed().as_secs() < secs {
        let launching = launcher::launch_in_progress() || !loading_title.is_empty();
        let setup_active = setup.is_active();
        let mut bridge_dirty = false;

        if !catalog_refresh_done {
            while let Ok(message) = catalog_rx.try_recv() {
                match message {
                    CatalogWorkerMessage::Progress { title, detail } => {
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        let visible = !catalog_ready
                            || title == "Indexing library"
                            || title == "Library changed"
                            || title == "Library scan failed"
                            || title == "Library load failed";
                        bridge.set_catalog_scan_visible(visible);
                        bridge.set_catalog_scan_title(title.into());
                        bridge.set_catalog_scan_detail(detail.into());
                        bridge_dirty = true;
                    }
                    CatalogWorkerMessage::Ready {
                        catalog: ready_catalog,
                        summary,
                        load_us,
                    } => {
                        catalog = ready_catalog;
                        catalog_ready = true;
                        catalog_refresh_done = true;
                        print_startup_event(
                            start,
                            "library_ready",
                            format!("games={} load_us={load_us}", catalog.len()),
                        );
                        if let Some(summary) = summary {
                            let event = if summary.skipped {
                                "library_db_unchanged"
                            } else {
                                "library_db_saved"
                            };
                            print_startup_event(
                                start,
                                event,
                                format!(
                                    "bytes={} scan_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
                                    summary.bytes,
                                    summary.scan_us,
                                    summary.import_us,
                                    summary.discoveries,
                                    summary.normal_files,
                                    summary.containers,
                                    summary.entries
                                ),
                            );
                        }
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        bridge.set_arcade_games(slint_arcade_games(&catalog.games));
                        bridge.set_catalog_scan_visible(false);
                        bridge.set_catalog_scan_title("".into());
                        bridge.set_catalog_scan_detail("".into());
                        sync_bridge_launcher(
                            &app,
                            &pad,
                            &nav,
                            &setup,
                            &loading_title,
                            "",
                            Some(&catalog),
                            &mut preview,
                        );
                        bridge_dirty = true;
                    }
                    CatalogWorkerMessage::Unchanged { summary } => {
                        catalog_refresh_done = true;
                        print_startup_event(
                            start,
                            "library_db_unchanged",
                            format!(
                                "bytes={} scan_us={} import_us={} discoveries={} normal_files={} containers={} entries={}",
                                summary.bytes,
                                summary.scan_us,
                                summary.import_us,
                                summary.discoveries,
                                summary.normal_files,
                                summary.containers,
                                summary.entries
                            ),
                        );
                        let bridge = app.global::<slint_ui::launcher::MisterBridge>();
                        bridge.set_catalog_scan_visible(false);
                        bridge.set_catalog_scan_title("".into());
                        bridge.set_catalog_scan_detail("".into());
                        bridge_dirty = true;
                    }
                }
            }
        }

        if !launching {
            let _changed = pad.poll();
            let frame_now = Instant::now();
            let state = pad.state();
            let active_idx = pad.active_idx();
            let info = pad.info();

            if setup_active {
                let setup_info = pad.info_at(setup.target_pad_idx);
                match setup.handle_input(&state, frame_now, setup_info, pad.db()) {
                    SetupAction::None => {}
                    SetupAction::RegisterNew => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.register_new_at(idx) {
                            eprintln!("controller setup: register new: {e}");
                        }
                    }
                    SetupAction::ClaimExisting { list_index } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.claim_existing_at(idx, list_index) {
                            eprintln!("controller setup: claim existing: {e}");
                        }
                    }
                    SetupAction::SaveFinish { label, kind } => {
                        let idx = setup.target_pad_idx;
                        if let Err(e) = pad.finish_setup_at(idx, label, kind) {
                            eprintln!("controller setup: save: {e}");
                        } else {
                            eprintln!(
                                "controller setup: saved \"{}\" ({})",
                                pad.db().display_label(pad.info_at(idx)),
                                kind.as_str()
                            );
                        }
                        setup.advance_to_next_pad(&pad);
                    }
                    SetupAction::Done => {
                        setup.advance_to_next_pad(&pad);
                    }
                }
                bridge_dirty = true;
            } else {
                if _changed {
                    setup.maybe_open(info, active_idx, pad.db(), true);
                }
                if !setup.is_active() {
                    if let Some(event) = nav.handle_input(&state, frame_now, &catalog) {
                        match event.action {
                            LauncherAction::ResetDatabase => {
                                loading_title = "Resetting database…".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Rebooting MiSTer",
                                    Some(&catalog),
                                    &mut preview,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = renderer.render(&mut cached, ui.render_w());
                                    let _ = region;
                                });
                                disp.wait_vsync();
                                copy_cached_rows(disp, ui, &cached, 0, ui.render_h());
                                match launcher::reset_database_and_reboot() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("reset database failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::Restart => {
                                loading_title = "Restarting MiSTer…".to_string();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    &loading_title,
                                    "Please wait",
                                    Some(&catalog),
                                    &mut preview,
                                );
                                window.request_redraw();
                                update_slint_animations(animation_clock);
                                window.draw_if_needed(|renderer| {
                                    let region = renderer.render(&mut cached, ui.render_w());
                                    let _ = region;
                                });
                                disp.wait_vsync();
                                copy_cached_rows(disp, ui, &cached, 0, ui.render_h());
                                match launcher::reboot_mister() {
                                    Ok(()) => continue,
                                    Err(e) => {
                                        eprintln!("restart failed: {e}");
                                        loading_title.clear();
                                    }
                                }
                            }
                            LauncherAction::LaunchGame => {}
                        }
                        let Some(mra) = event.path else {
                            continue;
                        };
                        loading_title =
                            format!("Loading {}…", launcher::game_title(&catalog, &mra));
                        sync_bridge_launcher(
                            &app,
                            &pad,
                            &nav,
                            &setup,
                            &loading_title,
                            "",
                            Some(&catalog),
                            &mut preview,
                        );
                        window.request_redraw();
                        update_slint_animations(animation_clock);
                        window.draw_if_needed(|renderer| {
                            let region = renderer.render(&mut cached, ui.render_w());
                            let _ = region;
                        });
                        disp.wait_vsync();
                        copy_cached_rows(disp, ui, &cached, 0, ui.render_h());

                        match launcher::execute_game_launch(&mra) {
                            Ok(spawned) => {
                                launch_started = Instant::now();
                                launch_spawned_mister = spawned;
                            }
                            Err(e) => {
                                eprintln!("game launch failed: {e}");
                                loading_title.clear();
                                launcher::reset_launch();
                                sync_bridge_launcher(
                                    &app,
                                    &pad,
                                    &nav,
                                    &setup,
                                    "",
                                    "",
                                    Some(&catalog),
                                    &mut preview,
                                );
                                recover_launcher_ui(f, &mut launch_spawned_mister);
                            }
                        }
                        window.request_redraw();
                    }
                    bridge_dirty = true;
                }
            }

            if bridge_dirty {
                sync_bridge_launcher(
                    &app,
                    &pad,
                    &nav,
                    &setup,
                    &loading_title,
                    "",
                    Some(&catalog),
                    &mut preview,
                );
                window.request_redraw();
            }
        } else {
            let _ = pad.poll();
            if launcher::mister_running_arcade_core()
                && launch_started.elapsed() > Duration::from_millis(500)
            {
                println!("arcade core running — handing off to MiSTer");
                std::process::exit(0);
            } else if launch_started.elapsed() > Duration::from_secs(90) {
                eprintln!("game launch timed out");
                recover_launcher_ui(f, &mut launch_spawned_mister);
                std::process::exit(1);
            }
        }

        if launching {
            window.request_redraw();
        }
        if !launching && apply_ready_preview(&app, &mut preview) {
            window.request_redraw();
        }

        update_slint_animations(animation_clock);
        let mut this_rect: Option<DirtyRect> = None;
        window.draw_if_needed(|renderer| {
            let region = renderer.render(&mut cached, ui.render_w());
            this_rect = dirty_rect(&region, ui.render_w(), ui.render_h());
        });
        disp.wait_vsync();
        if launching || setup.is_active() {
            copy_cached_rows(disp, ui, &cached, 0, ui.render_h());
        } else if let Some(rect) = this_rect {
            copy_cached_rect(disp, ui, &cached, rect);
        }
        if !first_frame_logged {
            first_frame_logged = true;
            print_startup_event(
                start,
                "first_frame",
                format!("catalog_ready={catalog_ready}"),
            );
        }
        frames += 1;
    }
    let elapsed = start.elapsed().as_secs_f64();
    println!(
        "done: {frames} frames in {elapsed:.1}s = {:.1} fps avg",
        frames as f64 / elapsed
    );
}
