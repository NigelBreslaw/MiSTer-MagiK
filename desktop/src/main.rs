mod agent_client;
mod app_state;
#[cfg(target_os = "macos")]
mod macos_titlebar;

use agent_client::fetch_dashboard;
use app_state::{DashboardSnapshot, DEFAULT_HOST};
#[cfg(feature = "live-ui")]
use slint::ComponentHandle;
use std::error::Error;
#[cfg(feature = "live-ui")]
use std::path::{Path, PathBuf};
#[cfg(feature = "live-ui")]
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "live-ui")]
use std::sync::Arc;
#[cfg(feature = "live-ui")]
use std::time::{Duration, SystemTime};

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

    let refresh_instance = instance.as_weak();
    let refresh_host = host.to_string();
    instance.set_global_callback("Actions", "refresh-status", move |_| {
        if let Some(instance) = refresh_instance.upgrade() {
            let snapshot = fetch_dashboard(&refresh_host);
            apply_live_snapshot(&instance, &snapshot);
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

#[cfg(feature = "compiled-ui")]
fn run_compiled_ui() -> Result<(), Box<dyn Error>> {
    use slint::ComponentHandle;

    let host = std::env::var("MISTER_IP").unwrap_or_else(|_| DEFAULT_HOST.to_string());
    let ui = AppWindow::new()?;
    let refresh_ui = ui.as_weak();
    let refresh_host = host.clone();
    ui.global::<Actions>().on_refresh_status(move || {
        if let Some(ui) = refresh_ui.upgrade() {
            let snapshot = fetch_dashboard(&refresh_host);
            apply_compiled_snapshot(&ui, &snapshot);
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
