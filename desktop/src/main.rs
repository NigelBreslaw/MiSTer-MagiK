mod agent_client;
mod app_state;
mod file_icons;
mod frame_profile;
mod library;
#[cfg(target_os = "macos")]
mod macos_titlebar;
mod sd_card;

use agent_client::{
    connect_framebuffer_stream, connect_framebuffer_stream_seeded, drain_framebuffer_stream,
    fetch_dashboard, fetch_framebuffer_capture, fetch_sd_directory, FramebufferStreamControl,
};
use app_state::{DashboardSnapshot, DEFAULT_HOST};
use sd_card::SdCardBrowser;
#[cfg(feature = "compiled-ui")]
use sd_card::SdTreeRow;
#[cfg(feature = "live-ui")]
use slint::ComponentHandle;
use std::collections::VecDeque;
use std::error::Error;
#[cfg(feature = "live-ui")]
use std::path::Path;
use std::path::PathBuf;
#[cfg(feature = "live-ui")]
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

type SharedSdBrowser = Arc<Mutex<SdCardBrowser>>;
type SharedLibraryBrowser = Arc<Mutex<LibraryBrowser>>;
type SharedFramebufferCapture = Arc<Mutex<Option<agent_client::FramebufferCapture>>>;
type SharedLiveStreamGeneration = Arc<AtomicU64>;
type SharedFramebufferStreamControl = Arc<Mutex<Option<(u64, FramebufferStreamControl)>>>;

const DIRTY_RECT_LINGER_FRAMES: usize = 8;
const MAX_DIRTY_RECT_OVERLAYS: usize = 12;

#[derive(Clone, Debug)]
struct DirtyRectOverlayState {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    kind: String,
}

#[cfg(feature = "compiled-ui")]
slint::include_modules!();

#[derive(Clone, Debug)]
struct LibraryBrowser {
    catalog: Option<library::LibraryCatalog>,
    query: library::LibraryQuery,
    selected_game_id: String,
    status: String,
    warning: String,
    last_error: String,
    loading: bool,
}

impl LibraryBrowser {
    fn new() -> Self {
        Self {
            catalog: None,
            query: library::LibraryQuery::default(),
            selected_game_id: String::new(),
            status: "Sync the MiSTer library database to browse games.".to_string(),
            warning: String::new(),
            last_error: String::new(),
            loading: false,
        }
    }

    fn start_sync(&mut self) {
        self.loading = true;
        self.status = "Copying the MagiK library database from the MiSTer...".to_string();
        self.warning.clear();
        self.last_error.clear();
    }

    fn apply_sync_result(&mut self, result: Result<library::LibrarySyncResult, String>) {
        self.loading = false;
        match result {
            Ok(result) => {
                self.catalog = Some(result.catalog);
                self.query.page = 1;
                self.selected_game_id = self
                    .current_view()
                    .and_then(|view| view.rows.first().map(|game| game.id.clone()))
                    .unwrap_or_default();
                self.status = result.status;
                self.warning = result.warning;
                self.last_error.clear();
            }
            Err(err) => {
                self.last_error = err;
                self.status = "Library sync failed.".to_string();
                self.warning.clear();
            }
        }
    }

    fn set_sort(&mut self, column_id: &str, direction_id: &str) {
        let Some(column) = library_sort_column(column_id) else {
            return;
        };
        let direction = library_sort_direction(direction_id);
        self.query.sort_column = column;
        self.query.sort_direction = direction;
        self.query.page = 1;
        self.normalize_selection();
    }

    fn set_query(&mut self, search: &str) {
        self.query.search = search.to_string();
        self.query.page = 1;
        self.normalize_selection();
    }

    fn set_filter(&mut self, filter: &str, value: &str) {
        match filter {
            "system" => self.query.system = value.to_string(),
            "category" => self.query.category = value.to_string(),
            "region" => self.query.region = value.to_string(),
            "manufacturer" => self.query.manufacturer = value.to_string(),
            "preview" => self.query.preview = value.to_string(),
            "confidence" => self.query.confidence = value.to_string(),
            _ => return,
        }
        self.query.page = 1;
        self.normalize_selection();
    }

    fn set_page(&mut self, page: i32) {
        self.query.page = usize::try_from(page).unwrap_or(1).max(1);
        self.normalize_selection();
    }

    fn select_row(&mut self, id: &str) {
        if let Some(catalog) = &self.catalog {
            if library::selected_game(catalog, id).is_some() {
                self.selected_game_id = id.to_string();
            }
        }
    }

    fn selected_game(&self) -> Option<&library::LibraryGame> {
        self.catalog
            .as_ref()
            .and_then(|catalog| library::selected_game(catalog, &self.selected_game_id))
    }

    fn current_view(&self) -> Option<library::LibraryView> {
        self.catalog
            .as_ref()
            .map(|catalog| library::apply_library_query(catalog, &self.query))
    }

    fn normalize_selection(&mut self) {
        let Some(view) = self.current_view() else {
            self.selected_game_id.clear();
            return;
        };
        if view
            .rows
            .iter()
            .any(|game| game.id == self.selected_game_id)
        {
            return;
        }
        self.selected_game_id = view
            .rows
            .first()
            .map(|game| game.id.clone())
            .unwrap_or_default();
    }

    fn result_summary(&self) -> String {
        match (&self.catalog, self.current_view()) {
            (Some(catalog), Some(view)) => {
                format!(
                    "{} of {} games in the local library snapshot.",
                    view.total_count,
                    catalog.games.len()
                )
            }
            _ => "No library loaded.".to_string(),
        }
    }
}

#[derive(Clone, Debug)]
struct LibrarySelectOptionItem {
    value: String,
    label: String,
    enabled: bool,
}

#[derive(Clone, Debug, Default)]
struct LibraryDetailSections {
    overview: Vec<LibraryDetailRow>,
    system: Vec<LibraryDetailRow>,
    media: Vec<LibraryDetailRow>,
    launch: Vec<LibraryDetailRow>,
    identity: Vec<LibraryDetailRow>,
    paths: Vec<LibraryDetailRow>,
}

#[derive(Clone, Debug)]
struct LibraryDetailRow {
    field: String,
    value: String,
}

fn library_sort_column(column_id: &str) -> Option<library::LibrarySortColumn> {
    match column_id {
        "title" => Some(library::LibrarySortColumn::Title),
        "system" => Some(library::LibrarySortColumn::System),
        "year" => Some(library::LibrarySortColumn::Year),
        "manufacturer" => Some(library::LibrarySortColumn::Manufacturer),
        "category" => Some(library::LibrarySortColumn::Category),
        "preview" => Some(library::LibrarySortColumn::Preview),
        "discovered" => Some(library::LibrarySortColumn::Discovered),
        _ => None,
    }
}

fn library_select_options(all_label: &str, values: &[String]) -> Vec<LibrarySelectOptionItem> {
    let mut options = Vec::with_capacity(values.len() + 1);
    options.push(LibrarySelectOptionItem {
        value: String::new(),
        label: all_label.to_string(),
        enabled: true,
    });
    options.extend(values.iter().map(|value| LibrarySelectOptionItem {
        value: value.clone(),
        label: value.clone(),
        enabled: true,
    }));
    options
}

fn library_preview_options() -> Vec<LibrarySelectOptionItem> {
    [
        ("", "All previews"),
        ("with-preview", "With preview"),
        ("missing-preview", "Missing preview"),
    ]
    .into_iter()
    .map(|(value, label)| LibrarySelectOptionItem {
        value: value.to_string(),
        label: label.to_string(),
        enabled: true,
    })
    .collect()
}

fn library_option_index(options: &[LibrarySelectOptionItem], value: &str) -> i32 {
    options
        .iter()
        .position(|option| option.value == value)
        .and_then(|index| i32::try_from(index).ok())
        .unwrap_or(0)
}

fn library_discovered_label(value: &str) -> String {
    if value.is_empty() {
        "-".to_string()
    } else {
        value.to_string()
    }
}

fn library_preview_label(has_preview: bool) -> &'static str {
    if has_preview {
        "Preview"
    } else {
        "Missing"
    }
}

fn library_detail_sections(game: Option<&library::LibraryGame>) -> LibraryDetailSections {
    let Some(game) = game else {
        return LibraryDetailSections::default();
    };
    let mut sections = LibraryDetailSections::default();
    push_detail(&mut sections.overview, "Title", &game.title);
    push_detail(&mut sections.overview, "Category", &game.category);
    push_detail(&mut sections.overview, "Manufacturer", &game.manufacturer);
    push_detail(&mut sections.overview, "Year", &game.year);
    push_detail(&mut sections.overview, "Region", &game.region);
    push_detail(
        &mut sections.overview,
        "Region confidence",
        &game.region_confidence,
    );
    push_detail(&mut sections.overview, "Confidence", &game.confidence);
    push_detail(
        &mut sections.overview,
        "Discovered",
        &library_discovered_label(&game.discovered_at_unix),
    );

    push_detail(&mut sections.system, "System", &game.system_title);
    push_detail(&mut sections.system, "System ID", &game.system_id);
    push_detail(&mut sections.system, "Core ID", &game.core_id);
    push_detail(&mut sections.system, "Hardware ID", &game.hardware_id);

    push_detail(
        &mut sections.media,
        "Preview",
        if game.has_preview {
            "Available"
        } else {
            "Missing"
        },
    );
    push_detail(&mut sections.media, "Preview key", &game.preview_asset_key);

    push_detail(&mut sections.launch, "Launch kind", &game.launch_kind);
    push_detail(&mut sections.launch, "Launch ref", &game.launch_ref);
    push_detail(
        &mut sections.launch,
        "Launch ID",
        &game.launch_id.to_string(),
    );

    push_detail(&mut sections.identity, "Setname", &game.setname);
    push_detail(&mut sections.identity, "Parent", &game.parent);
    for (index, identity) in game.identities.iter().enumerate() {
        let mut parts = Vec::new();
        push_part(&mut parts, &identity.identity_id);
        push_part(&mut parts, &identity.metadata_title);
        push_part(&mut parts, &identity.family_id);
        push_part(&mut parts, &identity.year);
        push_part(&mut parts, &identity.manufacturer);
        push_part(&mut parts, &identity.category);
        push_part(&mut parts, &identity.source);
        if !parts.is_empty() {
            let label = if identity.namespace.is_empty() {
                format!("Identity {}", index + 1)
            } else {
                identity.namespace.clone()
            };
            push_detail(&mut sections.identity, &label, &parts.join(" / "));
        }
    }

    push_detail(&mut sections.paths, "Source path", &game.source_path);
    push_detail(&mut sections.paths, "Payload path", &game.payload_path);
    sections
}

fn push_detail(rows: &mut Vec<LibraryDetailRow>, field: &str, value: &str) {
    let value = value.trim();
    if value.is_empty() || value == "-" {
        return;
    }
    rows.push(LibraryDetailRow {
        field: field.to_string(),
        value: value.to_string(),
    });
}

fn push_part(parts: &mut Vec<String>, value: &str) {
    let value = value.trim();
    if !value.is_empty() {
        parts.push(value.to_string());
    }
}

fn library_sort_direction(direction_id: &str) -> library::LibrarySortDirection {
    match direction_id {
        "descending" | "Descending" => library::LibrarySortDirection::Descending,
        _ => library::LibrarySortDirection::Ascending,
    }
}

fn library_sort_column_id(column: library::LibrarySortColumn) -> &'static str {
    match column {
        library::LibrarySortColumn::Title => "title",
        library::LibrarySortColumn::System => "system",
        library::LibrarySortColumn::Year => "year",
        library::LibrarySortColumn::Manufacturer => "manufacturer",
        library::LibrarySortColumn::Category => "category",
        library::LibrarySortColumn::Preview => "preview",
        library::LibrarySortColumn::Discovered => "discovered",
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    if let Some((mode, frames)) = framebuffer_stream_bench_args()? {
        run_framebuffer_stream_bench(mode, frames)?;
        return Ok(());
    }

    if std::env::var_os("SLINT_BACKEND").is_none() {
        std::env::set_var("SLINT_BACKEND", "winit-skia");
    }
    select_backend()?;

    #[cfg(feature = "live-ui")]
    {
        run_live_ui()
    }

    #[cfg(all(not(feature = "live-ui"), feature = "compiled-ui"))]
    {
        run_compiled_ui()
    }

    #[cfg(all(not(feature = "live-ui"), not(feature = "compiled-ui")))]
    {
        compile_error!("enable either live-ui or compiled-ui");
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FramebufferBenchMode {
    Poll,
    Stream,
    Drain,
    Dump(PathBuf),
}

impl FramebufferBenchMode {
    fn label(&self) -> &'static str {
        match self {
            Self::Poll => "poll",
            Self::Stream => "stream",
            Self::Drain => "drain",
            Self::Dump(_) => "dump",
        }
    }
}

fn framebuffer_stream_bench_args() -> Result<Option<(FramebufferBenchMode, u64)>, Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let Some(first) = args.first() else {
        return Ok(None);
    };
    let mode = match first.as_str() {
        "--framebuffer-stream-bench" => FramebufferBenchMode::Stream,
        "--framebuffer-poll-bench" => FramebufferBenchMode::Poll,
        "--framebuffer-stream-drain-bench" => FramebufferBenchMode::Drain,
        "--framebuffer-stream-dump" => {
            let dir = args
                .get(1)
                .ok_or("--framebuffer-stream-dump needs OUT_DIR [FRAMES]")?;
            FramebufferBenchMode::Dump(PathBuf::from(dir))
        }
        _ => return Ok(None),
    };
    let frame_arg_index = if matches!(mode, FramebufferBenchMode::Dump(_)) {
        2
    } else {
        1
    };
    let frames = match args.get(frame_arg_index) {
        Some(value) => value.parse::<u64>()?,
        None => 120,
    };
    Ok(Some((mode, frames.max(1))))
}

fn run_framebuffer_stream_bench(
    mode: FramebufferBenchMode,
    frames: u64,
) -> Result<(), Box<dyn Error>> {
    let host = std::env::var("MISTER_IP").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let started = Instant::now();
    let mut latencies = Vec::with_capacity(frames as usize);
    let mut payload_bytes = 0_u64;
    let mut raw_bytes = 0_u64;
    match mode {
        FramebufferBenchMode::Stream => {
            let mut stream = connect_framebuffer_stream(&host)?;
            for _ in 0..frames {
                let frame_started = Instant::now();
                let capture = stream.next_capture()?;
                let _image = framebuffer_capture_image(&capture);
                latencies.push(frame_started.elapsed());
                payload_bytes += capture.payload_bytes;
                raw_bytes += capture.raw_bytes;
            }
        }
        FramebufferBenchMode::Drain => {
            let stats = drain_framebuffer_stream(&host, frames)?;
            latencies = stats.latencies;
            payload_bytes = stats.payload_bytes;
            raw_bytes = stats.raw_bytes;
        }
        FramebufferBenchMode::Poll => {
            for _ in 0..frames {
                let frame_started = Instant::now();
                let capture = fetch_framebuffer_capture(&host)?;
                let _image = framebuffer_capture_image(&capture);
                latencies.push(frame_started.elapsed());
                payload_bytes += capture.payload_bytes;
                raw_bytes += capture.raw_bytes;
            }
        }
        FramebufferBenchMode::Dump(ref dir) => {
            std::fs::create_dir_all(&dir)?;
            let seed_capture = fetch_framebuffer_capture(&host).ok();
            if let Some(capture) = seed_capture.as_ref() {
                let png = framebuffer_capture_png_bytes(capture)?;
                std::fs::write(dir.join("frame-0000-seed.png"), png)?;
            }
            let mut stream = connect_framebuffer_stream_seeded(&host, seed_capture.as_ref())?;
            for idx in 0..frames {
                let frame_started = Instant::now();
                let capture = stream.next_capture()?;
                let png = framebuffer_capture_png_bytes(&capture)?;
                std::fs::write(dir.join(format!("frame-{idx:04}.png")), png)?;
                latencies.push(frame_started.elapsed());
                payload_bytes += capture.payload_bytes;
                raw_bytes += capture.raw_bytes;
            }
        }
    }
    latencies.sort();
    let elapsed = started.elapsed();
    let fps = frames as f64 / elapsed.as_secs_f64();
    let p50 = latency_percentile_ms(&latencies, 0.50);
    let p95 = latency_percentile_ms(&latencies, 0.95);
    let payload_avg = payload_bytes / frames;
    let raw_avg = raw_bytes / frames;
    println!(
        "framebuffer_stream_bench_tsv\tmode={}\tframes={frames}\tfps={fps:.2}\telapsed_ms={:.0}\tp50_ms={p50:.1}\tp95_ms={p95:.1}\tavg_payload_bytes={payload_avg}\tavg_raw_bytes={raw_avg}",
        mode.label(),
        elapsed.as_secs_f64() * 1000.0
    );
    Ok(())
}

fn latency_percentile_ms(latencies: &[Duration], percentile: f64) -> f64 {
    if latencies.is_empty() {
        return 0.0;
    }
    let rank = ((latencies.len() - 1) as f64 * percentile).round() as usize;
    latencies[rank].as_secs_f64() * 1000.0
}

fn select_backend() -> Result<(), slint::PlatformError> {
    let selector = slint::BackendSelector::new();
    #[cfg(target_os = "macos")]
    let selector =
        selector.with_winit_window_attributes_hook(macos_titlebar::apply_unified_titlebar);
    selector.select()
}

#[cfg(feature = "live-ui")]
fn run_live_ui() -> Result<(), Box<dyn Error>> {
    let ui_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("ui/main.slint");
    let host = std::env::var("MISTER_IP").unwrap_or_else(|_| DEFAULT_HOST.to_string());

    loop {
        let reload_requested = Arc::new(AtomicBool::new(false));
        let stop_watcher = Arc::new(AtomicBool::new(false));
        let instance = create_live_instance(&ui_path, &host)?;
        start_reload_watcher(
            &ui_path,
            Arc::clone(&reload_requested),
            Arc::clone(&stop_watcher),
        );
        instance.run()?;
        stop_watcher.store(true, Ordering::Relaxed);
        if !reload_requested.load(Ordering::Relaxed) {
            break;
        }
    }

    Ok(())
}

#[cfg(feature = "live-ui")]
fn create_live_instance(
    ui_path: &Path,
    host: &str,
) -> Result<slint_interpreter::ComponentInstance, Box<dyn Error>> {
    use slint::ComponentHandle;
    use slint_interpreter::{Compiler, Value};

    let compiler = Compiler::default();
    let result = spin_on::spin_on(compiler.build_from_path(ui_path));
    result.print_diagnostics();
    if result.has_errors() {
        return Err("Slint UI has compile errors".into());
    }
    let definition = result
        .component("AppWindow")
        .ok_or("ui/main.slint must export AppWindow")?;
    let instance = definition.create()?;
    let sd_browser = Arc::new(Mutex::new(SdCardBrowser::new()));
    let library_browser = Arc::new(Mutex::new(LibraryBrowser::new()));
    let framebuffer_capture = Arc::new(Mutex::new(None));
    let live_stream_generation = Arc::new(AtomicU64::new(0));
    let live_stream_control = Arc::new(Mutex::new(None));

    let refresh_instance = instance.as_weak();
    let refresh_host = host.to_string();
    instance.set_global_callback("Actions", "refresh-status", move |_| {
        if let Some(instance) = refresh_instance.upgrade() {
            let snapshot = fetch_dashboard(&refresh_host);
            apply_live_snapshot(&instance, &snapshot);
        }
        Value::Void
    })?;

    let select_instance = instance.as_weak();
    instance.set_global_callback("Actions", "select-page", move |args| {
        if let Some(instance) = select_instance.upgrade() {
            if let Some(Value::String(page)) = args.first() {
                let _ = instance.set_global_property(
                    "AppState",
                    "selected-page",
                    Value::String(page.clone()),
                );
            }
        }
        Value::Void
    })?;

    let capture_instance = instance.as_weak();
    let capture_host = host.to_string();
    let capture_state = Arc::clone(&framebuffer_capture);
    instance.set_global_callback("Actions", "capture-framebuffer", move |_| {
        if let Some(instance) = capture_instance.upgrade() {
            set_live_analytics_loading(&instance);
        }
        spawn_live_framebuffer_capture(
            capture_instance.clone(),
            Arc::clone(&capture_state),
            capture_host.clone(),
        );
        Value::Void
    })?;

    let save_instance = instance.as_weak();
    let save_capture = Arc::clone(&framebuffer_capture);
    instance.set_global_callback("Actions", "save-framebuffer-image", move |_| {
        if let Some(instance) = save_instance.upgrade() {
            apply_live_save_status(&instance, "Saving framebuffer PNG...", "");
        }
        spawn_live_save_framebuffer_capture(save_instance.clone(), Arc::clone(&save_capture));
        Value::Void
    })?;

    let stream_instance = instance.as_weak();
    let stream_host = host.to_string();
    let stream_capture = Arc::clone(&framebuffer_capture);
    let stream_generation = Arc::clone(&live_stream_generation);
    let stream_control = Arc::clone(&live_stream_control);
    instance.set_global_callback("Actions", "live-stream-changed", move |args| {
        let Some(Value::Bool(enabled)) = args.first() else {
            return Value::Void;
        };
        let generation = stream_generation.fetch_add(1, Ordering::SeqCst) + 1;
        cancel_framebuffer_stream(&stream_control);
        if let Some(instance) = stream_instance.upgrade() {
            apply_live_stream_summary(
                &instance,
                if *enabled {
                    "Live stream starting..."
                } else {
                    "Live stream off."
                },
            );
        }
        if *enabled {
            spawn_live_framebuffer_stream(
                stream_instance.clone(),
                Arc::clone(&stream_capture),
                Arc::clone(&stream_generation),
                Arc::clone(&stream_control),
                stream_host.clone(),
                generation,
            );
        }
        Value::Void
    })?;

    let sd_toggle_instance = instance.as_weak();
    let sd_toggle_host = host.to_string();
    let sd_toggle_browser = Arc::clone(&sd_browser);
    instance.set_global_callback("Actions", "sd-row-toggle", move |args| {
        let Some(Value::String(path)) = args.first() else {
            return Value::Void;
        };
        if let Some(fetch_path) = sd_toggle_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.toggle_directory(path.as_str()))
        {
            let show_hidden = sd_toggle_browser
                .lock()
                .map(|browser| browser.show_hidden())
                .unwrap_or(false);
            spawn_live_sd_fetch(
                sd_toggle_instance.clone(),
                Arc::clone(&sd_toggle_browser),
                sd_toggle_host.clone(),
                fetch_path,
                show_hidden,
            );
        }
        if let Some(instance) = sd_toggle_instance.upgrade() {
            apply_live_sd_state(&instance, &sd_toggle_browser);
        }
        Value::Void
    })?;

    let sd_current_instance = instance.as_weak();
    let sd_current_browser = Arc::clone(&sd_browser);
    instance.set_global_callback("Actions", "sd-row-current", move |args| {
        if let Some(Value::String(path)) = args.first() {
            if let Ok(mut browser) = sd_current_browser.lock() {
                browser.select_path(path.as_str());
            }
            if let Some(instance) = sd_current_instance.upgrade() {
                apply_live_sd_state(&instance, &sd_current_browser);
            }
        }
        Value::Void
    })?;

    let sd_refresh_instance = instance.as_weak();
    let sd_refresh_host = host.to_string();
    let sd_refresh_browser = Arc::clone(&sd_browser);
    instance.set_global_callback("Actions", "sd-refresh-folder", move |_| {
        if let Some(fetch_path) = sd_refresh_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.refresh_current_folder())
        {
            let show_hidden = sd_refresh_browser
                .lock()
                .map(|browser| browser.show_hidden())
                .unwrap_or(false);
            spawn_live_sd_fetch(
                sd_refresh_instance.clone(),
                Arc::clone(&sd_refresh_browser),
                sd_refresh_host.clone(),
                fetch_path,
                show_hidden,
            );
        }
        if let Some(instance) = sd_refresh_instance.upgrade() {
            apply_live_sd_state(&instance, &sd_refresh_browser);
        }
        Value::Void
    })?;

    let sd_hidden_instance = instance.as_weak();
    let sd_hidden_host = host.to_string();
    let sd_hidden_browser = Arc::clone(&sd_browser);
    instance.set_global_callback("Actions", "sd-show-hidden-changed", move |args| {
        let Some(Value::Bool(show_hidden)) = args.first() else {
            return Value::Void;
        };
        if let Some(fetch_path) = sd_hidden_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.set_show_hidden(*show_hidden))
        {
            spawn_live_sd_fetch(
                sd_hidden_instance.clone(),
                Arc::clone(&sd_hidden_browser),
                sd_hidden_host.clone(),
                fetch_path,
                *show_hidden,
            );
        }
        if let Some(instance) = sd_hidden_instance.upgrade() {
            apply_live_sd_state(&instance, &sd_hidden_browser);
        }
        Value::Void
    })?;

    let library_sync_instance = instance.as_weak();
    let library_sync_host = host.to_string();
    let library_sync_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "sync-library", move |_| {
        if let Ok(mut browser) = library_sync_browser.lock() {
            if browser.loading {
                return Value::Void;
            }
            browser.start_sync();
        }
        if let Some(instance) = library_sync_instance.upgrade() {
            apply_live_library_state(&instance, &library_sync_browser);
        }
        spawn_live_library_sync(
            library_sync_instance.clone(),
            Arc::clone(&library_sync_browser),
            library_sync_host.clone(),
        );
        Value::Void
    })?;

    let library_query_instance = instance.as_weak();
    let library_query_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-query-changed", move |args| {
        if let Some(Value::String(query)) = args.first() {
            if let Ok(mut browser) = library_query_browser.lock() {
                browser.set_query(query.as_str());
            }
            if let Some(instance) = library_query_instance.upgrade() {
                apply_live_library_state(&instance, &library_query_browser);
            }
        }
        Value::Void
    })?;
    let library_filter_instance = instance.as_weak();
    let library_filter_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-filter-changed", move |args| {
        let (Some(Value::String(filter)), Some(Value::String(value))) = (args.first(), args.get(1))
        else {
            return Value::Void;
        };
        if let Ok(mut browser) = library_filter_browser.lock() {
            browser.set_filter(filter.as_str(), value.as_str());
        }
        if let Some(instance) = library_filter_instance.upgrade() {
            apply_live_library_state(&instance, &library_filter_browser);
        }
        Value::Void
    })?;
    let library_sort_instance = instance.as_weak();
    let library_sort_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-sort-toggled", move |args| {
        let Some(Value::String(column)) = args.first() else {
            return Value::Void;
        };
        let direction = match args.get(1) {
            Some(Value::EnumerationValue(_, direction)) => direction.as_str(),
            _ => "ascending",
        };
        if let Ok(mut browser) = library_sort_browser.lock() {
            browser.set_sort(column.as_str(), direction);
        }
        if let Some(instance) = library_sort_instance.upgrade() {
            apply_live_library_state(&instance, &library_sort_browser);
        }
        Value::Void
    })?;
    let library_page_instance = instance.as_weak();
    let library_page_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-page-changed", move |args| {
        if let Some(Value::Number(page)) = args.first() {
            if let Ok(mut browser) = library_page_browser.lock() {
                browser.set_page(*page as i32);
            }
            if let Some(instance) = library_page_instance.upgrade() {
                apply_live_library_state(&instance, &library_page_browser);
            }
        }
        Value::Void
    })?;
    let library_row_instance = instance.as_weak();
    let library_row_browser = Arc::clone(&library_browser);
    instance.set_global_callback("Actions", "library-row-selected", move |args| {
        if let Some(Value::String(id)) = args.first() {
            if let Ok(mut browser) = library_row_browser.lock() {
                browser.select_row(id.as_str());
            }
            if let Some(instance) = library_row_instance.upgrade() {
                apply_live_library_state(&instance, &library_row_browser);
            }
        }
        Value::Void
    })?;

    let drag_instance = instance.as_weak();
    instance.set_global_callback("WindowActions", "start-window-drag", move |_| {
        if let Some(instance) = drag_instance.upgrade() {
            start_window_drag(instance.window());
        }
        Value::Void
    })?;

    let snapshot = fetch_dashboard(host);
    apply_live_snapshot(&instance, &snapshot);
    apply_live_sd_state(&instance, &sd_browser);
    apply_live_library_state(&instance, &library_browser);
    #[cfg(target_os = "macos")]
    setup_macos_titlebar_for_live_instance(&instance);
    Ok(instance)
}

#[cfg(feature = "live-ui")]
fn apply_live_snapshot(
    instance: &slint_interpreter::ComponentInstance,
    snapshot: &DashboardSnapshot,
) {
    use slint::SharedString;
    use slint_interpreter::Value;

    fn set(instance: &slint_interpreter::ComponentInstance, name: &str, value: &str) {
        let _ = instance.set_global_property(
            "DeviceState",
            name,
            Value::String(SharedString::from(value)),
        );
    }

    set(instance, "host", &snapshot.host);
    set(instance, "connection-state", &snapshot.connection_state);
    set(instance, "agent-status", &snapshot.agent_status);
    set(instance, "token-source", &snapshot.token_source);
    set(instance, "agent-version", &snapshot.agent_version);
    set(instance, "agent-uptime", &snapshot.agent_uptime);
    set(instance, "network-summary", &snapshot.network_summary);
    set(instance, "mac-address", &snapshot.mac_address);
    set(instance, "main-process", &snapshot.main_process);
    set(instance, "launcher-process", &snapshot.launcher_process);
    set(instance, "launcher-state", &snapshot.launcher_state);
    set(instance, "visible-owner", &snapshot.visible_owner);
    set(
        instance,
        "slint-status-freshness",
        &snapshot.slint_status_freshness,
    );
    set(instance, "catalog-summary", &snapshot.catalog_summary);
    set(instance, "screen-summary", &snapshot.screen_summary);
    set(instance, "input-summary", &snapshot.input_summary);
    set(instance, "last-error", &snapshot.last_error);
}

#[cfg(feature = "live-ui")]
fn apply_live_sd_state(instance: &slint_interpreter::ComponentInstance, browser: &SharedSdBrowser) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::Value;

    let Ok(browser) = browser.lock() else {
        return;
    };

    fn set(instance: &slint_interpreter::ComponentInstance, name: &str, value: Value) {
        let _ = instance.set_global_property("SdCardState", name, value);
    }

    set(
        instance,
        "current-path",
        Value::String(SharedString::from(browser.current_path())),
    );
    set(
        instance,
        "status",
        Value::String(SharedString::from(browser.status())),
    );
    set(
        instance,
        "last-error",
        Value::String(SharedString::from(browser.last_error())),
    );
    set(instance, "loading", Value::Bool(browser.loading()));
    set(instance, "show-hidden", Value::Bool(browser.show_hidden()));

    let rows = browser
        .rows()
        .iter()
        .map(|row| Value::Struct(live_tree_row_struct(row)))
        .collect::<Vec<_>>();
    set(
        instance,
        "rows",
        Value::Model(ModelRc::new(VecModel::from(rows))),
    );
}

#[cfg(feature = "live-ui")]
fn apply_live_library_state(
    instance: &slint_interpreter::ComponentInstance,
    browser: &SharedLibraryBrowser,
) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::Value;

    let Ok(browser) = browser.lock() else {
        return;
    };

    fn set(instance: &slint_interpreter::ComponentInstance, name: &str, value: Value) {
        let _ = instance.set_global_property("LibraryState", name, value);
    }

    set(instance, "updating_controls", Value::Bool(true));
    set(
        instance,
        "status",
        Value::String(SharedString::from(browser.status.as_str())),
    );
    set(
        instance,
        "warning",
        Value::String(SharedString::from(browser.warning.as_str())),
    );
    set(
        instance,
        "last-error",
        Value::String(SharedString::from(browser.last_error.as_str())),
    );
    set(instance, "loading", Value::Bool(browser.loading));
    set(
        instance,
        "query",
        Value::String(SharedString::from(browser.query.search.as_str())),
    );
    set(
        instance,
        "system-filter",
        Value::String(SharedString::from(browser.query.system.as_str())),
    );
    set(
        instance,
        "category-filter",
        Value::String(SharedString::from(browser.query.category.as_str())),
    );
    set(
        instance,
        "region-filter",
        Value::String(SharedString::from(browser.query.region.as_str())),
    );
    set(
        instance,
        "manufacturer-filter",
        Value::String(SharedString::from(browser.query.manufacturer.as_str())),
    );
    set(
        instance,
        "preview-filter",
        Value::String(SharedString::from(browser.query.preview.as_str())),
    );
    set(
        instance,
        "confidence-filter",
        Value::String(SharedString::from(browser.query.confidence.as_str())),
    );
    set(
        instance,
        "sort-column",
        Value::String(SharedString::from(library_sort_column_id(
            browser.query.sort_column,
        ))),
    );
    set(
        instance,
        "sort-direction",
        Value::EnumerationValue(
            "DataTableSortDirection".to_string(),
            match browser.query.sort_direction {
                library::LibrarySortDirection::Ascending => "ascending".to_string(),
                library::LibrarySortDirection::Descending => "descending".to_string(),
            },
        ),
    );
    set(
        instance,
        "result-summary",
        Value::String(SharedString::from(browser.result_summary().as_str())),
    );
    let view = browser.current_view().unwrap_or_default();
    set(instance, "page", Value::Number(view.page as f64));
    set(
        instance,
        "page-count",
        Value::Number(view.page_count as f64),
    );
    set(
        instance,
        "selected-game-id",
        Value::String(SharedString::from(browser.selected_game_id.as_str())),
    );
    if let Some(game) = browser.selected_game() {
        set(
            instance,
            "detail-title",
            Value::String(SharedString::from(game.title.as_str())),
        );
        set(
            instance,
            "detail-subtitle",
            Value::String(SharedString::from(
                format!("{} · {}", game.system_title, game.id).as_str(),
            )),
        );
    } else {
        set(
            instance,
            "detail-title",
            Value::String(SharedString::from("No game selected")),
        );
        set(
            instance,
            "detail-subtitle",
            Value::String(SharedString::from("Select a row to inspect catalog facts.")),
        );
    }
    let detail_sections = library_detail_sections(browser.selected_game());
    set_live_library_detail_rows(instance, "detail-overview-rows", &detail_sections.overview);
    set_live_library_detail_rows(instance, "detail-system-rows", &detail_sections.system);
    set_live_library_detail_rows(instance, "detail-media-rows", &detail_sections.media);
    set_live_library_detail_rows(instance, "detail-launch-rows", &detail_sections.launch);
    set_live_library_detail_rows(instance, "detail-identity-rows", &detail_sections.identity);
    set_live_library_detail_rows(instance, "detail-path-rows", &detail_sections.paths);
    set(
        instance,
        "rows",
        Value::Model(ModelRc::new(VecModel::from(
            view.rows
                .iter()
                .map(|game| live_library_row_struct(game, false))
                .collect::<Vec<_>>(),
        ))),
    );
    set(
        instance,
        "compact-rows",
        Value::Model(ModelRc::new(VecModel::from(
            view.rows
                .iter()
                .map(|game| live_library_row_struct(game, true))
                .collect::<Vec<_>>(),
        ))),
    );
    let (
        system_options,
        category_options,
        region_options,
        manufacturer_options,
        confidence_options,
    ) = match &browser.catalog {
        Some(catalog) => (
            library_select_options("All systems", &catalog.systems),
            library_select_options("All categories", &catalog.categories),
            library_select_options("All regions", &catalog.regions),
            library_select_options("All manufacturers", &catalog.manufacturers),
            library_select_options("All confidence", &catalog.confidences),
        ),
        None => (
            library_select_options("All systems", &[]),
            library_select_options("All categories", &[]),
            library_select_options("All regions", &[]),
            library_select_options("All manufacturers", &[]),
            library_select_options("All confidence", &[]),
        ),
    };
    let preview_options = library_preview_options();
    set_live_library_options(instance, "system-options", &system_options);
    set_live_library_options(instance, "category-options", &category_options);
    set_live_library_options(instance, "region-options", &region_options);
    set_live_library_options(instance, "manufacturer-options", &manufacturer_options);
    set_live_library_options(instance, "preview-options", &preview_options);
    set_live_library_options(instance, "confidence-options", &confidence_options);
    set(
        instance,
        "system-index",
        Value::Number(library_option_index(&system_options, &browser.query.system) as f64),
    );
    set(
        instance,
        "category-index",
        Value::Number(library_option_index(&category_options, &browser.query.category) as f64),
    );
    set(
        instance,
        "region-index",
        Value::Number(library_option_index(&region_options, &browser.query.region) as f64),
    );
    set(
        instance,
        "manufacturer-index",
        Value::Number(
            library_option_index(&manufacturer_options, &browser.query.manufacturer) as f64,
        ),
    );
    set(
        instance,
        "preview-index",
        Value::Number(library_option_index(&preview_options, &browser.query.preview) as f64),
    );
    set(
        instance,
        "confidence-index",
        Value::Number(library_option_index(&confidence_options, &browser.query.confidence) as f64),
    );
    set(instance, "updating_controls", Value::Bool(false));
}

#[cfg(feature = "live-ui")]
fn set_live_library_detail_rows(
    instance: &slint_interpreter::ComponentInstance,
    name: &str,
    rows: &[LibraryDetailRow],
) {
    use slint::{ModelRc, VecModel};
    use slint_interpreter::Value;

    let values = rows
        .iter()
        .enumerate()
        .map(|(index, row)| live_library_detail_row_struct(index, row))
        .collect::<Vec<_>>();
    let _ = instance.set_global_property(
        "LibraryState",
        name,
        Value::Model(ModelRc::new(VecModel::from(values))),
    );
}

#[cfg(feature = "live-ui")]
fn live_library_detail_row_struct(
    index: usize,
    row: &LibraryDetailRow,
) -> slint_interpreter::Value {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::{Struct, Value};

    Value::Struct(Struct::from_iter([
        (
            "id".to_string(),
            Value::String(SharedString::from(format!("detail-{index}").as_str())),
        ),
        (
            "cells".to_string(),
            Value::Model(ModelRc::new(VecModel::from(vec![
                live_library_text_cell(&row.field),
                live_library_text_cell(&row.value),
            ]))),
        ),
    ]))
}

#[cfg(feature = "live-ui")]
fn set_live_library_options(
    instance: &slint_interpreter::ComponentInstance,
    name: &str,
    options: &[LibrarySelectOptionItem],
) {
    use slint::{ModelRc, VecModel};
    use slint_interpreter::Value;

    let values = options
        .iter()
        .map(live_select_option_struct)
        .collect::<Vec<_>>();
    let _ = instance.set_global_property(
        "LibraryState",
        name,
        Value::Model(ModelRc::new(VecModel::from(values))),
    );
}

#[cfg(feature = "live-ui")]
fn live_select_option_struct(option: &LibrarySelectOptionItem) -> slint_interpreter::Value {
    use slint::SharedString;
    use slint_interpreter::{Struct, Value};

    Value::Struct(Struct::from_iter([
        (
            "value".to_string(),
            Value::String(SharedString::from(option.value.as_str())),
        ),
        (
            "label".to_string(),
            Value::String(SharedString::from(option.label.as_str())),
        ),
        ("enabled".to_string(), Value::Bool(option.enabled)),
    ]))
}

#[cfg(feature = "live-ui")]
fn live_library_row_struct(game: &library::LibraryGame, compact: bool) -> slint_interpreter::Value {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::{Struct, Value};

    let cells = if compact {
        vec![
            live_library_text_cell(&game.title),
            live_library_text_cell(&game.system_title),
            live_library_preview_cell(game.has_preview),
        ]
    } else {
        vec![
            live_library_text_cell(&game.title),
            live_library_text_cell(&game.system_title),
            live_library_text_cell(&game.year),
            live_library_text_cell(&game.manufacturer),
            live_library_text_cell(&game.category),
            live_library_preview_cell(game.has_preview),
            live_library_text_cell(&library_discovered_label(&game.discovered_at_unix)),
        ]
    };

    Value::Struct(Struct::from_iter([
        (
            "id".to_string(),
            Value::String(SharedString::from(game.id.as_str())),
        ),
        (
            "cells".to_string(),
            Value::Model(ModelRc::new(VecModel::from(cells))),
        ),
    ]))
}

#[cfg(feature = "live-ui")]
fn live_library_text_cell(text: &str) -> slint_interpreter::Value {
    live_library_cell("text", text, "default")
}

#[cfg(feature = "live-ui")]
fn live_library_preview_cell(has_preview: bool) -> slint_interpreter::Value {
    live_library_cell(
        "label",
        library_preview_label(has_preview),
        if has_preview { "success" } else { "secondary" },
    )
}

#[cfg(feature = "live-ui")]
fn live_library_cell(kind: &str, text: &str, label_variant: &str) -> slint_interpreter::Value {
    use slint::{Image, SharedString};
    use slint_interpreter::{Struct, Value};

    Value::Struct(Struct::from_iter([
        (
            "kind".to_string(),
            Value::EnumerationValue("DataTableCellKind".to_string(), kind.to_string()),
        ),
        (
            "text".to_string(),
            Value::String(SharedString::from(if text.is_empty() { "-" } else { text })),
        ),
        (
            "label-variant".to_string(),
            Value::EnumerationValue("LabelVariant".to_string(), label_variant.to_string()),
        ),
        (
            "label-size".to_string(),
            Value::EnumerationValue("LabelSize".to_string(), "small".to_string()),
        ),
        ("icon".to_string(), Value::Image(Image::default())),
        (
            "icon-tint".to_string(),
            Value::EnumerationValue("DataTableIconTint".to_string(), "default".to_string()),
        ),
    ]))
}

#[cfg(feature = "live-ui")]
fn live_tree_row_struct(row: &sd_card::SdTreeRow) -> slint_interpreter::Struct {
    use slint::{Image, SharedString};
    use slint_interpreter::{Struct, Value};

    Struct::from_iter([
        (
            "id".to_string(),
            Value::String(SharedString::from(row.id.as_str())),
        ),
        (
            "label".to_string(),
            Value::String(SharedString::from(row.label.as_str())),
        ),
        ("level".to_string(), Value::Number(f64::from(row.level))),
        ("has-children".to_string(), Value::Bool(row.has_children)),
        ("expanded".to_string(), Value::Bool(row.expanded)),
        ("current".to_string(), Value::Bool(row.current)),
        (
            "leading-is-directory".to_string(),
            Value::Bool(row.leading_is_directory),
        ),
        ("has-leading-visual".to_string(), Value::Bool(true)),
        ("preserve-leading-icon-color".to_string(), Value::Bool(true)),
        (
            "trailing".to_string(),
            Value::EnumerationValue("TreeViewTrailingVisual".to_string(), "none".to_string()),
        ),
        ("has-leading-action".to_string(), Value::Bool(false)),
        ("show-leading-action-icon".to_string(), Value::Bool(false)),
        (
            "leading-action-icon".to_string(),
            Value::Image(Image::default()),
        ),
        (
            "leading-file-icon".to_string(),
            Value::Image(file_icons::material_icon(row.icon_key.as_str())),
        ),
        ("interactive".to_string(), Value::Bool(row.interactive)),
        ("is-skeleton".to_string(), Value::Bool(row.is_skeleton)),
        ("has-secondary-actions".to_string(), Value::Bool(false)),
        (
            "secondary-actions-badge".to_string(),
            Value::String(SharedString::from("")),
        ),
        (
            "loading-children-badge".to_string(),
            Value::String(SharedString::from(row.loading_children_badge.as_str())),
        ),
    ])
}

#[cfg(feature = "compiled-ui")]
fn run_compiled_ui() -> Result<(), Box<dyn Error>> {
    use slint::ComponentHandle;

    let host = std::env::var("MISTER_IP").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let ui = AppWindow::new()?;
    let sd_browser = Arc::new(Mutex::new(SdCardBrowser::new()));
    let library_browser = Arc::new(Mutex::new(LibraryBrowser::new()));
    let framebuffer_capture = Arc::new(Mutex::new(None));
    let live_stream_generation = Arc::new(AtomicU64::new(0));
    let live_stream_control = Arc::new(Mutex::new(None));
    let refresh_ui = ui.as_weak();
    let refresh_host = host.clone();
    ui.global::<Actions>().on_refresh_status(move || {
        if let Some(ui) = refresh_ui.upgrade() {
            let snapshot = fetch_dashboard(&refresh_host);
            apply_compiled_snapshot(&ui, &snapshot);
        }
    });

    let select_ui = ui.as_weak();
    ui.global::<Actions>().on_select_page(move |page| {
        if let Some(ui) = select_ui.upgrade() {
            ui.global::<AppState>().set_selected_page(page);
        }
    });

    let capture_ui = ui.as_weak();
    let capture_host = host.clone();
    let capture_state = Arc::clone(&framebuffer_capture);
    ui.global::<Actions>().on_capture_framebuffer(move || {
        if let Some(ui) = capture_ui.upgrade() {
            set_compiled_analytics_loading(&ui);
        }
        spawn_compiled_framebuffer_capture(
            capture_ui.clone(),
            Arc::clone(&capture_state),
            capture_host.clone(),
        );
    });

    let save_ui = ui.as_weak();
    let save_capture = Arc::clone(&framebuffer_capture);
    ui.global::<Actions>().on_save_framebuffer_image(move || {
        if let Some(ui) = save_ui.upgrade() {
            apply_compiled_save_status(&ui, "Saving framebuffer PNG...", "");
        }
        spawn_compiled_save_framebuffer_capture(save_ui.clone(), Arc::clone(&save_capture));
    });

    let stream_ui = ui.as_weak();
    let stream_host = host.clone();
    let stream_capture = Arc::clone(&framebuffer_capture);
    let stream_generation = Arc::clone(&live_stream_generation);
    let stream_control = Arc::clone(&live_stream_control);
    ui.global::<Actions>()
        .on_live_stream_changed(move |enabled| {
            let generation = stream_generation.fetch_add(1, Ordering::SeqCst) + 1;
            cancel_framebuffer_stream(&stream_control);
            if let Some(ui) = stream_ui.upgrade() {
                apply_compiled_stream_summary(
                    &ui,
                    if enabled {
                        "Live stream starting..."
                    } else {
                        "Live stream off."
                    },
                );
            }
            if enabled {
                spawn_compiled_framebuffer_stream(
                    stream_ui.clone(),
                    Arc::clone(&stream_capture),
                    Arc::clone(&stream_generation),
                    Arc::clone(&stream_control),
                    stream_host.clone(),
                    generation,
                );
            }
        });

    let sd_toggle_ui = ui.as_weak();
    let sd_toggle_host = host.clone();
    let sd_toggle_browser = Arc::clone(&sd_browser);
    ui.global::<Actions>().on_sd_row_toggle(move |path| {
        if let Some(fetch_path) = sd_toggle_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.toggle_directory(path.as_str()))
        {
            let show_hidden = sd_toggle_browser
                .lock()
                .map(|browser| browser.show_hidden())
                .unwrap_or(false);
            spawn_compiled_sd_fetch(
                sd_toggle_ui.clone(),
                Arc::clone(&sd_toggle_browser),
                sd_toggle_host.clone(),
                fetch_path,
                show_hidden,
            );
        }
        if let Some(ui) = sd_toggle_ui.upgrade() {
            apply_compiled_sd_state(&ui, &sd_toggle_browser);
        }
    });

    let sd_current_ui = ui.as_weak();
    let sd_current_browser = Arc::clone(&sd_browser);
    ui.global::<Actions>().on_sd_row_current(move |path| {
        if let Ok(mut browser) = sd_current_browser.lock() {
            browser.select_path(path.as_str());
        }
        if let Some(ui) = sd_current_ui.upgrade() {
            apply_compiled_sd_state(&ui, &sd_current_browser);
        }
    });

    let sd_refresh_ui = ui.as_weak();
    let sd_refresh_host = host.clone();
    let sd_refresh_browser = Arc::clone(&sd_browser);
    ui.global::<Actions>().on_sd_refresh_folder(move || {
        if let Some(fetch_path) = sd_refresh_browser
            .lock()
            .ok()
            .and_then(|mut browser| browser.refresh_current_folder())
        {
            let show_hidden = sd_refresh_browser
                .lock()
                .map(|browser| browser.show_hidden())
                .unwrap_or(false);
            spawn_compiled_sd_fetch(
                sd_refresh_ui.clone(),
                Arc::clone(&sd_refresh_browser),
                sd_refresh_host.clone(),
                fetch_path,
                show_hidden,
            );
        }
        if let Some(ui) = sd_refresh_ui.upgrade() {
            apply_compiled_sd_state(&ui, &sd_refresh_browser);
        }
    });

    let sd_hidden_ui = ui.as_weak();
    let sd_hidden_host = host.clone();
    let sd_hidden_browser = Arc::clone(&sd_browser);
    ui.global::<Actions>()
        .on_sd_show_hidden_changed(move |show_hidden| {
            if let Some(fetch_path) = sd_hidden_browser
                .lock()
                .ok()
                .and_then(|mut browser| browser.set_show_hidden(show_hidden))
            {
                spawn_compiled_sd_fetch(
                    sd_hidden_ui.clone(),
                    Arc::clone(&sd_hidden_browser),
                    sd_hidden_host.clone(),
                    fetch_path,
                    show_hidden,
                );
            }
            if let Some(ui) = sd_hidden_ui.upgrade() {
                apply_compiled_sd_state(&ui, &sd_hidden_browser);
            }
        });

    let library_sync_ui = ui.as_weak();
    let library_sync_host = host.clone();
    let library_sync_browser = Arc::clone(&library_browser);
    ui.global::<Actions>().on_sync_library(move || {
        if let Ok(mut browser) = library_sync_browser.lock() {
            if browser.loading {
                return;
            }
            browser.start_sync();
        }
        if let Some(ui) = library_sync_ui.upgrade() {
            apply_compiled_library_state(&ui, &library_sync_browser);
        }
        spawn_compiled_library_sync(
            library_sync_ui.clone(),
            Arc::clone(&library_sync_browser),
            library_sync_host.clone(),
        );
    });
    let library_query_ui = ui.as_weak();
    let library_query_browser = Arc::clone(&library_browser);
    ui.global::<Actions>()
        .on_library_query_changed(move |query| {
            if let Ok(mut browser) = library_query_browser.lock() {
                browser.set_query(query.as_str());
            }
            if let Some(ui) = library_query_ui.upgrade() {
                apply_compiled_library_state(&ui, &library_query_browser);
            }
        });
    let library_filter_ui = ui.as_weak();
    let library_filter_browser = Arc::clone(&library_browser);
    ui.global::<Actions>()
        .on_library_filter_changed(move |filter, value| {
            if let Ok(mut browser) = library_filter_browser.lock() {
                browser.set_filter(filter.as_str(), value.as_str());
            }
            if let Some(ui) = library_filter_ui.upgrade() {
                apply_compiled_library_state(&ui, &library_filter_browser);
            }
        });
    let library_sort_ui = ui.as_weak();
    let library_sort_browser = Arc::clone(&library_browser);
    ui.global::<Actions>()
        .on_library_sort_toggled(move |column, direction| {
            let direction_id = if direction == DataTableSortDirection::Descending {
                "descending"
            } else {
                "ascending"
            };
            if let Ok(mut browser) = library_sort_browser.lock() {
                browser.set_sort(column.as_str(), direction_id);
            }
            if let Some(ui) = library_sort_ui.upgrade() {
                apply_compiled_library_state(&ui, &library_sort_browser);
            }
        });
    let library_page_ui = ui.as_weak();
    let library_page_browser = Arc::clone(&library_browser);
    ui.global::<Actions>().on_library_page_changed(move |page| {
        if let Ok(mut browser) = library_page_browser.lock() {
            browser.set_page(page);
        }
        if let Some(ui) = library_page_ui.upgrade() {
            apply_compiled_library_state(&ui, &library_page_browser);
        }
    });
    let library_row_ui = ui.as_weak();
    let library_row_browser = Arc::clone(&library_browser);
    ui.global::<Actions>().on_library_row_selected(move |id| {
        if let Ok(mut browser) = library_row_browser.lock() {
            browser.select_row(id.as_str());
        }
        if let Some(ui) = library_row_ui.upgrade() {
            apply_compiled_library_state(&ui, &library_row_browser);
        }
    });

    let drag_ui = ui.as_weak();
    ui.global::<WindowActions>().on_start_window_drag(move || {
        if let Some(ui) = drag_ui.upgrade() {
            start_window_drag(ui.window());
        }
    });

    let snapshot = fetch_dashboard(&host);
    apply_compiled_snapshot(&ui, &snapshot);
    apply_compiled_sd_state(&ui, &sd_browser);
    apply_compiled_library_state(&ui, &library_browser);
    #[cfg(target_os = "macos")]
    setup_macos_titlebar_for_compiled_ui(&ui);
    ui.run()?;
    Ok(())
}

#[cfg(target_os = "macos")]
#[cfg(feature = "live-ui")]
fn setup_macos_titlebar_for_live_instance(instance: &slint_interpreter::ComponentInstance) {
    let instance_weak = instance.as_weak();
    slint::spawn_local(async move {
        let Some(instance) = instance_weak.upgrade() else {
            return;
        };
        let _ = macos_titlebar::setup_window(instance.window()).await;
    })
    .ok();
}

#[cfg(target_os = "macos")]
#[cfg(feature = "compiled-ui")]
fn setup_macos_titlebar_for_compiled_ui(ui: &AppWindow) {
    let ui_weak = ui.as_weak();
    slint::spawn_local(async move {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let _ = macos_titlebar::setup_window(ui.window()).await;
    })
    .ok();
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_snapshot(ui: &AppWindow, snapshot: &DashboardSnapshot) {
    let state = ui.global::<DeviceState>();
    state.set_host(snapshot.host.as_str().into());
    state.set_connection_state(snapshot.connection_state.as_str().into());
    state.set_agent_status(snapshot.agent_status.as_str().into());
    state.set_token_source(snapshot.token_source.as_str().into());
    state.set_agent_version(snapshot.agent_version.as_str().into());
    state.set_agent_uptime(snapshot.agent_uptime.as_str().into());
    state.set_network_summary(snapshot.network_summary.as_str().into());
    state.set_mac_address(snapshot.mac_address.as_str().into());
    state.set_main_process(snapshot.main_process.as_str().into());
    state.set_launcher_process(snapshot.launcher_process.as_str().into());
    state.set_launcher_state(snapshot.launcher_state.as_str().into());
    state.set_visible_owner(snapshot.visible_owner.as_str().into());
    state.set_slint_status_freshness(snapshot.slint_status_freshness.as_str().into());
    state.set_catalog_summary(snapshot.catalog_summary.as_str().into());
    state.set_screen_summary(snapshot.screen_summary.as_str().into());
    state.set_input_summary(snapshot.input_summary.as_str().into());
    state.set_last_error(snapshot.last_error.as_str().into());
}

#[cfg(feature = "live-ui")]
fn set_live_analytics_loading(instance: &slint_interpreter::ComponentInstance) {
    use slint::SharedString;
    use slint_interpreter::Value;

    let _ = instance.set_global_property("AnalyticsState", "loading", Value::Bool(true));
    let _ = instance.set_global_property(
        "AnalyticsState",
        "status",
        Value::String(SharedString::from("Capturing framebuffer stream...")),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "last-error",
        Value::String(SharedString::from("")),
    );
}

#[cfg(feature = "live-ui")]
fn apply_live_framebuffer_capture_result(
    instance: &slint_interpreter::ComponentInstance,
    result: Result<agent_client::FramebufferCapture, String>,
) {
    use slint::SharedString;
    use slint_interpreter::Value;

    let _ = instance.set_global_property("AnalyticsState", "loading", Value::Bool(false));
    match result {
        Ok(capture) => {
            let image = framebuffer_capture_image(&capture);
            let _ = instance.set_global_property(
                "AnalyticsState",
                "framebuffer-image",
                Value::Image(image),
            );
            let _ = instance.set_global_property("AnalyticsState", "has-image", Value::Bool(true));
            let _ =
                instance.set_global_property("AnalyticsState", "can-save-image", Value::Bool(true));
            let _ = instance.set_global_property(
                "AnalyticsState",
                "framebuffer-width",
                Value::Number(capture.width as f64),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "framebuffer-height",
                Value::Number(capture.height as f64),
            );
            clear_live_dirty_rects(instance);
            let _ = instance.set_global_property(
                "AnalyticsState",
                "status",
                Value::String(SharedString::from(framebuffer_capture_status(&capture))),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "last-error",
                Value::String(SharedString::from("")),
            );
        }
        Err(err) => {
            let _ = instance.set_global_property(
                "AnalyticsState",
                "status",
                Value::String(SharedString::from("Framebuffer capture failed.")),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "last-error",
                Value::String(SharedString::from(err)),
            );
        }
    }
}

#[cfg(feature = "compiled-ui")]
fn set_compiled_analytics_loading(ui: &AppWindow) {
    let state = ui.global::<AnalyticsState>();
    state.set_loading(true);
    state.set_status("Capturing framebuffer stream...".into());
    state.set_last_error("".into());
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_framebuffer_capture_result(
    ui: &AppWindow,
    result: Result<agent_client::FramebufferCapture, String>,
) {
    let state = ui.global::<AnalyticsState>();
    state.set_loading(false);
    match result {
        Ok(capture) => {
            state.set_framebuffer_image(framebuffer_capture_image(&capture));
            state.set_has_image(true);
            state.set_can_save_image(true);
            state.set_framebuffer_width(capture.width as i32);
            state.set_framebuffer_height(capture.height as i32);
            clear_compiled_dirty_rects(ui);
            state.set_status(framebuffer_capture_status(&capture).into());
            state.set_last_error("".into());
        }
        Err(err) => {
            state.set_status("Framebuffer capture failed.".into());
            state.set_last_error(err.into());
        }
    }
}

fn framebuffer_capture_status(capture: &agent_client::FramebufferCapture) -> String {
    format!(
        "Captured {}x{} {}bpp framebuffer ({} payload; {} raw; {}).",
        capture.width,
        capture.height,
        capture.bpp,
        format_byte_size(capture.payload_bytes),
        format_byte_size(capture.raw_bytes),
        capture.encoding
    )
}

fn framebuffer_capture_image(capture: &agent_client::FramebufferCapture) -> slint::Image {
    if capture.rgba_pixels.is_empty() && !capture.png_path.as_os_str().is_empty() {
        return slint::Image::load_from_path(&capture.png_path).unwrap_or_default();
    }
    let width = u32::try_from(capture.width).unwrap_or(0);
    let height = u32::try_from(capture.height).unwrap_or(0);
    if width == 0 || height == 0 {
        return slint::Image::default();
    }
    let mut pixels = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(width, height);
    let dst = pixels.make_mut_slice();
    if capture.rgba_pixels.len() != dst.len().saturating_mul(4) {
        return slint::Image::default();
    }
    for (pixel, rgba) in dst.iter_mut().zip(capture.rgba_pixels.chunks_exact(4)) {
        *pixel = slint::Rgba8Pixel {
            r: rgba[0],
            g: rgba[1],
            b: rgba[2],
            a: rgba[3],
        };
    }
    slint::Image::from_rgba8(pixels)
}

fn framebuffer_capture_png_bytes(
    capture: &agent_client::FramebufferCapture,
) -> Result<Vec<u8>, String> {
    if capture.rgba_pixels.is_empty() {
        if !capture.png_path.as_os_str().is_empty() {
            return std::fs::read(&capture.png_path).map_err(|err| {
                format!("read framebuffer PNG {}: {err}", capture.png_path.display())
            });
        }
        return Err("No framebuffer image is available to save.".to_string());
    }

    let width = u32::try_from(capture.width).map_err(|_| "framebuffer width too large")?;
    let height = u32::try_from(capture.height).map_err(|_| "framebuffer height too large")?;
    if width == 0 || height == 0 {
        return Err("No framebuffer image is available to save.".to_string());
    }
    let expected_len = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "framebuffer image dimensions are too large".to_string())?;
    if capture.rgba_pixels.len() != expected_len {
        return Err(format!(
            "framebuffer RGBA size mismatch expected={expected_len} actual={}",
            capture.rgba_pixels.len()
        ));
    }

    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|err| format!("write PNG header: {err}"))?;
        writer
            .write_image_data(&capture.rgba_pixels)
            .map_err(|err| format!("write PNG pixels: {err}"))?;
        writer
            .finish()
            .map_err(|err| format!("finish PNG: {err}"))?;
    }
    Ok(png_bytes)
}

fn save_framebuffer_capture_png(
    capture: &agent_client::FramebufferCapture,
) -> Result<PathBuf, String> {
    let png_bytes = framebuffer_capture_png_bytes(capture)?;
    let desktop = std::env::var_os("HOME")
        .map(PathBuf::from)
        .map(|home| home.join("Desktop"))
        .ok_or_else(|| "HOME is not set; cannot find the Desktop folder.".to_string())?;
    if !desktop.is_dir() {
        return Err(format!(
            "Desktop folder does not exist: {}",
            desktop.display()
        ));
    }
    let millis = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|err| format!("system clock before Unix epoch: {err}"))?
        .as_millis();
    let path = desktop.join(format!("mister-magik-framebuffer-{millis}.png"));
    std::fs::write(&path, png_bytes)
        .map_err(|err| format!("write framebuffer PNG {}: {err}", path.display()))?;
    Ok(path)
}

#[cfg(feature = "live-ui")]
fn apply_live_save_status(
    instance: &slint_interpreter::ComponentInstance,
    status: &str,
    last_error: &str,
) {
    use slint::SharedString;
    use slint_interpreter::Value;

    let _ = instance.set_global_property(
        "AnalyticsState",
        "status",
        Value::String(SharedString::from(status)),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "last-error",
        Value::String(SharedString::from(last_error)),
    );
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_save_status(ui: &AppWindow, status: &str, last_error: &str) {
    let state = ui.global::<AnalyticsState>();
    state.set_status(status.into());
    state.set_last_error(last_error.into());
}

#[cfg(feature = "live-ui")]
fn apply_live_stream_summary(instance: &slint_interpreter::ComponentInstance, summary: &str) {
    use slint::SharedString;
    use slint_interpreter::Value;

    let _ = instance.set_global_property(
        "AnalyticsState",
        "live-stream-summary",
        Value::String(SharedString::from(summary)),
    );
}

#[cfg(feature = "live-ui")]
fn apply_live_stream_disconnected(instance: &slint_interpreter::ComponentInstance, err: &str) {
    use slint::SharedString;
    use slint_interpreter::Value;

    let _ = instance.set_global_property("AnalyticsState", "live-stream", Value::Bool(false));
    let _ = instance.set_global_property(
        "AnalyticsState",
        "live-stream-summary",
        Value::String(SharedString::from("Live stream disconnected.")),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "last-error",
        Value::String(SharedString::from(err)),
    );
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_stream_summary(ui: &AppWindow, summary: &str) {
    ui.global::<AnalyticsState>()
        .set_live_stream_summary(summary.into());
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_stream_disconnected(ui: &AppWindow, err: &str) {
    let state = ui.global::<AnalyticsState>();
    state.set_live_stream(false);
    state.set_live_stream_summary("Live stream disconnected.".into());
    state.set_last_error(err.into());
}

fn cancel_framebuffer_stream(stream_control: &SharedFramebufferStreamControl) {
    if let Ok(mut active) = stream_control.lock() {
        if let Some((_, control)) = active.take() {
            control.shutdown();
        }
    }
}

fn register_framebuffer_stream(
    stream_control: &SharedFramebufferStreamControl,
    generation: u64,
    control: FramebufferStreamControl,
) {
    if let Ok(mut active) = stream_control.lock() {
        if let Some((_, old_control)) = active.replace((generation, control)) {
            old_control.shutdown();
        }
    }
}

fn unregister_framebuffer_stream(stream_control: &SharedFramebufferStreamControl, generation: u64) {
    if let Ok(mut active) = stream_control.lock() {
        if active
            .as_ref()
            .is_some_and(|(active_generation, _)| *active_generation == generation)
        {
            active.take();
        }
    }
}

fn framebuffer_stream_summary(frames: u64, elapsed: Duration, last_frame: Duration) -> String {
    let fps = if elapsed.is_zero() {
        0.0
    } else {
        frames as f64 / elapsed.as_secs_f64()
    };
    format!(
        "{fps:.1} fps avg, {:.0} ms last frame",
        last_frame.as_secs_f64() * 1000.0
    )
}

fn dirty_rect_from_stream_frame(
    frame: &agent_client::FramebufferStreamFrame,
) -> DirtyRectOverlayState {
    DirtyRectOverlayState {
        x: frame.rect.x.min(i32::MAX as u32) as i32,
        y: frame.rect.y.min(i32::MAX as u32) as i32,
        width: frame.rect.width.min(i32::MAX as u32) as i32,
        height: frame.rect.height.min(i32::MAX as u32) as i32,
        kind: match frame.kind {
            mister_magik_framebuffer_stream::FrameKind::Keyframe => "keyframe",
            mister_magik_framebuffer_stream::FrameKind::RectDelta => "delta",
            _ => "frame",
        }
        .to_string(),
    }
}

fn push_recent_dirty_rect(
    recent: &mut VecDeque<DirtyRectOverlayState>,
    frame: &agent_client::FramebufferStreamFrame,
) {
    recent.push_back(dirty_rect_from_stream_frame(frame));
    while recent.len() > DIRTY_RECT_LINGER_FRAMES || recent.len() > MAX_DIRTY_RECT_OVERLAYS {
        recent.pop_front();
    }
}

fn dirty_rect_summary(frame: &agent_client::FramebufferStreamFrame, visible_count: usize) -> String {
    let kind = match frame.kind {
        mister_magik_framebuffer_stream::FrameKind::Keyframe => "keyframe",
        mister_magik_framebuffer_stream::FrameKind::RectDelta => "delta",
        _ => "frame",
    };
    format!(
        "{kind} #{} rect {}x{}+{},{}; showing {} recent.",
        frame.sequence,
        frame.rect.width,
        frame.rect.height,
        frame.rect.x,
        frame.rect.y,
        visible_count
    )
}

#[cfg(feature = "live-ui")]
fn apply_live_dirty_rects(
    instance: &slint_interpreter::ComponentInstance,
    rects: &VecDeque<DirtyRectOverlayState>,
    summary: &str,
) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::{Struct, Value};

    let values = rects
        .iter()
        .map(|rect| {
            Value::Struct(Struct::from_iter([
                ("x".to_string(), Value::Number(rect.x as f64)),
                ("y".to_string(), Value::Number(rect.y as f64)),
                ("width".to_string(), Value::Number(rect.width as f64)),
                ("height".to_string(), Value::Number(rect.height as f64)),
                (
                    "kind".to_string(),
                    Value::String(SharedString::from(rect.kind.as_str())),
                ),
            ]))
        })
        .collect::<Vec<_>>();
    let _ = instance.set_global_property(
        "AnalyticsState",
        "dirty-rects",
        Value::Model(ModelRc::new(VecModel::from(values))),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "dirty-rect-summary",
        Value::String(SharedString::from(summary)),
    );
}

#[cfg(feature = "live-ui")]
fn clear_live_dirty_rects(instance: &slint_interpreter::ComponentInstance) {
    use slint::{ModelRc, SharedString, VecModel};
    use slint_interpreter::Value;

    let _ = instance.set_global_property(
        "AnalyticsState",
        "dirty-rects",
        Value::Model(ModelRc::new(VecModel::<Value>::from(Vec::new()))),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "dirty-rect-summary",
        Value::String(SharedString::from("Dirty overlay idle.")),
    );
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_dirty_rects(
    ui: &AppWindow,
    rects: &VecDeque<DirtyRectOverlayState>,
    summary: &str,
) {
    use slint::{ModelRc, SharedString, VecModel};

    let values = rects
        .iter()
        .map(|rect| DirtyRectOverlay {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
            kind: SharedString::from(rect.kind.as_str()),
        })
        .collect::<Vec<_>>();
    let state = ui.global::<AnalyticsState>();
    state.set_dirty_rects(ModelRc::new(VecModel::from(values)));
    state.set_dirty_rect_summary(summary.into());
}

#[cfg(feature = "compiled-ui")]
fn clear_compiled_dirty_rects(ui: &AppWindow) {
    use slint::{ModelRc, VecModel};

    let state = ui.global::<AnalyticsState>();
    state.set_dirty_rects(ModelRc::new(VecModel::<DirtyRectOverlay>::from(Vec::new())));
    state.set_dirty_rect_summary("Dirty overlay idle.".into());
}

fn format_byte_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / MB)
    } else if bytes >= 1024 {
        format!("{:.0} KB", bytes as f64 / KB)
    } else {
        format!("{bytes} B")
    }
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_sd_state(ui: &AppWindow, browser: &SharedSdBrowser) {
    use slint::{ModelRc, VecModel};

    let Ok(browser) = browser.lock() else {
        return;
    };
    let state = ui.global::<SdCardState>();
    state.set_current_path(browser.current_path().into());
    state.set_status(browser.status().into());
    state.set_last_error(browser.last_error().into());
    state.set_loading(browser.loading());
    state.set_show_hidden(browser.show_hidden());
    state.set_rows(ModelRc::new(VecModel::from(
        browser
            .rows()
            .iter()
            .map(compiled_tree_row)
            .collect::<Vec<_>>(),
    )));
}

#[cfg(feature = "compiled-ui")]
fn apply_compiled_library_state(ui: &AppWindow, browser: &SharedLibraryBrowser) {
    use slint::{ModelRc, VecModel};

    let Ok(browser) = browser.lock() else {
        return;
    };
    let state = ui.global::<LibraryState>();
    state.set_updating_controls(true);
    state.set_status(browser.status.as_str().into());
    state.set_warning(browser.warning.as_str().into());
    state.set_last_error(browser.last_error.as_str().into());
    state.set_loading(browser.loading);
    state.set_query(browser.query.search.as_str().into());
    state.set_system_filter(browser.query.system.as_str().into());
    state.set_category_filter(browser.query.category.as_str().into());
    state.set_region_filter(browser.query.region.as_str().into());
    state.set_manufacturer_filter(browser.query.manufacturer.as_str().into());
    state.set_preview_filter(browser.query.preview.as_str().into());
    state.set_confidence_filter(browser.query.confidence.as_str().into());
    state.set_sort_column(library_sort_column_id(browser.query.sort_column).into());
    state.set_sort_direction(match browser.query.sort_direction {
        library::LibrarySortDirection::Ascending => DataTableSortDirection::Ascending,
        library::LibrarySortDirection::Descending => DataTableSortDirection::Descending,
    });
    state.set_result_summary(browser.result_summary().as_str().into());
    let view = browser.current_view().unwrap_or_default();
    state.set_page(i32::try_from(view.page).unwrap_or(1));
    state.set_page_count(i32::try_from(view.page_count).unwrap_or(1));
    state.set_selected_game_id(browser.selected_game_id.as_str().into());
    if let Some(game) = browser.selected_game() {
        state.set_detail_title(game.title.as_str().into());
        state.set_detail_subtitle(format!("{} · {}", game.system_title, game.id).into());
    } else {
        state.set_detail_title("No game selected".into());
        state.set_detail_subtitle("Select a row to inspect catalog facts.".into());
    }
    let detail_sections = library_detail_sections(browser.selected_game());
    state.set_detail_overview_rows(compiled_library_detail_rows(&detail_sections.overview));
    state.set_detail_system_rows(compiled_library_detail_rows(&detail_sections.system));
    state.set_detail_media_rows(compiled_library_detail_rows(&detail_sections.media));
    state.set_detail_launch_rows(compiled_library_detail_rows(&detail_sections.launch));
    state.set_detail_identity_rows(compiled_library_detail_rows(&detail_sections.identity));
    state.set_detail_path_rows(compiled_library_detail_rows(&detail_sections.paths));
    state.set_rows(ModelRc::new(VecModel::from(
        view.rows
            .iter()
            .map(|game| compiled_library_row(game, false))
            .collect::<Vec<_>>(),
    )));
    state.set_compact_rows(ModelRc::new(VecModel::from(
        view.rows
            .iter()
            .map(|game| compiled_library_row(game, true))
            .collect::<Vec<_>>(),
    )));
    let (
        system_options,
        category_options,
        region_options,
        manufacturer_options,
        confidence_options,
    ) = match &browser.catalog {
        Some(catalog) => (
            library_select_options("All systems", &catalog.systems),
            library_select_options("All categories", &catalog.categories),
            library_select_options("All regions", &catalog.regions),
            library_select_options("All manufacturers", &catalog.manufacturers),
            library_select_options("All confidence", &catalog.confidences),
        ),
        None => (
            library_select_options("All systems", &[]),
            library_select_options("All categories", &[]),
            library_select_options("All regions", &[]),
            library_select_options("All manufacturers", &[]),
            library_select_options("All confidence", &[]),
        ),
    };
    let preview_options = library_preview_options();
    state.set_system_options(compiled_select_options(&system_options));
    state.set_category_options(compiled_select_options(&category_options));
    state.set_region_options(compiled_select_options(&region_options));
    state.set_manufacturer_options(compiled_select_options(&manufacturer_options));
    state.set_preview_options(compiled_select_options(&preview_options));
    state.set_confidence_options(compiled_select_options(&confidence_options));
    state.set_system_index(library_option_index(&system_options, &browser.query.system));
    state.set_category_index(library_option_index(
        &category_options,
        &browser.query.category,
    ));
    state.set_region_index(library_option_index(&region_options, &browser.query.region));
    state.set_manufacturer_index(library_option_index(
        &manufacturer_options,
        &browser.query.manufacturer,
    ));
    state.set_preview_index(library_option_index(
        &preview_options,
        &browser.query.preview,
    ));
    state.set_confidence_index(library_option_index(
        &confidence_options,
        &browser.query.confidence,
    ));
    state.set_updating_controls(false);
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_detail_rows(rows: &[LibraryDetailRow]) -> slint::ModelRc<DataTableRow> {
    use slint::{ModelRc, VecModel};

    ModelRc::new(VecModel::from(
        rows.iter()
            .enumerate()
            .map(|(index, row)| DataTableRow {
                id: format!("detail-{index}").into(),
                cells: ModelRc::new(VecModel::from(vec![
                    compiled_library_text_cell(&row.field),
                    compiled_library_text_cell(&row.value),
                ])),
            })
            .collect::<Vec<_>>(),
    ))
}

#[cfg(feature = "compiled-ui")]
fn compiled_select_options(options: &[LibrarySelectOptionItem]) -> slint::ModelRc<SelectOption> {
    use slint::{ModelRc, VecModel};

    ModelRc::new(VecModel::from(
        options
            .iter()
            .map(|option| SelectOption {
                value: option.value.as_str().into(),
                label: option.label.as_str().into(),
                enabled: option.enabled,
            })
            .collect::<Vec<_>>(),
    ))
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_row(game: &library::LibraryGame, compact: bool) -> DataTableRow {
    use slint::{ModelRc, VecModel};

    let cells = if compact {
        vec![
            compiled_library_text_cell(&game.title),
            compiled_library_text_cell(&game.system_title),
            compiled_library_preview_cell(game.has_preview),
        ]
    } else {
        vec![
            compiled_library_text_cell(&game.title),
            compiled_library_text_cell(&game.system_title),
            compiled_library_text_cell(&game.year),
            compiled_library_text_cell(&game.manufacturer),
            compiled_library_text_cell(&game.category),
            compiled_library_preview_cell(game.has_preview),
            compiled_library_text_cell(&library_discovered_label(&game.discovered_at_unix)),
        ]
    };

    DataTableRow {
        id: game.id.as_str().into(),
        cells: ModelRc::new(VecModel::from(cells)),
    }
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_text_cell(text: &str) -> DataTableCell {
    compiled_library_cell(DataTableCellKind::Text, text, LabelVariant::Default)
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_preview_cell(has_preview: bool) -> DataTableCell {
    compiled_library_cell(
        DataTableCellKind::Label,
        library_preview_label(has_preview),
        if has_preview {
            LabelVariant::Success
        } else {
            LabelVariant::Secondary
        },
    )
}

#[cfg(feature = "compiled-ui")]
fn compiled_library_cell(
    kind: DataTableCellKind,
    text: &str,
    label_variant: LabelVariant,
) -> DataTableCell {
    DataTableCell {
        kind,
        text: if text.is_empty() { "-" } else { text }.into(),
        label_variant,
        label_size: LabelSize::Small,
        icon: slint::Image::default(),
        icon_tint: DataTableIconTint::Default,
    }
}

#[cfg(feature = "compiled-ui")]
fn compiled_tree_row(row: &SdTreeRow) -> TreeViewRow {
    TreeViewRow {
        id: row.id.as_str().into(),
        label: row.label.as_str().into(),
        level: row.level,
        has_children: row.has_children,
        expanded: row.expanded,
        current: row.current,
        leading_is_directory: row.leading_is_directory,
        has_leading_visual: true,
        preserve_leading_icon_color: true,
        trailing: TreeViewTrailingVisual::None,
        has_leading_action: false,
        show_leading_action_icon: false,
        leading_action_icon: slint::Image::default(),
        leading_file_icon: file_icons::material_icon(row.icon_key.as_str()),
        interactive: row.interactive,
        is_skeleton: row.is_skeleton,
        has_secondary_actions: false,
        secondary_actions_badge: "".into(),
        loading_children_badge: row.loading_children_badge.as_str().into(),
    }
}

#[cfg(feature = "live-ui")]
fn spawn_live_library_sync(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    browser: SharedLibraryBrowser,
    host: String,
) {
    std::thread::spawn(move || {
        let result = library::sync_library_catalog(&host);
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_sync_result(result);
            }
            if let Some(instance) = instance.upgrade() {
                apply_live_library_state(&instance, &browser);
            }
        });
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_sd_fetch(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    browser: SharedSdBrowser,
    host: String,
    path: String,
    show_hidden: bool,
) {
    std::thread::spawn(move || {
        let result = fetch_sd_directory(&host, &path, show_hidden).map_err(|err| err.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_listing_if_current_policy(&path, show_hidden, result);
            }
            if let Some(instance) = instance.upgrade() {
                apply_live_sd_state(&instance, &browser);
            }
        });
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_framebuffer_capture(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    capture_state: SharedFramebufferCapture,
    host: String,
) {
    std::thread::spawn(move || {
        let result = fetch_framebuffer_capture(&host).map_err(|err| err.to_string());
        let capture = result.as_ref().ok().cloned();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(capture) = capture {
                if let Ok(mut state) = capture_state.lock() {
                    *state = Some(capture);
                }
            }
            if let Some(instance) = instance.upgrade() {
                apply_live_framebuffer_capture_result(&instance, result);
            }
        });
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_save_framebuffer_capture(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    capture_state: SharedFramebufferCapture,
) {
    std::thread::spawn(move || {
        let result = capture_state
            .lock()
            .ok()
            .and_then(|state| state.clone())
            .ok_or_else(|| "Capture a framebuffer before saving.".to_string())
            .and_then(|capture| save_framebuffer_capture_png(&capture));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(instance) = instance.upgrade() {
                match result {
                    Ok(path) => apply_live_save_status(
                        &instance,
                        &format!("Saved framebuffer PNG to {}.", path.display()),
                        "",
                    ),
                    Err(err) => {
                        apply_live_save_status(&instance, "Framebuffer PNG save failed.", &err)
                    }
                }
            }
        });
    });
}

#[cfg(feature = "live-ui")]
fn spawn_live_framebuffer_stream(
    instance: slint::Weak<slint_interpreter::ComponentInstance>,
    capture_state: SharedFramebufferCapture,
    stream_generation: SharedLiveStreamGeneration,
    stream_control: SharedFramebufferStreamControl,
    host: String,
    generation: u64,
) {
    std::thread::spawn(move || {
        let stream_start = Instant::now();
        let mut frames = 0_u64;
        let mut recent_dirty_rects = VecDeque::new();
        let seed_capture = fetch_framebuffer_capture(&host).ok();
        if let Some(capture) = seed_capture.clone() {
            let event_generation = Arc::clone(&stream_generation);
            let event_capture_state = Arc::clone(&capture_state);
            let event_instance = instance.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if event_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Ok(mut state) = event_capture_state.lock() {
                    *state = Some(capture.clone());
                }
                if let Some(instance) = event_instance.upgrade() {
                    apply_live_framebuffer_capture_result(&instance, Ok(capture));
                }
            });
        }
        let mut stream = match connect_framebuffer_stream_seeded(&host, seed_capture.as_ref()) {
            Ok(stream) => stream,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(instance) = instance.upgrade() {
                        apply_live_stream_disconnected(&instance, &err);
                    }
                });
                return;
            }
        };
        let control = match stream.control() {
            Ok(control) => control,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(instance) = instance.upgrade() {
                        apply_live_stream_disconnected(&instance, &err);
                    }
                });
                return;
            }
        };
        if stream_generation.load(Ordering::SeqCst) != generation {
            control.shutdown();
            return;
        }
        register_framebuffer_stream(&stream_control, generation, control);
        while stream_generation.load(Ordering::SeqCst) == generation {
            let frame_start = Instant::now();
            let result = stream.next_frame().map_err(|err| err.to_string());
            let frame_elapsed = frame_start.elapsed();
            if let Ok(frame) = &result {
                frames += 1;
                push_recent_dirty_rect(&mut recent_dirty_rects, frame);
            }
            let summary = result
                .as_ref()
                .map(|_| framebuffer_stream_summary(frames, stream_start.elapsed(), frame_elapsed))
                .unwrap_or_else(|_| "Live stream disconnected.".to_string());
            let dirty_summary = result
                .as_ref()
                .map(|frame| dirty_rect_summary(frame, recent_dirty_rects.len()))
                .unwrap_or_else(|_| "Dirty overlay idle.".to_string());
            let dirty_rects = recent_dirty_rects.clone();
            let capture = result.as_ref().ok().map(|frame| frame.capture.clone());
            let capture_result = result
                .as_ref()
                .map(|frame| frame.capture.clone())
                .map_err(Clone::clone);
            let should_continue = stream_generation.load(Ordering::SeqCst) == generation;
            let disconnected = result.is_err();
            let event_generation = Arc::clone(&stream_generation);
            let event_capture_state = Arc::clone(&capture_state);
            let event_instance = instance.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if event_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Some(capture) = capture {
                    if let Ok(mut state) = event_capture_state.lock() {
                        *state = Some(capture);
                    }
                }
                if let Some(instance) = event_instance.upgrade() {
                    if disconnected {
                        let err = result
                            .err()
                            .unwrap_or_else(|| "framebuffer stream disconnected".to_string());
                        apply_live_stream_disconnected(&instance, &err);
                    } else {
                        apply_live_framebuffer_capture_result(&instance, capture_result);
                        apply_live_dirty_rects(&instance, &dirty_rects, &dirty_summary);
                        apply_live_stream_summary(&instance, &summary);
                    }
                }
            });
            if !should_continue || disconnected {
                break;
            }
        }
        unregister_framebuffer_stream(&stream_control, generation);
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_library_sync(
    ui: slint::Weak<AppWindow>,
    browser: SharedLibraryBrowser,
    host: String,
) {
    std::thread::spawn(move || {
        let result = library::sync_library_catalog(&host);
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_sync_result(result);
            }
            if let Some(ui) = ui.upgrade() {
                apply_compiled_library_state(&ui, &browser);
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_sd_fetch(
    ui: slint::Weak<AppWindow>,
    browser: SharedSdBrowser,
    host: String,
    path: String,
    show_hidden: bool,
) {
    std::thread::spawn(move || {
        let result = fetch_sd_directory(&host, &path, show_hidden).map_err(|err| err.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            if let Ok(mut browser) = browser.lock() {
                browser.apply_listing_if_current_policy(&path, show_hidden, result);
            }
            if let Some(ui) = ui.upgrade() {
                apply_compiled_sd_state(&ui, &browser);
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_framebuffer_capture(
    ui: slint::Weak<AppWindow>,
    capture_state: SharedFramebufferCapture,
    host: String,
) {
    std::thread::spawn(move || {
        let result = fetch_framebuffer_capture(&host).map_err(|err| err.to_string());
        let capture = result.as_ref().ok().cloned();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(capture) = capture {
                if let Ok(mut state) = capture_state.lock() {
                    *state = Some(capture);
                }
            }
            if let Some(ui) = ui.upgrade() {
                apply_compiled_framebuffer_capture_result(&ui, result);
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_save_framebuffer_capture(
    ui: slint::Weak<AppWindow>,
    capture_state: SharedFramebufferCapture,
) {
    std::thread::spawn(move || {
        let result = capture_state
            .lock()
            .ok()
            .and_then(|state| state.clone())
            .ok_or_else(|| "Capture a framebuffer before saving.".to_string())
            .and_then(|capture| save_framebuffer_capture_png(&capture));
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                match result {
                    Ok(path) => apply_compiled_save_status(
                        &ui,
                        &format!("Saved framebuffer PNG to {}.", path.display()),
                        "",
                    ),
                    Err(err) => {
                        apply_compiled_save_status(&ui, "Framebuffer PNG save failed.", &err)
                    }
                }
            }
        });
    });
}

#[cfg(feature = "compiled-ui")]
fn spawn_compiled_framebuffer_stream(
    ui: slint::Weak<AppWindow>,
    capture_state: SharedFramebufferCapture,
    stream_generation: SharedLiveStreamGeneration,
    stream_control: SharedFramebufferStreamControl,
    host: String,
    generation: u64,
) {
    std::thread::spawn(move || {
        let stream_start = Instant::now();
        let mut frames = 0_u64;
        let mut recent_dirty_rects = VecDeque::new();
        let seed_capture = fetch_framebuffer_capture(&host).ok();
        if let Some(capture) = seed_capture.clone() {
            let event_generation = Arc::clone(&stream_generation);
            let event_capture_state = Arc::clone(&capture_state);
            let event_ui = ui.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if event_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Ok(mut state) = event_capture_state.lock() {
                    *state = Some(capture.clone());
                }
                if let Some(ui) = event_ui.upgrade() {
                    apply_compiled_framebuffer_capture_result(&ui, Ok(capture));
                }
            });
        }
        let mut stream = match connect_framebuffer_stream_seeded(&host, seed_capture.as_ref()) {
            Ok(stream) => stream,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(ui) = ui.upgrade() {
                        apply_compiled_stream_disconnected(&ui, &err);
                    }
                });
                return;
            }
        };
        let control = match stream.control() {
            Ok(control) => control,
            Err(err) => {
                let err = err.to_string();
                let event_generation = Arc::clone(&stream_generation);
                let _ = slint::invoke_from_event_loop(move || {
                    if event_generation.load(Ordering::SeqCst) != generation {
                        return;
                    }
                    if let Some(ui) = ui.upgrade() {
                        apply_compiled_stream_disconnected(&ui, &err);
                    }
                });
                return;
            }
        };
        if stream_generation.load(Ordering::SeqCst) != generation {
            control.shutdown();
            return;
        }
        register_framebuffer_stream(&stream_control, generation, control);
        while stream_generation.load(Ordering::SeqCst) == generation {
            let frame_start = Instant::now();
            let result = stream.next_frame().map_err(|err| err.to_string());
            let frame_elapsed = frame_start.elapsed();
            if let Ok(frame) = &result {
                frames += 1;
                push_recent_dirty_rect(&mut recent_dirty_rects, frame);
            }
            let summary = result
                .as_ref()
                .map(|_| framebuffer_stream_summary(frames, stream_start.elapsed(), frame_elapsed))
                .unwrap_or_else(|_| "Live stream disconnected.".to_string());
            let dirty_summary = result
                .as_ref()
                .map(|frame| dirty_rect_summary(frame, recent_dirty_rects.len()))
                .unwrap_or_else(|_| "Dirty overlay idle.".to_string());
            let dirty_rects = recent_dirty_rects.clone();
            let capture = result.as_ref().ok().map(|frame| frame.capture.clone());
            let capture_result = result
                .as_ref()
                .map(|frame| frame.capture.clone())
                .map_err(Clone::clone);
            let should_continue = stream_generation.load(Ordering::SeqCst) == generation;
            let disconnected = result.is_err();
            let event_generation = Arc::clone(&stream_generation);
            let event_capture_state = Arc::clone(&capture_state);
            let event_ui = ui.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if event_generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                if let Some(capture) = capture {
                    if let Ok(mut state) = event_capture_state.lock() {
                        *state = Some(capture);
                    }
                }
                if let Some(ui) = event_ui.upgrade() {
                    if disconnected {
                        let err = result
                            .err()
                            .unwrap_or_else(|| "framebuffer stream disconnected".to_string());
                        apply_compiled_stream_disconnected(&ui, &err);
                    } else {
                        apply_compiled_framebuffer_capture_result(&ui, capture_result);
                        apply_compiled_dirty_rects(&ui, &dirty_rects, &dirty_summary);
                        apply_compiled_stream_summary(&ui, &summary);
                    }
                }
            });
            if !should_continue || disconnected {
                break;
            }
        }
        unregister_framebuffer_stream(&stream_control, generation);
    });
}

fn start_window_drag(window: &slint::Window) {
    use slint::winit_030::WinitWindowAccessor;
    window.with_winit_window(|winit_window| {
        let _ = winit_window.drag_window();
    });
}

#[cfg(feature = "live-ui")]
fn start_reload_watcher(path: &Path, reload_requested: Arc<AtomicBool>, stop: Arc<AtomicBool>) {
    let path = path.to_path_buf();
    let initial_mtime = modified_time(&path);
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(500));
            if modified_time(&path) != initial_mtime {
                reload_requested.store(true, Ordering::Relaxed);
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = slint::quit_event_loop();
                });
                break;
            }
        }
    });
}

#[cfg(feature = "live-ui")]
fn modified_time(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory_row() -> sd_card::SdTreeRow {
        sd_card::SdTreeRow {
            id: "/games".to_string(),
            label: "games".to_string(),
            icon_key: "folder-base".to_string(),
            level: 2,
            has_children: true,
            expanded: true,
            current: false,
            leading_is_directory: true,
            interactive: true,
            is_skeleton: false,
            loading_children_badge: "loading".to_string(),
        }
    }

    #[cfg(feature = "live-ui")]
    #[test]
    fn live_tree_row_struct_preserves_directory_flags() {
        let value = live_tree_row_struct(&directory_row());

        assert!(matches!(
            value.get_field("leading-is-directory"),
            Some(slint_interpreter::Value::Bool(true))
        ));
        assert!(matches!(
            value.get_field("has-children"),
            Some(slint_interpreter::Value::Bool(true))
        ));
        assert!(matches!(
            value.get_field("loading-children-badge"),
            Some(slint_interpreter::Value::String(text)) if text.as_str() == "loading"
        ));
    }

    #[cfg(feature = "compiled-ui")]
    #[test]
    fn compiled_tree_row_preserves_directory_flags() {
        let value = compiled_tree_row(&directory_row());

        assert!(value.leading_is_directory);
        assert!(value.has_children);
        assert_eq!(value.loading_children_badge.as_str(), "loading");
    }

    #[test]
    fn byte_size_labels_use_kb_and_mb() {
        assert_eq!(format_byte_size(512), "512 B");
        assert_eq!(format_byte_size(1536), "2 KB");
        assert_eq!(format_byte_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn framebuffer_capture_status_includes_payload_and_raw_sizes() {
        let capture = agent_client::FramebufferCapture {
            png_path: std::path::PathBuf::from("/tmp/fb.png"),
            rgba_pixels: Vec::new(),
            raw_pixels: Vec::new(),
            raw_stride_bytes: 0,
            width: 960,
            height: 540,
            bpp: 16,
            raw_bytes: 1_036_800,
            payload_bytes: 10_212,
            encoding: "lz4-block-size-prepended".to_string(),
            png_bytes: 0,
            png_hex_bytes: 0,
            timing: agent_client::FramebufferCaptureTiming::default(),
        };

        assert_eq!(
            framebuffer_capture_status(&capture),
            "Captured 960x540 16bpp framebuffer (10 KB payload; 1012 KB raw; lz4-block-size-prepended)."
        );
    }

    #[test]
    fn framebuffer_capture_png_bytes_encodes_rgba_pixels() {
        let capture = agent_client::FramebufferCapture {
            png_path: std::path::PathBuf::new(),
            rgba_pixels: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
            raw_pixels: Vec::new(),
            raw_stride_bytes: 0,
            width: 2,
            height: 2,
            bpp: 16,
            raw_bytes: 8,
            payload_bytes: 8,
            encoding: "lz4-block-size-prepended".to_string(),
            png_bytes: 0,
            png_hex_bytes: 0,
            timing: agent_client::FramebufferCaptureTiming::default(),
        };

        let png_bytes =
            framebuffer_capture_png_bytes(&capture).expect("RGBA pixels should encode as PNG");

        assert!(png_bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    }

    #[test]
    fn framebuffer_stream_helpers_report_fps_and_latency() {
        let latencies = [
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
        ];

        assert_eq!(latency_percentile_ms(&latencies, 0.50), 20.0);
        assert_eq!(
            framebuffer_stream_summary(30, Duration::from_secs(3), Duration::from_millis(75)),
            "10.0 fps avg, 75 ms last frame"
        );
    }
}
