// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later
//! Single serialized writer. Only bounded per-controller deltas cross the input
//! boundary; registry cloning, serialization, filesystem I/O and retirement
//! stay on its owner. Explicit flush/shutdown are for lifecycle boundaries.
use crate::{ControllerDb, ControllerEntry, ControllerKind, input_info::PadInfo};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};
pub const MAX_PENDING_CONTROLLERS: usize = 64;
pub const MAX_PENDING_BYTES: usize = 256 * 1024;
#[derive(Clone, Debug, Default)]
pub struct SaveStatus {
    pub requested: u64,
    pub committed: u64,
    pub error: Option<String>,
    pub failed_revision: u64,
    pub queued_controllers: usize,
    pub queued_bytes: usize,
}
impl SaveStatus {
    pub fn is_pending(&self) -> bool {
        self.committed < self.requested
    }
    pub fn is_failed(&self) -> bool {
        self.is_pending() && self.failed_revision >= self.requested && self.error.is_some()
    }
}
#[derive(Clone, Debug)]
pub struct SaveCompletion {
    pub revision: u64,
    pub result: Result<(), String>,
}
struct Pending {
    entries: HashMap<String, ControllerEntry>,
    bytes: usize,
    save: bool,
    stopping: bool,
    exited: bool,
    status: SaveStatus,
    completion: Option<SaveCompletion>,
}
struct Shared {
    pending: Mutex<Pending>,
    wake: Condvar,
    wake_ui: Box<dyn Fn() + Send + Sync>,
}
/// Read-only view of the same persistence revisions used by the writer.
#[derive(Clone)]
pub struct SaveObserver {
    shared: Arc<Shared>,
}
impl std::fmt::Debug for SaveObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ControllerSaveObserver")
    }
}
impl SaveObserver {
    pub fn is_pending(&self) -> bool {
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .status
            .is_pending()
    }
}
pub struct ControllerPersistence {
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
}
impl ControllerPersistence {
    pub fn start(db: &ControllerDb) -> io::Result<Self> {
        Self::start_with_waker(db, || {})
    }
    pub fn start_with_waker(
        db: &ControllerDb,
        wake_ui: impl Fn() + Send + Sync + 'static,
    ) -> io::Result<Self> {
        // One startup snapshot, before entering the input loop. Subsequent
        // actions capture one entry regardless of registry size.
        let state = db.clone();
        let shared = Arc::new(Shared {
            pending: Mutex::new(Pending {
                entries: HashMap::with_capacity(MAX_PENDING_CONTROLLERS),
                bytes: 0,
                save: false,
                stopping: false,
                exited: false,
                status: SaveStatus::default(),
                completion: None,
            }),
            wake: Condvar::new(),
            wake_ui: Box::new(wake_ui),
        });
        let owner = shared.clone();
        let worker = thread::Builder::new()
            .name("controller-persist".into())
            .spawn(move || run_owner(state, owner))?;
        Ok(Self {
            shared,
            worker: Some(worker),
        })
    }
    fn submit(&mut self, id: String, entry: &ControllerEntry, save: bool) -> io::Result<u64> {
        if charge(id.capacity(), entry) > MAX_PENDING_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "controller change exceeds persistence byte bound",
            ));
        }
        let snapshot = entry.clone();
        let mut pending = self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if pending.stopping || pending.exited {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "controller writer stopped",
            ));
        }
        let (old, key_capacity) = pending
            .entries
            .get_key_value(&id)
            .map(|(key, entry)| (charge(key.capacity(), entry), key.capacity()))
            .unwrap_or((0, id.capacity()));
        let bytes = charge(key_capacity, &snapshot);
        if (!pending.entries.contains_key(&id) && pending.entries.len() >= MAX_PENDING_CONTROLLERS)
            || pending.bytes - old + bytes > MAX_PENDING_BYTES
        {
            return Err(io::Error::new(
                io::ErrorKind::WouldBlock,
                "controller persistence queue is full",
            ));
        }
        let revision = if save {
            pending
                .status
                .requested
                .checked_add(1)
                .ok_or_else(|| io::Error::other("controller revision exhausted"))?
        } else {
            pending.status.requested
        };
        pending.bytes = pending.bytes - old + bytes;
        pending.entries.insert(id, snapshot);
        if save {
            pending.status.requested = revision;
            pending.status.error = None;
            pending.save = true;
        }
        self.shared.wake.notify_one();
        Ok(revision)
    }
    pub fn register_new(&mut self, db: &mut ControllerDb, info: &PadInfo) -> io::Result<u64> {
        let id = ControllerDb::logical_id(info);
        let entry = ControllerDb::default_entry(info);
        let revision = self.submit(id.clone(), &entry, true)?;
        db.replace_entry(id, entry);
        Ok(revision)
    }
    pub fn claim_existing(
        &mut self,
        db: &mut ControllerDb,
        info: &PadInfo,
        index: usize,
    ) -> io::Result<u64> {
        let id = db.list_entry_id(index).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "list index out of range")
        })?;
        let mut entry = db.get_by_id(&id).cloned().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "controller entry disappeared")
        })?;
        entry.last_usb_port = info.usb_port.clone();
        if entry.kernel_name.is_empty() {
            entry.kernel_name = info.name.clone();
        }
        let revision = self.submit(id.clone(), &entry, true)?;
        db.replace_entry(id, entry);
        Ok(revision)
    }
    pub fn finish_setup(
        &mut self,
        db: &mut ControllerDb,
        info: &PadInfo,
        label: String,
        kind: ControllerKind,
    ) -> io::Result<u64> {
        let id = ControllerDb::logical_id(info);
        let mut entry = db
            .get(info)
            .cloned()
            .unwrap_or_else(|| ControllerDb::default_entry(info));
        entry.label = label;
        entry.kind = kind;
        entry.kernel_name = info.name.clone();
        entry.setup_complete = true;
        entry.last_usb_port = info.usb_port.clone();
        let revision = self.submit(id.clone(), &entry, true)?;
        db.replace_entry(id, entry);
        Ok(revision)
    }
    /// Sightings update the writer's in-memory copy, without adding a save that
    /// the previous registry did not perform. The next explicit save includes
    /// them, including when the caller queue coalesces repeated edits.
    pub fn note_sighting(&mut self, db: &mut ControllerDb, info: &PadInfo) -> io::Result<bool> {
        let Some(mut entry) = db.get(info).cloned() else {
            return Ok(false);
        };
        if entry.setup_complete && db.port_changed(info) {
            return Ok(false);
        }
        if entry.last_usb_port == info.usb_port {
            return Ok(true);
        }
        entry.last_usb_port = info.usb_port.clone();
        let id = ControllerDb::logical_id(info);
        self.submit(id.clone(), &entry, false)?;
        db.replace_entry(id, entry);
        Ok(true)
    }
    /// Explicit retry after a failed batch, using the writer's latest applied
    /// state. It does not copy the input-side registry or arm automatic retries.
    pub fn retry(&mut self) -> io::Result<u64> {
        let mut pending = self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if pending.stopping || pending.exited {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "controller writer stopped",
            ));
        }
        let revision = pending
            .status
            .requested
            .checked_add(1)
            .ok_or_else(|| io::Error::other("controller revision exhausted"))?;
        pending.status.requested = revision;
        pending.status.error = None;
        pending.save = true;
        self.shared.wake.notify_one();
        Ok(revision)
    }
    pub fn observer(&self) -> SaveObserver {
        SaveObserver {
            shared: self.shared.clone(),
        }
    }
    pub fn status(&self) -> SaveStatus {
        let pending = self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut status = pending.status.clone();
        status.queued_controllers = pending.entries.len();
        status.queued_bytes = pending.bytes;
        status
    }
    pub fn take_completion(&self) -> Option<SaveCompletion> {
        self.shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .completion
            .take()
    }
    /// Wait for all requests accepted before this call. Use outside the input
    /// loop; a timeout/failure reports missing durability rather than success.
    pub fn flush(&self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut pending = self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let target = pending.status.requested;
        loop {
            if pending.status.committed >= target {
                return Ok(());
            }
            if pending.status.failed_revision >= target || pending.exited {
                return Err(io::Error::other(
                    pending.status.error.clone().unwrap_or_else(|| {
                        "controller writer exited before committing changes".into()
                    }),
                ));
            }
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "controller save remains pending",
                ));
            };
            pending = self
                .shared
                .wake
                .wait_timeout(pending, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }
    pub fn shutdown(&mut self, timeout: Duration) -> io::Result<()> {
        let deadline = Instant::now() + timeout;
        let mut pending = self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending.stopping = true;
        self.shared.wake.notify_one();
        while !pending.exited {
            let Some(remaining) = deadline.checked_duration_since(Instant::now()) else {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "controller shutdown has pending work",
                ));
            };
            pending = self
                .shared
                .wake
                .wait_timeout(pending, remaining)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        let result = if pending.status.committed >= pending.status.requested {
            Ok(())
        } else {
            Err(io::Error::other(
                pending
                    .status
                    .error
                    .clone()
                    .unwrap_or_else(|| "controller save failed during shutdown".into()),
            ))
        };
        drop(pending);
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("controller writer panicked"))?;
        }
        result
    }
}
impl Drop for ControllerPersistence {
    fn drop(&mut self) {
        let mut pending = self
            .shared
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        pending.stopping = true;
        self.shared.wake.notify_one(); /* JoinHandle drops without blocking the input thread. */
    }
}
fn charge(id_capacity: usize, entry: &ControllerEntry) -> usize {
    std::mem::size_of::<(String, ControllerEntry)>()
        + id_capacity
        + entry.label.capacity()
        + entry.kernel_name.capacity()
        + entry.last_usb_port.capacity()
}
fn run_owner(mut db: ControllerDb, shared: Arc<Shared>) {
    struct Exited(Arc<Shared>);
    impl Drop for Exited {
        fn drop(&mut self) {
            let mut pending = self.0.pending.lock().unwrap_or_else(|e| e.into_inner());
            pending.exited = true;
            if std::thread::panicking() {
                pending.status.error = Some("controller writer panicked".into());
                pending.status.failed_revision = pending.status.requested;
            }
            self.0.wake.notify_all();
            drop(pending);
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (self.0.wake_ui)()));
        }
    }
    let _exited = Exited(shared.clone());
    loop {
        // Allocate replacement bookkeeping before taking the short queue lock.
        let empty = HashMap::with_capacity(MAX_PENDING_CONTROLLERS);
        let mut pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
        while pending.entries.is_empty() && !pending.save && !pending.stopping {
            pending = shared.wake.wait(pending).unwrap_or_else(|e| e.into_inner());
        }
        if pending.entries.is_empty() && !pending.save && pending.stopping {
            break;
        }
        let entries = std::mem::replace(&mut pending.entries, empty);
        pending.bytes = 0;
        let save = std::mem::take(&mut pending.save);
        let revision = pending.status.requested;
        drop(pending);
        for (id, entry) in entries {
            db.replace_entry(id, entry);
        }
        if save {
            let result = db.save().map_err(|e| e.to_string());
            let mut pending = shared.pending.lock().unwrap_or_else(|e| e.into_inner());
            if result.is_ok() {
                pending.status.committed = revision;
                pending.status.failed_revision = 0;
                pending.status.error = None;
            } else {
                pending.status.failed_revision = revision;
                pending.status.error = result.as_ref().err().cloned();
            }
            pending.completion = Some(SaveCompletion { revision, result });
            shared.wake.notify_all();
            drop(pending);
            (shared.wake_ui)();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{SavePhase, SaveProbe};
    use std::path::PathBuf;
    struct Fixture(PathBuf);
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn fixture(name: &str) -> Fixture {
        static SERIAL: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "magik-controller-owner-{name}-{}-{}",
            std::process::id(),
            SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        Fixture(root)
    }
    fn info(i: usize) -> PadInfo {
        PadInfo {
            name: format!("Kernel {i}"),
            vendor_id: "1234".into(),
            product_id: format!("{i:04x}"),
            serial: format!("serial{i}"),
            usb_port: format!("port{i}"),
            js_axes: 6,
            js_buttons: 13,
            ..Default::default()
        }
    }
    #[derive(Default)]
    struct Gate {
        state: Mutex<(bool, bool)>,
        cv: Condvar,
    }
    impl Gate {
        fn wait(&self) {
            let mut state = self.state.lock().unwrap();
            state.0 = true;
            self.cv.notify_all();
            while !state.1 {
                state = self.cv.wait(state).unwrap();
            }
        }
        fn entered(&self) {
            let mut state = self.state.lock().unwrap();
            let deadline = Instant::now() + Duration::from_secs(2);
            while !state.0 {
                let remain = deadline
                    .checked_duration_since(Instant::now())
                    .expect("writer did not enter gate");
                state = self.cv.wait_timeout(state, remain).unwrap().0;
            }
        }
        fn release(&self) {
            self.state.lock().unwrap().1 = true;
            self.cv.notify_all();
        }
    }
    struct Release(Vec<Arc<Gate>>);
    impl Drop for Release {
        fn drop(&mut self) {
            for gate in &self.0 {
                gate.release();
            }
        }
    }
    #[test]
    fn older_completion_cannot_commit_newer_revision() {
        let f = fixture("ordered");
        let mut db = ControllerDb::load_from(&f.0.join("controllers.json").to_string_lossy());
        let first = Arc::new(Gate::default());
        let second = Arc::new(Gate::default());
        let _release = Release(vec![first.clone(), second.clone()]);
        let gates = [first.clone(), second.clone()];
        let count = std::sync::atomic::AtomicUsize::new(0);
        db.set_save_probe(SaveProbe::new(move |phase| {
            if matches!(phase, SavePhase::Started) {
                let i = count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if let Some(gate) = gates.get(i) {
                    gate.wait();
                }
            }
        }));
        let mut owner = ControllerPersistence::start(&db).unwrap();
        let device = info(0);
        let old = owner.register_new(&mut db, &device).unwrap();
        first.entered();
        let latest = owner
            .finish_setup(&mut db, &device, "Newest".into(), ControllerKind::Arcade)
            .unwrap();
        assert!(owner.status().is_pending());
        assert_eq!(
            owner.flush(Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        first.release();
        second.entered();
        let status = owner.status();
        assert_eq!((status.committed, status.requested), (old, latest));
        assert!(status.is_pending());
        assert_eq!(owner.take_completion().unwrap().revision, old);
        let disk = ControllerDb::load_from(db.path());
        assert!(!disk.get(&device).unwrap().setup_complete);
        second.release();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        let disk = ControllerDb::load_from(db.path());
        assert_eq!(disk.get(&device), db.get(&device));
        assert!(!owner.status().is_pending());
    }
    #[test]
    fn repeated_edits_coalesce_and_distinct_queue_saturates_without_mutating_view() {
        let f = fixture("bounds");
        let mut db = ControllerDb::load_from(&f.0.join("controllers.json").to_string_lossy());
        let gate = Arc::new(Gate::default());
        let _release = Release(vec![gate.clone()]);
        let g = gate.clone();
        db.set_save_probe(SaveProbe::new(move |phase| {
            if matches!(phase, SavePhase::Started) {
                g.wait();
            }
        }));
        let mut owner = ControllerPersistence::start(&db).unwrap();
        owner.register_new(&mut db, &info(0)).unwrap();
        gate.entered();
        for i in 1..=MAX_PENDING_CONTROLLERS {
            owner.register_new(&mut db, &info(i)).unwrap();
        }
        let before = db.len();
        let revision = owner.status().requested;
        assert_eq!(
            owner.register_new(&mut db, &info(999)).unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(db.len(), before);
        assert_eq!(owner.status().requested, revision);
        for i in 0..100 {
            owner
                .finish_setup(
                    &mut db,
                    &info(1),
                    format!("Newest {i}"),
                    ControllerKind::FightStick,
                )
                .unwrap();
            assert_eq!(owner.status().queued_controllers, MAX_PENDING_CONTROLLERS);
            assert!(owner.status().queued_bytes <= MAX_PENDING_BYTES);
        }
        gate.release();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        let disk = ControllerDb::load_from(db.path());
        assert_eq!(disk.len(), db.len());
        for item in db.list_entries() {
            assert_eq!(db.get_by_id(&item.id), disk.get_by_id(&item.id));
        }
    }
    #[test]
    fn huge_change_is_rejected_before_view_mutation() {
        let f = fixture("large");
        let mut db = ControllerDb::load_from(&f.0.join("controllers.json").to_string_lossy());
        let mut owner = ControllerPersistence::start(&db).unwrap();
        assert_eq!(
            owner
                .finish_setup(
                    &mut db,
                    &info(0),
                    "x".repeat(MAX_PENDING_BYTES),
                    ControllerKind::Arcade
                )
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert!(db.is_empty());
        assert_eq!(owner.status().requested, 0);
        owner.shutdown(Duration::from_secs(2)).unwrap();
    }
    #[test]
    fn failure_requires_explicit_retry_and_preserves_latest_memory_state() {
        let f = fixture("retry");
        std::fs::write(f.0.join("blocked"), b"file").unwrap();
        let mut db =
            ControllerDb::load_from(&f.0.join("blocked/controllers.json").to_string_lossy());
        let mut owner = ControllerPersistence::start(&db).unwrap();
        let device = info(0);
        let revision = owner
            .finish_setup(&mut db, &device, "Pending".into(), ControllerKind::Simple)
            .unwrap();
        assert!(owner.flush(Duration::from_secs(2)).is_err());
        let status = owner.status();
        assert_eq!(status.requested, revision);
        assert!(status.is_failed());
        assert!(db.get(&device).unwrap().setup_complete);
        let failure = owner.take_completion().unwrap();
        assert_eq!(failure.revision, revision);
        assert!(failure.result.is_err());
        std::fs::remove_file(f.0.join("blocked")).unwrap();
        std::fs::create_dir(f.0.join("blocked")).unwrap();
        let retried = owner.retry().unwrap();
        assert!(retried > revision);
        owner.shutdown(Duration::from_secs(2)).unwrap();
        let disk = ControllerDb::load_from(db.path());
        assert_eq!(disk.get(&device), db.get(&device));
        assert!(!owner.status().is_failed());
    }
    #[test]
    fn failed_old_save_does_not_mark_pending_new_generation_failed() {
        let f = fixture("stale-failure");
        std::fs::write(f.0.join("blocked"), b"file").unwrap();
        let mut db =
            ControllerDb::load_from(&f.0.join("blocked/controllers.json").to_string_lossy());
        let first = Arc::new(Gate::default());
        let second = Arc::new(Gate::default());
        let _release = Release(vec![first.clone(), second.clone()]);
        let gates = [first.clone(), second.clone()];
        let n = std::sync::atomic::AtomicUsize::new(0);
        db.set_save_probe(SaveProbe::new(move |phase| {
            if matches!(phase, SavePhase::Started)
                && let Some(gate) = gates.get(n.fetch_add(1, std::sync::atomic::Ordering::Relaxed))
            {
                gate.wait();
            }
        }));
        let mut owner = ControllerPersistence::start(&db).unwrap();
        owner.register_new(&mut db, &info(0)).unwrap();
        first.entered();
        let latest = owner
            .finish_setup(&mut db, &info(0), "Latest".into(), ControllerKind::Arcade)
            .unwrap();
        first.release();
        second.entered();
        let status = owner.status();
        assert_eq!(status.requested, latest);
        assert!(status.is_pending());
        assert!(!status.is_failed());
        assert!(owner.take_completion().unwrap().result.is_err());
        std::fs::remove_file(f.0.join("blocked")).unwrap();
        std::fs::create_dir(f.0.join("blocked")).unwrap();
        second.release();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        assert_eq!(
            ControllerDb::load_from(db.path()).get(&info(0)),
            db.get(&info(0))
        );
    }
    #[test]
    fn sighting_deltas_are_included_in_next_save_without_extra_save() {
        let f = fixture("sightings");
        let mut db = ControllerDb::load_from(&f.0.join("controllers.json").to_string_lossy());
        let mut first = info(0);
        db.upsert(&first, ControllerDb::default_entry(&first));
        let mut owner = ControllerPersistence::start(&db).unwrap();
        first.usb_port = "moved-pending-port".into();
        assert!(owner.note_sighting(&mut db, &first).unwrap());
        assert_eq!(owner.status().requested, 0);
        owner.register_new(&mut db, &info(1)).unwrap();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        let disk = ControllerDb::load_from(db.path());
        assert_eq!(disk.get(&first), db.get(&first));
    }
    #[test]
    fn shutdown_timeout_does_not_block_drop_and_accepted_writes_finish() {
        let f = fixture("shutdown");
        let mut db = ControllerDb::load_from(&f.0.join("controllers.json").to_string_lossy());
        let gate = Arc::new(Gate::default());
        let _release = Release(vec![gate.clone()]);
        let g = gate.clone();
        db.set_save_probe(SaveProbe::new(move |phase| {
            if matches!(phase, SavePhase::Started) {
                g.wait();
            }
        }));
        let mut owner = ControllerPersistence::start(&db).unwrap();
        let device = info(0);
        owner.register_new(&mut db, &device).unwrap();
        gate.entered();
        owner
            .finish_setup(
                &mut db,
                &device,
                "Before exit".into(),
                ControllerKind::Arcade,
            )
            .unwrap();
        assert_eq!(
            owner.shutdown(Duration::ZERO).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        assert_eq!(owner.retry().unwrap_err().kind(), io::ErrorKind::BrokenPipe);
        gate.release();
        owner.shutdown(Duration::from_secs(2)).unwrap();
        assert_eq!(
            ControllerDb::load_from(db.path()).get(&device),
            db.get(&device)
        );
    }
    #[test]
    fn ordinary_drop_detaches_even_while_writer_is_gated() {
        let f = fixture("drop");
        let mut db = ControllerDb::load_from(&f.0.join("controllers.json").to_string_lossy());
        let gate = Arc::new(Gate::default());
        let _release = Release(vec![gate.clone()]);
        let g = gate.clone();
        let (finished_tx, finished_rx) = std::sync::mpsc::channel();
        db.set_save_probe(SaveProbe::new(move |phase| match phase {
            SavePhase::Started => g.wait(),
            SavePhase::Finished(ok) => {
                assert!(ok);
                let _ = finished_tx.send(());
            }
        }));
        let mut owner = ControllerPersistence::start(&db).unwrap();
        owner.register_new(&mut db, &info(0)).unwrap();
        gate.entered();
        let (dropped_tx, dropped_rx) = std::sync::mpsc::channel();
        let t = std::thread::spawn(move || {
            drop(owner);
            dropped_tx.send(()).unwrap();
        });
        dropped_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert!(finished_rx.try_recv().is_err());
        gate.release();
        finished_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        t.join().unwrap();
        assert_eq!(
            ControllerDb::load_from(db.path()).get(&info(0)),
            db.get(&info(0))
        );
    }
    #[test]
    fn writer_panic_reports_failure_and_wakes_lifecycle_waiter() {
        let f = fixture("panic");
        let mut db = ControllerDb::load_from(&f.0.join("controllers.json").to_string_lossy());
        db.set_save_probe(SaveProbe::new(|phase| {
            if matches!(phase, SavePhase::Started) {
                panic!("injected writer failure");
            }
        }));
        let mut owner = ControllerPersistence::start(&db).unwrap();
        owner.register_new(&mut db, &info(0)).unwrap();
        assert!(owner.flush(Duration::from_secs(2)).is_err());
        assert!(owner.shutdown(Duration::from_secs(2)).is_err());
        assert!(owner.status().is_failed());
    }
}
