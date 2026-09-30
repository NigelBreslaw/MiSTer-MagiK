// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Actual controller changes and save phases on isolated device storage.
use mister_magik_controller_registry::{
    ControllerDb, ControllerEntry, ControllerKind, ControllerPersistence,
    input_info::PadInfo,
    probe::{SavePhase, SaveProbe},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex, mpsc},
    time::{Duration, Instant},
};
const ROWS: usize = 1000;
const FIXTURE: &str = "9ed460ca65e1e241cfe7a4926af4b61e812b3cadb61b6dd47a7baf7b26a53fc4";
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture() -> Result<Fixture, String> {
    #[cfg(all(target_os = "linux", target_arch = "arm", not(test)))]
    let base = PathBuf::from("/media/fat/mister-magik-dev/benchmark-fixtures");
    #[cfg(not(all(target_os = "linux", target_arch = "arm", not(test))))]
    let base = std::env::temp_dir().join("mister-magik-benchmark-fixtures");
    let _removed = remove_abandoned_preview_fixtures(&base);
    let root = base.join(format!(
        "controller-save-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    Ok(Fixture(root))
}
fn info() -> PadInfo {
    PadInfo {
        name: "Fixture target kernel".into(),
        vendor_id: "1234".into(),
        product_id: "9999".into(),
        serial: "target".into(),
        usb_port: "target-port".into(),
        js_axes: 6,
        js_buttons: 13,
        ..Default::default()
    }
}
fn database(path: &Path) -> ControllerDb {
    let mut db = ControllerDb::load_from(&path.to_string_lossy());
    for i in 0..ROWS {
        let info = PadInfo {
            name: format!("Fixture kernel {i}"),
            vendor_id: "1234".into(),
            product_id: format!("{i:04x}"),
            serial: format!("fixture{i}"),
            usb_port: format!("fixture-port-{i}"),
            ..Default::default()
        };
        let entry = ControllerEntry {
            label: format!("Controller {i:04}"),
            kernel_name: if i == 0 {
                String::new()
            } else {
                info.name.clone()
            },
            kind: ControllerKind::Gamepad,
            setup_complete: true,
            last_usb_port: info.usb_port.clone(),
        };
        db.upsert(&info, entry);
    }
    db
}
fn verify(db: &ControllerDb, target: &PadInfo) -> Result<(), String> {
    let loaded = ControllerDb::load_from(db.path());
    if db.len() != loaded.len() {
        return Err("saved registry count mismatch".into());
    }
    for item in db.list_entries() {
        if db.get_by_id(&item.id) != loaded.get_by_id(&item.id) {
            return Err("saved final controller fields differ".into());
        }
    }
    let entry = loaded.get(target).ok_or("target entry missing")?;
    if !entry.setup_complete
        || entry.label != "Final controller"
        || entry.kind != ControllerKind::Arcade
        || entry.kernel_name != target.name
        || entry.last_usb_port != target.usb_port
    {
        return Err("finished target fields differ".into());
    }
    Ok(())
}
fn navigation_step() -> Result<(), String> {
    use mister_magik_core::input_event::*;
    let event = InputEvent {
        source: InputSourceId {
            kind: InputSourceKind::Automation,
            instance: 0,
        },
        source_epoch: SourceEpoch(1),
        sequence: 1,
        press_id: PressId(1),
        captured_at_us: 1,
        action: LogicalAction::Down,
        phase: InputPhase::Pressed,
    };
    let mut held = HeldState::default();
    held.apply_event(&event).map_err(|e| format!("{e:?}"))?;
    if !held.is_held(LogicalAction::Down) {
        return Err("navigation not processed".into());
    }
    Ok(())
}
fn gate_observation(path: &Path) -> Result<bool, String> {
    let mut db = database(path);
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let worker_gate = gate.clone();
    let (entered_tx, entered_rx) = mpsc::channel();
    db.set_save_probe(SaveProbe::new(move |phase| {
        if matches!(phase, SavePhase::Started) {
            let _ = entered_tx.send(());
            let (lock, cv) = &*worker_gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = cv.wait(released).unwrap();
            }
        }
    }));
    let mut owner = ControllerPersistence::start(&db).map_err(|e| e.to_string())?;
    let (ack_tx, ack_rx) = mpsc::channel();
    let thread = std::thread::spawn(move || -> Result<(), String> {
        owner
            .register_new(&mut db, &info())
            .map_err(|e| e.to_string())?;
        navigation_step()?;
        let _ = ack_tx.send(());
        owner
            .shutdown(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        Ok(())
    });
    let entered = entered_rx.recv_timeout(Duration::from_secs(2));
    let before_release = entered.is_ok() && ack_rx.recv_timeout(Duration::from_millis(100)).is_ok();
    let (lock, cv) = &*gate;
    *lock.lock().unwrap() = true;
    cv.notify_all();
    let joined = thread.join().map_err(|_| "action thread panicked")?;
    joined?;
    entered.map_err(|e| e.to_string())?;
    Ok(before_release)
}
pub(super) fn run() -> Result<serde_json::Value, String> {
    let f = fixture()?;
    let _logs = crate::preview_shards::QuietLogs::new(&f.0)?;
    let mut samples = Vec::new();
    let work_count = 3;
    let caller = format!("{:?}", std::thread::current().id());
    for repetition in 0..2 {
        let mut db = database(&f.0.join(format!("controllers-{repetition}.json")));
        db.save().map_err(|e| e.to_string())?;
        let phases = Arc::new(Mutex::new(Vec::new()));
        let trace = phases.clone();
        let epoch = Instant::now();
        db.set_save_probe(SaveProbe::new(move |phase| {
            trace.lock().unwrap().push((
                phase,
                epoch.elapsed().as_nanos() as u64,
                format!("{:?}", std::thread::current().id()),
            ));
        }));
        let mut owner = ControllerPersistence::start(&db).map_err(|e| e.to_string())?;
        let target = info();
        let mut actions = Vec::new();
        let mut duration_ns = 0u64;
        for action in 0..3 {
            // Matched TLS allocation accounting is enabled on the caller;
            // these instrumented action times are not scanout latency.
            let started = Instant::now();
            let (result, allocations) = crate::catalog_sort::observe_allocations(|| match action {
                0 => owner.register_new(&mut db, &target),
                1 => owner.claim_existing(&mut db, &target, 0),
                _ => owner.finish_setup(
                    &mut db,
                    &target,
                    "Final controller".into(),
                    ControllerKind::Arcade,
                ),
            });
            let latency = started.elapsed().as_nanos() as u64;
            let revision = result.map_err(|e| e.to_string())?;
            duration_ns += latency;
            let pending = owner.status();
            navigation_step()?;
            owner
                .flush(Duration::from_secs(2))
                .map_err(|e| e.to_string())?;
            let committed = owner.status();
            if committed.committed < revision {
                return Err("flush did not commit requested revision".into());
            }
            actions.push(serde_json::json!({"action":(["register","claim","finish"][action]),"caller_duration_ns":latency,"caller_allocations":allocations,"revision":revision,"committed_on_ack":pending.committed,"committed_after_flush":committed.committed}));
        }
        owner
            .shutdown(Duration::from_secs(2))
            .map_err(|e| e.to_string())?;
        verify(&db, &target)?;
        let phases = phases.lock().unwrap();
        let mut saves = Vec::new();
        let mut start = None;
        for (phase, at, thread) in phases.iter() {
            match phase {
                SavePhase::Started => start = Some((*at, thread)),
                SavePhase::Finished(ok) => {
                    let (began, owner_thread) = start.take().ok_or("save finish without start")?;
                    if !ok || owner_thread != thread {
                        return Err("save outcome/thread mismatch".into());
                    }
                    saves.push(serde_json::json!({"duration_ns":at-began,"thread":thread,"on_caller":thread==&caller}));
                }
            }
        }
        if saves.len() != 3 {
            return Err("expected three separately flushed saves".into());
        }
        drop(phases);
        let input_before_release = gate_observation(&f.0.join(format!("gate-{repetition}.json")))?;
        samples.push(serde_json::json!({"repetition":repetition,"fixture_identity":FIXTURE,"work_count":work_count,"duration_ns":duration_ns,"ns_per_pixel":duration_ns as f64/work_count as f64,"actions":actions,"saves":saves,"input_processed_before_writer_release":input_before_release}));
    }
    let artifact =
        std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256").map_err(|_| "artifact identity missing")?;
    Ok(
        serde_json::json!({"schema_version":1,"workload":"controller-save","mode":"timing","artifact_sha256":artifact,"correctness":"passed","work_count":work_count,"fixture":{"identity":FIXTURE,"initial_controllers":ROWS,"storage":"isolated development SD fixture; RAII cleanup","actions":"register new, claim existing, finish setup; flush separately after each caller window","oracle":"every loaded final field equals immediate in-memory registry","input_probe":"portable production HeldState consumes Down after registration; gated storage is a mechanism probe, not physical input-to-scanout latency"},"samples":samples}),
    )
}
// Remove only abandoned fixtures created by our preview-shard benchmark. A
// service timeout can bypass the fixture's Drop. Unknown names, files, symlinks,
// active owners or contents are retained. Never traverse user catalog roots.
fn remove_abandoned_preview_fixtures(base: &Path) -> usize {
    #[cfg(unix)]
    {
        use std::io::Read;
        let Ok(entries) = std::fs::read_dir(base) else {
            return 0;
        };
        let mut removed = 0;
        for entry in entries.take(128).flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Some(parts) = name
                .strip_prefix("preview-shards-")
                .and_then(|n| n.split_once('-'))
            else {
                continue;
            };
            let (Ok(pid), Ok(_timestamp)) =
                (parts.0.parse::<libc::pid_t>(), parts.1.parse::<u128>())
            else {
                continue;
            };
            if pid <= 0
                || unsafe { libc::kill(pid, 0) } == 0
                || std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
                || !entry.file_type().is_ok_and(|t| t.is_dir())
            {
                continue;
            }
            let root = entry.path();
            let files = [
                "_Console/SNES_20260826.rbf",
                "_Console/Saturn_20260826.rbf",
                "games/SNES/Title00000.sfc",
                "games/Saturn/Title00000.chd",
                "mister-magik/magik-metadata-v1.bin",
            ];
            let directories = [
                "_Console",
                "games",
                "games/SNES",
                "games/Saturn",
                "mister-magik",
            ];
            let mut pending = vec![root.clone()];
            let mut found = std::collections::BTreeSet::new();
            let mut valid = true;
            while let Some(directory) = pending.pop() {
                let Ok(entries) = std::fs::read_dir(directory) else {
                    valid = false;
                    break;
                };
                for child in entries.take(17) {
                    let Ok(child) = child else {
                        valid = false;
                        break;
                    };
                    let path = child.path();
                    let Ok(relative) = path.strip_prefix(&root) else {
                        valid = false;
                        break;
                    };
                    let Some(relative) = relative.to_str() else {
                        valid = false;
                        break;
                    };
                    let Ok(kind) = child.file_type() else {
                        valid = false;
                        break;
                    };
                    if kind.is_dir() && directories.contains(&relative) {
                        pending.push(path);
                    } else if kind.is_file() && files.contains(&relative) {
                        found.insert(relative.to_owned());
                    } else {
                        valid = false;
                        break;
                    }
                }
                if !valid || found.len() > 5 || pending.len() > 5 {
                    valid = false;
                    break;
                }
            }
            if !valid || found.len() != files.len() {
                continue;
            }
            if files[..4]
                .iter()
                .any(|p| std::fs::read(root.join(p)).ok().as_deref() != Some(b"fixture"))
            {
                continue;
            }
            let Ok(mut metadata) = std::fs::File::open(root.join(files[4])) else {
                continue;
            };
            if !metadata
                .metadata()
                .is_ok_and(|m| m.len() <= 8 * 1024 * 1024)
            {
                continue;
            }
            let mut magic = [0; 8];
            if metadata.read_exact(&mut magic).is_err() || &magic != b"MMMETA1\0" {
                continue;
            }
            if std::fs::remove_dir_all(root).is_ok() {
                removed += 1;
            }
        }
        removed
    }
    #[cfg(not(unix))]
    {
        let _ = base;
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn final_controller_fields_match_serialized_registry() {
        let f = fixture().unwrap();
        let mut db = database(&f.0.join("controllers.json"));
        let mut owner = ControllerPersistence::start(&db).unwrap();
        let info = info();
        owner.register_new(&mut db, &info).unwrap();
        owner.claim_existing(&mut db, &info, 0).unwrap();
        owner
            .finish_setup(
                &mut db,
                &info,
                "Final controller".into(),
                ControllerKind::Arcade,
            )
            .unwrap();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        verify(&db, &info).unwrap();
    }
}
