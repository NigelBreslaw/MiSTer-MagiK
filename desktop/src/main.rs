mod agent_client;
mod app_state;
mod file_icons;
#[cfg(target_os = "macos")]
mod macos_titlebar;
mod sd_card;

use agent_client::{fetch_dashboard, fetch_framebuffer_capture, fetch_sd_directory};
use app_state::{DashboardSnapshot, DEFAULT_HOST};
use sd_card::SdCardBrowser;
#[cfg(feature = "compiled-ui")]
use sd_card::SdTreeRow;
#[cfg(feature = "live-ui")]
use slint::ComponentHandle;
use std::error::Error;
#[cfg(feature = "live-ui")]
use std::path::{Path, PathBuf};
#[cfg(feature = "live-ui")]
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
#[cfg(feature = "live-ui")]
use std::time::{Duration, SystemTime};

type SharedSdBrowser = Arc<Mutex<SdCardBrowser>>;

#[cfg(feature = "compiled-ui")]
slint::include_modules!();

fn main() -> Result<(), Box<dyn Error>> {
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
    instance.set_global_callback("Actions", "capture-framebuffer", move |_| {
        if let Some(instance) = capture_instance.upgrade() {
            set_live_analytics_loading(&instance);
        }
        spawn_live_framebuffer_capture(capture_instance.clone(), capture_host.clone());
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
    ui.global::<Actions>().on_capture_framebuffer(move || {
        if let Some(ui) = capture_ui.upgrade() {
            set_compiled_analytics_loading(&ui);
        }
        spawn_compiled_framebuffer_capture(capture_ui.clone(), capture_host.clone());
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

    let drag_ui = ui.as_weak();
    ui.global::<WindowActions>().on_start_window_drag(move || {
        if let Some(ui) = drag_ui.upgrade() {
            start_window_drag(ui.window());
        }
    });

    let snapshot = fetch_dashboard(&host);
    apply_compiled_snapshot(&ui, &snapshot);
    apply_compiled_sd_state(&ui, &sd_browser);
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
        Value::String(SharedString::from("Capturing /dev/fb0...")),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "last-error",
        Value::String(SharedString::from("")),
    );
    let _ = instance.set_global_property(
        "AnalyticsState",
        "details",
        Value::String(SharedString::from("Waiting for MiSTer timing stats...")),
    );
}

#[cfg(feature = "live-ui")]
fn apply_live_framebuffer_capture_result(
    instance: &slint_interpreter::ComponentInstance,
    result: Result<agent_client::FramebufferCapture, String>,
) {
    use slint::{Image, SharedString};
    use slint_interpreter::Value;

    let _ = instance.set_global_property("AnalyticsState", "loading", Value::Bool(false));
    match result {
        Ok(capture) => {
            let image = Image::load_from_path(&capture.png_path).unwrap_or_default();
            let _ = instance.set_global_property(
                "AnalyticsState",
                "framebuffer-image",
                Value::Image(image),
            );
            let _ = instance.set_global_property("AnalyticsState", "has-image", Value::Bool(true));
            let _ = instance.set_global_property(
                "AnalyticsState",
                "status",
                Value::String(SharedString::from(framebuffer_capture_status(&capture))),
            );
            let _ = instance.set_global_property(
                "AnalyticsState",
                "details",
                Value::String(SharedString::from(framebuffer_capture_details(&capture))),
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
                "details",
                Value::String(SharedString::from("")),
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
    state.set_status("Capturing /dev/fb0...".into());
    state.set_details("Waiting for MiSTer timing stats...".into());
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
            let image = slint::Image::load_from_path(&capture.png_path).unwrap_or_default();
            state.set_framebuffer_image(image);
            state.set_has_image(true);
            state.set_status(framebuffer_capture_status(&capture).into());
            state.set_details(framebuffer_capture_details(&capture).into());
            state.set_last_error("".into());
        }
        Err(err) => {
            state.set_status("Framebuffer capture failed.".into());
            state.set_details("".into());
            state.set_last_error(err.into());
        }
    }
}

fn framebuffer_capture_status(capture: &agent_client::FramebufferCapture) -> String {
    format!(
        "Captured {}x{} {}bpp framebuffer ({} raw; {} PNG).",
        capture.width,
        capture.height,
        capture.bpp,
        format_byte_size(capture.raw_bytes),
        format_byte_size(capture.png_bytes)
    )
}

fn framebuffer_capture_details(capture: &agent_client::FramebufferCapture) -> String {
    let timing = &capture.timing;
    format!(
        "bytes: raw={} rgba={} png={} hex={}\n\
         timing: dispatch={} geometry={} read={} rgba={} zlib={} wrap={} png_total={} hex={} total={}\n\
         request_uptime={}ms",
        format_byte_size(capture.raw_bytes),
        format_byte_size(rgba_bytes(capture)),
        format_byte_size(capture.png_bytes),
        format_byte_size(capture.png_hex_bytes),
        format_us(timing.dispatch_us),
        format_us(timing.geometry_us),
        format_us(timing.raw_read_us),
        format_us(timing.rgba_convert_us),
        format_us(timing.zlib_encode_us),
        format_us(timing.png_wrap_us),
        format_us(timing.png_total_us),
        format_us(timing.hex_encode_us),
        format_us(timing.total_us),
        timing.request_received_uptime_ms,
    )
}

fn rgba_bytes(capture: &agent_client::FramebufferCapture) -> u64 {
    capture
        .width
        .checked_mul(4)
        .and_then(|row| row.checked_add(1))
        .and_then(|row| row.checked_mul(capture.height))
        .unwrap_or(0)
}

fn format_byte_size(bytes: u64) -> String {
    const KB: f64 = 1024.0;
    const MB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / MB)
    } else if bytes >= 1024 {
        format!("{:.1} KB", bytes as f64 / KB)
    } else {
        format!("{bytes} B")
    }
}

fn format_us(us: u64) -> String {
    if us >= 1000 {
        format!("{:.2}ms", us as f64 / 1000.0)
    } else {
        format!("{us}us")
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
    host: String,
) {
    std::thread::spawn(move || {
        let result = fetch_framebuffer_capture(&host).map_err(|err| err.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(instance) = instance.upgrade() {
                apply_live_framebuffer_capture_result(&instance, result);
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
fn spawn_compiled_framebuffer_capture(ui: slint::Weak<AppWindow>, host: String) {
    std::thread::spawn(move || {
        let result = fetch_framebuffer_capture(&host).map_err(|err| err.to_string());
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(ui) = ui.upgrade() {
                apply_compiled_framebuffer_capture_result(&ui, result);
            }
        });
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
        assert_eq!(format_byte_size(1536), "1.5 KB");
        assert_eq!(format_byte_size(2 * 1024 * 1024), "2.0 MB");
    }

    #[test]
    fn framebuffer_capture_status_includes_raw_and_png_sizes() {
        let capture = agent_client::FramebufferCapture {
            png_path: std::path::PathBuf::from("/tmp/fb.png"),
            width: 960,
            height: 540,
            bpp: 16,
            raw_bytes: 1_036_800,
            png_bytes: 2_074_363,
            png_hex_bytes: 4_148_726,
            timing: agent_client::FramebufferCaptureTiming::default(),
        };

        assert_eq!(
            framebuffer_capture_status(&capture),
            "Captured 960x540 16bpp framebuffer (1012.5 KB raw; 2.0 MB PNG)."
        );
    }

    #[test]
    fn framebuffer_capture_details_include_stage_timings() {
        let capture = agent_client::FramebufferCapture {
            png_path: std::path::PathBuf::from("/tmp/fb.png"),
            width: 960,
            height: 540,
            bpp: 16,
            raw_bytes: 1_036_800,
            png_bytes: 64 * 1024,
            png_hex_bytes: 128 * 1024,
            timing: agent_client::FramebufferCaptureTiming {
                raw_read_us: 1200,
                zlib_encode_us: 34_567,
                total_us: 45_678,
                ..Default::default()
            },
        };

        let details = framebuffer_capture_details(&capture);
        assert!(details.contains("rgba=2.0 MB"));
        assert!(details.contains("read=1.20ms"));
        assert!(details.contains("zlib=34.57ms"));
        assert!(details.contains("total=45.68ms"));
    }
}
