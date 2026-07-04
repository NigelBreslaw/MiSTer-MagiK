//! Persistent registry of known game controllers on `/media/fat`.
//!
//! Database keys are **logical ids** (`vid:pid[:serial]`) — stable across USB port
//! changes. `last_usb_port` records where the pad was last seen; when it differs
//! from the current port the setup UI can offer "existing controller" vs "new".

use crate::input_info::PadInfo;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::path::Path;

pub const CONTROLLERS_PATH: &str = "/media/fat/mister-magik/controllers.json";

const DB_VERSION: u32 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControllerKind {
    Gamepad,
    FightStick,
    Arcade,
    Simple,
    Unknown,
}

impl ControllerKind {
    pub const ALL: [Self; 5] = [
        Self::Gamepad,
        Self::FightStick,
        Self::Arcade,
        Self::Simple,
        Self::Unknown,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Gamepad => "gamepad",
            Self::FightStick => "fight_stick",
            Self::Arcade => "arcade",
            Self::Simple => "simple",
            Self::Unknown => "unknown",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Gamepad => "Gamepad",
            Self::FightStick => "Fight stick",
            Self::Arcade => "Arcade",
            Self::Simple => "Simple",
            Self::Unknown => "Unknown",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&k| k == self).unwrap_or(4)
    }

    pub fn from_index(i: usize) -> Self {
        Self::ALL[i.min(Self::ALL.len() - 1)]
    }
}

/// How a plugged-in pad relates to the registry (for setup UI routing).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PadRegistryStatus {
    /// No entry — truly new hardware (for this logical id).
    Unknown,
    /// Entry exists but user has not finished setup.
    PendingSetup,
    /// Known, plugged in at the same port as last time.
    Known,
    /// Known logical device, but USB port changed — ask new vs existing.
    MovedPort,
}

impl PadRegistryStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown (not in database)",
            Self::PendingSetup => "pending setup",
            Self::Known => "known",
            Self::MovedPort => "moved USB port",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ControllerEntry {
    pub label: String,
    pub kernel_name: String,
    pub kind: ControllerKind,
    pub setup_complete: bool,
    /// Last USB hub port this device was confirmed on (e.g. `1-1.3`).
    #[serde(default)]
    pub last_usb_port: String,
}

/// One row for the "pick existing controller" list in setup UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerListItem {
    pub id: String,
    pub label: String,
    pub kind: ControllerKind,
    pub last_usb_port: String,
    pub setup_complete: bool,
}

#[derive(Debug, Serialize, Deserialize)]
struct ControllerFile {
    version: u32,
    controllers: HashMap<String, ControllerEntry>,
}

#[derive(Debug, Clone)]
pub struct ControllerDb {
    path: String,
    entries: HashMap<String, ControllerEntry>,
}

impl ControllerDb {
    pub fn load() -> Self {
        Self::load_from(CONTROLLERS_PATH)
    }

    pub fn load_from(path: &str) -> Self {
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<ControllerFile>(&text) {
                Ok(file) if file.version == DB_VERSION => Self {
                    path: path.to_string(),
                    entries: file.controllers,
                },
                Ok(file) if file.version == 1 => {
                    crate::ui_errln!("controller db: migrating v1 → v2 in {path}");
                    Self {
                        path: path.to_string(),
                        entries: migrate_v1_entries(file.controllers),
                    }
                }
                Ok(file) => {
                    crate::ui_errln!(
                        "controller db: unsupported version {} in {path}, starting empty",
                        file.version
                    );
                    Self::empty(path)
                }
                Err(e) => {
                    crate::ui_errln!("controller db: parse error in {path}: {e}, starting empty");
                    Self::empty(path)
                }
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                crate::ui_errln!("controller db: no file at {path} (starting empty)");
                Self::empty(path)
            }
            Err(e) => {
                crate::ui_errln!("controller db: read {path}: {e}, starting empty");
                Self::empty(path)
            }
        }
    }

    fn empty(path: &str) -> Self {
        Self {
            path: path.to_string(),
            entries: HashMap::new(),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Registry key: `vid:pid` or `vid:pid:serial` — **no USB port**.
    pub fn logical_id(info: &PadInfo) -> String {
        let vid = strip_hex_prefix(&info.vendor_id);
        let pid = strip_hex_prefix(&info.product_id);
        if info.serial.is_empty() {
            format!("{vid}:{pid}")
        } else {
            format!("{vid}:{pid}:{}", info.serial)
        }
    }

    /// Runtime plug identity including port (logging / hot-plug only).
    pub fn plug_id(info: &PadInfo) -> String {
        format!("{}:{}", info.usb_port, Self::logical_id(info))
    }

    pub fn get(&self, info: &PadInfo) -> Option<&ControllerEntry> {
        self.entries.get(&Self::logical_id(info))
    }

    pub fn get_by_id(&self, logical_id: &str) -> Option<&ControllerEntry> {
        self.entries.get(logical_id)
    }

    pub fn get_mut(&mut self, info: &PadInfo) -> Option<&mut ControllerEntry> {
        self.entries.get_mut(&Self::logical_id(info))
    }

    pub fn contains(&self, info: &PadInfo) -> bool {
        self.entries.contains_key(&Self::logical_id(info))
    }

    pub fn is_setup(&self, info: &PadInfo) -> bool {
        self.get(info).is_some_and(|e| e.setup_complete)
    }

    /// True until the user finishes the setup wizard for this pad.
    pub fn needs_setup(&self, info: &PadInfo) -> bool {
        !matches!(self.registry_status(info), PadRegistryStatus::Known)
    }

    /// Whether this pad needs the "new or existing?" setup prompt.
    pub fn registry_status(&self, info: &PadInfo) -> PadRegistryStatus {
        match self.get(info) {
            None => PadRegistryStatus::Unknown,
            Some(e) if !e.setup_complete => PadRegistryStatus::PendingSetup,
            Some(e) if port_changed(e, info) => PadRegistryStatus::MovedPort,
            Some(_) => PadRegistryStatus::Known,
        }
    }

    pub fn port_changed(&self, info: &PadInfo) -> bool {
        self.get(info).is_some_and(|e| port_changed(e, info))
    }

    /// Sorted list for setup UI when user picks an existing controller.
    pub fn list_entries(&self) -> Vec<ControllerListItem> {
        let mut items: Vec<_> = self
            .entries
            .iter()
            .map(|(id, e)| ControllerListItem {
                id: id.clone(),
                label: e.label.clone(),
                kind: e.kind,
                last_usb_port: e.last_usb_port.clone(),
                setup_complete: e.setup_complete,
            })
            .collect();
        items.sort_by_key(|item| item.label.to_lowercase());
        items
    }

    /// User chose an existing registry entry for this physical plug (USB port change).
    pub fn claim_existing(&mut self, info: &PadInfo, logical_id: &str) -> io::Result<()> {
        let entry = self.entries.get_mut(logical_id).ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("no controller entry {logical_id}"),
            )
        })?;
        entry.last_usb_port = info.usb_port.clone();
        if entry.kernel_name.is_empty() {
            entry.kernel_name = info.name.clone();
        }
        Ok(())
    }

    pub fn display_label(&self, info: &PadInfo) -> String {
        if let Some(entry) = self.get(info) {
            if !entry.label.is_empty() {
                return entry.label.clone();
            }
        }
        if !info.name.is_empty() {
            return info.name.clone();
        }
        format!(
            "Controller {}:{}",
            strip_hex_prefix(&info.vendor_id),
            strip_hex_prefix(&info.product_id)
        )
    }

    pub fn upsert(&mut self, info: &PadInfo, mut entry: ControllerEntry) {
        entry.last_usb_port = info.usb_port.clone();
        self.entries.insert(Self::logical_id(info), entry);
    }

    /// Save label + kind and mark setup finished for this pad.
    pub fn finish_setup(&mut self, info: &PadInfo, label: String, kind: ControllerKind) {
        let mut entry = self
            .get(info)
            .cloned()
            .unwrap_or_else(|| Self::default_entry(info));
        entry.label = label;
        entry.kind = kind;
        entry.kernel_name = info.name.clone();
        entry.setup_complete = true;
        self.upsert(info, entry);
    }

    pub fn upsert_id(&mut self, logical_id: &str, mut entry: ControllerEntry, usb_port: &str) {
        entry.last_usb_port = usb_port.to_string();
        self.entries.insert(logical_id.to_string(), entry);
    }

    /// Update port for a known pad at the same logical id and port (no UI needed).
    pub fn note_sighting(&mut self, info: &PadInfo) -> bool {
        let id = Self::logical_id(info);
        let Some(entry) = self.entries.get_mut(&id) else {
            return false;
        };
        if entry.setup_complete && port_changed(entry, info) {
            return false;
        }
        entry.last_usb_port = info.usb_port.clone();
        true
    }

    pub fn remove(&mut self, info: &PadInfo) -> Option<ControllerEntry> {
        self.entries.remove(&Self::logical_id(info))
    }

    pub fn remove_id(&mut self, logical_id: &str) -> Option<ControllerEntry> {
        self.entries.remove(logical_id)
    }

    pub fn save(&self) -> io::Result<()> {
        if let Some(parent) = Path::new(&self.path).parent() {
            fs::create_dir_all(parent)?;
        }
        let file = ControllerFile {
            version: DB_VERSION,
            controllers: self.entries.clone(),
        };
        let text = serde_json::to_string_pretty(&file)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        fs::write(&self.path, text)
    }

    pub fn default_entry(info: &PadInfo) -> ControllerEntry {
        ControllerEntry {
            label: default_label(info),
            kernel_name: info.name.clone(),
            kind: Self::infer_kind(info),
            setup_complete: false,
            last_usb_port: info.usb_port.clone(),
        }
    }

    pub fn infer_kind(info: &PadInfo) -> ControllerKind {
        if info.js_buttons >= 8 && info.js_axes <= 2 {
            ControllerKind::FightStick
        } else if info.js_axes >= 4 {
            ControllerKind::Gamepad
        } else if info.js_buttons <= 6 && info.js_axes <= 2 {
            ControllerKind::Simple
        } else if info.js_buttons >= 8 {
            ControllerKind::Arcade
        } else {
            ControllerKind::Unknown
        }
    }

    pub fn log_pad_status(&self, info: &PadInfo, js_path: &str) {
        let logical = Self::logical_id(info);
        let plug = Self::plug_id(info);
        match self.registry_status(info) {
            PadRegistryStatus::Unknown => {
                crate::ui_errln!(
                    "controller db: {js_path} plug={plug} unknown ({}) kind={:?}",
                    info.name,
                    Self::infer_kind(info)
                );
            }
            PadRegistryStatus::PendingSetup => {
                let e = self.get(info).unwrap();
                crate::ui_errln!(
                    "controller db: {js_path} id={logical} pending setup \"{}\" port={}",
                    e.label,
                    e.last_usb_port
                );
            }
            PadRegistryStatus::MovedPort => {
                let e = self.get(info).unwrap();
                crate::ui_errln!(
                    "controller db: {js_path} id={logical} moved \"{}\" was {} now {}",
                    e.label,
                    e.last_usb_port,
                    info.usb_port
                );
            }
            PadRegistryStatus::Known => {
                let e = self.get(info).unwrap();
                crate::ui_errln!(
                    "controller db: {js_path} id={logical} known \"{}\" port={}",
                    e.label,
                    e.last_usb_port
                );
            }
        }
    }
}

fn port_changed(entry: &ControllerEntry, info: &PadInfo) -> bool {
    !entry.last_usb_port.is_empty() && entry.last_usb_port != info.usb_port
}

/// v1 keys were `{usb_port}:{vid}:{pid}[:serial]` — strip the port prefix.
fn migrate_v1_entries(old: HashMap<String, ControllerEntry>) -> HashMap<String, ControllerEntry> {
    let mut out: HashMap<String, ControllerEntry> = HashMap::new();
    for (key, entry) in old {
        let logical = logical_id_from_v1_key(&key).unwrap_or(key);
        match out.get_mut(&logical) {
            Some(existing) => {
                if entry.setup_complete && !existing.setup_complete {
                    *existing = entry;
                } else if !entry.last_usb_port.is_empty() {
                    existing.last_usb_port = entry.last_usb_port.clone();
                }
            }
            None => {
                out.insert(logical, entry);
            }
        }
    }
    out
}

fn logical_id_from_v1_key(key: &str) -> Option<String> {
    let mut parts = key.splitn(4, ':');
    let first = parts.next()?;
    if !looks_like_usb_port(first) {
        return None;
    }
    let rest: Vec<&str> = parts.collect();
    if rest.len() >= 2 {
        Some(rest.join(":"))
    } else {
        None
    }
}

fn looks_like_usb_port(s: &str) -> bool {
    s.starts_with("1-") && s.contains('.')
}

fn default_label(info: &PadInfo) -> String {
    if !info.name.is_empty() {
        return info.name.clone();
    }
    format!(
        "Controller {}:{}",
        strip_hex_prefix(&info.vendor_id),
        strip_hex_prefix(&info.product_id)
    )
}

fn strip_hex_prefix(raw: &str) -> String {
    raw.trim()
        .strip_prefix("0x")
        .or_else(|| raw.trim().strip_prefix("0X"))
        .unwrap_or(raw.trim())
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn sample_info(vendor: &str, product: &str, port: &str, serial: &str) -> PadInfo {
        PadInfo {
            name: "Test Pad".into(),
            vendor_id: vendor.into(),
            product_id: product.into(),
            serial: serial.into(),
            phys: format!("usb-ffb40000.usb-{port}/input0"),
            usb_port: port.into(),
            js_buttons: 13,
            js_axes: 6,
            evdev_key_count: 0,
            evdev_abs_count: 0,
            capture_available: false,
        }
    }

    fn temp_path(name: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir()
            .join(format!("mister-magik-controller-db-{name}-{nanos}.json"))
            .display()
            .to_string()
    }

    #[test]
    fn logical_id_without_serial() {
        let info = sample_info("0x2563", "0x0575", "1-1.3", "");
        assert_eq!(ControllerDb::logical_id(&info), "2563:0575");
    }

    #[test]
    fn logical_id_with_serial() {
        let info = sample_info("0x2563", "0x0575", "1-1.3", "GH-SP-5027-1 A2");
        assert_eq!(ControllerDb::logical_id(&info), "2563:0575:GH-SP-5027-1 A2");
    }

    #[test]
    fn logical_id_stable_across_ports() {
        let a = sample_info("0x2563", "0x0575", "1-1.3", "SN");
        let b = sample_info("0x2563", "0x0575", "1-1.7", "SN");
        assert_eq!(ControllerDb::logical_id(&a), ControllerDb::logical_id(&b));
    }

    #[test]
    fn migrate_v1_key() {
        assert_eq!(
            logical_id_from_v1_key("1-1.3:2563:0575"),
            Some("2563:0575".into())
        );
        assert_eq!(
            logical_id_from_v1_key("1-1.3:2563:0575:GH-SP-5027-1 A2"),
            Some("2563:0575:GH-SP-5027-1 A2".into())
        );
    }

    #[test]
    fn moved_port_status() {
        let mut db = ControllerDb::empty("/tmp/t.json");
        let old = sample_info("0x2563", "0x0575", "1-1.3", "SN-A");
        db.upsert(
            &old,
            ControllerEntry {
                label: "My Pad".into(),
                kernel_name: "Pad".into(),
                kind: ControllerKind::Gamepad,
                setup_complete: true,
                last_usb_port: "1-1.3".into(),
            },
        );
        let new_port = sample_info("0x2563", "0x0575", "1-1.7", "SN-A");
        assert_eq!(db.registry_status(&new_port), PadRegistryStatus::MovedPort);
        assert!(!db.note_sighting(&new_port));
        db.claim_existing(&new_port, "2563:0575:SN-A").unwrap();
        assert_eq!(db.registry_status(&new_port), PadRegistryStatus::Known);
        assert_eq!(db.get(&new_port).unwrap().last_usb_port, "1-1.7");
    }

    #[test]
    fn list_entries_sorted() {
        let mut db = ControllerDb::empty("/tmp/t.json");
        db.upsert_id(
            "aaa:1111",
            ControllerEntry {
                label: "Zebra".into(),
                kernel_name: String::new(),
                kind: ControllerKind::Gamepad,
                setup_complete: true,
                last_usb_port: "1-1.1".into(),
            },
            "1-1.1",
        );
        db.upsert_id(
            "bbb:2222",
            ControllerEntry {
                label: "Alpha".into(),
                kernel_name: String::new(),
                kind: ControllerKind::FightStick,
                setup_complete: true,
                last_usb_port: "1-1.2".into(),
            },
            "1-1.2",
        );
        let list = db.list_entries();
        assert_eq!(list[0].label, "Alpha");
        assert_eq!(list[1].label, "Zebra");
    }

    #[test]
    fn finish_setup_marks_complete() {
        let mut db = ControllerDb::empty("/tmp/t.json");
        let info = sample_info("0x2563", "0x0575", "1-1.3", "SN");
        db.finish_setup(&info, "My A2".into(), ControllerKind::Gamepad);
        assert!(db.is_setup(&info));
        let e = db.get(&info).unwrap();
        assert_eq!(e.label, "My A2");
        assert_eq!(e.kind, ControllerKind::Gamepad);
    }

    #[test]
    fn kind_labels_and_indexes_are_stable_for_setup_ui() {
        for (idx, kind) in ControllerKind::ALL.into_iter().enumerate() {
            assert_eq!(kind.index(), idx);
            assert_eq!(ControllerKind::from_index(idx), kind);
            assert!(!kind.as_str().is_empty());
            assert!(!kind.label().is_empty());
        }
        assert_eq!(
            ControllerKind::from_index(usize::MAX),
            ControllerKind::Unknown
        );
        assert_eq!(ControllerKind::FightStick.as_str(), "fight_stick");
        assert_eq!(ControllerKind::FightStick.label(), "Fight stick");
    }

    #[test]
    fn registry_status_labels_are_agent_readable() {
        assert_eq!(
            PadRegistryStatus::Unknown.as_str(),
            "unknown (not in database)"
        );
        assert_eq!(PadRegistryStatus::PendingSetup.as_str(), "pending setup");
        assert_eq!(PadRegistryStatus::Known.as_str(), "known");
        assert_eq!(PadRegistryStatus::MovedPort.as_str(), "moved USB port");
    }

    #[test]
    fn display_label_falls_back_to_kernel_name_then_vid_pid() {
        let db = ControllerDb::empty("/tmp/t.json");
        let named = sample_info("0x2563", "0x0575", "1-1.3", "");
        assert_eq!(db.display_label(&named), "Test Pad");

        let unnamed = PadInfo {
            name: String::new(),
            vendor_id: " 0X2563 ".into(),
            product_id: "0x0575".into(),
            usb_port: "1-1.3".into(),
            ..PadInfo::default()
        };
        assert_eq!(db.display_label(&unnamed), "Controller 2563:0575");
        assert_eq!(
            ControllerDb::default_entry(&unnamed).label,
            "Controller 2563:0575"
        );
    }

    #[test]
    fn claim_existing_reports_missing_id_and_fills_empty_kernel_name() {
        let mut db = ControllerDb::empty("/tmp/t.json");
        let old = sample_info("0x2563", "0x0575", "1-1.3", "SN-A");
        let new_port = sample_info("0x2563", "0x0575", "1-1.7", "SN-A");

        let missing = db.claim_existing(&new_port, "2563:0575:SN-A").unwrap_err();
        assert_eq!(missing.kind(), io::ErrorKind::NotFound);

        db.upsert(
            &old,
            ControllerEntry {
                label: "My Pad".into(),
                kernel_name: String::new(),
                kind: ControllerKind::Gamepad,
                setup_complete: true,
                last_usb_port: "1-1.3".into(),
            },
        );
        db.claim_existing(&new_port, "2563:0575:SN-A").unwrap();
        let entry = db.get(&new_port).unwrap();
        assert_eq!(entry.last_usb_port, "1-1.7");
        assert_eq!(entry.kernel_name, "Test Pad");
    }

    #[test]
    fn remove_by_info_and_id_delete_entries() {
        let mut db = ControllerDb::empty("/tmp/t.json");
        let first = sample_info("2563", "0575", "1-1.3", "A");
        let second = sample_info("2563", "0575", "1-1.4", "B");
        db.upsert(&first, ControllerDb::default_entry(&first));
        db.upsert(&second, ControllerDb::default_entry(&second));

        assert!(db.remove(&first).is_some());
        assert!(!db.contains(&first));
        assert!(db.remove_id("2563:0575:B").is_some());
        assert!(db.is_empty());
    }

    #[test]
    fn infer_kind_heuristics() {
        let fight = PadInfo {
            js_buttons: 10,
            js_axes: 2,
            ..PadInfo::default()
        };
        assert_eq!(ControllerDb::infer_kind(&fight), ControllerKind::FightStick);
    }

    #[test]
    fn load_save_round_trips_entries_and_path() {
        let path = temp_path("round-trip");
        let info = sample_info("0X2563", " 0x0575 ", "1-1.3", "");
        let mut db = ControllerDb::load_from(&path);

        assert_eq!(db.path(), path);
        assert!(db.is_empty());
        assert_eq!(db.registry_status(&info), PadRegistryStatus::Unknown);
        assert!(db.needs_setup(&info));

        db.finish_setup(&info, "Arcade Pad".into(), ControllerKind::Arcade);
        db.save().expect("save controller db");

        let loaded = ControllerDb::load_from(&path);
        let entry = loaded.get(&info).expect("saved controller");
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains(&info));
        assert_eq!(loaded.display_label(&info), "Arcade Pad");
        assert_eq!(entry.kind, ControllerKind::Arcade);
        assert_eq!(entry.last_usb_port, "1-1.3");
        assert_eq!(loaded.registry_status(&info), PadRegistryStatus::Known);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn load_unsupported_or_malformed_file_starts_empty() {
        let unsupported = temp_path("unsupported");
        fs::write(&unsupported, r#"{"version":999,"controllers":{}}"#)
            .expect("write unsupported controller db");
        assert!(ControllerDb::load_from(&unsupported).is_empty());

        let malformed = temp_path("malformed");
        fs::write(&malformed, "{not-json").expect("write malformed controller db");
        assert!(ControllerDb::load_from(&malformed).is_empty());

        let _ = fs::remove_file(unsupported);
        let _ = fs::remove_file(malformed);
    }

    #[test]
    fn note_sighting_updates_pending_but_not_moved_complete_pad() {
        let mut db = ControllerDb::empty("/tmp/t.json");
        let first = sample_info("2563", "0575", "1-1.3", "SN");
        db.upsert(&first, ControllerDb::default_entry(&first));

        let pending_move = sample_info("2563", "0575", "1-1.7", "SN");
        assert_eq!(
            db.registry_status(&pending_move),
            PadRegistryStatus::PendingSetup
        );
        assert!(db.note_sighting(&pending_move));
        assert_eq!(db.get(&pending_move).unwrap().last_usb_port, "1-1.7");

        db.finish_setup(&pending_move, "Done".into(), ControllerKind::Gamepad);
        let moved_again = sample_info("2563", "0575", "1-1.9", "SN");
        assert!(!db.note_sighting(&moved_again));
        assert_eq!(
            db.registry_status(&moved_again),
            PadRegistryStatus::MovedPort
        );
    }
}
