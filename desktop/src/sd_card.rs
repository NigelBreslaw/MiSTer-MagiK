use std::collections::{HashMap, HashSet};

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
    pub size: u64,
    pub modified_unix_ms: u64,
    pub readonly: bool,
    pub hidden: bool,
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
    directory_cache: HashMap<String, CachedDirectory>,
    tree_rows: Vec<SdTreeRow>,
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
            directory_cache: HashMap::new(),
            tree_rows: Vec::new(),
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

    pub fn refresh_current_folder(&mut self) -> Option<String> {
        let path = self.refresh_target();
        self.directory_cache.remove(&path);
        self.expanded_paths.insert(path.clone());
        self.current_path = path.clone();
        self.last_error.clear();
        self.begin_fetch(path)
    }

    pub fn apply_listing(&mut self, raw_path: &str, result: Result<SdDirectoryListing, String>) {
        let path = normalize_ui_path(raw_path);
        self.loading_paths.remove(&path);
        match result {
            Ok(listing) => {
                let entry_count = listing.entries.len();
                let elapsed_ms = listing.elapsed_ms;
                self.directory_cache.insert(
                    path.clone(),
                    CachedDirectory::Loaded {
                        entries: listing.entries,
                        elapsed_ms,
                    },
                );
                self.last_error.clear();
                self.status = format!("Loaded {path}: {entry_count} entries in {elapsed_ms}ms");
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
        self.status = format!("Loading {path}...");
        self.rebuild_rows();
        Some(path)
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
            loading_children_badge: if self.loading_paths.contains(path) {
                "loading".to_string()
            } else {
                String::new()
            },
        });

        if !expanded {
            return;
        }

        if self.loading_paths.contains(path) {
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
        if part.is_empty() || part == "." {
            continue;
        }
        parts.push(part);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str, path: &str) -> SdEntry {
        SdEntry {
            name: name.to_string(),
            path: path.to_string(),
            kind: SdEntryKind::Directory,
            size: 0,
            modified_unix_ms: 0,
            readonly: false,
            hidden: false,
        }
    }

    fn file(name: &str, path: &str) -> SdEntry {
        SdEntry {
            name: name.to_string(),
            path: path.to_string(),
            kind: SdEntryKind::File,
            size: 123,
            modified_unix_ms: 0,
            readonly: false,
            hidden: false,
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
        assert!(browser.rows().iter().any(|row| row.is_skeleton));

        browser.apply_listing(
            ROOT_PATH,
            Ok(SdDirectoryListing {
                path: ROOT_PATH.to_string(),
                entries: vec![
                    dir("_Arcade", "/_Arcade"),
                    file("MiSTer.ini", "/MiSTer.ini"),
                ],
                elapsed_ms: 7,
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
            }),
        );

        browser.select_path("/MiSTer.ini");

        assert_eq!(browser.current_path(), "/MiSTer.ini");
        assert_eq!(browser.status(), "Selected /MiSTer.ini");
        assert!(!browser.loading());
        assert!(browser
            .rows()
            .iter()
            .any(|row| row.id == "/MiSTer.ini" && row.current));
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
}
