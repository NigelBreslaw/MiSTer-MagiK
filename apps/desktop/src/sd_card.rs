// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub const ROOT_PATH: &str = "/";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SdEntryKind {
    Directory,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdEntry {
    pub name: String,
    pub path: String,
    pub kind: SdEntryKind,
}

impl SdEntry {
    pub fn is_directory(&self) -> bool {
        self.kind == SdEntryKind::Directory
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdDirectoryListing {
    pub path: String,
    pub entries: Vec<SdEntry>,
    pub elapsed_ms: u64,
    pub round_trip_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdMetadataRow {
    pub label: String,
    pub value: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdItemDetail {
    pub path: String,
    pub title: String,
    pub subtitle: String,
    pub kind: String,
    pub icon_key: String,
    pub size_label: String,
    pub modified_label: String,
    pub flags_label: String,
    pub loading: bool,
    pub error: String,
    pub has_image: bool,
    pub image_path: String,
    pub image_summary: String,
    pub is_mra: bool,
    pub overview_rows: Vec<SdMetadataRow>,
    pub mra_summary_rows: Vec<SdMetadataRow>,
    pub mra_xml_rows: Vec<SdMetadataRow>,
    pub mra_path_rows: Vec<SdMetadataRow>,
    pub mra_warnings: Vec<SdMetadataRow>,
    pub raw_xml: String,
    pub raw_xml_truncated: bool,
}

impl SdItemDetail {
    pub fn empty() -> Self {
        Self {
            path: ROOT_PATH.to_string(),
            title: "SD Card".to_string(),
            subtitle: "Select a file or folder to inspect details.".to_string(),
            kind: "directory".to_string(),
            icon_key: "folder-base".to_string(),
            size_label: "-".to_string(),
            modified_label: "-".to_string(),
            flags_label: "-".to_string(),
            loading: false,
            error: String::new(),
            has_image: false,
            image_path: String::new(),
            image_summary: String::new(),
            is_mra: false,
            overview_rows: Vec::new(),
            mra_summary_rows: Vec::new(),
            mra_xml_rows: Vec::new(),
            mra_path_rows: Vec::new(),
            mra_warnings: Vec::new(),
            raw_xml: String::new(),
            raw_xml_truncated: false,
        }
    }

    pub fn loading_for(path: &str) -> Self {
        let name = item_name(path);
        let kind = fallback_kind_for_path(path);
        Self {
            path: path.to_string(),
            title: name.clone(),
            subtitle: format!("Loading details for {path}..."),
            kind: kind.to_string(),
            icon_key: fallback_icon_key(kind, &name).to_string(),
            loading: true,
            ..Self::empty()
        }
    }

    pub fn folder_for(path: &str) -> Self {
        let path = normalize_ui_path(path);
        let name = item_name(&path);
        Self {
            path: path.clone(),
            title: name,
            subtitle: "Folder on /media/fat".to_string(),
            kind: "directory".to_string(),
            icon_key: "folder-base".to_string(),
            overview_rows: vec![
                SdMetadataRow {
                    label: "Path".to_string(),
                    value: path,
                    kind: "path".to_string(),
                },
                SdMetadataRow {
                    label: "Type".to_string(),
                    value: "directory".to_string(),
                    kind: "text".to_string(),
                },
            ],
            ..Self::empty()
        }
    }

    pub fn error_for(path: &str, error: String) -> Self {
        let name = item_name(path);
        let kind = fallback_kind_for_path(path);
        Self {
            path: path.to_string(),
            title: name.clone(),
            subtitle: "Could not load item details.".to_string(),
            kind: kind.to_string(),
            icon_key: fallback_icon_key(kind, &name).to_string(),
            error,
            ..Self::empty()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdDetailRequest {
    pub path: String,
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SdTreeRow {
    pub id: String,
    pub label: String,
    pub icon_key: String,
    pub level: i32,
    pub has_children: bool,
    pub expanded: bool,
    pub current: bool,
    pub leading_is_directory: bool,
    pub interactive: bool,
    pub is_skeleton: bool,
    pub loading_children_badge: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CachedDirectory {
    Loaded {
        entries: Vec<SdEntry>,
        elapsed_ms: u64,
    },
    Failed(String),
}

#[derive(Clone, Debug)]
pub struct SdCardBrowser {
    expanded_paths: HashSet<String>,
    current_path: String,
    loading_paths: HashSet<String>,
    loading_started_at: HashMap<String, Instant>,
    visible_loading_paths: HashSet<String>,
    directory_cache: HashMap<String, CachedDirectory>,
    detail_cache: HashMap<String, SdItemDetail>,
    tree_rows: Vec<SdTreeRow>,
    selected_detail: SdItemDetail,
    detail_generation: u64,
    show_hidden: bool,
    status: String,
    last_error: String,
}

impl Default for SdCardBrowser {
    fn default() -> Self {
        Self::new()
    }
}

impl SdCardBrowser {
    pub fn new() -> Self {
        let mut browser = Self {
            expanded_paths: HashSet::new(),
            current_path: ROOT_PATH.to_string(),
            loading_paths: HashSet::new(),
            loading_started_at: HashMap::new(),
            visible_loading_paths: HashSet::new(),
            directory_cache: HashMap::new(),
            detail_cache: HashMap::new(),
            tree_rows: Vec::new(),
            selected_detail: SdItemDetail::empty(),
            detail_generation: 0,
            show_hidden: false,
            status: "Ready to browse /media/fat.".to_string(),
            last_error: String::new(),
        };
        browser.rebuild_rows();
        browser
    }

    pub fn rows(&self) -> &[SdTreeRow] {
        &self.tree_rows
    }

    pub fn selected_detail(&self) -> &SdItemDetail {
        &self.selected_detail
    }

    pub fn current_path(&self) -> &str {
        &self.current_path
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn last_error(&self) -> &str {
        &self.last_error
    }

    pub fn loading(&self) -> bool {
        !self.loading_paths.is_empty()
    }

    pub fn show_hidden(&self) -> bool {
        self.show_hidden
    }

    pub fn set_show_hidden(&mut self, show_hidden: bool) -> Option<String> {
        if self.show_hidden == show_hidden {
            self.rebuild_rows();
            return None;
        }

        self.show_hidden = show_hidden;
        self.directory_cache.clear();
        self.loading_paths.clear();
        self.loading_started_at.clear();
        self.visible_loading_paths.clear();
        self.last_error.clear();
        self.status = if show_hidden {
            "Showing hidden SD Card entries.".to_string()
        } else {
            "Hiding hidden SD Card entries.".to_string()
        };

        if self.expanded_paths.is_empty() {
            self.rebuild_rows();
            None
        } else {
            let path = self.refresh_target();
            self.expanded_paths.insert(path.clone());
            self.begin_fetch(path)
        }
    }

    pub fn toggle_directory(&mut self, raw_path: &str) -> Option<String> {
        let path = normalize_ui_path(raw_path);
        self.current_path = path.clone();
        self.detail_generation = self.detail_generation.saturating_add(1);
        self.selected_detail = SdItemDetail::folder_for(&path);
        self.last_error.clear();

        if self.expanded_paths.remove(&path) {
            self.status = format!("Collapsed {path}");
            self.rebuild_rows();
            return None;
        }

        self.expanded_paths.insert(path.clone());
        if self.directory_cache.contains_key(&path) {
            self.apply_cached_status(&path);
            self.rebuild_rows();
            return None;
        }

        self.begin_fetch(path)
    }

    pub fn select_path(&mut self, raw_path: &str) {
        self.current_path = normalize_ui_path(raw_path);
        self.last_error.clear();
        self.status = format!("Selected {}", self.current_path);
        self.rebuild_rows();
    }

    pub fn begin_detail_fetch_current(&mut self, force: bool) -> Option<SdDetailRequest> {
        let path = normalize_ui_path(&self.current_path);
        self.detail_generation = self.detail_generation.saturating_add(1);
        let generation = self.detail_generation;
        if self.selected_detail.path == path && self.selected_detail.kind == "directory" {
            self.selected_detail = SdItemDetail::folder_for(&path);
            return None;
        }
        if !force {
            if let Some(detail) = self.detail_cache.get(&path).cloned() {
                self.selected_detail = detail;
                return None;
            }
        }
        self.selected_detail = SdItemDetail::loading_for(&path);
        Some(SdDetailRequest { path, generation })
    }

    pub fn apply_detail_result(
        &mut self,
        path: &str,
        generation: u64,
        result: Result<SdItemDetail, String>,
    ) {
        if generation != self.detail_generation || normalize_ui_path(path) != self.current_path {
            return;
        }
        let detail = match result {
            Ok(detail) => detail,
            Err(err) => SdItemDetail::error_for(path, err),
        };
        self.detail_cache.insert(path.to_string(), detail.clone());
        self.selected_detail = detail;
    }

    pub fn refresh_current_folder(&mut self) -> Option<String> {
        let path = self.refresh_target();
        self.directory_cache.remove(&path);
        self.detail_cache.remove(&path);
        self.expanded_paths.insert(path.clone());
        self.current_path = path.clone();
        self.last_error.clear();
        self.begin_fetch(path)
    }

    pub fn apply_listing(&mut self, raw_path: &str, result: Result<SdDirectoryListing, String>) {
        let path = normalize_ui_path(raw_path);
        self.loading_paths.remove(&path);
        self.loading_started_at.remove(&path);
        self.visible_loading_paths.remove(&path);
        match result {
            Ok(listing) => {
                let entry_count = listing.entries.len();
                let elapsed_ms = listing.elapsed_ms;
                let round_trip_ms = listing.round_trip_ms;
                self.directory_cache.insert(
                    path.clone(),
                    CachedDirectory::Loaded {
                        entries: listing.entries,
                        elapsed_ms,
                    },
                );
                self.last_error.clear();
                self.status = format!(
                    "Loaded {path}: {entry_count} entries in {elapsed_ms}ms agent / {round_trip_ms}ms total"
                );
            }
            Err(err) => {
                self.directory_cache
                    .insert(path.clone(), CachedDirectory::Failed(err.clone()));
                self.last_error = format!("{path}: {err}");
                self.status = format!("Could not load {path}");
            }
        }
        self.rebuild_rows();
    }

    pub fn apply_listing_if_current_policy(
        &mut self,
        raw_path: &str,
        show_hidden: bool,
        result: Result<SdDirectoryListing, String>,
    ) {
        if self.show_hidden != show_hidden {
            return;
        }
        self.apply_listing(raw_path, result);
    }

    #[cfg(test)]
    pub fn has_cached_directory(&self, raw_path: &str) -> bool {
        self.directory_cache
            .contains_key(&normalize_ui_path(raw_path))
    }

    fn begin_fetch(&mut self, path: String) -> Option<String> {
        if !self.loading_paths.insert(path.clone()) {
            self.rebuild_rows();
            return None;
        }
        self.loading_started_at.insert(path.clone(), Instant::now());
        self.rebuild_rows();
        Some(path)
    }

    pub fn reveal_loading_after(&mut self, raw_path: &str, delay: Duration) -> bool {
        let path = normalize_ui_path(raw_path);
        let delay_elapsed = self
            .loading_started_at
            .get(&path)
            .is_some_and(|started_at| started_at.elapsed() >= delay);
        if !self.loading_paths.contains(&path)
            || !delay_elapsed
            || !self.visible_loading_paths.insert(path.clone())
        {
            return false;
        }
        self.status = format!("Loading {path}...");
        self.rebuild_rows();
        true
    }

    fn refresh_target(&self) -> String {
        let current = normalize_ui_path(&self.current_path);
        if self.directory_cache.contains_key(&current) || self.expanded_paths.contains(&current) {
            return current;
        }
        parent_path(&current)
    }

    fn apply_cached_status(&mut self, path: &str) {
        match self.directory_cache.get(path) {
            Some(CachedDirectory::Loaded {
                entries,
                elapsed_ms,
            }) => {
                self.status = format!(
                    "Showing cached {path}: {} entries loaded in {elapsed_ms}ms",
                    entries.len()
                );
            }
            Some(CachedDirectory::Failed(err)) => {
                self.last_error = format!("{path}: {err}");
                self.status = format!("Showing cached error for {path}");
            }
            None => {}
        }
    }

    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        self.push_directory_row(&mut rows, ROOT_PATH, "SD Card", 1);
        self.tree_rows = rows;
    }

    fn push_directory_row(&self, rows: &mut Vec<SdTreeRow>, path: &str, label: &str, level: i32) {
        let expanded = self.expanded_paths.contains(path);
        rows.push(SdTreeRow {
            id: path.to_string(),
            label: label.to_string(),
            icon_key: "folder-base".to_string(),
            level,
            has_children: true,
            expanded,
            current: self.current_path == path,
            leading_is_directory: true,
            interactive: true,
            is_skeleton: false,
            loading_children_badge: if self.visible_loading_paths.contains(path) {
                "loading".to_string()
            } else {
                String::new()
            },
        });

        if !expanded {
            return;
        }

        if self.loading_paths.contains(path) {
            if self.visible_loading_paths.contains(path) {
                for index in 0..3 {
                    rows.push(SdTreeRow {
                        id: format!("{path}::loading-{index}"),
                        label: String::new(),
                        icon_key: "document".to_string(),
                        level: level + 1,
                        has_children: false,
                        expanded: false,
                        current: false,
                        leading_is_directory: false,
                        interactive: false,
                        is_skeleton: true,
                        loading_children_badge: String::new(),
                    });
                }
            }
            return;
        }

        match self.directory_cache.get(path) {
            Some(CachedDirectory::Loaded { entries, .. }) => {
                for entry in entries {
                    if entry.is_directory() {
                        self.push_directory_row(rows, &entry.path, &entry.name, level + 1);
                    } else {
                        rows.push(SdTreeRow {
                            id: entry.path.clone(),
                            label: entry.name.clone(),
                            icon_key: material_icon_key_for_file_name(&entry.name).to_string(),
                            level: level + 1,
                            has_children: false,
                            expanded: false,
                            current: self.current_path == entry.path,
                            leading_is_directory: false,
                            interactive: true,
                            is_skeleton: false,
                            loading_children_badge: String::new(),
                        });
                    }
                }
            }
            Some(CachedDirectory::Failed(err)) => {
                rows.push(message_row(path, &format!("Error: {err}"), level + 1));
            }
            None => {
                rows.push(message_row(path, "Not loaded", level + 1));
            }
        }
    }
}

fn message_row(parent: &str, label: &str, level: i32) -> SdTreeRow {
    SdTreeRow {
        id: format!("{parent}::{label}"),
        label: label.to_string(),
        icon_key: "document".to_string(),
        level,
        has_children: false,
        expanded: false,
        current: false,
        leading_is_directory: false,
        interactive: false,
        is_skeleton: false,
        loading_children_badge: String::new(),
    }
}

pub fn normalize_ui_path(path: &str) -> String {
    let mut parts = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => continue,
            ".." => {
                parts.pop();
            }
            _ => parts.push(part),
        }
    }
    if parts.is_empty() {
        ROOT_PATH.to_string()
    } else {
        format!("/{}", parts.join("/"))
    }
}

pub fn material_icon_key_for_file_name(name: &str) -> &'static str {
    let lowered = name.trim().to_ascii_lowercase();
    let base = lowered.as_str();
    if base == "license" || base == "copying" || base == "unlicense" {
        return "license";
    }
    if base == "readme" || base.starts_with("readme.") {
        return "readme";
    }
    if base.ends_with(".lock") {
        return "lock";
    }

    match base.rsplit_once('.').map(|(_, extension)| extension) {
        Some("7z" | "gz" | "rar" | "tar" | "tgz" | "zip") => "zip",
        Some("bmp" | "gif" | "icns" | "ico" | "jpeg" | "jpg" | "png" | "svg" | "webp") => "image",
        Some("aac" | "flac" | "m4a" | "mp3" | "ogg" | "wav") => "audio",
        Some("avi" | "mkv" | "mov" | "mp4" | "mpeg" | "mpg" | "webm") => "video",
        Some("md" | "mdown" | "markdown") => "markdown",
        Some("json" | "jsonc") => "json",
        Some("xml") => "xml",
        Some("yaml" | "yml") => "yaml",
        Some("toml") => "toml",
        Some("ini" | "cfg" | "conf") => "settings",
        Some("log") => "log",
        Some("pdf") => "pdf",
        Some("mra" | "mgl") => "console",
        Some("cue") => "cue",
        Some("bin" | "chd" | "cso" | "img" | "iso") => "disc",
        Some("rbf" | "sv" | "v" | "vhd" | "vhdl") => "flash",
        Some(
            "gb" | "gba" | "gbc" | "gen" | "mfc" | "n64" | "neo" | "nes" | "pce" | "rom" | "sfc"
            | "smc" | "smd" | "sms" | "z64",
        ) => "disc",
        Some("sav" | "srm" | "sqlite" | "sqlite3" | "db") => "database",
        Some("rs") => "rust",
        Some("sh" | "bash" | "zsh") => "shellcheck",
        Some("ps1" | "psm1") => "powershell",
        Some("exe") => "exe",
        Some("dll") => "dll",
        Some("hex") => "hex",
        Some("c") => "c",
        Some("cc" | "cpp" | "cxx") => "cpp",
        Some("h" | "hh" | "hpp" | "hxx") => "h",
        Some("py") => "python",
        Some("js" | "mjs" | "cjs") => "javascript",
        Some("ts" | "tsx") => "typescript",
        Some("java" | "class" | "jar") => "java",
        Some("ttf" | "otf" | "woff" | "woff2") => "font",
        Some("key" | "pem" | "pub") => "key",
        _ => "document",
    }
}

fn parent_path(path: &str) -> String {
    let path = normalize_ui_path(path);
    if path == ROOT_PATH {
        return path;
    }
    match path.rsplit_once('/') {
        Some(("", _)) | None => ROOT_PATH.to_string(),
        Some((parent, _)) => parent.to_string(),
    }
}

pub fn item_name(path: &str) -> String {
    if path == ROOT_PATH {
        "SD Card".to_string()
    } else {
        path.rsplit('/').next().unwrap_or(path).to_string()
    }
}

fn fallback_kind_for_path(path: &str) -> &'static str {
    if path == ROOT_PATH || !item_name(path).contains('.') {
        "directory"
    } else {
        "file"
    }
}

fn fallback_icon_key(kind: &str, name: &str) -> &'static str {
    if kind == "directory" {
        "folder-base"
    } else {
        material_icon_key_for_file_name(name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, path: &str) -> SdEntry {
        SdEntry {
            name: name.to_string(),
            path: path.to_string(),
            kind: SdEntryKind::Directory,
        }
    }

    fn file(name: &str, path: &str) -> SdEntry {
        SdEntry {
            name: name.to_string(),
            path: path.to_string(),
            kind: SdEntryKind::File,
        }
    }

    #[test]
    fn first_expand_fetches_then_second_expand_uses_cache() {
        let mut browser = SdCardBrowser::new();

        assert_eq!(
            browser.toggle_directory(ROOT_PATH).as_deref(),
            Some(ROOT_PATH)
        );
        assert!(browser.loading());
        assert!(!browser.rows().iter().any(|row| row.is_skeleton));
        assert!(!browser.status().contains("Loading"));

        assert!(browser.reveal_loading_after(ROOT_PATH, Duration::ZERO));
        assert!(browser.rows().iter().any(|row| row.is_skeleton));
        assert!(browser.status().contains("Loading"));
        assert_eq!(
            browser
                .rows()
                .iter()
                .find(|row| row.id == ROOT_PATH)
                .unwrap()
                .loading_children_badge,
            "loading"
        );

        browser.apply_listing(
            ROOT_PATH,
            Ok(SdDirectoryListing {
                path: ROOT_PATH.to_string(),
                entries: vec![
                    dir("_Arcade", "/_Arcade"),
                    file("MiSTer.ini", "/MiSTer.ini"),
                ],
                elapsed_ms: 7,
                round_trip_ms: 11,
            }),
        );
        assert!(browser.has_cached_directory(ROOT_PATH));
        assert!(!browser.loading());
        assert!(browser.rows().iter().any(|row| row.id == "/_Arcade"));

        assert_eq!(browser.toggle_directory(ROOT_PATH), None);
        assert_eq!(browser.toggle_directory(ROOT_PATH), None);
        assert!(browser.status().contains("cached"));
    }

    #[test]
    fn fast_listing_never_reveals_loading_state() {
        let mut browser = SdCardBrowser::new();

        assert_eq!(
            browser.toggle_directory(ROOT_PATH).as_deref(),
            Some(ROOT_PATH)
        );
        browser.apply_listing(
            ROOT_PATH,
            Ok(SdDirectoryListing {
                path: ROOT_PATH.to_string(),
                entries: vec![file("MiSTer.ini", "/MiSTer.ini")],
                elapsed_ms: 4,
                round_trip_ms: 8,
            }),
        );

        assert!(!browser.reveal_loading_after(ROOT_PATH, Duration::ZERO));
        assert!(!browser.rows().iter().any(|row| row.is_skeleton));
        assert_eq!(
            browser
                .rows()
                .iter()
                .find(|row| row.id == ROOT_PATH)
                .unwrap()
                .loading_children_badge,
            ""
        );
    }

    #[test]
    fn toggling_directory_shows_local_folder_detail() {
        let mut browser = SdCardBrowser::new();

        assert_eq!(
            browser.toggle_directory("/_Arcade").as_deref(),
            Some("/_Arcade")
        );
        let detail = browser.selected_detail();
        assert_eq!(detail.path, "/_Arcade");
        assert_eq!(detail.title, "_Arcade");
        assert_eq!(detail.kind, "directory");
        assert!(!detail.loading);
        assert_eq!(detail.overview_rows[0].value, "/_Arcade");
        assert_eq!(browser.begin_detail_fetch_current(true), None);
    }

    #[test]
    fn refresh_invalidates_only_current_folder() {
        let mut browser = SdCardBrowser::new();
        assert_eq!(
            browser.toggle_directory(ROOT_PATH).as_deref(),
            Some(ROOT_PATH)
        );
        browser.apply_listing(
            ROOT_PATH,
            Ok(SdDirectoryListing {
                path: ROOT_PATH.to_string(),
                entries: vec![dir("games", "/games")],
                elapsed_ms: 2,
                round_trip_ms: 4,
            }),
        );

        assert_eq!(
            browser.toggle_directory("/games").as_deref(),
            Some("/games")
        );
        browser.apply_listing(
            "/games",
            Ok(SdDirectoryListing {
                path: "/games".to_string(),
                entries: vec![file("test.rom", "/games/test.rom")],
                elapsed_ms: 3,
                round_trip_ms: 5,
            }),
        );

        assert_eq!(browser.refresh_current_folder().as_deref(), Some("/games"));
        assert!(browser.has_cached_directory(ROOT_PATH));
        assert!(!browser.has_cached_directory("/games"));
    }

    #[test]
    fn cached_errors_are_reused_until_refresh() {
        let mut browser = SdCardBrowser::new();
        assert_eq!(
            browser.toggle_directory(ROOT_PATH).as_deref(),
            Some(ROOT_PATH)
        );
        browser.apply_listing(ROOT_PATH, Err("permission denied".to_string()));
        assert!(browser.last_error().contains("permission denied"));

        assert_eq!(browser.toggle_directory(ROOT_PATH), None);
        assert_eq!(browser.toggle_directory(ROOT_PATH), None);
        assert!(browser.last_error().contains("permission denied"));

        assert_eq!(browser.refresh_current_folder().as_deref(), Some(ROOT_PATH));
    }

    #[test]
    fn loaded_empty_directories_do_not_show_placeholder_rows() {
        let mut browser = SdCardBrowser::new();
        assert_eq!(
            browser.toggle_directory(ROOT_PATH).as_deref(),
            Some(ROOT_PATH)
        );

        browser.apply_listing(
            ROOT_PATH,
            Ok(SdDirectoryListing {
                path: ROOT_PATH.to_string(),
                entries: vec![],
                elapsed_ms: 1,
                round_trip_ms: 2,
            }),
        );

        assert_eq!(browser.rows().len(), 1);
        assert_eq!(browser.rows()[0].id, ROOT_PATH);
    }

    #[test]
    fn toggling_hidden_entries_clears_cache_and_reloads_expanded_folder() {
        let mut browser = SdCardBrowser::new();
        assert_eq!(
            browser.toggle_directory(ROOT_PATH).as_deref(),
            Some(ROOT_PATH)
        );
        browser.apply_listing(
            ROOT_PATH,
            Ok(SdDirectoryListing {
                path: ROOT_PATH.to_string(),
                entries: vec![dir("games", "/games")],
                elapsed_ms: 1,
                round_trip_ms: 2,
            }),
        );
        assert!(browser.has_cached_directory(ROOT_PATH));

        assert_eq!(browser.set_show_hidden(true).as_deref(), Some(ROOT_PATH));
        assert!(browser.show_hidden());
        assert!(!browser.has_cached_directory(ROOT_PATH));
        assert!(browser.loading());

        browser.apply_listing_if_current_policy(
            ROOT_PATH,
            false,
            Ok(SdDirectoryListing {
                path: ROOT_PATH.to_string(),
                entries: vec![dir("stale", "/stale")],
                elapsed_ms: 1,
                round_trip_ms: 2,
            }),
        );
        assert!(browser.loading());

        browser.apply_listing_if_current_policy(
            ROOT_PATH,
            true,
            Ok(SdDirectoryListing {
                path: ROOT_PATH.to_string(),
                entries: vec![dir(".hidden", "/.hidden")],
                elapsed_ms: 1,
                round_trip_ms: 2,
            }),
        );
        assert!(browser.has_cached_directory(ROOT_PATH));
    }

    #[test]
    fn toggling_hidden_to_current_value_is_a_noop_fetch() {
        let mut browser = SdCardBrowser::new();

        assert_eq!(browser.set_show_hidden(false), None);
        assert!(!browser.show_hidden());
        assert_eq!(browser.rows().len(), 1);
    }

    #[test]
    fn selecting_file_updates_current_path_without_fetching() {
        let mut browser = SdCardBrowser::new();
        assert_eq!(
            browser.toggle_directory(ROOT_PATH).as_deref(),
            Some(ROOT_PATH)
        );
        browser.apply_listing(
            ROOT_PATH,
            Ok(SdDirectoryListing {
                path: ROOT_PATH.to_string(),
                entries: vec![file("MiSTer.ini", "/MiSTer.ini")],
                elapsed_ms: 1,
                round_trip_ms: 2,
            }),
        );

        browser.select_path("/MiSTer.ini");

        assert_eq!(browser.current_path(), "/MiSTer.ini");
        assert_eq!(browser.status(), "Selected /MiSTer.ini");
        assert!(!browser.loading());
        assert!(
            browser
                .rows()
                .iter()
                .any(|row| row.id == "/MiSTer.ini" && row.current)
        );
    }

    #[test]
    fn refreshing_selected_file_fetches_parent_folder() {
        let mut browser = SdCardBrowser::new();
        browser.select_path("/games/NES/game.nes");

        assert_eq!(
            browser.refresh_current_folder().as_deref(),
            Some("/games/NES")
        );
        assert_eq!(browser.current_path(), "/games/NES");
        assert!(browser.loading());
    }

    #[test]
    fn refreshing_folder_already_loading_does_not_queue_duplicate_fetch() {
        let mut browser = SdCardBrowser::new();

        assert_eq!(browser.refresh_current_folder().as_deref(), Some(ROOT_PATH));
        assert_eq!(browser.refresh_current_folder(), None);
        assert!(browser.loading());
    }

    #[test]
    fn ui_paths_normalize_duplicate_slashes_and_dots() {
        assert_eq!(normalize_ui_path(""), ROOT_PATH);
        assert_eq!(normalize_ui_path("///games//NES/./"), "/games/NES");
        assert_eq!(
            normalize_ui_path("/games/NES/../../MiSTer.ini"),
            "/MiSTer.ini"
        );
        assert_eq!(normalize_ui_path("/../../etc/passwd"), "/etc/passwd");
    }

    #[test]
    fn material_icon_keys_cover_sd_card_file_types() {
        assert_eq!(material_icon_key_for_file_name("LICENSE"), "license");
        assert_eq!(material_icon_key_for_file_name("Cargo.lock"), "lock");
        assert_eq!(material_icon_key_for_file_name("MiSTer.ini"), "settings");
        assert_eq!(material_icon_key_for_file_name("menu.rbf"), "flash");
        assert_eq!(material_icon_key_for_file_name("game.mra"), "console");
        assert_eq!(material_icon_key_for_file_name("disc.cue"), "cue");
        assert_eq!(material_icon_key_for_file_name("archive.7z"), "zip");
        assert_eq!(material_icon_key_for_file_name("save.srm"), "database");
        assert_eq!(material_icon_key_for_file_name("README.md"), "readme");
        assert_eq!(material_icon_key_for_file_name("image.webp"), "image");
        assert_eq!(material_icon_key_for_file_name("song.flac"), "audio");
        assert_eq!(material_icon_key_for_file_name("movie.mkv"), "video");
        assert_eq!(
            material_icon_key_for_file_name("notes.markdown"),
            "markdown"
        );
        assert_eq!(material_icon_key_for_file_name("config.yaml"), "yaml");
        assert_eq!(material_icon_key_for_file_name("script.ps1"), "powershell");
        assert_eq!(material_icon_key_for_file_name("source.cpp"), "cpp");
        assert_eq!(material_icon_key_for_file_name("module.py"), "python");
        assert_eq!(material_icon_key_for_file_name("font.woff2"), "font");
        assert_eq!(material_icon_key_for_file_name("unknown.xyz"), "document");
    }

    #[test]
    fn parent_paths_normalize_to_existing_ui_roots() {
        assert_eq!(parent_path(ROOT_PATH), ROOT_PATH);
        assert_eq!(parent_path("/MiSTer.ini"), ROOT_PATH);
        assert_eq!(parent_path("//games/NES/./game.nes"), "/games/NES");
    }

    #[test]
    fn detail_results_ignore_stale_generation_and_wrong_path() {
        let mut browser = SdCardBrowser::new();
        browser.select_path("/ReadMe.txt");
        let first = browser.begin_detail_fetch_current(false).unwrap();
        browser.select_path("/MiSTer.ini");
        let second = browser.begin_detail_fetch_current(false).unwrap();

        let mut stale = SdItemDetail::empty();
        stale.path = first.path.clone();
        stale.title = "Stale".to_string();
        browser.apply_detail_result(&first.path, first.generation, Ok(stale));
        assert_eq!(browser.selected_detail().path, "/MiSTer.ini");
        assert_ne!(browser.selected_detail().title, "Stale");

        let mut current = SdItemDetail::empty();
        current.path = second.path.clone();
        current.title = "MiSTer.ini".to_string();
        current.kind = "file".to_string();
        browser.apply_detail_result(&second.path, second.generation, Ok(current));
        assert_eq!(browser.selected_detail().title, "MiSTer.ini");

        assert_eq!(browser.begin_detail_fetch_current(false), None);
        assert!(browser.begin_detail_fetch_current(true).is_some());
    }

    #[test]
    fn detail_errors_keep_file_fallback_shape() {
        let detail = SdItemDetail::error_for("/ReadMe.txt", "unknown cmd".to_string());

        assert_eq!(detail.kind, "file");
        assert_eq!(detail.icon_key, "readme");
        assert!(detail.error.contains("unknown cmd"));
    }
}
