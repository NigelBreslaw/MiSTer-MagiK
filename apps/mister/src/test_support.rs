// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::PathBuf;

use crate::arcade_catalog::{ArcadeCatalog, ArcadeGameEntry, DEFAULT_ARCADE_ROOT, GameSystemEntry};

pub struct ArcadeGameFixture {
    title: String,
    mra_path: String,
    preview_archive_path: String,
    preview_asset_key: String,
    system_id: String,
    year: Option<u16>,
    manufacturer: String,
    category: String,
    players: Option<u8>,
    control: String,
    is_new: bool,
}

impl ArcadeGameFixture {
    pub fn path(mut self, path: impl Into<String>) -> Self {
        self.mra_path = path.into();
        self
    }

    pub fn preview(mut self, asset_key: impl Into<String>) -> Self {
        self.preview_archive_path =
            "/media/fat/mister-magik/assets/arcade-screenshots.mmlz4b".to_string();
        self.preview_asset_key = asset_key.into();
        self
    }

    pub fn system_id(mut self, system_id: impl Into<String>) -> Self {
        self.system_id = system_id.into();
        self
    }

    pub fn year(mut self, year: u16) -> Self {
        self.year = Some(year);
        self
    }

    pub fn manufacturer(mut self, manufacturer: impl Into<String>) -> Self {
        self.manufacturer = manufacturer.into();
        self
    }

    pub fn players(mut self, players: u8) -> Self {
        self.players = Some(players);
        self
    }

    pub fn control(mut self, control: impl Into<String>) -> Self {
        self.control = control.into();
        self
    }

    pub fn build(self) -> ArcadeGameEntry {
        let has_preview =
            !self.preview_archive_path.is_empty() && !self.preview_asset_key.is_empty();
        ArcadeGameEntry {
            title: self.title.into(),
            mra_path: self.mra_path.into(),
            preview_archive_path: self.preview_archive_path.into(),
            preview_asset_key: self.preview_asset_key.into(),
            has_preview,
            system_id: self.system_id.into(),
            year: self.year,
            manufacturer: self.manufacturer.into(),
            category: self.category.into(),
            players: self.players,
            control: self.control.into(),
            is_new: self.is_new,
        }
    }
}

pub fn arcade_game(title: impl Into<String>) -> ArcadeGameFixture {
    let title = title.into();
    let mra_path = format!("{DEFAULT_ARCADE_ROOT}/{title}.mra");
    ArcadeGameFixture {
        title,
        mra_path,
        preview_archive_path: String::new(),
        preview_asset_key: String::new(),
        system_id: "arcade".to_string(),
        year: None,
        manufacturer: String::new(),
        category: String::new(),
        players: None,
        control: String::new(),
        is_new: false,
    }
}

pub fn arcade_system(id: impl Into<String>, count: usize) -> GameSystemEntry {
    let id = id.into();
    GameSystemEntry {
        title: system_title(&id),
        id,
        count,
    }
}

pub fn arcade_catalog(games: Vec<ArcadeGameEntry>, systems: Vec<GameSystemEntry>) -> ArcadeCatalog {
    ArcadeCatalog::new(PathBuf::from(DEFAULT_ARCADE_ROOT), games, systems)
}

fn system_title(id: &str) -> String {
    match id {
        "arcade" => "Arcade".to_string(),
        "amiga" => "Amiga".to_string(),
        other => other.to_string(),
    }
}
