use super::*;
use crate::preview_worker;
use mister_magik_fb::camera_effects::{CameraImage, CameraPixel};
use mister_magik_fb::raw565;
use std::fs::File;
use std::io::Write;
use std::path::Path;

pub(super) fn parse_effects_env<T: Copy>(
    env_name: &str,
    family_label: &str,
    all: &[T],
    parse: fn(&str) -> Option<T>,
    default: T,
) -> Vec<T> {
    let spec = std::env::var(env_name).unwrap_or_else(|_| "mega".into());
    if matches!(
        spec.trim().to_ascii_lowercase().as_str(),
        "" | "mega" | "all" | "demo"
    ) {
        return all.to_vec();
    }
    let mut effects = Vec::new();
    for part in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(effect) = parse(part) {
            effects.push(effect);
        } else {
            eprintln!("{family_label}: unknown effect {part:?}");
        }
    }
    if effects.is_empty() {
        vec![default]
    } else {
        effects
    }
}

pub(super) fn selected_effect<T: Copy>(
    effects: &[T],
    auto: bool,
    segment: Duration,
    elapsed: Duration,
    selected_idx: usize,
    default: T,
) -> T {
    if effects.is_empty() {
        return default;
    }
    let idx = if auto {
        (elapsed.as_micros() / segment.as_micros().max(1)) as usize % effects.len()
    } else {
        selected_idx % effects.len()
    };
    effects.get(idx).copied().unwrap_or(default)
}

pub(super) fn segment_from_env(name: &str, default_secs: u64) -> Duration {
    Duration::from_secs(
        std::env::var(name)
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(default_secs)
            .max(1),
    )
}

pub(super) fn cache_cap_from_env(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(default)
        .clamp(1, 512)
}

pub(super) fn env_truthy(name: &str) -> bool {
    matches!(
        std::env::var(name)
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str(),
        "1" | "true" | "yes" | "on"
    )
}

pub(super) fn create_trace(
    env_name: &str,
    family_label: &str,
    header: &'static [u8],
) -> Option<File> {
    std::env::var(env_name).ok().and_then(|path| {
        let mut f = File::create(&path)
            .map_err(|e| eprintln!("{family_label} trace: create {path} failed: {e}"))
            .ok()?;
        f.write_all(header).ok()?;
        println!("{}_trace={path}", family_label.replace('-', "_"));
        Some(f)
    })
}

pub(super) fn process_cpu_us() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_PROCESS_CPUTIME_ID, &mut ts) };
    if rc == 0 {
        (ts.tv_sec as u64)
            .saturating_mul(1_000_000)
            .saturating_add((ts.tv_nsec as u64) / 1_000)
    } else {
        0
    }
}

pub(super) fn arcade_root_from_env() -> String {
    std::env::var("MISTER_ARCADE_ROOT")
        .unwrap_or_else(|_| arcade_catalog::DEFAULT_ARCADE_ROOT.to_string())
}

pub(super) fn load_effect_images(
    root: &str,
    cap: usize,
    synthetic_min: usize,
    synthetic_max: usize,
    synthetic: fn(usize) -> Vec<CameraImage>,
) -> Vec<CameraImage> {
    let resize = preview_worker::PreviewResizeSpec::from_env();
    let mut paths = Vec::new();
    if let Ok(loaded) = library_db::load_arcade_catalog_from_sqlite(root) {
        paths.extend(
            loaded
                .catalog
                .games
                .iter()
                .filter(|game| game.has_image && !game.image_path.is_empty())
                .map(|game| game.image_path.clone())
                .take(cap * 4),
        );
    }
    let mut images = Vec::new();
    for path in paths {
        if images.len() >= cap {
            break;
        }
        let cache = preview_worker::raw565_preview_cache_path(&path, resize);
        if let Some(image) = read_raw565_image(&cache) {
            images.push(image);
        }
    }
    if images.is_empty() {
        synthetic(cap.min(synthetic_max).max(synthetic_min))
    } else {
        images
    }
}

fn read_raw565_image(path: &Path) -> Option<CameraImage> {
    let data = std::fs::read(path).ok()?;
    let image = raw565::decode_raw565(&data).ok()?;
    let pixels = image.words.into_iter().map(CameraPixel).collect();
    Some(CameraImage {
        pixels,
        w: image.width,
        h: image.height,
        stride: image.stride_words,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum FakeEffect {
        First,
        Second,
    }

    fn parse_fake(label: &str) -> Option<FakeEffect> {
        match label {
            "first" => Some(FakeEffect::First),
            "second" => Some(FakeEffect::Second),
            _ => None,
        }
    }

    fn fake_synthetic(count: usize) -> Vec<CameraImage> {
        (0..count)
            .map(|idx| CameraImage {
                pixels: vec![CameraPixel(idx as u16)],
                w: 1,
                h: 1,
                stride: 1,
            })
            .collect()
    }

    #[test]
    fn parse_effects_defaults_to_all_for_mega() {
        let parsed = parse_effects_env(
            "MISTER_TEST_EFFECTS_UNSET",
            "test-effects",
            &[FakeEffect::First, FakeEffect::Second],
            parse_fake,
            FakeEffect::First,
        );

        assert_eq!(parsed, vec![FakeEffect::First, FakeEffect::Second]);
    }

    #[test]
    fn selected_effect_uses_auto_segment_or_manual_index() {
        let effects = [FakeEffect::First, FakeEffect::Second];

        assert_eq!(
            selected_effect(
                &effects,
                true,
                Duration::from_secs(2),
                Duration::from_secs(3),
                0,
                FakeEffect::First,
            ),
            FakeEffect::Second
        );
        assert_eq!(
            selected_effect(
                &effects,
                false,
                Duration::from_secs(2),
                Duration::from_secs(3),
                1,
                FakeEffect::First,
            ),
            FakeEffect::Second
        );
    }

    #[test]
    fn load_effect_images_falls_back_to_synthetic_when_cache_is_empty() {
        let images = load_effect_images("/path/that/does/not/exist", 64, 2, 8, fake_synthetic);

        assert_eq!(images.len(), 8);
        assert_eq!(images[0].pixels[0], CameraPixel(0));
    }
}
