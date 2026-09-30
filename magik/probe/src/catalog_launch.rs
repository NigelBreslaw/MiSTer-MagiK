// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Capsule second-pass lookup against real mapped navigation rows. Preparation
//! and full hot-row hydration are excluded, matching the capsule's first pass.
use mister_magik_catalog::arcade_catalog::PlatformKind;
use mister_magik_catalog::arcade_catalog::{
    ArcadeCatalog, ArcadeGameView, LaunchTarget, SystemCollection,
};
use mister_magik_catalog::system_shard::{SystemGame, SystemLaunchPlan, SystemNavigationIndexes};
use std::{path::PathBuf, sync::Arc, time::Instant};
const SIZES: [usize; 3] = [1_000, 2_000, 4_000];
const FIXTURE: &str = "7f0a6769b316c968fd878256eb5dde1589eb0862380c5ff00026c1860c88cd90";
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn fixture(count: usize) -> Result<(Fixture, ArcadeCatalog), String> {
    let root = std::env::temp_dir().join(format!(
        "magik-catalog-launch-{}-{count}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let owned = Fixture(root);
    let games = (0..count)
        .map(|i| {
            let title = format!("Game {i:04}");
            let launch_ref = format!("magik-plan:fixture:{i:04}");
            SystemGame {
                stable_key: format!("fixture:{i:04}"),
                title: title.clone(),
                launch_ref: launch_ref.clone(),
                launch_plan: Some(SystemLaunchPlan {
                    launch_ref,
                    title,
                    system_id: "fixture".into(),
                    core_path: "Fixture".into(),
                    payload_path: format!("/games/fixture/{i:04}.bin"),
                    mount_kind: "load-file".into(),
                    mount_index: 0,
                    delay_secs: 1,
                }),
                ..Default::default()
            }
        })
        .collect::<Vec<_>>();
    let indexes = SystemNavigationIndexes {
        title_ordinals: (0..count as u32).collect(),
        launch_ordinals: (0..count as u32).collect(),
        ..Default::default()
    };
    let bytes = mister_magik_catalog::navpack::encode("fixture", 7, &games, &indexes)?;
    let path = owned.0.join("fixture.navpack");
    std::fs::write(&path, &bytes).map_err(|e| e.to_string())?;
    let (collection, _) = SystemCollection::open_navpack(
        "fixture",
        &path,
        bytes.len() as u64,
        7,
        count,
        PlatformKind::Computer,
    )?;
    let catalog = ArcadeCatalog::new(owned.0.clone(), vec![], vec![])
        .with_system_collection(Arc::new(collection));
    if catalog.system_game_view("fixture").iter().count() != count {
        return Err("missing mapped rows".into());
    }
    Ok((owned, catalog))
}
fn lookup(
    catalog: &ArcadeCatalog,
    view: ArcadeGameView<'_>,
    ordinal: usize,
) -> Option<LaunchTarget> {
    catalog.launch_target_in_view(view, ordinal)
}
fn verify(catalog: &ArcadeCatalog, count: usize) -> Result<(), String> {
    let view = catalog.system_game_view("fixture");
    for i in 0..count {
        let Some(LaunchTarget::Structured(plan)) = lookup(catalog, view, i) else {
            return Err("missing structured plan".into());
        };
        if plan.launch_ref.as_ref() != format!("magik-plan:fixture:{i:04}")
            || plan.title.as_ref() != format!("Game {i:04}")
            || plan.system_id.as_ref() != "fixture"
            || plan.core_path.as_ref() != "Fixture"
            || plan.payload_path.as_ref() != format!("/games/fixture/{i:04}.bin")
            || plan.mount_kind.as_ref() != "load-file"
            || plan.mount_index != 0
            || plan.delay_secs != 1
        {
            return Err("launch plan parity failed".into());
        }
    }
    Ok(())
}
fn measure(catalog: &ArcadeCatalog, count: usize) -> Result<u64, String> {
    let view = catalog.system_game_view("fixture");
    let started = Instant::now();
    for i in 0..count {
        std::hint::black_box(lookup(catalog, view, i).ok_or("missing timed row")?);
    }
    Ok(started.elapsed().as_nanos().try_into().unwrap_or(u64::MAX))
}
pub(super) fn run() -> Result<serde_json::Value, String> {
    let fixtures = SIZES
        .into_iter()
        .map(fixture)
        .collect::<Result<Vec<_>, _>>()?;
    for ((_, catalog), count) in fixtures.iter().zip(SIZES) {
        verify(catalog, count)?;
    }
    let work_count = SIZES.iter().sum::<usize>();
    let mut samples = Vec::new();
    for repetition in 0..2 {
        let mut cases = Vec::new();
        let mut duration_ns = 0;
        for ((_, catalog), count) in fixtures.iter().zip(SIZES) {
            let ns = measure(catalog, count)?;
            duration_ns += ns;
            cases.push(serde_json::json!({"rows":count,"duration_ns":ns}));
        }
        samples.push(serde_json::json!({"repetition":repetition,"fixture_identity":FIXTURE,"work_count":work_count,"duration_ns":duration_ns,"ns_per_pixel":duration_ns as f64 / work_count as f64,"cases":cases}));
    }
    let sha = std::env::var("MISTER_MAGIK2_ARTIFACT_SHA256")
        .map_err(|_| "native artifact identity missing")?;
    Ok(
        serde_json::json!({"schema_version":1,"workload":"catalog-launch","mode":"timing","artifact_sha256":sha,"correctness":"passed","work_count":work_count,"fixture":{"identity":FIXTURE,"cases":SIZES,"storage":"temporary mapped files","scope":"mapped launch lookup after hot-row hydration; excludes full capsule encoding and Main handoff"},"samples":samples}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mapped_launch_fixture_preserves_all_plan_fields() {
        let (_owned, catalog) = fixture(130).unwrap();
        verify(&catalog, 130).unwrap();
        assert!(lookup(&catalog, catalog.system_game_view("fixture"), 130).is_none());
    }
}
