// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Portable projection of launcher navigation state into the compiled Slint UI.
//!
//! Device lifecycle, controller discovery, preview loading, and scanout remain
//! in `ui_runner`; this presenter is deliberately shared with the macOS host.

use crate::arcade_catalog::{ARCADE_ROW_HEIGHT, ArcadeCatalog, ArcadeGameView};
use crate::arcade_list_renderer::{ARCADE_LIST_W, arcade_focus_highlight_rgb565};
use crate::launcher::{CatalogMenuItemStatus, DisplayTransactionPhase, LauncherNav, Screen};
use crate::launcher_taxonomy::{LauncherMenuItemKind, ROOT_MENU_ID};
use crate::launcher_view_types::{
    active_display_choice, arcade_list_mode, arcade_search_pane, arcade_search_status, device_kind,
    display_transaction_state, home_scroll_phase, launcher_screen, menu_hierarchy, orientation_at,
    screen_orientation, selected_display_choice, settings_display_choice, settings_popup,
    settings_section, system_hub_section, system_page_mode,
};
use mister_magik_framebuffer_scenes::Rgb565Pixel;
use mister_magik_framebuffer_scenes::arcade_card::{CABINET_HEIGHT, CABINET_WIDTH};
use mister_magik_framebuffer_scenes::dithered_gradient::{
    HorizontalGradientStop, Rgb8Color, horizontal_rgb565,
};
use mister_magik_framebuffer_scenes::settings_cog::{
    COG_ASSET_HEIGHT, COG_ASSET_WIDTH, CrtSettingsGeometry,
};
use mister_magik_ui::launcher::{
    ArcadeLoadState, ArcadeSearchMode, ArcadeView, ChoiceOption, FeedbackView, Launcher, MenuItem,
    MenuItemKind, MenuItemPresentation, MenuItemStatus, MisterUi, NavigationView, SettingsView,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const SELECTION_FEEDBACK_MIN_VISIBLE: Duration = Duration::from_millis(80);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionFeedbackTarget {
    pub surface: String,
    pub item: String,
}

impl SelectionFeedbackTarget {
    pub fn new(surface: impl Into<String>, item: impl Into<String>) -> Self {
        Self {
            surface: surface.into(),
            item: item.into(),
        }
    }

    pub fn home(nav: &LauncherNav) -> Option<Self> {
        (nav.screen == Screen::Home)
            .then(|| Self {
                surface: nav.current_menu_id().to_string(),
                item: nav.current_menu_selected_item_id().to_string(),
            })
            .filter(|target| !target.item.is_empty())
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionFeedbackStamp {
    pub revision: u64,
    pub entries: Vec<SelectionFeedbackEntryStamp>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionFeedbackEntryStamp {
    pub event_id: u64,
    pub target: SelectionFeedbackTarget,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SelectionFeedbackConfirmation {
    Visible {
        event_id: u64,
        target: SelectionFeedbackTarget,
        confirmed_at: Instant,
    },
    Hidden {
        event_id: u64,
        target: SelectionFeedbackTarget,
        visible_for: Duration,
        confirmed_at: Instant,
    },
    Cancelled {
        event_id: u64,
        target: SelectionFeedbackTarget,
        confirmed_at: Instant,
    },
}

#[derive(Clone, Debug)]
struct ActiveSelectionFeedback {
    event_id: u64,
    target: SelectionFeedbackTarget,
    visible_since: Option<Instant>,
}

#[derive(Clone, Debug)]
struct PendingSelectionFeedbackRemoval {
    event_id: u64,
    target: SelectionFeedbackTarget,
    visible_since: Option<Instant>,
    requested_revision: u64,
}

#[derive(Default)]
struct SelectionFeedback {
    revision: u64,
    next_event_id: u64,
    surface: Option<String>,
    active: Vec<ActiveSelectionFeedback>,
    pending_removals: Vec<PendingSelectionFeedbackRemoval>,
}

impl SelectionFeedback {
    fn sync_surface(&mut self, target: Option<&SelectionFeedbackTarget>) -> bool {
        let surface = target.map(|target| target.surface.as_str());
        if self.surface.as_deref() == surface {
            return false;
        }
        let changed = self.retire_active();
        self.surface = surface.map(str::to_string);
        changed
    }

    fn register(&mut self, target: SelectionFeedbackTarget) -> bool {
        if self.surface.as_deref() != Some(target.surface.as_str()) {
            self.sync_surface(Some(&target));
        }
        self.next_event_id = self.next_event_id.wrapping_add(1).max(1);
        let event_id = self.next_event_id;
        let replaced = self
            .active
            .iter()
            .position(|entry| entry.target == target)
            .map(|index| self.active.remove(index));
        self.active.push(ActiveSelectionFeedback {
            event_id,
            target,
            visible_since: None,
        });
        self.bump_revision();
        if let Some(replaced) = replaced {
            self.pending_removals.push(PendingSelectionFeedbackRemoval {
                event_id: replaced.event_id,
                target: replaced.target,
                visible_since: replaced.visible_since,
                requested_revision: self.revision,
            });
        }
        true
    }

    fn retire_active(&mut self) -> bool {
        if self.active.is_empty() {
            return false;
        }
        self.bump_revision();
        let requested_revision = self.revision;
        self.pending_removals
            .extend(
                self.active
                    .drain(..)
                    .map(|entry| PendingSelectionFeedbackRemoval {
                        event_id: entry.event_id,
                        target: entry.target,
                        visible_since: entry.visible_since,
                        requested_revision,
                    }),
            );
        true
    }

    fn expire_due(&mut self, now: Instant) -> bool {
        let mut expired = Vec::new();
        self.active.retain(|entry| {
            let due = entry.visible_since.is_some_and(|since| {
                now.saturating_duration_since(since) >= SELECTION_FEEDBACK_MIN_VISIBLE
            });
            if due {
                expired.push(entry.clone());
            }
            !due
        });
        if expired.is_empty() {
            return false;
        }
        self.bump_revision();
        self.pending_removals
            .extend(
                expired
                    .into_iter()
                    .map(|entry| PendingSelectionFeedbackRemoval {
                        event_id: entry.event_id,
                        target: entry.target,
                        visible_since: entry.visible_since,
                        requested_revision: self.revision,
                    }),
            );
        true
    }

    fn stamp(&self) -> SelectionFeedbackStamp {
        SelectionFeedbackStamp {
            revision: self.revision,
            entries: self
                .active
                .iter()
                .map(|entry| SelectionFeedbackEntryStamp {
                    event_id: entry.event_id,
                    target: entry.target.clone(),
                })
                .collect(),
        }
    }

    fn confirm(
        &mut self,
        stamp: &SelectionFeedbackStamp,
        confirmed_at: Instant,
    ) -> Vec<SelectionFeedbackConfirmation> {
        let mut confirmations = Vec::new();
        for stamped in &stamp.entries {
            if let Some(active) = self
                .active
                .iter_mut()
                .find(|entry| entry.event_id == stamped.event_id && entry.target == stamped.target)
                && active.visible_since.is_none()
            {
                active.visible_since = Some(confirmed_at);
                confirmations.push(SelectionFeedbackConfirmation::Visible {
                    event_id: active.event_id,
                    target: active.target.clone(),
                    confirmed_at,
                });
            } else if let Some(pending) = self.pending_removals.iter_mut().find(|entry| {
                entry.event_id == stamped.event_id
                    && entry.target == stamped.target
                    && entry.requested_revision > stamp.revision
                    && entry.visible_since.is_none()
            }) {
                pending.visible_since = Some(confirmed_at);
                confirmations.push(SelectionFeedbackConfirmation::Visible {
                    event_id: pending.event_id,
                    target: pending.target.clone(),
                    confirmed_at,
                });
            }
        }
        self.pending_removals.retain(|entry| {
            let removed = entry.requested_revision <= stamp.revision
                && !stamp.entries.iter().any(|stamped| {
                    stamped.event_id == entry.event_id && stamped.target == entry.target
                });
            if removed {
                if let Some(visible_since) = entry.visible_since {
                    confirmations.push(SelectionFeedbackConfirmation::Hidden {
                        event_id: entry.event_id,
                        target: entry.target.clone(),
                        visible_for: confirmed_at.saturating_duration_since(visible_since),
                        confirmed_at,
                    });
                } else {
                    confirmations.push(SelectionFeedbackConfirmation::Cancelled {
                        event_id: entry.event_id,
                        target: entry.target.clone(),
                        confirmed_at,
                    });
                }
            }
            !removed
        });
        confirmations
    }

    fn bump_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1).max(1);
    }
}

macro_rules! set_if_changed {
    ($bridge:expr, $getter:ident, $setter:ident, $value:expr) => {{
        let value = $value;
        if $bridge.$getter() != value {
            $bridge.$setter(value);
        }
    }};
}

macro_rules! set_view_string_if_changed {
    ($view:expr, $getter:ident, $setter:ident, $value:expr) => {{
        let source = $value;
        let source = AsRef::<str>::as_ref(&source);
        if $view.$getter().as_str() != source {
            $view.$setter(SharedString::from(source));
        }
    }};
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct BridgeChurnCounters {
    pub(crate) model_replacements: u64,
    pub(crate) row_mutations: u64,
    pub(crate) row_allocations: u64,
    pub(crate) shared_string_constructions: u64,
    pub(crate) model_allocation_us: u64,
}

impl BridgeChurnCounters {
    #[cfg(feature = "ui")]
    pub(crate) fn saturating_sub(self, earlier: Self) -> Self {
        Self {
            model_replacements: self
                .model_replacements
                .saturating_sub(earlier.model_replacements),
            row_mutations: self.row_mutations.saturating_sub(earlier.row_mutations),
            row_allocations: self.row_allocations.saturating_sub(earlier.row_allocations),
            shared_string_constructions: self
                .shared_string_constructions
                .saturating_sub(earlier.shared_string_constructions),
            model_allocation_us: self
                .model_allocation_us
                .saturating_sub(earlier.model_allocation_us),
        }
    }
}

thread_local! {
    static BRIDGE_CHURN_ENABLED: Cell<bool> = const { Cell::new(false) };
    static BRIDGE_CHURN_COUNTERS: RefCell<BridgeChurnCounters> = const {
        RefCell::new(BridgeChurnCounters {
            model_replacements: 0,
            row_mutations: 0,
            row_allocations: 0,
            shared_string_constructions: 0,
            model_allocation_us: 0,
        })
    };
}

#[cfg(feature = "ui")]
pub(crate) fn bridge_churn_begin() {
    BRIDGE_CHURN_COUNTERS.with(|counters| *counters.borrow_mut() = BridgeChurnCounters::default());
    BRIDGE_CHURN_ENABLED.with(|enabled| enabled.set(true));
}

#[cfg(feature = "ui")]
pub(crate) fn bridge_churn_end() -> BridgeChurnCounters {
    BRIDGE_CHURN_ENABLED.with(|enabled| enabled.set(false));
    bridge_churn_snapshot()
}

#[cfg(feature = "ui")]
pub(crate) fn bridge_churn_snapshot() -> BridgeChurnCounters {
    BRIDGE_CHURN_COUNTERS.with(|counters| *counters.borrow())
}

pub(crate) fn bridge_churn_record_model_replacements(count: u64) {
    bridge_churn_record(|counters| {
        counters.model_replacements = counters.model_replacements.saturating_add(count);
    });
}

pub(crate) fn bridge_churn_record_row_mutations(count: u64) {
    bridge_churn_record(|counters| {
        counters.row_mutations = counters.row_mutations.saturating_add(count);
    });
}

pub(crate) fn bridge_churn_record_row_allocations(count: u64) {
    bridge_churn_record(|counters| {
        counters.row_allocations = counters.row_allocations.saturating_add(count);
    });
}

pub(crate) fn bridge_churn_record_shared_strings(count: u64) {
    bridge_churn_record(|counters| {
        counters.shared_string_constructions =
            counters.shared_string_constructions.saturating_add(count);
    });
}

pub(crate) fn bridge_churn_record_model_allocation_us(elapsed_us: u128) {
    bridge_churn_record(|counters| {
        counters.model_allocation_us = counters
            .model_allocation_us
            .saturating_add(elapsed_us.min(u128::from(u64::MAX)) as u64);
    });
}

fn bridge_churn_record(update: impl FnOnce(&mut BridgeChurnCounters)) {
    BRIDGE_CHURN_ENABLED.with(|enabled| {
        if enabled.get() {
            BRIDGE_CHURN_COUNTERS.with(|counters| update(&mut counters.borrow_mut()));
        }
    });
}

#[derive(Default)]
struct NavigationViewPresenter {
    /// Favourite and recent counts of the system page, recomputed only when
    /// the collection or its user lists change.
    hub_counts_key: Option<(String, u64, usize, Option<String>)>,
    hub_counts: (usize, usize),
    menu_items_key: Option<(usize, String)>,
    menu_items: Option<Rc<VecModel<MenuItem>>>,
    menu_item_presentation: Option<Rc<VecModel<MenuItemPresentation>>>,
    projected_selected_index: Option<usize>,
    selection_feedback: SelectionFeedback,
    projected_selection_feedback: SelectionFeedbackStamp,
    published_selection_feedback: Rc<RefCell<SelectionFeedbackStamp>>,
    selection_feedback_callback_installed: bool,
}

#[derive(Default)]
struct SettingsViewPresenter {
    license_lines_key: Option<(usize, crate::licenses::LicenseViewport)>,
    license_lines: Option<Rc<VecModel<SharedString>>>,
    display_options: Option<Rc<VecModel<ChoiceOption>>>,
    orientation_options: Option<Rc<VecModel<ChoiceOption>>>,
    license_titles: Option<Rc<VecModel<SharedString>>>,
    license_kinds: Option<Rc<VecModel<SharedString>>>,
    fixed_visual_assets_installed: bool,
    crt_visual_assets_geometry: Option<SettingsVisualAssetGeometry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SettingsVisualAssetGeometry {
    width: i32,
    height: i32,
    content_x: i32,
    content_width: i32,
}

const SETTINGS_FOCUS_HEIGHT: usize = 36;
const SETTINGS_FOCUS_SETTINGS_WIDTH: usize = 534;
const SETTINGS_FOCUS_WIDE_WIDTH: usize = 638;
const SETTINGS_FOCUS_PORTRAIT_WIDTH: usize = 476;
const SETTINGS_FOCUS_STOPS: [HorizontalGradientStop; 3] = [
    HorizontalGradientStop::percent(0, Rgb8Color::new(0x22, 0x18, 0x43)),
    HorizontalGradientStop::percent(60, Rgb8Color::new(0x12, 0x0d, 0x24)),
    HorizontalGradientStop::percent(100, Rgb8Color::new(0, 0, 0)),
];

fn prepared_cog() -> &'static (
    mister_magik_framebuffer_scenes::settings_cog::CogArtwork,
    Vec<Rgb565Pixel>,
) {
    static COG: std::sync::OnceLock<(
        mister_magik_framebuffer_scenes::settings_cog::CogArtwork,
        Vec<Rgb565Pixel>,
    )> = std::sync::OnceLock::new();
    COG.get_or_init(|| {
        std::thread::Builder::new()
            .name("settings-artwork".into())
            .spawn(|| {
                use mister_magik_catalog::runtime_thread::{
                    RuntimeThreadRole, apply_runtime_thread_policy,
                };
                apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardRenderer);
                let texture =
                    mister_magik_framebuffer_scenes::settings_cog::CogTexture::from_rgb888(
                        include_bytes!("../assets/ui/settings/cog-backdrop-412x374.rgb888"),
                    )
                    .expect("embedded cog geometry");
                (texture.artwork(), texture.destination_pixels())
            })
            .expect("start cog preparation")
            .join()
            .expect("prepare cog artwork")
    })
}
pub fn warm_settings_cog() -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("settings-artwork-warm".into())
        .spawn(|| {
            let _ = prepared_cog();
        })
        .expect("warm cog artwork")
}
pub fn settings_cog_artwork() -> &'static mister_magik_framebuffer_scenes::settings_cog::CogArtwork
{
    &prepared_cog().0
}
/// Resting artwork has the moving cog's final destination-space quantisation.
pub fn settings_cog_backdrop_rgb565() -> &'static [Rgb565Pixel] {
    &prepared_cog().1
}

fn prepared_cabinet() -> &'static (
    mister_magik_framebuffer_scenes::arcade_card::CabinetArtwork,
    Vec<Rgb565Pixel>,
) {
    static CABINET: std::sync::OnceLock<(
        mister_magik_framebuffer_scenes::arcade_card::CabinetArtwork,
        Vec<Rgb565Pixel>,
    )> = std::sync::OnceLock::new();
    CABINET.get_or_init(|| {
        // All texture/cache computation stays off the UI thread. Production
        // warms this while the first launcher faces are being prepared.
        std::thread::Builder::new()
            .name("arcade-artwork".into())
            .spawn(|| {
                use mister_magik_catalog::runtime_thread::{
                    RuntimeThreadRole, apply_runtime_thread_policy,
                };
                apply_runtime_thread_policy(RuntimeThreadRole::LauncherCardRenderer);
                let texture =
                    mister_magik_framebuffer_scenes::arcade_card::CabinetTexture::from_rgb888(
                        include_bytes!("../assets/ui/arcade/cabinet-483x519.rgb888"),
                    )
                    .expect("embedded cabinet geometry");
                (texture.artwork(), texture.destination_pixels())
            })
            .expect("start cabinet preparation")
            .join()
            .expect("prepare cabinet artwork")
    })
}

pub fn warm_arcade_cabinet() -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("arcade-artwork-warm".into())
        .spawn(|| {
            let _ = prepared_cabinet();
        })
        .expect("warm cabinet artwork")
}

pub fn arcade_cabinet_artwork()
-> &'static mister_magik_framebuffer_scenes::arcade_card::CabinetArtwork {
    &prepared_cabinet().0
}

/// Resting artwork uses the moving cabinet's destination-space quantisation.
pub fn arcade_cabinet_rgb565() -> &'static [Rgb565Pixel] {
    &prepared_cabinet().1
}

pub fn system_device_rgb565(kind: Option<crate::device_art::DeviceKind>) -> &'static [Rgb565Pixel] {
    use crate::device_art::DeviceKind;
    static DEVICES: [std::sync::OnceLock<Vec<Rgb565Pixel>>; 3] = [
        std::sync::OnceLock::new(),
        std::sync::OnceLock::new(),
        std::sync::OnceLock::new(),
    ];
    let Some(kind) = kind else {
        return arcade_cabinet_rgb565();
    };
    let index = match kind {
        DeviceKind::Tv => 0,
        DeviceKind::Monitor => 1,
        DeviceKind::Handheld => 2,
    };
    DEVICES[index].get_or_init(|| {
        crate::device_art::device_rgb565(kind)
            .iter()
            .copied()
            .map(Rgb565Pixel)
            .collect()
    })
}

pub fn device_reveal_spec(
    kind: Option<crate::device_art::DeviceKind>,
    crt: bool,
    hub: bool,
) -> mister_magik_framebuffer_scenes::device_card::DeviceCardReveal {
    use crate::device_art::DeviceKind;
    use mister_magik_framebuffer_scenes::navigation::NavigationTransitionRect;
    let Some(kind) = kind else {
        return mister_magik_framebuffer_scenes::device_card::DeviceCardReveal {
            hub,
            ..mister_magik_framebuffer_scenes::device_card::DeviceCardReveal::cabinet(crt)
        };
    };
    mister_magik_framebuffer_scenes::device_card::DeviceCardReveal {
        region: NavigationTransitionRect {
            x: 117,
            y: 46,
            width: 250,
            height: 350,
        },
        accent: match kind {
            DeviceKind::Tv => 0x5b9c,
            DeviceKind::Monitor => 0xe607,
            DeviceKind::Handheld => 0x3631,
        },
        crt,
        hub,
    }
}

/// Slint 1.18 images have no RGB565 format. Bit replication makes the RGB565
/// software renderer's truncation return exactly the stored pixels.
fn rgb565_image(width: usize, height: usize, pixels: &[Rgb565Pixel]) -> slint::Image {
    assert_eq!(
        pixels.len(),
        width * height,
        "RGB565 image geometry must match its pixels"
    );
    let mut buffer = slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width as u32, height as u32);
    for (pixel, packed) in buffer.make_mut_slice().iter_mut().zip(pixels) {
        let (r, g, b) = (packed.0 >> 11, (packed.0 >> 5) & 0x3f, packed.0 & 0x1f);
        *pixel = slint::Rgb8Pixel {
            r: ((r << 3) | (r >> 2)) as u8,
            g: ((g << 2) | (g >> 4)) as u8,
            b: ((b << 3) | (b >> 2)) as u8,
        };
    }
    slint::Image::from_rgb8(buffer)
}

fn settings_cog_backdrop_image() -> slint::Image {
    rgb565_image(
        COG_ASSET_WIDTH,
        COG_ASSET_HEIGHT,
        settings_cog_backdrop_rgb565(),
    )
}

/// Raster size of the CRT system page's hero: 232 display pixels wide, with
/// half-height rows on the native 15 kHz rasters. Portrait has no room for it.
/// A generic TV, monitor or handheld backdrop, or the cabinet for `None`.
fn device_image(kind: Option<crate::device_art::DeviceKind>) -> slint::Image {
    let Some(kind) = kind else {
        return arcade_cabinet_image();
    };
    let pixels: Vec<Rgb565Pixel> = crate::device_art::device_rgb565(kind)
        .iter()
        .map(|pixel| Rgb565Pixel(*pixel))
        .collect();
    rgb565_image(
        crate::device_art::DEVICE_WIDTH,
        crate::device_art::DEVICE_HEIGHT,
        &pixels,
    )
}

fn arcade_cabinet_image() -> slint::Image {
    rgb565_image(CABINET_WIDTH, CABINET_HEIGHT, arcade_cabinet_rgb565())
}

fn arcade_focus_highlight_image() -> slint::Image {
    let height = ARCADE_ROW_HEIGHT.max(1) as usize;
    let pixels = arcade_focus_highlight_rgb565(ARCADE_LIST_W, height);
    rgb565_image(ARCADE_LIST_W, height, &pixels)
}

fn settings_focus_highlight_image(width: usize, height: usize) -> slint::Image {
    let pixels = horizontal_rgb565(width, height, &SETTINGS_FOCUS_STOPS)
        .expect("fixed Settings focus gradient is valid");
    rgb565_image(width, height, &pixels)
}

fn settings_visual_asset_geometry(ui: &MisterUi) -> SettingsVisualAssetGeometry {
    SettingsVisualAssetGeometry {
        width: ui.get_window_width(),
        height: ui.get_window_height(),
        content_x: ui.get_crt_content_x(),
        content_width: ui.get_crt_content_width(),
    }
}

fn crt_focus_highlight_geometry(
    geometry: SettingsVisualAssetGeometry,
) -> ((usize, usize), (usize, usize)) {
    let width = geometry.width.max(1) as usize;
    let height = geometry.height.max(1) as usize;
    let content_x = geometry.content_x.max(0) as usize;
    let content_width = geometry.content_width.max(0) as usize;
    let safe_x = content_x.max(width.saturating_sub(content_x.saturating_add(content_width)));
    let layout = CrtSettingsGeometry::for_viewport(width, height, safe_x, 0)
        .expect("CRT focus assets use a supported viewport");
    let row_width = width.saturating_sub(2 * layout.margin_x()).max(1);
    let menu_width = (240 * layout.scale_x()).min(row_width).max(1);
    (
        (row_width, layout.row_height()),
        (menu_width, 12 * layout.scale_y()),
    )
}

fn install_fixed_settings_visual_assets(settings: &SettingsView) {
    settings.set_cog_backdrop(settings_cog_backdrop_image());
    settings.set_focus_highlight_settings(settings_focus_highlight_image(
        SETTINGS_FOCUS_SETTINGS_WIDTH,
        SETTINGS_FOCUS_HEIGHT,
    ));
    settings.set_focus_highlight_wide(settings_focus_highlight_image(
        SETTINGS_FOCUS_WIDE_WIDTH,
        SETTINGS_FOCUS_HEIGHT,
    ));
    settings.set_focus_highlight_portrait(settings_focus_highlight_image(
        SETTINGS_FOCUS_PORTRAIT_WIDTH,
        SETTINGS_FOCUS_HEIGHT,
    ));
}

fn install_crt_settings_visual_assets(
    settings: &SettingsView,
    geometry: SettingsVisualAssetGeometry,
) {
    let (row, menu) = crt_focus_highlight_geometry(geometry);
    settings.set_focus_highlight_crt_row(settings_focus_highlight_image(row.0, row.1));
    settings.set_focus_highlight_crt_menu(settings_focus_highlight_image(menu.0, menu.1));
}

pub fn install_settings_visual_assets(app: &Launcher) {
    let settings = app.global::<SettingsView>();
    install_fixed_settings_visual_assets(&settings);
    let ui = app.global::<MisterUi>();
    if ui.get_crt_layout() {
        install_crt_settings_visual_assets(&settings, settings_visual_asset_geometry(&ui));
    }
}

pub fn install_arcade_visual_assets(app: &Launcher) {
    let arcade = app.global::<ArcadeView>();
    arcade.set_device_backdrop(arcade_cabinet_image());
    arcade.set_focus_highlight(arcade_focus_highlight_image());
}

#[derive(Default)]
pub struct LauncherViewPresenters {
    navigation: NavigationViewPresenter,
    settings: SettingsViewPresenter,
    arcade_visual_assets_installed: bool,
    /// The device backdrop currently installed, and the images already built.
    arcade_device_installed: Option<mister_magik_ui::launcher::DeviceKind>,
    device_images: [Option<slint::Image>; 4],
    drawer_projection: Option<std::sync::Arc<Vec<crate::launcher::ArcadeDrawerItem>>>,
    drawer_initialized: bool,
}

impl LauncherViewPresenters {
    pub fn sync(
        &mut self,
        app: &Launcher,
        nav: &LauncherNav,
        catalog: &ArcadeCatalog,
        catalog_version: Option<usize>,
        defer_arcade_overlay: bool,
        active_display_fallback: Option<(u16, u16)>,
    ) {
        let navigation = app.global::<NavigationView>();
        if !self.arcade_visual_assets_installed {
            install_arcade_visual_assets(app);
            self.arcade_visual_assets_installed = true;
            self.arcade_device_installed = Some(mister_magik_ui::launcher::DeviceKind::Cabinet);
            self.device_images[0] = Some(app.global::<ArcadeView>().get_device_backdrop());
        }
        set_if_changed!(
            navigation,
            get_screen,
            set_screen,
            launcher_screen(nav.screen)
        );
        set_if_changed!(
            navigation,
            get_home_selected_index,
            set_home_selected_index,
            nav.selected as i32
        );
        set_if_changed!(
            navigation,
            get_menu_hierarchy,
            set_menu_hierarchy,
            menu_hierarchy(nav.current_menu_id() == ROOT_MENU_ID)
        );
        set_if_changed!(
            navigation,
            get_home_scroll_phase,
            set_home_scroll_phase,
            home_scroll_phase(
                nav.home_horizontal_held(),
                nav.home_horizontal_repeat_active()
            )
        );
        set_if_changed!(
            navigation,
            get_home_scroll_x,
            set_home_scroll_x,
            nav.scroll_x
        );
        set_view_string_if_changed!(
            navigation,
            get_menu_title,
            set_menu_title,
            nav.current_menu_title()
        );
        set_view_string_if_changed!(
            navigation,
            get_menu_breadcrumb,
            set_menu_breadcrumb,
            nav.current_menu_breadcrumb()
        );
        set_if_changed!(
            navigation,
            get_system_hub_section,
            set_system_hub_section,
            system_hub_section(nav.system_hub_selected)
        );
        set_if_changed!(
            navigation,
            get_system_device,
            set_system_device,
            device_kind(nav.device_kind())
        );
        set_if_changed!(
            navigation,
            get_system_page_mode,
            set_system_page_mode,
            system_page_mode(nav.system_page_mode)
        );
        set_if_changed!(
            navigation,
            get_system_title_wraps,
            set_system_title_wraps,
            nav.active_collection()
                .is_some_and(|c| c.title.chars().count() > 13)
        );
        if nav.is_system_hub() {
            let collection = nav.active_collection();
            let system_id = collection
                .map(|collection| {
                    collection
                        .system_id
                        .as_deref()
                        .unwrap_or(&collection.legacy_system_id)
                })
                .unwrap_or("");
            set_view_string_if_changed!(
                navigation,
                get_system_title,
                set_system_title,
                collection.map_or_else(String::new, |collection| collection.title.to_uppercase())
            );
            set_view_string_if_changed!(
                navigation,
                get_system_subtitle,
                set_system_subtitle,
                crate::system_facts::system_subtitle(system_id)
            );
            set_if_changed!(
                navigation,
                get_system_hub_games_count,
                set_system_hub_games_count,
                collection.map_or(0, |collection| collection.count) as i32
            );
            let key = (
                nav.active_collection_id().unwrap_or("").to_owned(),
                nav.favourite_launch_refs_revision(),
                nav.recent_count(),
                nav.recent_launch_refs().first().cloned(),
            );
            if self.navigation.hub_counts_key.as_ref() != Some(&key) {
                self.navigation.hub_counts = (
                    nav.active_collection_recent_count(catalog),
                    nav.active_collection_favourite_count(catalog),
                );
                self.navigation.hub_counts_key = Some(key);
            }
            let (recent, favourites) = self.navigation.hub_counts;
            set_view_string_if_changed!(
                navigation,
                get_system_hub_caption,
                set_system_hub_caption,
                crate::system_facts::hub_caption(
                    nav.system_hub_selected,
                    collection.map_or(0, |collection| collection.count),
                    recent,
                    favourites,
                )
            );
            set_if_changed!(
                navigation,
                get_system_hub_recent_count,
                set_system_hub_recent_count,
                recent as i32
            );
            set_if_changed!(
                navigation,
                get_system_hub_favourites_count,
                set_system_hub_favourites_count,
                favourites as i32
            );
        }
        let settings = app.global::<SettingsView>();
        if !self.settings.fixed_visual_assets_installed {
            install_fixed_settings_visual_assets(&settings);
            self.settings.fixed_visual_assets_installed = true;
        }
        let ui = app.global::<MisterUi>();
        if ui.get_crt_layout() {
            let geometry = settings_visual_asset_geometry(&ui);
            if self.settings.crt_visual_assets_geometry != Some(geometry) {
                install_crt_settings_visual_assets(&settings, geometry);
                self.settings.crt_visual_assets_geometry = Some(geometry);
            }
        }
        if self.settings.display_options.is_none() {
            let choices = crate::launcher::settings_display_resolutions()
                .map(|mode| ChoiceOption {
                    id: mode.id.into(),
                    label: mode.label.into(),
                })
                .collect::<Vec<_>>();
            self.settings.display_options = Some(Rc::new(VecModel::from(choices)));
            settings.set_display_options(ModelRc::from(
                self.settings
                    .display_options
                    .as_ref()
                    .expect("display choices initialized")
                    .clone(),
            ));
        }
        if self.settings.orientation_options.is_none() {
            let choices = crate::settings::ScreenOrientation::ALL
                .iter()
                .map(|orientation| ChoiceOption {
                    id: orientation.id().into(),
                    label: orientation.label().into(),
                })
                .collect::<Vec<_>>();
            self.settings.orientation_options = Some(Rc::new(VecModel::from(choices)));
            settings.set_orientation_options(ModelRc::from(
                self.settings
                    .orientation_options
                    .as_ref()
                    .expect("orientation choices initialized")
                    .clone(),
            ));
        }
        if self.settings.license_titles.is_none() {
            self.settings.license_titles = Some(Rc::new(VecModel::from(
                crate::licenses::LICENSE_TITLES
                    .iter()
                    .map(|title| SharedString::from(*title))
                    .collect::<Vec<_>>(),
            )));
            settings.set_license_titles(ModelRc::from(
                self.settings
                    .license_titles
                    .as_ref()
                    .expect("license titles initialized")
                    .clone(),
            ));
        }
        if self.settings.license_kinds.is_none() {
            self.settings.license_kinds = Some(Rc::new(VecModel::from(
                crate::licenses::LICENSE_KINDS
                    .iter()
                    .map(|kind| SharedString::from(*kind))
                    .collect::<Vec<_>>(),
            )));
            settings.set_license_kinds(ModelRc::from(
                self.settings
                    .license_kinds
                    .as_ref()
                    .expect("license kinds initialized")
                    .clone(),
            ));
        }
        set_if_changed!(
            settings,
            get_section,
            set_section,
            settings_section(nav.settings_selected)
        );
        set_if_changed!(
            settings,
            get_popup,
            set_popup,
            settings_popup(nav.display_combo_open, nav.orientation_combo_open)
        );
        set_if_changed!(
            settings,
            get_selected_display,
            set_selected_display,
            selected_display_choice(nav.display_selected)
        );
        let active_display = active_display_choice(nav.display_selected, active_display_fallback);
        set_if_changed!(
            settings,
            get_active_display,
            set_active_display,
            active_display.clone()
        );
        set_if_changed!(
            settings,
            get_highlighted_display,
            set_highlighted_display,
            settings_display_choice(nav.display_highlighted)
        );
        set_if_changed!(
            settings,
            get_display_transaction,
            set_display_transaction,
            display_transaction_state(settings_transaction_phase(
                nav.display_confirm_remaining,
                nav.display_confirm_busy,
                nav.display_error.as_deref()
            ))
        );
        set_if_changed!(
            settings,
            get_display_confirm_remaining,
            set_display_confirm_remaining,
            nav.display_confirm_remaining as i32
        );
        set_if_changed!(
            settings,
            get_active_orientation,
            set_active_orientation,
            screen_orientation(nav.settings.screen_orientation)
        );
        set_if_changed!(
            settings,
            get_selected_orientation,
            set_selected_orientation,
            orientation_at(nav.orientation_selected)
        );
        set_if_changed!(
            settings,
            get_highlighted_orientation,
            set_highlighted_orientation,
            orientation_at(nav.orientation_highlighted)
        );
        set_if_changed!(
            settings,
            get_orientation_transaction,
            set_orientation_transaction,
            display_transaction_state(settings_transaction_phase(
                nav.orientation_confirm_remaining,
                nav.orientation_confirm_busy,
                nav.orientation_error.as_deref()
            ))
        );
        set_if_changed!(
            settings,
            get_orientation_confirm_remaining,
            set_orientation_confirm_remaining,
            nav.orientation_confirm_remaining as i32
        );
        set_if_changed!(
            settings,
            get_simple_joystick_handling,
            set_simple_joystick_handling,
            nav.settings.simple_joystick_handling
        );
        set_if_changed!(
            settings,
            get_reduce_motion,
            set_reduce_motion,
            nav.settings.reduce_motion
        );
        set_if_changed!(
            settings,
            get_screensaver_enabled,
            set_screensaver_enabled,
            nav.settings.screensaver_enabled
        );
        set_if_changed!(
            settings,
            get_screensaver_delay_minutes,
            set_screensaver_delay_minutes,
            nav.settings.screensaver_delay_minutes as i32
        );
        set_if_changed!(
            settings,
            get_selected_license_index,
            set_selected_license_index,
            nav.licenses_selected as i32
        );
        set_if_changed!(
            settings,
            get_license_scroll_y,
            set_license_scroll_y,
            nav.licenses_scroll_y()
        );
        let license_lines_key = (nav.licenses_selected, nav.license_viewport());
        if self.settings.license_lines_key != Some(license_lines_key) {
            let lines = self.license_lines(nav.licenses_selected, nav.license_viewport());
            settings.set_license_lines(lines);
        }

        if let Some(catalog_version) = catalog_version {
            let key = (catalog_version, nav.current_menu_id().to_string());
            if self.navigation.menu_items_key.as_ref() != Some(&key) {
                let menu_items = self.menu_items(nav, catalog_version);
                let menu_item_presentation = self.menu_item_presentation();
                bridge_churn_record_model_replacements(2);
                navigation.set_menu_item_presentation(menu_item_presentation);
                navigation.set_menu_items(menu_items);
            }
        }
        self.sync_menu_item_state(nav);
        self.publish_selection_feedback(&app.global::<FeedbackView>());

        let games = active_game_view(catalog, nav);
        let count = active_count(catalog, nav, games.len());
        let arcade = app.global::<ArcadeView>();
        let device = device_kind(nav.device_kind());
        set_if_changed!(arcade, get_device, set_device, device);
        if self.arcade_device_installed != Some(device) {
            let index = device as usize;
            let image = self.device_images[index]
                .get_or_insert_with(|| device_image(nav.device_kind()))
                .clone();
            arcade.set_device_backdrop(image);
            self.arcade_device_installed = Some(device);
        }
        set_view_string_if_changed!(
            arcade,
            get_collection_title,
            set_collection_title,
            nav.active_collection().map_or_else(
                || "ARCADE".to_owned(),
                |collection| collection.title.to_uppercase()
            )
        );
        set_if_changed!(
            arcade,
            get_list_mode,
            set_list_mode,
            arcade_list_mode(nav.arcade_user_list_mode())
        );
        set_if_changed!(
            arcade,
            get_load_state,
            set_load_state,
            active_games_load_state(catalog, nav)
        );
        set_if_changed!(arcade, get_active_count, set_active_count, count as i32);
        if !(defer_arcade_overlay && nav.screen == Screen::Arcade) {
            set_if_changed!(
                arcade,
                get_selected_game_index,
                set_selected_game_index,
                nav.arcade.selected as i32
            );
            set_view_string_if_changed!(
                arcade,
                get_selected_game_id,
                set_selected_game_id,
                games
                    .get(nav.arcade.selected)
                    .map(|game| game.mra_path.as_ref())
                    .unwrap_or("")
            );
        }
        sync_arcade_search(&arcade, nav);
        arcade.set_drawer_open(nav.screen == Screen::Arcade && nav.arcade_filter.drawer_open);
        set_view_string_if_changed!(
            arcade,
            get_drawer_title,
            set_drawer_title,
            nav.arcade_filter.title()
        );
        set_if_changed!(
            arcade,
            get_drawer_selected_index,
            set_drawer_selected_index,
            nav.arcade_filter.selected as i32
        );
        set_view_string_if_changed!(
            arcade,
            get_active_filter_label,
            set_active_filter_label,
            nav.arcade_filter.active_label()
        );
        let projection =
            (nav.screen == Screen::Arcade && nav.arcade_filter.drawer_open).then(|| {
                nav.arcade_filter_projection(catalog, nav.active_collection_scope_id(catalog))
            });
        let unchanged = match (&self.drawer_projection, &projection) {
            (Some(previous), Some(current)) => std::sync::Arc::ptr_eq(previous, current),
            (None, None) => self.drawer_initialized,
            _ => false,
        };
        if !unchanged {
            let drawer_items = projection.as_ref().map_or_else(Vec::new, |items| {
                items
                    .iter()
                    .map(|item| SharedString::from(item.label.as_str()))
                    .collect()
            });
            arcade.set_drawer_items(ModelRc::from(Rc::new(VecModel::from(drawer_items))));
            bridge_churn_record_model_replacements(1);
            self.drawer_projection = projection;
            self.drawer_initialized = true;
        }
    }

    pub fn menu_items(&mut self, nav: &LauncherNav, catalog_version: usize) -> ModelRc<MenuItem> {
        let key = (catalog_version, nav.current_menu_id().to_string());
        if self.navigation.menu_items_key.as_ref() != Some(&key) {
            let feedback = self.navigation.selection_feedback.stamp();
            self.navigation.menu_items = Some(build_menu_items(nav));
            self.navigation.menu_item_presentation =
                Some(build_menu_item_presentation(nav, &feedback));
            self.navigation.menu_items_key = Some(key);
            self.navigation.projected_selected_index = Some(nav.selected);
            self.navigation.projected_selection_feedback = feedback;
        }
        ModelRc::from(
            self.navigation
                .menu_items
                .as_ref()
                .expect("launcher menu model initialized")
                .clone(),
        )
    }

    pub fn menu_item_presentation(&self) -> ModelRc<MenuItemPresentation> {
        ModelRc::from(
            self.navigation
                .menu_item_presentation
                .as_ref()
                .expect("launcher menu presentation initialized")
                .clone(),
        )
    }

    #[cfg(feature = "ui")]
    pub(crate) fn republish_cached_menu_models(&self, app: &Launcher) {
        let (Some(items), Some(presentation)) = (
            self.navigation.menu_items.as_ref(),
            self.navigation.menu_item_presentation.as_ref(),
        ) else {
            return;
        };
        let navigation = app.global::<NavigationView>();
        bridge_churn_record_model_replacements(2);
        navigation.set_menu_items(ModelRc::from(items.clone()));
        navigation.set_menu_item_presentation(ModelRc::from(presentation.clone()));
    }

    pub fn license_lines(
        &mut self,
        index: usize,
        viewport: crate::licenses::LicenseViewport,
    ) -> ModelRc<SharedString> {
        let key = (index, viewport);
        if self.settings.license_lines_key != Some(key) {
            let lines = crate::licenses::wrapped_lines(index, viewport)
                .iter()
                .map(|line| SharedString::from(line.as_str()))
                .collect::<Vec<_>>();
            self.settings.license_lines = Some(Rc::new(VecModel::from(lines)));
            self.settings.license_lines_key = Some(key);
        }
        ModelRc::from(
            self.settings
                .license_lines
                .as_ref()
                .expect("license line model initialized")
                .clone(),
        )
    }

    pub fn sync_selection_feedback_surface(
        &mut self,
        target: Option<&SelectionFeedbackTarget>,
    ) -> bool {
        self.navigation.selection_feedback.sync_surface(target)
    }

    pub fn note_selection_feedback_change(
        &mut self,
        before: Option<&SelectionFeedbackTarget>,
        after: Option<&SelectionFeedbackTarget>,
    ) -> bool {
        let Some(after) = after else {
            return false;
        };
        if before.is_some_and(|before| before.surface == after.surface && before.item != after.item)
        {
            self.navigation.selection_feedback.register(after.clone())
        } else {
            false
        }
    }

    pub fn expire_selection_feedback(&mut self, now: Instant) -> bool {
        self.navigation.selection_feedback.expire_due(now)
    }

    pub fn selection_feedback_stamp(&self) -> SelectionFeedbackStamp {
        self.navigation.projected_selection_feedback.clone()
    }

    pub fn confirm_selection_feedback(
        &mut self,
        stamp: &SelectionFeedbackStamp,
        confirmed_at: Instant,
    ) -> Vec<SelectionFeedbackConfirmation> {
        self.navigation
            .selection_feedback
            .confirm(stamp, confirmed_at)
    }

    fn sync_menu_item_state(&mut self, nav: &LauncherNav) {
        let stamp = self.navigation.selection_feedback.stamp();
        if self.navigation.projected_selected_index == Some(nav.selected)
            && self.navigation.projected_selection_feedback == stamp
        {
            return;
        }
        if let Some(model) = self.navigation.menu_item_presentation.as_ref() {
            if self.navigation.projected_selection_feedback == stamp {
                if let Some(previous) = self.navigation.projected_selected_index {
                    sync_menu_item_presentation_row(model, nav, &stamp, previous);
                }
                if self.navigation.projected_selected_index != Some(nav.selected) {
                    sync_menu_item_presentation_row(model, nav, &stamp, nav.selected);
                }
            } else {
                for index in 0..model.row_count() {
                    sync_menu_item_presentation_row(model, nav, &stamp, index);
                }
            }
        }
        self.navigation.projected_selected_index = Some(nav.selected);
        self.navigation.projected_selection_feedback = stamp;
    }

    fn publish_selection_feedback(&mut self, feedback: &FeedbackView) {
        if !self.navigation.selection_feedback_callback_installed {
            let published = self.navigation.published_selection_feedback.clone();
            feedback.on_acknowledged(move |surface, item, _revision| {
                published.borrow().entries.iter().any(|entry| {
                    entry.target.surface == surface.as_str() && entry.target.item == item.as_str()
                })
            });
            self.navigation.selection_feedback_callback_installed = true;
        }
        if *self.navigation.published_selection_feedback.borrow()
            != self.navigation.projected_selection_feedback
        {
            *self.navigation.published_selection_feedback.borrow_mut() =
                self.navigation.projected_selection_feedback.clone();
            feedback.set_revision(self.navigation.projected_selection_feedback.revision as i32);
        }
    }
}

fn sync_menu_item_presentation_row(
    model: &VecModel<MenuItemPresentation>,
    nav: &LauncherNav,
    stamp: &SelectionFeedbackStamp,
    index: usize,
) {
    let Some(mut row) = model.row_data(index) else {
        return;
    };
    let selected = index == nav.selected;
    let item_id = nav
        .current_menu_items()
        .get(index)
        .map(|item| item.id.as_str())
        .unwrap_or_default();
    let acknowledged = stamp
        .entries
        .iter()
        .any(|entry| entry.target.surface == nav.current_menu_id() && entry.target.item == item_id);
    if row.selected != selected || row.acknowledged != acknowledged {
        row.selected = selected;
        row.acknowledged = acknowledged;
        bridge_churn_record_row_mutations(1);
        model.set_row_data(index, row);
    }
}

fn settings_transaction_phase(
    remaining: u8,
    busy: bool,
    error: Option<&str>,
) -> DisplayTransactionPhase {
    if error.is_some() {
        DisplayTransactionPhase::Failed
    } else if busy {
        DisplayTransactionPhase::Persisting
    } else if remaining > 0 {
        DisplayTransactionPhase::Provisional
    } else {
        DisplayTransactionPhase::Idle
    }
}

fn build_menu_items(nav: &LauncherNav) -> Rc<VecModel<MenuItem>> {
    let allocation_started = Instant::now();
    let rows = nav
        .current_menu_items()
        .iter()
        .map(|item| {
            let presentation = nav.menu_item_catalog_presentation(item);
            MenuItem {
                id: item.id.clone().into(),
                label: item.title.clone().into(),
                subtitle: match presentation.status {
                    CatalogMenuItemStatus::Scanning => match item.kind {
                        LauncherMenuItemKind::Menu => {
                            let systems = nav.menu_discovered_system_count(&item.id);
                            format!(
                                "{systems} system{} found",
                                if systems == 1 { "" } else { "s" }
                            )
                            .into()
                        }
                        LauncherMenuItemKind::Collection => {
                            if presentation.available {
                                format!("{} games available", item.count).into()
                            } else {
                                "".into()
                            }
                        }
                    },
                    CatalogMenuItemStatus::Partial => "Some items failed".into(),
                    CatalogMenuItemStatus::UpdateFailed if presentation.available => {
                        format!("Update failed • {} games", item.count).into()
                    }
                    CatalogMenuItemStatus::UpdateFailed => "Update failed".into(),
                    CatalogMenuItemStatus::LoadFailed => "Load failed — A to retry".into(),
                    CatalogMenuItemStatus::Ready => format!("{} games", item.count).into(),
                },
                available: presentation.available,
                node_kind: match item.kind {
                    LauncherMenuItemKind::Menu => MenuItemKind::Group,
                    LauncherMenuItemKind::Collection => MenuItemKind::Collection,
                },
                status: match presentation.status {
                    CatalogMenuItemStatus::Ready => MenuItemStatus::Ready,
                    CatalogMenuItemStatus::Scanning => MenuItemStatus::Scanning,
                    CatalogMenuItemStatus::Partial => MenuItemStatus::Partial,
                    CatalogMenuItemStatus::UpdateFailed if presentation.available => {
                        MenuItemStatus::UpdateFailed
                    }
                    CatalogMenuItemStatus::UpdateFailed | CatalogMenuItemStatus::LoadFailed => {
                        MenuItemStatus::Failed
                    }
                },
            }
        })
        .collect::<Vec<_>>();
    bridge_churn_record_row_allocations(rows.len() as u64);
    bridge_churn_record_shared_strings(rows.len().saturating_mul(3) as u64);
    bridge_churn_record_model_allocation_us(allocation_started.elapsed().as_micros());
    Rc::new(VecModel::from(rows))
}

fn build_menu_item_presentation(
    nav: &LauncherNav,
    feedback: &SelectionFeedbackStamp,
) -> Rc<VecModel<MenuItemPresentation>> {
    let allocation_started = Instant::now();
    let rows = nav
        .current_menu_items()
        .iter()
        .enumerate()
        .map(|(index, item)| MenuItemPresentation {
            selected: index == nav.selected,
            acknowledged: feedback.entries.iter().any(|entry| {
                entry.target.surface == nav.current_menu_id() && entry.target.item == item.id
            }),
        })
        .collect::<Vec<_>>();
    bridge_churn_record_row_allocations(rows.len() as u64);
    bridge_churn_record_model_allocation_us(allocation_started.elapsed().as_micros());
    Rc::new(VecModel::from(rows))
}

fn active_game_view<'a>(catalog: &'a ArcadeCatalog, nav: &'a LauncherNav) -> ArcadeGameView<'a> {
    nav.active_collection()
        .map(|collection| nav.active_arcade_game_view(catalog, &collection.id))
        .unwrap_or_else(ArcadeGameView::empty)
}

fn active_count(catalog: &ArcadeCatalog, nav: &LauncherNav, fallback_count: usize) -> usize {
    let Some(collection) = nav.active_collection() else {
        return fallback_count;
    };
    let hydrated_count = nav.active_arcade_game_count(catalog, &collection.id);
    if hydrated_count == 0 && collection.count > 0 {
        collection.count
    } else {
        hydrated_count
    }
}

pub(crate) fn active_games_load_state(
    catalog: &ArcadeCatalog,
    nav: &LauncherNav,
) -> ArcadeLoadState {
    let Some(collection) = nav.active_collection() else {
        return ArcadeLoadState::Empty;
    };
    if nav.catalog_system_hydration_has_failed(&collection.id)
        || (catalog.system_game_count(&collection.id) == 0
            && nav.collection_update_has_failed(&collection.id))
    {
        ArcadeLoadState::Failed
    } else if nav.catalog_system_hydration_is_loading(&collection.id)
        || nav.collection_is_scanning(&collection.id)
    {
        ArcadeLoadState::Loading
    } else if catalog.system_game_count(&collection.id) > 0 {
        ArcadeLoadState::Ready
    } else if nav.collection_declared_count(&collection.id) > 0 {
        ArcadeLoadState::Failed
    } else {
        ArcadeLoadState::Empty
    }
}

fn sync_arcade_search(arcade: &ArcadeView, nav: &LauncherNav) {
    set_if_changed!(
        arcade,
        get_search_mode,
        set_search_mode,
        if nav.arcade_search.is_active(&nav.arcade_filter.active) {
            ArcadeSearchMode::Active
        } else {
            ArcadeSearchMode::Inactive
        }
    );
    set_view_string_if_changed!(
        arcade,
        get_search_query,
        set_search_query,
        &nav.arcade_search.query
    );
    set_view_string_if_changed!(
        arcade,
        get_search_suggestion,
        set_search_suggestion,
        &nav.arcade_search.suggestion
    );
    set_if_changed!(
        arcade,
        get_search_status,
        set_search_status,
        arcade_search_status(nav.arcade_search.status)
    );
    set_if_changed!(
        arcade,
        get_selected_search_key_index,
        set_selected_search_key_index,
        nav.arcade_search.selected_key as i32
    );
    set_if_changed!(
        arcade,
        get_search_pane,
        set_search_pane,
        arcade_search_pane(nav.arcade_search.pane)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arcade_status_tracks_explicit_hydration_and_empty_library() {
        let empty = ArcadeCatalog::new(std::path::PathBuf::new(), vec![], vec![]);
        let mut nav = LauncherNav::new();
        assert!(nav.open_default_arcade(&empty));
        assert_eq!(
            active_games_load_state(&empty, &nav),
            ArcadeLoadState::Empty
        );
        let registered = ArcadeCatalog::new(
            std::path::PathBuf::new(),
            vec![],
            vec![crate::test_support::arcade_system("arcade", 7)],
        );
        nav.sync_launcher_taxonomy(&registered);
        let id = crate::arcade_catalog::MENU_ARCADE_SYSTEM_ID;
        nav.catalog_system_hydration_started(id);
        assert_eq!(
            active_games_load_state(&registered, &nav),
            ArcadeLoadState::Loading
        );
        nav.catalog_system_hydration_failed(id);
        assert_eq!(
            active_games_load_state(&registered, &nav),
            ArcadeLoadState::Failed
        );
        assert_eq!(nav.screen, Screen::Arcade);
        nav.catalog_system_hydration_finished(id);
        // Declared rows with no usable shard is failure, not endless loading.
        assert_eq!(
            active_games_load_state(&registered, &nav),
            ArcadeLoadState::Failed
        );
        nav.sync_launcher_taxonomy(&empty);
        assert_eq!(
            active_games_load_state(&empty, &nav),
            ArcadeLoadState::Empty
        );
        assert_eq!(nav.screen, Screen::Arcade);
    }

    #[test]
    fn arcade_publication_clears_failure_and_scanning_is_explicit() {
        let catalog = ArcadeCatalog::new(std::path::PathBuf::new(), vec![], vec![]);
        let mut nav = LauncherNav::new();
        nav.open_default_arcade(&catalog);
        nav.catalog_system_scanning("arcade");
        assert_eq!(
            active_games_load_state(&catalog, &nav),
            ArcadeLoadState::Loading
        );
        nav.catalog_system_hydration_failed("arcade");
        assert_eq!(
            active_games_load_state(&catalog, &nav),
            ArcadeLoadState::Failed
        );
        nav.catalog_system_update_ready("arcade");
        assert_eq!(
            active_games_load_state(&catalog, &nav),
            ArcadeLoadState::Empty
        );
        assert_eq!(nav.screen, Screen::Arcade);
    }

    fn target(item: &str) -> SelectionFeedbackTarget {
        SelectionFeedbackTarget {
            surface: "menu:computers".to_string(),
            item: item.to_string(),
        }
    }

    #[test]
    fn feedback_clock_starts_on_exact_visible_confirmation() {
        let mut feedback = SelectionFeedback::default();
        feedback.register(target("apple-ii"));
        let visible_stamp = feedback.stamp();
        let origin = Instant::now();

        assert!(
            feedback
                .confirm(&SelectionFeedbackStamp::default(), origin)
                .is_empty()
        );
        assert!(!feedback.expire_due(origin + Duration::from_secs(1)));

        let confirmations = feedback.confirm(&visible_stamp, origin);
        assert!(matches!(
            confirmations.as_slice(),
            [SelectionFeedbackConfirmation::Visible { event_id: 1, .. }]
        ));
        assert!(!feedback.expire_due(origin + Duration::from_millis(79)));
        assert!(feedback.expire_due(origin + SELECTION_FEEDBACK_MIN_VISIBLE));

        let removal_stamp = feedback.stamp();
        assert!(
            feedback
                .confirm(&visible_stamp, origin + Duration::from_millis(81))
                .is_empty()
        );
        let confirmations = feedback.confirm(&removal_stamp, origin + Duration::from_millis(83));
        assert!(matches!(
            confirmations.as_slice(),
            [SelectionFeedbackConfirmation::Hidden { visible_for, .. }]
                if *visible_for >= SELECTION_FEEDBACK_MIN_VISIBLE
        ));
    }

    #[test]
    fn feedback_overlaps_and_reentry_rearms_from_confirmation() {
        let mut feedback = SelectionFeedback::default();
        let origin = Instant::now();
        feedback.register(target("apple-ii"));
        let first_stamp = feedback.stamp();
        feedback.confirm(&first_stamp, origin);

        feedback.register(target("commodore"));
        let overlap_stamp = feedback.stamp();
        assert_eq!(overlap_stamp.entries.len(), 2);
        feedback.confirm(&overlap_stamp, origin + Duration::from_millis(50));

        feedback.register(target("apple-ii"));
        let reentry_stamp = feedback.stamp();
        let apple_event = reentry_stamp
            .entries
            .iter()
            .find(|entry| entry.target.item == "apple-ii")
            .expect("re-entered Apple II feedback");
        assert_eq!(apple_event.event_id, 3);
        let confirmations = feedback.confirm(&reentry_stamp, origin + Duration::from_millis(70));
        assert!(matches!(
            confirmations.as_slice(),
            [
                SelectionFeedbackConfirmation::Visible { event_id: 3, .. },
                SelectionFeedbackConfirmation::Hidden { event_id: 1, .. }
            ]
        ));

        assert!(!feedback.expire_due(origin + Duration::from_millis(129)));
        assert!(feedback.expire_due(origin + Duration::from_millis(150)));
        let remaining = feedback.stamp();
        assert!(remaining.entries.is_empty());
    }

    #[test]
    fn replacing_surface_cancels_feedback_proven_never_visible() {
        let mut feedback = SelectionFeedback::default();
        feedback.register(target("apple-ii"));
        assert_eq!(feedback.stamp().entries.len(), 1);

        let other = SelectionFeedbackTarget {
            surface: "menu:consoles".to_string(),
            item: "nintendo".to_string(),
        };
        assert!(feedback.sync_surface(Some(&other)));
        let removal_stamp = feedback.stamp();
        assert!(removal_stamp.entries.is_empty());
        assert!(matches!(
            feedback.confirm(&removal_stamp, Instant::now()).as_slice(),
            [SelectionFeedbackConfirmation::Cancelled { event_id: 1, .. }]
        ));
    }

    #[test]
    fn replacing_surface_retires_physically_visible_feedback() {
        let mut feedback = SelectionFeedback::default();
        let origin = Instant::now();
        feedback.register(target("apple-ii"));
        let visible_stamp = feedback.stamp();
        feedback.confirm(&visible_stamp, origin);

        let other = SelectionFeedbackTarget::new("menu:consoles", "nintendo");
        assert!(feedback.sync_surface(Some(&other)));
        let removal_stamp = feedback.stamp();
        assert!(matches!(
            feedback
                .confirm(&removal_stamp, origin + Duration::from_millis(5))
                .as_slice(),
            [SelectionFeedbackConfirmation::Hidden { event_id: 1, .. }]
        ));
    }

    #[test]
    fn in_flight_visibility_survives_surface_retirement() {
        let mut feedback = SelectionFeedback::default();
        let origin = Instant::now();
        feedback.register(target("apple-ii"));
        let visible_stamp = feedback.stamp();

        let other = SelectionFeedbackTarget::new("menu:consoles", "nintendo");
        assert!(feedback.sync_surface(Some(&other)));
        let removal_stamp = feedback.stamp();
        assert!(matches!(
            feedback.confirm(&visible_stamp, origin).as_slice(),
            [SelectionFeedbackConfirmation::Visible { event_id: 1, .. }]
        ));
        assert!(matches!(
            feedback
                .confirm(&removal_stamp, origin + Duration::from_millis(5))
                .as_slice(),
            [SelectionFeedbackConfirmation::Hidden { event_id: 1, .. }]
        ));
    }

    #[test]
    fn repeated_surface_changes_preserve_pending_retirement() {
        let mut feedback = SelectionFeedback::default();
        let origin = Instant::now();
        feedback.register(target("apple-ii"));
        let visible_stamp = feedback.stamp();
        feedback.confirm(&visible_stamp, origin);

        let consoles = SelectionFeedbackTarget::new("menu:consoles", "nintendo");
        let settings = SelectionFeedbackTarget::new("settings", "audio");
        assert!(feedback.sync_surface(Some(&consoles)));
        assert!(!feedback.sync_surface(Some(&settings)));
        let removal_stamp = feedback.stamp();
        assert!(matches!(
            feedback
                .confirm(&removal_stamp, origin + Duration::from_millis(5))
                .as_slice(),
            [SelectionFeedbackConfirmation::Hidden { event_id: 1, .. }]
        ));
    }

    #[test]
    fn complete_computers_path_keeps_every_destination_pending() {
        let mut feedback = SelectionFeedback::default();
        let path = [
            "apple-ii",
            "commodore",
            "atari",
            "sinclair",
            "coco2",
            "dos",
            "japanese",
            "other",
        ];
        for item in path {
            feedback.register(target(item));
        }

        let stamp = feedback.stamp();
        assert_eq!(stamp.entries.len(), path.len());
        assert_eq!(
            stamp
                .entries
                .iter()
                .map(|entry| entry.target.item.as_str())
                .collect::<Vec<_>>(),
            path
        );
    }

    #[test]
    fn unchanged_boundaries_and_replaced_surfaces_do_not_register_feedback() {
        let mut presenter = LauncherViewPresenters::default();
        let apple = target("apple-ii");
        let consoles = SelectionFeedbackTarget::new("menu:consoles", "nintendo");

        assert!(!presenter.note_selection_feedback_change(Some(&apple), Some(&apple)));
        assert!(!presenter.note_selection_feedback_change(None, Some(&apple)));
        assert!(!presenter.note_selection_feedback_change(Some(&apple), None));
        assert!(!presenter.note_selection_feedback_change(Some(&apple), Some(&consoles)));
        assert!(presenter.selection_feedback_stamp().entries.is_empty());
    }
}
