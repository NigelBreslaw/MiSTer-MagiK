// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Declarative deterministic launcher scene matrix.

use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub(super) const LAUNCHER_SCENE_MANIFEST_JSON: &str =
    include_str!("../../../tests/launcher-scenes.json");

const SCHEMA: &str = "mister-magik-launcher-scenes-v1";
const EXPECTED_SCENE_COUNT: usize = 18;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LauncherSceneManifest {
    pub schema: String,
    pub scenes: Vec<LauncherScene>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct LauncherScene {
    pub id: String,
    pub profile: SceneProfile,
    pub scenario: SceneScenario,
    pub content: SceneContent,
    pub orientation: SceneOrientation,
    pub refresh_hz: u32,
    pub frame: u64,
    pub scan: bool,
    pub download: bool,
    pub transition: Option<SceneTransition>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SceneProfile {
    Hdmi,
    #[serde(rename = "crt-240p")]
    Crt240p,
    #[serde(rename = "crt-480p")]
    Crt480p,
}

impl SceneProfile {
    const fn id(self) -> &'static str {
        match self {
            Self::Hdmi => "hdmi",
            Self::Crt240p => "crt-240p",
            Self::Crt480p => "crt",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SceneScenario {
    Home,
    Arcade,
    Settings,
    ControllerSetup,
    CatalogScan,
    NavigationTransitionMidpoint,
}

impl SceneScenario {
    const fn id(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Arcade => "arcade",
            Self::Settings => "settings",
            Self::ControllerSetup => "controller-setup",
            Self::CatalogScan => "catalog-scan",
            Self::NavigationTransitionMidpoint => "navigation-transition-midpoint",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SceneContent {
    Fixtures,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SceneOrientation {
    Normal,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct SceneTransition {
    pub edge: SceneTransitionEdge,
    pub duration_ms: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SceneTransitionEdge {
    HomeArcade,
}

pub(super) fn launcher_scene_manifest() -> Result<LauncherSceneManifest, String> {
    parse_and_validate(LAUNCHER_SCENE_MANIFEST_JSON)
}

fn parse_and_validate(input: &str) -> Result<LauncherSceneManifest, String> {
    let manifest: LauncherSceneManifest = serde_json::from_str(input)
        .map_err(|error| format!("parse launcher scene manifest: {error}"))?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(manifest: &LauncherSceneManifest) -> Result<(), String> {
    if manifest.schema != SCHEMA {
        return Err(format!(
            "unknown launcher scene manifest schema {:?}",
            manifest.schema
        ));
    }
    if manifest.scenes.len() != EXPECTED_SCENE_COUNT {
        return Err(format!(
            "launcher scene manifest must contain {EXPECTED_SCENE_COUNT} scenes, found {}",
            manifest.scenes.len()
        ));
    }

    let mut ids = BTreeSet::new();
    let mut combinations = BTreeSet::new();
    for scene in &manifest.scenes {
        if !ids.insert(scene.id.as_str()) {
            return Err(format!("duplicate launcher scene id {:?}", scene.id));
        }
        let expected_id = format!("{}-{}", scene.profile.id(), scene.scenario.id());
        if scene.id != expected_id {
            return Err(format!(
                "launcher scene id {:?} must be {expected_id:?}",
                scene.id
            ));
        }
        if !combinations.insert((scene.profile, scene.scenario)) {
            return Err(format!(
                "duplicate launcher scene combination {expected_id}"
            ));
        }
        if scene.content != SceneContent::Fixtures
            || scene.orientation != SceneOrientation::Normal
            || scene.refresh_hz != 60
            || scene.scan
            || scene.download
        {
            return Err(format!("launcher scene {:?} has unpinned inputs", scene.id));
        }
        match scene.scenario {
            SceneScenario::NavigationTransitionMidpoint => {
                let transition = scene
                    .transition
                    .as_ref()
                    .ok_or_else(|| format!("launcher scene {:?} needs a transition", scene.id))?;
                if transition.edge != SceneTransitionEdge::HomeArcade
                    || transition.duration_ms != 600
                    || scene.frame != 17
                {
                    return Err(format!(
                        "launcher scene {:?} must pin the 600ms Home-to-Arcade midpoint at frame 17",
                        scene.id
                    ));
                }
            }
            _ if scene.transition.is_some() || scene.frame != 0 => {
                return Err(format!(
                    "launcher scene {:?} must use frame zero without a transition",
                    scene.id
                ));
            }
            _ => {}
        }
    }

    for profile in [
        SceneProfile::Hdmi,
        SceneProfile::Crt240p,
        SceneProfile::Crt480p,
    ] {
        for scenario in [
            SceneScenario::Home,
            SceneScenario::Arcade,
            SceneScenario::Settings,
            SceneScenario::ControllerSetup,
            SceneScenario::CatalogScan,
            SceneScenario::NavigationTransitionMidpoint,
        ] {
            if !combinations.contains(&(profile, scenario)) {
                return Err(format!(
                    "launcher scene manifest is missing {}-{}",
                    profile.id(),
                    scenario.id()
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_manifest_is_complete_and_stably_ordered() {
        let manifest = launcher_scene_manifest().expect("checked-in launcher scene manifest");
        assert_eq!(manifest.scenes.len(), EXPECTED_SCENE_COUNT);
        let ids = manifest
            .scenes
            .iter()
            .map(|scene| scene.id.as_str())
            .collect::<Vec<_>>();
        let mut sorted = ids.clone();
        sorted.sort_unstable();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn manifest_rejects_unknown_duplicate_and_incomplete_scenes() {
        let unknown = LAUNCHER_SCENE_MANIFEST_JSON.replace(
            "\"refresh_hz\": 60,",
            "\"refresh_hz\": 60, \"host\": \"local\",",
        );
        assert!(parse_and_validate(&unknown).is_err());

        let mut duplicate = launcher_scene_manifest().unwrap();
        duplicate.scenes[1].id = duplicate.scenes[0].id.clone();
        assert!(validate(&duplicate).is_err());

        let mut incomplete = launcher_scene_manifest().unwrap();
        incomplete.scenes.pop();
        assert!(validate(&incomplete).is_err());
    }
}
