// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use slint::Image;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

const FALLBACK_ICON: &str = "document";
const ICON_ENV: &str = "MISTER_MAGIK_DESKTOP_MATERIAL_ICON_DIR";

thread_local! {
    static ICON_CACHE: RefCell<HashMap<String, Image>> = RefCell::new(HashMap::new());
}

pub fn material_icon(icon_key: &str) -> Image {
    let key = sanitize_icon_key(icon_key);
    ICON_CACHE.with_borrow_mut(|cache| {
        if let Some(image) = cache.get(key) {
            return image.clone();
        }
        let image = load_material_icon(key);
        cache.insert(key.to_string(), image.clone());
        image
    })
}

fn sanitize_icon_key(icon_key: &str) -> &str {
    let trimmed = icon_key.trim();
    if trimmed.is_empty()
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.contains("..")
    {
        FALLBACK_ICON
    } else {
        trimmed
    }
}

fn load_material_icon(icon_key: &str) -> Image {
    load_material_icon_from_dir(icon_key, &material_icon_dir())
}

fn load_material_icon_from_dir(icon_key: &str, directory: &Path) -> Image {
    let path = directory.join(format!("{icon_key}.svg"));
    Image::load_from_path(&path).unwrap_or_else(|_| {
        if icon_key == FALLBACK_ICON {
            Image::default()
        } else {
            load_material_icon_from_dir(FALLBACK_ICON, directory)
        }
    })
}

fn material_icon_dir() -> PathBuf {
    let configured = std::env::var_os(ICON_ENV).map(PathBuf::from);
    material_icon_dir_from_override(configured.as_deref())
}

fn material_icon_dir_from_override(configured: Option<&Path>) -> PathBuf {
    let mut candidates = Vec::new();
    if let Some(path) = configured {
        candidates.push(path.to_path_buf());
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("assets/material-icon-theme/icons"));
            candidates.push(dir.join("../Resources/material-icon-theme/icons"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("apps/desktop/vendor/material-icon-theme/icons"));
        candidates.push(cwd.join("vendor/material-icon-theme/icons"));
    }
    candidates
        .push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor/material-icon-theme/icons"));

    candidates
        .into_iter()
        .find(|path| path.join(format!("{FALLBACK_ICON}.svg")).is_file())
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("vendor/material-icon-theme/icons")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_key_rejects_path_like_values() {
        assert_eq!(sanitize_icon_key("json"), "json");
        assert_eq!(sanitize_icon_key(" json "), "json");
        assert_eq!(sanitize_icon_key("../json"), FALLBACK_ICON);
        assert_eq!(sanitize_icon_key("foo/bar"), FALLBACK_ICON);
        assert_eq!(sanitize_icon_key(r"foo\bar"), FALLBACK_ICON);
        assert_eq!(sanitize_icon_key(""), FALLBACK_ICON);
    }

    #[test]
    fn material_icon_dir_prefers_env_when_it_contains_fallback_icon() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-icon-dir-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("document.svg"), "<svg/>").unwrap();

        assert_eq!(material_icon_dir_from_override(Some(&root)), root);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn material_icon_falls_back_to_document_for_missing_or_unsafe_keys() {
        let root =
            std::env::temp_dir().join(format!("mister-magik-icon-fallback-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("document.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"/>"#,
        )
        .unwrap();

        assert!(
            load_material_icon_from_dir("missing", &root)
                .to_rgba8()
                .is_some()
        );
        assert!(
            load_material_icon_from_dir("../missing", &root)
                .to_rgba8()
                .is_some()
        );

        let _ = std::fs::remove_dir_all(&root);
    }
}
