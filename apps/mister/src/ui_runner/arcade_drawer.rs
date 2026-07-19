// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::*;

#[derive(Default)]
pub(super) struct ArcadeDrawerViewCache {
    key: Option<ArcadeDrawerViewKey>,
    items: Vec<ArcadeListItem>,
    pub(super) rebuilds: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct ArcadeDrawerViewKey {
    catalog_version: usize,
    system_id: String,
    level: launcher::ArcadeFilterLevel,
    active_filter: String,
}

impl ArcadeDrawerViewCache {
    pub(super) fn items(
        &mut self,
        catalog: &ArcadeCatalog,
        nav: &LauncherNav,
        catalog_version: usize,
    ) -> &[ArcadeListItem] {
        let system_id = active_system(catalog, nav)
            .map(|system| system.id.as_str())
            .unwrap_or("");
        let key = ArcadeDrawerViewKey {
            catalog_version,
            system_id: system_id.to_string(),
            level: nav.arcade_filter.level,
            active_filter: arcade_filter_cache_token(&nav.arcade_filter.active),
        };
        if self.key.as_ref() != Some(&key) {
            self.items = arcade_filter_list_items_for_system(catalog, nav, system_id);
            self.key = Some(key);
            self.rebuilds = self.rebuilds.wrapping_add(1);
        }
        &self.items
    }
}

pub(super) fn arcade_filter_cache_token(filter: &arcade_catalog::ArcadeFilter) -> String {
    match filter {
        arcade_catalog::ArcadeFilter::All => "all".to_string(),
        arcade_catalog::ArcadeFilter::Search => "search".to_string(),
        arcade_catalog::ArcadeFilter::Decade(decade) => format!("decade:{decade}"),
        arcade_catalog::ArcadeFilter::Manufacturer(manufacturer) => {
            format!("manufacturer:{manufacturer}")
        }
        arcade_catalog::ArcadeFilter::Players(players) => format!("players:{players}"),
        arcade_catalog::ArcadeFilter::Control(control) => format!("control:{control}"),
    }
}

fn arcade_filter_list_items_for_system(
    catalog: &ArcadeCatalog,
    nav: &LauncherNav,
    system_id: &str,
) -> Vec<ArcadeListItem> {
    nav.arcade_filter_items(catalog, system_id)
        .into_iter()
        .map(|item| ArcadeListItem {
            title: item.label,
            count: Some(item.count),
            active: item.active,
        })
        .collect()
}
