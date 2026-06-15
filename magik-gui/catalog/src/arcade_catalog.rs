//! Arcade catalog helpers: recursive `.mra` scan + optional `gamelist.xml` metadata.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

pub const DEFAULT_ARCADE_ROOT: &str = "/media/fat/_Arcade";

/// Logical row height for the Rust-painted arcade list viewport.
pub const ARCADE_ROW_HEIGHT: i32 = 48;
/// Visible list height: 8 exact arcade rows (matches `arcade_list.slint` left pane).
pub const ARCADE_LIST_VISIBLE_H: i32 = 384;
pub const HOME_TILE_WIDTH: i32 = 220;
pub const HOME_TILE_GAP: i32 = 16;
pub const HOME_LIST_VISIBLE_W: i32 = 912;

#[derive(Clone, Debug, Default)]
pub struct PhaseTiming {
    pub ms: u64,
    pub count: u64,
    pub notes: String,
}

#[derive(Clone, Debug, Default)]
pub struct CatalogTimings {
    pub walk_mra: PhaseTiming,
    pub parse_gamelist: PhaseTiming,
    pub merge_entries: PhaseTiming,
    pub resolve_setname_images: PhaseTiming,
    pub resolve_images: PhaseTiming,
    pub sort_catalog: PhaseTiming,
    pub decode_sample_pngs: PhaseTiming,
    pub total_ms: u64,
}

impl CatalogTimings {
    pub fn print_summary(&self) {
        println!("catalog phase          ms    count   notes");
        print_phase("walk_mra", &self.walk_mra);
        print_phase("parse_gamelist", &self.parse_gamelist);
        print_phase("merge_entries", &self.merge_entries);
        print_phase("resolve_setname_images", &self.resolve_setname_images);
        print_phase("resolve_images", &self.resolve_images);
        print_phase("sort_catalog", &self.sort_catalog);
        print_phase("decode_sample_pngs", &self.decode_sample_pngs);
        println!("total                 {:5}", self.total_ms);
    }
}

fn print_phase(name: &str, p: &PhaseTiming) {
    println!("{name:<22}{:5}   {:5}   {}", p.ms, p.count, p.notes);
}

#[derive(Clone, Debug)]
pub struct ArcadeGameEntry {
    pub title: Arc<str>,
    pub mra_path: Arc<str>,
    pub image_path: Arc<str>,
    pub has_image: bool,
    pub system_id: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSystemEntry {
    pub id: String,
    pub title: String,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct ArcadeCatalog {
    pub root: PathBuf,
    pub games: Vec<ArcadeGameEntry>,
    pub systems: Vec<GameSystemEntry>,
    games_by_system: HashMap<String, Vec<ArcadeGameEntry>>,
    preview_games_by_system: HashMap<String, Vec<ArcadeGameEntry>>,
}

impl ArcadeCatalog {
    pub fn new(root: PathBuf, games: Vec<ArcadeGameEntry>, systems: Vec<GameSystemEntry>) -> Self {
        let games_by_system = games_by_system(&games);
        let preview_games_by_system = preview_games_by_system(&games);
        Self {
            root,
            games,
            systems,
            games_by_system,
            preview_games_by_system,
        }
    }

    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn is_empty(&self) -> bool {
        self.games.is_empty()
    }

    pub fn title_for_path(&self, mra_path: &str) -> &str {
        self.games
            .iter()
            .find(|g| g.mra_path.as_ref() == mra_path)
            .map(|g| g.title.as_ref())
            .unwrap_or("Game")
    }

    pub fn system_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.system_game_slice(system_id).to_vec()
    }

    pub fn system_game_count(&self, system_id: &str) -> usize {
        self.system_game_slice(system_id).len()
    }

    pub fn system_game_at(&self, system_id: &str, index: usize) -> Option<&ArcadeGameEntry> {
        self.system_game_slice(system_id).get(index)
    }

    pub fn system_game_slice(&self, system_id: &str) -> &[ArcadeGameEntry] {
        self.games_by_system
            .get(system_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    pub fn system_preview_games(&self, system_id: &str) -> Vec<ArcadeGameEntry> {
        self.system_preview_game_slice(system_id).to_vec()
    }

    pub fn system_preview_game_count(&self, system_id: &str) -> usize {
        self.system_preview_game_slice(system_id).len()
    }

    pub fn system_preview_game_at(&self, system_id: &str, index: usize) -> Option<ArcadeGameEntry> {
        self.system_preview_game_slice(system_id)
            .get(index)
            .cloned()
    }

    pub fn system_preview_game_slice(&self, system_id: &str) -> &[ArcadeGameEntry] {
        self.preview_games_by_system
            .get(system_id)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

fn games_by_system(games: &[ArcadeGameEntry]) -> HashMap<String, Vec<ArcadeGameEntry>> {
    let mut by_system: HashMap<String, Vec<ArcadeGameEntry>> = HashMap::new();
    for game in games {
        by_system
            .entry(game.system_id.to_string())
            .or_default()
            .push(game.clone());
    }
    by_system
}

fn preview_games_by_system(games: &[ArcadeGameEntry]) -> HashMap<String, Vec<ArcadeGameEntry>> {
    let mut by_system: HashMap<String, Vec<&ArcadeGameEntry>> = HashMap::new();
    for game in games {
        by_system
            .entry(game.system_id.to_string())
            .or_default()
            .push(game);
    }
    by_system
        .into_iter()
        .map(|(system_id, games)| (system_id, preview_games(games.into_iter())))
        .collect()
}

fn preview_games<'a>(games: impl Iterator<Item = &'a ArcadeGameEntry>) -> Vec<ArcadeGameEntry> {
    let mut best_idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<ArcadeGameEntry> = Vec::new();

    for game in games {
        if !has_preview_image(game) {
            continue;
        }
        let key = preview_dedupe_key(&game.title);
        if let Some(&idx) = best_idx.get(&key) {
            if prefer_preview_game(game, &out[idx]) {
                out[idx] = game.clone();
            }
        } else {
            best_idx.insert(key, out.len());
            out.push(game.clone());
        }
    }

    out
}

fn preview_dedupe_key(title: &str) -> String {
    let base = title
        .split_once('(')
        .map(|(before, _)| before.trim())
        .filter(|before| !before.is_empty())
        .unwrap_or(title);
    base.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn prefer_preview_game(a: &ArcadeGameEntry, b: &ArcadeGameEntry) -> bool {
    let a_exact = !a.title.contains('(');
    let b_exact = !b.title.contains('(');
    if a_exact != b_exact {
        return a_exact;
    }
    if a.title.len() != b.title.len() {
        return a.title.len() < b.title.len();
    }
    a.mra_path < b.mra_path
}

fn has_preview_image(game: &ArcadeGameEntry) -> bool {
    game.has_image
        && Path::new(game.image_path.as_ref())
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| {
                ext.eq_ignore_ascii_case("png")
                    || ext.eq_ignore_ascii_case("jpg")
                    || ext.eq_ignore_ascii_case("jpeg")
            })
}

#[derive(Clone, Debug, Default)]
struct GamelistEntry {
    name: String,
    image: String,
}

#[derive(Clone, Debug)]
struct IndexedGamelistEntry {
    rel: PathBuf,
    entry: GamelistEntry,
}

struct GamelistIndex {
    by_rel: HashMap<PathBuf, GamelistEntry>,
    by_basename: HashMap<String, IndexedGamelistEntry>,
}

#[derive(Default)]
pub struct BuildOptions {
    pub sample_image_decodes: usize,
}

type ProgressCallback<'a> = Option<&'a mut dyn FnMut(&str, &str)>;

pub fn build(root: impl AsRef<Path>) -> (ArcadeCatalog, CatalogTimings) {
    build_with_options(root, BuildOptions::default(), None)
}

pub fn build_with_options(
    root: impl AsRef<Path>,
    opts: BuildOptions,
    mut progress: ProgressCallback<'_>,
) -> (ArcadeCatalog, CatalogTimings) {
    let mut report = |title: &str, detail: &str| {
        if let Some(f) = progress.as_mut() {
            f(title, detail);
        }
    };

    let root = root.as_ref().to_path_buf();
    let t0 = Instant::now();
    let mut timings = CatalogTimings::default();

    report("Indexing arcade…", "Scanning .mra files…");
    let mra_paths = {
        let t = Instant::now();
        let paths = walk_mra(&root);
        timings.walk_mra = PhaseTiming {
            ms: t.elapsed().as_millis() as u64,
            count: paths.len() as u64,
            notes: String::new(),
        };
        report("Indexing arcade…", &format!("Found {} games", paths.len()));
        paths
    };

    report("Indexing arcade…", "Reading gamelist.xml…");
    let gamelist = {
        let t = Instant::now();
        let index = parse_gamelist(&root);
        timings.parse_gamelist = PhaseTiming {
            ms: t.elapsed().as_millis() as u64,
            count: index.by_rel.len() as u64,
            notes: format!("basename_index={}", index.by_basename.len()),
        };
        index
    };

    let mut games = {
        let t = Instant::now();
        let (rows, matched) = merge_entries(&root, &mra_paths, &gamelist, |done, total| {
            if done == total || done % 400 == 0 {
                report(
                    "Indexing arcade…",
                    &format!("Matching metadata {done}/{total}…"),
                );
            }
        });
        timings.merge_entries = PhaseTiming {
            ms: t.elapsed().as_millis() as u64,
            count: rows.len() as u64,
            notes: format!(
                "matched={matched} unmatched={}",
                rows.len().saturating_sub(matched)
            ),
        };
        rows
    };

    report("Indexing arcade…", "Resolving screenshots…");
    let setname_resolved = {
        let t = Instant::now();
        let n = resolve_setname_images(&root, &mut games, |done, total| {
            if done == total || done % 400 == 0 {
                report(
                    "Indexing arcade…",
                    &format!("Reading setnames {done}/{total}…"),
                );
            }
        });
        eprintln!("catalog: setname_resolved={n}");
        timings.resolve_setname_images = PhaseTiming {
            ms: t.elapsed().as_millis() as u64,
            count: games.len() as u64,
            notes: format!("setname_resolved={n}"),
        };
        n
    };

    report("Indexing arcade…", "Checking screenshots…");
    {
        let t = Instant::now();
        let (found, missing) = resolve_images(&mut games);
        timings.resolve_images = PhaseTiming {
            ms: t.elapsed().as_millis() as u64,
            count: (found + missing) as u64,
            notes: format!(
                "png_found={found} png_missing={missing} setname_resolved={setname_resolved}"
            ),
        };
    }

    report("Indexing arcade…", "Sorting…");
    {
        let t = Instant::now();
        games.sort_by_cached_key(|game| game.title.to_lowercase());
        timings.sort_catalog = PhaseTiming {
            ms: t.elapsed().as_millis() as u64,
            count: games.len() as u64,
            notes: String::new(),
        };
    }

    if opts.sample_image_decodes > 0 {
        let t = Instant::now();
        let stats = bench_decode_sample_pngs(&games, opts.sample_image_decodes);
        timings.decode_sample_pngs = PhaseTiming {
            ms: t.elapsed().as_millis() as u64,
            count: stats.decoded as u64,
            notes: format!("avg={}us max={}us", stats.avg_us, stats.max_us),
        };
    }

    report(
        "Indexing arcade…",
        &format!("Ready — {} games", games.len()),
    );
    timings.total_ms = t0.elapsed().as_millis() as u64;

    let systems = systems_from_games(&games);
    (ArcadeCatalog::new(root, games, systems), timings)
}

pub fn systems_from_games(games: &[ArcadeGameEntry]) -> Vec<GameSystemEntry> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for game in games {
        *counts.entry(game.system_id.to_string()).or_default() += 1;
    }
    let mut systems: Vec<GameSystemEntry> = counts
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(id, count)| GameSystemEntry {
            title: system_title(&id),
            id,
            count,
        })
        .collect();
    systems.sort_by_cached_key(system_sort_key);
    systems
}

fn system_sort_key(system: &GameSystemEntry) -> String {
    let rank = match system.id.as_str() {
        "arcade" => 0,
        "amiga" => 1,
        "neogeo" => 2,
        "nes" => 3,
        "snes" => 4,
        "saturn" => 5,
        "megadrive" => 6,
        "gba" => 7,
        "gbc" => 8,
        "n64" => 9,
        "gamegear" => 10,
        "vectrex" => 11,
        "ao486" => 12,
        "unknown" => 999,
        _ => 100,
    };
    format!("{rank:03}-{}", system.title.to_lowercase())
}

pub fn system_title(id: &str) -> String {
    match id {
        "arcade" => "Arcade".to_string(),
        "neogeo" | "neo-geo" | "snk-neo-geo" => "NeoGeo".to_string(),
        "cps1" | "capcom-cps1" => "CPS1".to_string(),
        "cps2" | "capcom-cps2" => "CPS2".to_string(),
        "cps3" | "capcom-cps3" => "CPS3".to_string(),
        "system16" | "sega-system16" => "System 16".to_string(),
        "system18" | "sega-system18" => "System 18".to_string(),
        "m72" | "irem-m72" => "Irem M72".to_string(),
        "m92" | "irem-m92" => "Irem M92".to_string(),
        "gba" => "GBA".to_string(),
        "gbc" => "GBC".to_string(),
        "gb" => "GB".to_string(),
        "nes" => "NES".to_string(),
        "snes" => "SNES".to_string(),
        "n64" => "N64".to_string(),
        "sms" => "SMS".to_string(),
        "psx" => "PSX".to_string(),
        "ao486" => "ao486".to_string(),
        "megadrive" => "Mega Drive".to_string(),
        "megacd" => "Mega CD".to_string(),
        "gamegear" => "Game Gear".to_string(),
        "unknown" => "Unknown".to_string(),
        other => other
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                match chars.next() {
                    Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    None => String::new(),
                }
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

fn walk_mra(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if is_organized_mirror(path) {
            continue;
        }
        if path.extension().and_then(|e| e.to_str()) != Some("mra") {
            continue;
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("._") {
            continue;
        }
        out.push(path.to_path_buf());
    }
    out.sort();
    out
}

fn is_organized_mirror(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case("_organized")
    })
}

fn parse_gamelist(root: &Path) -> GamelistIndex {
    let path = root.join("gamelist.xml");
    let Ok(data) = std::fs::read_to_string(&path) else {
        return GamelistIndex {
            by_rel: HashMap::new(),
            by_basename: HashMap::new(),
        };
    };

    let mut by_rel = HashMap::new();
    let mut by_basename = HashMap::new();
    let mut in_game = false;
    let mut cur_path = String::new();
    let mut cur_name = String::new();
    let mut cur_image = String::new();
    let mut field = String::new();
    let mut text = String::new();

    let mut reader = quick_xml::Reader::from_str(&data);
    reader.config_mut().trim_text(true);

    loop {
        match reader.read_event() {
            Ok(quick_xml::events::Event::Start(e)) => {
                field.clear();
                text.clear();
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if tag == "game" {
                    in_game = true;
                    cur_path.clear();
                    cur_name.clear();
                    cur_image.clear();
                } else if in_game {
                    field = tag;
                }
            }
            Ok(quick_xml::events::Event::Text(e)) => {
                if in_game && !field.is_empty() {
                    text = e.xml10_content().unwrap_or_default().into_owned();
                }
            }
            Ok(quick_xml::events::Event::End(e)) => {
                let tag = String::from_utf8_lossy(e.name().as_ref()).into_owned();
                if tag == "game" && in_game {
                    if !cur_path.is_empty() {
                        let rel = es_relative_path(&cur_path);
                        let entry = GamelistEntry {
                            name: cur_name.clone(),
                            image: cur_image.clone(),
                        };
                        by_rel.insert(rel.clone(), entry.clone());
                        if let Some(base) = rel.file_name().and_then(|n| n.to_str()) {
                            let key = base.to_ascii_lowercase();
                            match by_basename.get(&key) {
                                None => {
                                    by_basename.insert(
                                        key,
                                        IndexedGamelistEntry {
                                            rel: rel.clone(),
                                            entry,
                                        },
                                    );
                                }
                                Some(existing) => {
                                    if prefer_gamelist_entry(
                                        &existing.entry,
                                        &existing.rel,
                                        &entry,
                                        &rel,
                                    ) {
                                        by_basename
                                            .insert(key, IndexedGamelistEntry { rel, entry });
                                    }
                                }
                            }
                        }
                    }
                    in_game = false;
                } else if in_game && !field.is_empty() && !text.is_empty() {
                    match field.as_str() {
                        "path" => cur_path = text.clone(),
                        "name" => cur_name = text.clone(),
                        "image" => cur_image = text.clone(),
                        _ => {}
                    }
                    field.clear();
                    text.clear();
                }
            }
            Ok(quick_xml::events::Event::Eof) => break,
            Err(e) => {
                eprintln!("gamelist.xml parse error: {e}");
                break;
            }
            _ => {}
        }
    }

    GamelistIndex {
        by_rel,
        by_basename,
    }
}

fn prefer_gamelist_entry(
    existing: &GamelistEntry,
    existing_rel: &Path,
    new: &GamelistEntry,
    new_rel: &Path,
) -> bool {
    let existing_org = is_organized_mirror(existing_rel);
    let new_org = is_organized_mirror(new_rel);
    if existing_org != new_org {
        return !new_org;
    }
    if existing.image.is_empty() != new.image.is_empty() {
        return !new.image.is_empty();
    }
    false
}

fn lookup_gamelist<'a>(
    index: &'a GamelistIndex,
    rel: &Path,
    basename: &str,
) -> Option<&'a GamelistEntry> {
    if let Some(entry) = index.by_rel.get(rel) {
        return Some(entry);
    }
    index
        .by_basename
        .get(&basename.to_ascii_lowercase())
        .filter(|indexed| !is_organized_mirror(&indexed.rel) || is_organized_mirror(rel))
        .map(|i| &i.entry)
}

fn es_relative_path(rel: &str) -> PathBuf {
    let trimmed = rel.trim();
    let rel = trimmed.strip_prefix("./").unwrap_or(trimmed);
    PathBuf::from(rel)
}

fn mra_relative_key(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| path.to_path_buf())
}

fn merge_entries(
    root: &Path,
    mra_paths: &[PathBuf],
    gamelist: &GamelistIndex,
    mut on_progress: impl FnMut(usize, usize),
) -> (Vec<ArcadeGameEntry>, usize) {
    let mut matched = 0usize;
    let mut games = Vec::with_capacity(mra_paths.len());
    let total = mra_paths.len();

    for (i, path) in mra_paths.iter().enumerate() {
        let rel = mra_relative_key(root, path);
        let basename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let meta = lookup_gamelist(gamelist, &rel, basename);
        if meta.is_some() {
            matched += 1;
        }

        let file_stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Unknown");

        let title = meta
            .map(|m| m.name.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| file_stem.to_string());

        let image_rel = meta.map(|m| m.image.as_str()).unwrap_or("");
        let image_path = if image_rel.is_empty() {
            String::new()
        } else {
            resolve_image_path(root, image_rel)
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        };

        games.push(ArcadeGameEntry {
            title: title.into(),
            mra_path: path.display().to_string().into(),
            image_path: image_path.into(),
            has_image: false,
            system_id: "arcade".into(),
        });
        on_progress(i + 1, total);
    }

    (games, matched)
}

fn resolve_image_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let trimmed = rel.trim();
    let rel = trimmed.strip_prefix("./").unwrap_or(trimmed);
    let path = root.join(rel);
    if path.is_file() {
        Some(path)
    } else {
        None
    }
}

const SETNAME_READ_BYTES: usize = 4096;
const MEDIA_IMAGE_DIRS: &[&str] = &[
    "media/screenshot",
    "media/screenshots",
    "media/boxart",
    "media/images",
];

fn read_mra_setname(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut buf = vec![0u8; SETNAME_READ_BYTES];
    let n = file.read(&mut buf).ok()?;
    if n == 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&buf[..n]);
    let open = "<setname>";
    let close = "</setname>";
    let start = text.find(open)? + open.len();
    let end = start + text[start..].find(close)?;
    let name = text[start..end].trim();
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

fn resolve_setname_media(root: &Path, setname: &str) -> Option<PathBuf> {
    for dir in MEDIA_IMAGE_DIRS {
        let path = root.join(dir).join(format!("{setname}.png"));
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

fn resolve_setname_images(
    root: &Path,
    games: &mut [ArcadeGameEntry],
    mut on_progress: impl FnMut(usize, usize),
) -> usize {
    let mut resolved = 0usize;
    let total = games.len();
    for (i, game) in games.iter_mut().enumerate() {
        if game.image_path.is_empty() {
            if let Some(setname) = read_mra_setname(Path::new(game.mra_path.as_ref())) {
                if let Some(path) = resolve_setname_media(root, &setname) {
                    game.image_path = path.display().to_string().into();
                    resolved += 1;
                }
            }
        }
        on_progress(i + 1, total);
    }
    resolved
}

fn resolve_images(games: &mut [ArcadeGameEntry]) -> (usize, usize) {
    let mut found = 0usize;
    let mut missing = 0usize;
    for g in games.iter_mut() {
        if g.image_path.is_empty() {
            continue;
        }
        if Path::new(g.image_path.as_ref()).is_file() {
            g.has_image = true;
            found += 1;
        } else {
            g.has_image = false;
            missing += 1;
        }
    }
    (found, missing)
}

struct DecodeStats {
    decoded: usize,
    avg_us: u64,
    max_us: u64,
}

fn bench_decode_sample_pngs(games: &[ArcadeGameEntry], limit: usize) -> DecodeStats {
    let mut decoded = 0usize;
    let mut total_us = 0u64;
    let mut max_us = 0u64;

    for g in games.iter().filter(|g| g.has_image).take(limit) {
        let t = Instant::now();
        if decode_png_rgb8(&g.image_path).is_some() {
            let us = t.elapsed().as_micros() as u64;
            decoded += 1;
            total_us += us;
            max_us = max_us.max(us);
        }
    }

    DecodeStats {
        decoded,
        avg_us: if decoded > 0 {
            total_us / decoded as u64
        } else {
            0
        },
        max_us,
    }
}

pub fn decode_png_rgb8(path: &str) -> Option<(u32, u32, Vec<u8>)> {
    load_png_rgb8_timed(path)
        .ok()
        .map(|loaded| (loaded.image.width, loaded.image.height, loaded.image.rgb))
}

#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ImageLoadTiming {
    pub read_us: u64,
    pub decode_us: u64,
    pub resize_us: u64,
    pub total_us: u64,
    pub encoded_bytes: usize,
    pub decoded_bytes: usize,
    pub source_width: u32,
    pub source_height: u32,
}

#[derive(Clone, Debug)]
pub struct LoadedImage {
    pub image: DecodedImage,
    pub timing: ImageLoadTiming,
}

pub fn load_png_rgb8_timed(path: &str) -> Result<LoadedImage, String> {
    let total_t = Instant::now();
    let read_t = Instant::now();
    let data = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
    let read_us = read_t.elapsed().as_micros() as u64;

    let decode_t = Instant::now();
    let image = decode_png_rgb8_bytes(&data)?;
    let decode_us = decode_t.elapsed().as_micros() as u64;
    let total_us = total_t.elapsed().as_micros() as u64;
    let source_width = image.width;
    let source_height = image.height;

    Ok(LoadedImage {
        timing: ImageLoadTiming {
            read_us,
            decode_us,
            resize_us: 0,
            total_us,
            encoded_bytes: data.len(),
            decoded_bytes: image.rgb.len(),
            source_width,
            source_height,
        },
        image,
    })
}

fn decode_png_rgb8_bytes(data: &[u8]) -> Result<DecodedImage, String> {
    use zune_png::zune_core::bytestream::ZCursor;
    use zune_png::zune_core::colorspace::ColorSpace;
    use zune_png::zune_core::options::DecoderOptions;
    use zune_png::PngDecoder;

    let options = DecoderOptions::default().png_set_strip_to_8bit(true);
    let mut decoder = PngDecoder::new_with_options(ZCursor::new(data), options);
    decoder
        .decode_headers()
        .map_err(|e| format!("zune png headers: {e}"))?;
    let (width, height) = decoder
        .dimensions()
        .ok_or_else(|| "zune png missing dimensions".to_string())?;
    let width = u32::try_from(width).map_err(|_| "zune png width too large".to_string())?;
    let height = u32::try_from(height).map_err(|_| "zune png height too large".to_string())?;
    let colorspace = decoder
        .colorspace()
        .ok_or_else(|| "zune png missing colorspace".to_string())?;
    let buf = decoder
        .decode_raw()
        .map_err(|e| format!("zune png decode: {e}"))?;
    let rgb = match colorspace {
        ColorSpace::RGB => buf,
        ColorSpace::RGBA => {
            let mut out = Vec::with_capacity(buf.len() / 4 * 3);
            for px in buf.chunks_exact(4) {
                out.extend_from_slice(&px[..3]);
            }
            out
        }
        ColorSpace::Luma => {
            let mut out = Vec::with_capacity(buf.len() * 3);
            for luma in buf {
                out.extend_from_slice(&[luma, luma, luma]);
            }
            out
        }
        ColorSpace::LumaA => {
            let mut out = Vec::with_capacity(buf.len() / 2 * 3);
            for px in buf.chunks_exact(2) {
                let luma = px[0];
                out.extend_from_slice(&[luma, luma, luma]);
            }
            out
        }
        other => return Err(format!("unsupported zune png colorspace: {other:?}")),
    };
    Ok(DecodedImage { width, height, rgb })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("mister-magik-arcade-catalog-{name}-{nanos}"))
    }

    fn write_png(path: &Path) {
        let mut data = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut data, 1, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0x11, 0x22, 0x33]).unwrap();
        }
        fs::write(path, data).expect("write png");
    }

    #[test]
    fn es_relative_strips_dot_slash() {
        let p = es_relative_path("./foo/bar.mra");
        assert_eq!(p, PathBuf::from("foo/bar.mra"));
    }

    #[test]
    fn walk_mra_ignores_organized_mirrors() {
        let root = temp_root("walk");
        fs::create_dir_all(root.join("_organized/_2 Region/_USA")).expect("create organized dir");
        fs::write(root.join("Donkey Kong (US, Set 1).mra"), "root").expect("write root mra");
        fs::write(
            root.join("_organized/_2 Region/_USA/Donkey Kong (US, Set 1).mra"),
            "organized",
        )
        .expect("write organized mra");

        let paths = walk_mra(&root);

        assert_eq!(paths, vec![root.join("Donkey Kong (US, Set 1).mra")]);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn preview_games_require_images_and_collapse_parenthetical_clones() {
        let root = PathBuf::from("/media/fat/_Arcade");
        let systems = vec![GameSystemEntry {
            id: "arcade".into(),
            title: "Arcade".into(),
            count: 5,
        }];
        let games = vec![
            ArcadeGameEntry {
                title: "1941: Counter Attack (Japan)".into(),
                mra_path: "/media/fat/_Arcade/1941 Japan.mra".into(),
                image_path: "/media/fat/_Arcade/media/screenshot/1941u.png".into(),
                has_image: true,
                system_id: "arcade".into(),
            },
            ArcadeGameEntry {
                title: "1941: Counter Attack (World)".into(),
                mra_path: "/media/fat/_Arcade/1941 World.mra".into(),
                image_path: "/media/fat/_Arcade/media/screenshot/1941u.png".into(),
                has_image: true,
                system_id: "arcade".into(),
            },
            ArcadeGameEntry {
                title: "1942".into(),
                mra_path: "/media/fat/_Arcade/1942.mra".into(),
                image_path: "".into(),
                has_image: false,
                system_id: "arcade".into(),
            },
            ArcadeGameEntry {
                title: "1943".into(),
                mra_path: "/media/fat/_Arcade/1943.mra".into(),
                image_path: "/media/fat/_Arcade/media/screenshot/1943.png".into(),
                has_image: true,
                system_id: "arcade".into(),
            },
            ArcadeGameEntry {
                title: "Astra SuperStars".into(),
                mra_path: "/media/fat/_Arcade/Astra SuperStars.mra".into(),
                image_path: "/media/fat/_Arcade/media/screenshot/astrass.jpg".into(),
                has_image: true,
                system_id: "arcade".into(),
            },
        ];
        let catalog = ArcadeCatalog::new(root, games, systems);

        let games = catalog.system_preview_games("arcade");
        assert_eq!(games.len(), 3);
        assert_eq!(catalog.system_preview_game_count("arcade"), 3);
        assert_eq!(games[0].title.as_ref(), "1941: Counter Attack (Japan)");
        assert_eq!(games[1].title.as_ref(), "1943");
        assert_eq!(games[2].title.as_ref(), "Astra SuperStars");
        assert_eq!(
            catalog
                .system_preview_game_at("arcade", 1)
                .map(|game| game.title.to_string()),
            Some("1943".to_string())
        );
    }

    #[test]
    fn system_game_count_includes_games_without_preview_images() {
        let root = PathBuf::from("/media/fat/_Arcade");
        let systems = vec![GameSystemEntry {
            id: "amiga".into(),
            title: "Amiga".into(),
            count: 1,
        }];
        let games = vec![ArcadeGameEntry {
            title: "Agony".into(),
            mra_path: "magik-plan:amiga-agony".into(),
            image_path: "".into(),
            has_image: false,
            system_id: "amiga".into(),
        }];
        let catalog = ArcadeCatalog::new(root, games, systems);

        assert_eq!(catalog.system_game_count("amiga"), 1);
        assert_eq!(catalog.system_game_slice("amiga").len(), 1);
        assert_eq!(catalog.system_preview_game_count("amiga"), 0);
    }

    #[test]
    fn decode_png_rgb8_accepts_grayscale_cache_pngs() {
        let mut data = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut data, 2, 1);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0x22, 0xcc]).unwrap();
        }

        let decoded = decode_png_rgb8_bytes(&data).unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 1);
        assert_eq!(decoded.rgb, vec![0x22, 0x22, 0x22, 0xcc, 0xcc, 0xcc]);
    }

    #[test]
    fn build_walks_merges_resolves_and_sorts_real_files() {
        let root = temp_root("build");
        fs::create_dir_all(root.join("media/screenshot")).expect("create screenshot dir");
        fs::create_dir_all(root.join("_Organized/_Region")).expect("create organized dir");
        fs::write(
            root.join("1942.mra"),
            "<misterromdescription><setname>1942</setname>",
        )
        .expect("write mra");
        fs::write(
            root.join("_Organized/_Region/1942.mra"),
            "<misterromdescription><setname>1942-clone</setname>",
        )
        .expect("write organized mra");
        fs::write(root.join("Zaxxon.mra"), "<misterromdescription/>").expect("write mra");
        fs::write(root.join("._AppleDouble.mra"), "ignored").expect("write apple double");
        write_png(&root.join("media/screenshot/1942.png"));
        write_png(&root.join("media/screenshot/zaxxon.png"));
        fs::write(
            root.join("gamelist.xml"),
            r#"<gameList>
  <game>
    <path>./Zaxxon.mra</path>
    <name>Zaxxon</name>
    <image>./media/screenshot/zaxxon.png</image>
  </game>
  <game>
    <path>./_Organized/_Region/1942.mra</path>
    <name>Organized 1942</name>
    <image>./media/screenshot/missing.png</image>
  </game>
</gameList>"#,
        )
        .expect("write gamelist");

        let (catalog, timings) = build_with_options(
            &root,
            BuildOptions {
                sample_image_decodes: 2,
            },
            None,
        );

        assert_eq!(catalog.len(), 2);
        assert_eq!(catalog.system_game_count("arcade"), 2);
        assert_eq!(catalog.system_preview_game_count("arcade"), 2);
        assert_eq!(catalog.games[0].title.as_ref(), "1942");
        assert!(catalog.games[0].mra_path.ends_with("/1942.mra"));
        assert!(!catalog.games[0].mra_path.contains("_Organized"));
        assert!(catalog.games[0]
            .image_path
            .ends_with("media/screenshot/1942.png"));
        assert!(catalog.games[0].has_image);
        assert_eq!(catalog.games[1].title.as_ref(), "Zaxxon");
        assert!(catalog.games[1].has_image);
        assert_eq!(
            catalog.systems,
            vec![GameSystemEntry {
                id: "arcade".into(),
                title: "Arcade".into(),
                count: 2,
            }]
        );
        assert_eq!(timings.walk_mra.count, 2);
        assert_eq!(timings.merge_entries.count, 2);
        assert_eq!(timings.resolve_setname_images.notes, "setname_resolved=1");
        assert_eq!(timings.resolve_images.count, 2);
        assert_eq!(timings.decode_sample_pngs.count, 2);

        let _ = fs::remove_dir_all(root);
    }
}
