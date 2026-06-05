//! Arcade catalog: recursive `.mra` scan + optional `gamelist.xml` metadata.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub const DEFAULT_ARCADE_ROOT: &str = "/media/fat/_Arcade";

/// Logical row height for arcade ListView (matches `arcade_list.slint`).
pub const ARCADE_ROW_HEIGHT: i32 = 48;
/// Visible list height: 540 − 72 layout chrome (matches `arcade_list.slint` left pane).
pub const ARCADE_LIST_VISIBLE_H: i32 = 468;

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
        print_phase("resolve_images", &self.resolve_images);
        print_phase("sort_catalog", &self.sort_catalog);
        print_phase("decode_sample_pngs", &self.decode_sample_pngs);
        println!("total                 {:5}", self.total_ms);
    }
}

fn print_phase(name: &str, p: &PhaseTiming) {
    println!(
        "{name:<22}{:5}   {:5}   {}",
        p.ms, p.count, p.notes
    );
}

#[derive(Clone, Debug)]
pub struct ArcadeGameEntry {
    pub title: String,
    pub mra_path: String,
    pub image_path: String,
    pub has_image: bool,
}

#[derive(Clone, Debug)]
pub struct ArcadeCatalog {
    pub root: PathBuf,
    pub games: Vec<ArcadeGameEntry>,
}

impl ArcadeCatalog {
    pub fn len(&self) -> usize {
        self.games.len()
    }

    pub fn title_for_path(&self, mra_path: &str) -> &str {
        self.games
            .iter()
            .find(|g| g.mra_path == mra_path)
            .map(|g| g.title.as_str())
            .unwrap_or("Game")
    }

    pub fn path_at(&self, index: usize) -> Option<&str> {
        self.games.get(index).map(|g| g.mra_path.as_str())
    }
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

pub struct BuildOptions {
    pub sample_image_decodes: usize,
}

impl Default for BuildOptions {
    fn default() -> Self {
        Self {
            sample_image_decodes: 0,
        }
    }
}

pub fn build(root: impl AsRef<Path>) -> (ArcadeCatalog, CatalogTimings) {
    build_with_options(root, BuildOptions::default(), None)
}

pub fn build_with_options(
    root: impl AsRef<Path>,
    opts: BuildOptions,
    mut progress: Option<&mut dyn FnMut(&str, &str)>,
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
        let raw = walk_mra(&root);
        let raw_count = raw.len();
        let paths = dedupe_mra_paths(raw);
        timings.walk_mra = PhaseTiming {
            ms: t.elapsed().as_millis() as u64,
            count: paths.len() as u64,
            notes: format!("{raw_count} raw → {} unique", paths.len()),
        };
        report(
            "Indexing arcade…",
            &format!("Found {} games ({raw_count} files on disk)", paths.len()),
        );
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
        let n = resolve_setname_images(&root, &mut games, |done, total| {
            if done == total || done % 400 == 0 {
                report(
                    "Indexing arcade…",
                    &format!("Reading setnames {done}/{total}…"),
                );
            }
        });
        eprintln!("catalog: setname_resolved={n}");
        n
    };

    report("Indexing arcade…", "Checking screenshots…");
    {
        let t = Instant::now();
        let (found, missing) = resolve_images(&mut games);
        timings.resolve_images = PhaseTiming {
            ms: t.elapsed().as_millis() as u64,
            count: (found + missing) as u64,
            notes: format!("png_found={found} png_missing={missing} setname_resolved={setname_resolved}"),
        };
    }

    report("Indexing arcade…", "Removing duplicates…");
    {
        let before = games.len();
        games = dedupe_by_title(games);
        if games.len() != before {
            eprintln!(
                "catalog: deduped {before} → {} entries by display title",
                games.len()
            );
        }
    }

    report("Indexing arcade…", "Sorting…");
    {
        let t = Instant::now();
        games.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
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
            notes: format!(
                "avg={}us max={}us",
                stats.avg_us, stats.max_us
            ),
        };
    }

    report("Indexing arcade…", &format!("Ready — {} games", games.len()));
    timings.total_ms = t0.elapsed().as_millis() as u64;

    (
        ArcadeCatalog {
            root,
            games,
        },
        timings,
    )
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

/// Same `.mra` filename often appears many times under `_Organized/` mirror folders.
/// Keep one path per basename — prefer root-level copies over organized mirrors.
fn dedupe_mra_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut best: HashMap<String, PathBuf> = HashMap::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let key = name.to_ascii_lowercase();
        match best.get(&key) {
            None => {
                best.insert(key, path);
            }
            Some(existing) => {
                let pick = prefer_mra_path(existing, &path);
                best.insert(key, pick);
            }
        }
    }
    let mut out: Vec<PathBuf> = best.into_values().collect();
    out.sort();
    out
}

fn prefer_mra_path(a: &Path, b: &Path) -> PathBuf {
    let a_org = is_organized_mirror(a);
    let b_org = is_organized_mirror(b);
    if a_org != b_org {
        return if a_org { b.to_path_buf() } else { a.to_path_buf() };
    }
    let a_depth = path_depth(a);
    let b_depth = path_depth(b);
    if a_depth != b_depth {
        return if a_depth < b_depth {
            a.to_path_buf()
        } else {
            b.to_path_buf()
        };
    }
    if a <= b {
        a.to_path_buf()
    } else {
        b.to_path_buf()
    }
}

fn is_organized_mirror(path: &Path) -> bool {
    path.to_string_lossy()
        .split('/')
        .any(|c| c == "_Organized")
}

fn path_depth(path: &Path) -> usize {
    path.components().count()
}

/// Collapse entries that share the same display title (gamelist mirrors / variants).
fn dedupe_by_title(games: Vec<ArcadeGameEntry>) -> Vec<ArcadeGameEntry> {
    let mut best_idx: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<ArcadeGameEntry> = Vec::with_capacity(games.len());

    for game in games {
        let key = game.title.to_ascii_lowercase();
        if let Some(&idx) = best_idx.get(&key) {
            if prefer_game_entry(&game, &out[idx]) {
                out[idx] = game;
            }
        } else {
            best_idx.insert(key, out.len());
            out.push(game);
        }
    }
    out
}

fn prefer_game_entry(a: &ArcadeGameEntry, b: &ArcadeGameEntry) -> bool {
    if a.has_image != b.has_image {
        return a.has_image;
    }
    let a_path = Path::new(&a.mra_path);
    let b_path = Path::new(&b.mra_path);
    let a_org = is_organized_mirror(a_path);
    let b_org = is_organized_mirror(b_path);
    if a_org != b_org {
        return !a_org;
    }
    let a_depth = path_depth(a_path);
    let b_depth = path_depth(b_path);
    if a_depth != b_depth {
        return a_depth < b_depth;
    }
    a.mra_path < b.mra_path
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
                    text = e.unescape().unwrap_or_default().into_owned();
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
                                        by_basename.insert(
                                            key,
                                            IndexedGamelistEntry { rel, entry },
                                        );
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

fn lookup_gamelist<'a>(index: &'a GamelistIndex, rel: &Path, basename: &str) -> Option<&'a GamelistEntry> {
    if let Some(entry) = index.by_rel.get(rel) {
        return Some(entry);
    }
    index
        .by_basename
        .get(&basename.to_ascii_lowercase())
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
        let basename = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
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
            title,
            mra_path: path.display().to_string(),
            image_path,
            has_image: false,
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
            if let Some(setname) = read_mra_setname(Path::new(&game.mra_path)) {
                if let Some(path) = resolve_setname_media(root, &setname) {
                    game.image_path = path.display().to_string();
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
        if Path::new(&g.image_path).is_file() {
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
        if decode_png_rgba8(&g.image_path).is_some() {
            let us = t.elapsed().as_micros() as u64;
            decoded += 1;
            total_us += us;
            max_us = max_us.max(us);
        }
    }

    DecodeStats {
        decoded,
        avg_us: if decoded > 0 { total_us / decoded as u64 } else { 0 },
        max_us,
    }
}

pub fn decode_png_rgba8(path: &str) -> Option<(u32, u32, Vec<u8>)> {
    load_png_rgba8_timed(path).ok().map(|loaded| {
        (
            loaded.image.width,
            loaded.image.height,
            loaded.image.rgba,
        )
    })
}

#[derive(Clone, Debug)]
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct ImageLoadTiming {
    pub read_us: u64,
    pub decode_us: u64,
    pub total_us: u64,
    pub encoded_bytes: usize,
    pub rgba_bytes: usize,
}

#[derive(Clone, Debug)]
pub struct LoadedImage {
    pub image: DecodedImage,
    pub timing: ImageLoadTiming,
}

pub fn load_png_rgba8_timed(path: &str) -> Result<LoadedImage, String> {
    let total_t = Instant::now();
    let read_t = Instant::now();
    let data = std::fs::read(path).map_err(|e| format!("read {path}: {e}"))?;
    let read_us = read_t.elapsed().as_micros() as u64;

    let decode_t = Instant::now();
    let image = decode_png_rgba8_bytes(&data)?;
    let decode_us = decode_t.elapsed().as_micros() as u64;
    let total_us = total_t.elapsed().as_micros() as u64;

    Ok(LoadedImage {
        timing: ImageLoadTiming {
            read_us,
            decode_us,
            total_us,
            encoded_bytes: data.len(),
            rgba_bytes: image.rgba.len(),
        },
        image,
    })
}

fn decode_png_rgba8_bytes(data: &[u8]) -> Result<DecodedImage, String> {
    use png::{ColorType, Transformations};
    let reader = std::io::Cursor::new(data);
    let mut decoder = png::Decoder::new(reader);
    decoder.set_transformations(Transformations::EXPAND | Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|e| format!("png read_info: {e}"))?;
    let width = reader.info().width;
    let height = reader.info().height;
    let out_size = reader
        .output_buffer_size()
        .ok_or_else(|| "png output buffer too large".to_string())?;
    let mut buf = vec![0u8; out_size];
    let out = reader.next_frame(&mut buf).map_err(|e| format!("png next_frame: {e}"))?;
    let used = out.buffer_size();
    buf.truncate(used);
    let rgba = match out.color_type {
        ColorType::Rgba => buf,
        ColorType::Rgb => {
            let mut out = Vec::with_capacity(buf.len() / 3 * 4);
            for px in buf.chunks_exact(3) {
                out.extend_from_slice(px);
                out.push(0xff);
            }
            out
        }
        other => return Err(format!("unsupported png color type: {other:?}")),
    };
    Ok(DecodedImage { width, height, rgba })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn es_relative_strips_dot_slash() {
        let p = es_relative_path("./foo/bar.mra");
        assert_eq!(p, PathBuf::from("foo/bar.mra"));
    }

    #[test]
    fn dedupe_prefers_root_over_organized() {
        let root = PathBuf::from("/media/fat/_Arcade/Donkey Kong (US, Set 1).mra");
        let organized = PathBuf::from(
            "/media/fat/_Arcade/_Organized/_2 Region/_USA/Donkey Kong (US, Set 1).mra",
        );
        let paths = dedupe_mra_paths(vec![organized.clone(), root.clone()]);
        assert_eq!(paths, vec![root]);
    }

    #[test]
    fn dedupe_by_title_keeps_one() {
        let games = vec![
            ArcadeGameEntry {
                title: "1943- Kai Midway Kaisen (JP)".into(),
                mra_path: "/media/fat/_Arcade/_Organized/a/1943- Kai Midway Kaisen (JP).mra"
                    .into(),
                image_path: String::new(),
                has_image: false,
            },
            ArcadeGameEntry {
                title: "1943- Kai Midway Kaisen (JP)".into(),
                mra_path: "/media/fat/_Arcade/1943- Kai Midway Kaisen (JP).mra".into(),
                image_path: String::new(),
                has_image: true,
            },
        ];
        let out = dedupe_by_title(games);
        assert_eq!(out.len(), 1);
        assert!(out[0].has_image);
    }
}
