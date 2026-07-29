// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Platform-selected persistence for shared launcher settings.

use crate::settings::MagikSettings;
use std::io;
use std::path::{Path, PathBuf};

pub trait SettingsStore {
    fn load(&self) -> MagikSettings;
    fn save(&self, settings: &MagikSettings) -> io::Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSettingsStore {
    path: PathBuf,
}

impl FileSettingsStore {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SettingsStore for FileSettingsStore {
    fn load(&self) -> MagikSettings {
        MagikSettings::load_from(&self.path)
    }

    fn save(&self, settings: &MagikSettings) -> io::Result<()> {
        settings.save_to(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn file_store_round_trips_settings_at_injected_path() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let store = FileSettingsStore::new(
            std::env::temp_dir()
                .join(format!("mister-magik-settings-store-{stamp}"))
                .join("settings.json"),
        );
        let expected = MagikSettings {
            screensaver_enabled: false,
            screensaver_delay_minutes: 7,
            ..MagikSettings::default()
        };

        store.save(&expected).expect("save settings");

        assert_eq!(store.load(), expected);
    }
}
