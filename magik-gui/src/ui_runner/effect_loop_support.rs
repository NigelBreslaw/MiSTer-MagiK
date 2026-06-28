use super::*;
use crate::preview_worker;
use mister_magik_fb::camera_effects::{CameraImage, CameraPixel};
use mister_magik_fb::framebuffer::mapped::MappedRgb565Framebuffer;
use slint::platform::software_renderer::Rgb565Pixel;
use std::fs::File;
use std::io::Write;

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

pub(super) fn present_camera_pixels_565(
    disp: &mut MappedRgb565Framebuffer,
    src: &[CameraPixel],
    scratch: &mut [Rgb565Pixel],
    y0: usize,
    y1: usize,
) {
    debug_assert!(scratch.len() >= src.len());
    for (dst, src) in scratch.iter_mut().zip(src.iter()) {
        *dst = Rgb565Pixel(src.0);
    }
    if let Err(e) = disp.present_rows_565(scratch, y0, y1) {
        eprintln!("effect present failed: {e}");
    }
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
    // SAFETY: ts points to initialized writable storage for the duration of the
    // syscall; failures are converted to 0.
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
    let mut assets = Vec::new();
    if let Ok(loaded) = library_db::load_arcade_catalog_from_sqlite(root) {
        assets.extend(
            loaded
                .catalog
                .games
                .iter()
                .filter(|game| {
                    game.has_preview
                        && !game.preview_archive_path.is_empty()
                        && !game.preview_asset_key.is_empty()
                })
                .map(|game| {
                    (
                        game.preview_archive_path.to_string(),
                        game.preview_asset_key.to_string(),
                    )
                })
                .take(cap * 4),
        );
    }
    let mut images = Vec::new();
    for (archive_path, asset_key) in assets {
        if images.len() >= cap {
            break;
        }
        if let Ok(image) = preview_worker::load_preview_asset_pixels(&archive_path, &asset_key) {
            let image = preview_pixels_to_camera_image(image);
            images.push(image);
        }
    }
    if images.is_empty() {
        synthetic(cap.min(synthetic_max).max(synthetic_min))
    } else {
        images
    }
}

fn preview_pixels_to_camera_image(image: preview_worker::PreviewPixels) -> CameraImage {
    match image {
        preview_worker::PreviewPixels::Rgb565 {
            width,
            height,
            stride_bytes,
            words,
        } => CameraImage {
            pixels: words.iter().copied().map(CameraPixel).collect(),
            w: width as usize,
            h: height as usize,
            stride: stride_bytes as usize / 2,
        },
    }
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
