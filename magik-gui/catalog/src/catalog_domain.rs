// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Physical scan-unit ownership and global discovery-claim relationships.

use crate::catalog_classify::SystemId;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ScanUnitId(String);

impl ScanUnitId {
    pub fn parse(value: &str) -> Result<Self, CatalogDomainError> {
        let value = value.trim().to_ascii_lowercase().replace('_', "-");
        if value.is_empty()
            || value.len() > 64
            || value.starts_with('-')
            || value.ends_with('-')
            || value.contains("--")
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(CatalogDomainError::new("invalid scan-unit ID"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct InputId {
    scan_unit_id: ScanUnitId,
    relative_path: PathBuf,
}

impl InputId {
    pub fn new(
        scan_unit_id: ScanUnitId,
        relative_path: PathBuf,
    ) -> Result<Self, CatalogDomainError> {
        if relative_path.as_os_str().is_empty()
            || relative_path.is_absolute()
            || relative_path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            return Err(CatalogDomainError::new(
                "input path must be a non-empty normalized relative path",
            ));
        }
        Ok(Self {
            scan_unit_id,
            relative_path,
        })
    }

    pub fn scan_unit_id(&self) -> &ScanUnitId {
        &self.scan_unit_id
    }

    pub fn relative_path(&self) -> &Path {
        &self.relative_path
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryClaim {
    pub input_id: InputId,
    pub produced_systems: BTreeSet<SystemId>,
    pub preference_key: Option<String>,
    pub covers: Option<InputId>,
}

impl DiscoveryClaim {
    pub fn new(
        input_id: InputId,
        produced_systems: BTreeSet<SystemId>,
    ) -> Result<Self, CatalogDomainError> {
        if produced_systems.is_empty() {
            return Err(CatalogDomainError::new(
                "a discovery claim must produce at least one system",
            ));
        }
        Ok(Self {
            input_id,
            produced_systems,
            preference_key: None,
            covers: None,
        })
    }

    pub fn with_preference_key(mut self, key: impl Into<String>) -> Self {
        self.preference_key = Some(key.into());
        self
    }

    pub fn covering(mut self, input_id: InputId) -> Self {
        self.covers = Some(input_id);
        self
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanUnitInventory {
    produced_systems: BTreeMap<ScanUnitId, BTreeSet<SystemId>>,
    claims: Vec<DiscoveryClaim>,
}

impl ScanUnitInventory {
    pub fn register_scan_unit(
        &mut self,
        scan_unit_id: ScanUnitId,
        produced_systems: BTreeSet<SystemId>,
    ) -> Result<(), CatalogDomainError> {
        if self.produced_systems.contains_key(&scan_unit_id) {
            return Err(CatalogDomainError::new("duplicate scan-unit ID"));
        }
        self.produced_systems.insert(scan_unit_id, produced_systems);
        Ok(())
    }

    pub fn add_claim(&mut self, claim: DiscoveryClaim) -> Result<(), CatalogDomainError> {
        if !self
            .produced_systems
            .contains_key(claim.input_id.scan_unit_id())
        {
            return Err(CatalogDomainError::new(
                "claim belongs to an unknown scan unit",
            ));
        }
        self.produced_systems
            .get_mut(claim.input_id.scan_unit_id())
            .expect("checked scan unit")
            .extend(claim.produced_systems.iter().cloned());
        self.claims.push(claim);
        Ok(())
    }

    pub fn produced_systems(&self, scan_unit_id: &ScanUnitId) -> Option<&BTreeSet<SystemId>> {
        self.produced_systems.get(scan_unit_id)
    }

    pub fn claims(&self) -> &[DiscoveryClaim] {
        &self.claims
    }

    pub fn all_systems(&self) -> BTreeSet<SystemId> {
        self.produced_systems
            .values()
            .flat_map(|systems| systems.iter().cloned())
            .collect()
    }

    pub fn affected_systems(&self, dirty_units: &BTreeSet<ScanUnitId>) -> BTreeSet<SystemId> {
        let mut affected = dirty_units
            .iter()
            .filter_map(|unit| self.produced_systems.get(unit))
            .flat_map(|systems| systems.iter().cloned())
            .collect::<BTreeSet<_>>();
        let mut affected_inputs = self
            .claims
            .iter()
            .filter(|claim| dirty_units.contains(claim.input_id.scan_unit_id()))
            .map(|claim| claim.input_id.clone())
            .collect::<BTreeSet<_>>();
        let mut affected_preference_keys = BTreeSet::new();
        loop {
            let inputs_before = affected_inputs.len();
            let keys_before = affected_preference_keys.len();
            for claim in &self.claims {
                let input_affected = affected_inputs.contains(&claim.input_id);
                let preference_affected = claim
                    .preference_key
                    .as_ref()
                    .is_some_and(|key| affected_preference_keys.contains(key));
                let covers_affected = claim
                    .covers
                    .as_ref()
                    .is_some_and(|covered| affected_inputs.contains(covered));
                if input_affected || preference_affected || covers_affected {
                    affected_inputs.insert(claim.input_id.clone());
                    if let Some(key) = &claim.preference_key {
                        affected_preference_keys.insert(key.clone());
                    }
                    if let Some(covered) = &claim.covers {
                        affected_inputs.insert(covered.clone());
                    }
                    affected.extend(claim.produced_systems.iter().cloned());
                }
            }
            if affected_inputs.len() == inputs_before
                && affected_preference_keys.len() == keys_before
            {
                break;
            }
        }
        affected
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogDomainError {
    message: &'static str,
}

impl CatalogDomainError {
    fn new(message: &'static str) -> Self {
        Self { message }
    }
}

impl fmt::Display for CatalogDomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for CatalogDomainError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_ids_reject_absolute_and_parent_paths() {
        let unit = scan_unit("arcade-root");
        assert!(InputId::new(unit.clone(), PathBuf::from("/absolute.mra")).is_err());
        assert!(InputId::new(unit, PathBuf::from("../escape.mra")).is_err());
    }

    #[test]
    fn one_scan_unit_can_produce_several_systems() {
        let mut inventory = ScanUnitInventory::default();
        let arcade = scan_unit("arcade-root");
        inventory
            .register_scan_unit(arcade.clone(), systems(&["arcade", "sms", "gamegear"]))
            .unwrap();
        assert_eq!(inventory.produced_systems(&arcade).unwrap().len(), 3);
    }

    #[test]
    fn one_system_can_be_owned_by_several_scan_units() {
        let mut inventory = ScanUnitInventory::default();
        inventory
            .register_scan_unit(scan_unit("snes-primary"), systems(&["snes"]))
            .unwrap();
        inventory
            .register_scan_unit(scan_unit("snes-usb"), systems(&["snes"]))
            .unwrap();
        let dirty = [scan_unit("snes-usb")].into_iter().collect();
        assert_eq!(inventory.affected_systems(&dirty), systems(&["snes"]));
    }

    #[test]
    fn dirty_claims_expand_through_global_preference_and_coverage() {
        let mut inventory = ScanUnitInventory::default();
        let arcade = scan_unit("arcade-root");
        let payloads = scan_unit("payload-root");
        inventory
            .register_scan_unit(arcade.clone(), systems(&["arcade"]))
            .unwrap();
        inventory
            .register_scan_unit(payloads.clone(), systems(&["sms"]))
            .unwrap();
        let payload = InputId::new(payloads, PathBuf::from("SMS/Game.sms")).unwrap();
        inventory
            .add_claim(
                DiscoveryClaim::new(payload.clone(), systems(&["sms"]))
                    .unwrap()
                    .with_preference_key("game-family"),
            )
            .unwrap();
        inventory
            .add_claim(
                DiscoveryClaim::new(
                    InputId::new(arcade.clone(), PathBuf::from("Game.mgl")).unwrap(),
                    systems(&["arcade"]),
                )
                .unwrap()
                .with_preference_key("game-family")
                .covering(payload),
            )
            .unwrap();

        let dirty = [arcade].into_iter().collect();
        assert_eq!(
            inventory.affected_systems(&dirty),
            systems(&["arcade", "sms"])
        );
    }

    #[test]
    fn claim_expansion_reaches_a_transitive_fixed_point() {
        let mut inventory = ScanUnitInventory::default();
        let first = scan_unit("first-root");
        let second = scan_unit("second-root");
        let third = scan_unit("third-root");
        inventory
            .register_scan_unit(first.clone(), systems(&["arcade"]))
            .unwrap();
        inventory
            .register_scan_unit(second.clone(), systems(&["sms"]))
            .unwrap();
        inventory
            .register_scan_unit(third.clone(), systems(&["gamegear"]))
            .unwrap();
        let first_input = InputId::new(first.clone(), PathBuf::from("first.mgl")).unwrap();
        let second_input = InputId::new(second, PathBuf::from("second.sms")).unwrap();
        let third_input = InputId::new(third, PathBuf::from("third.gg")).unwrap();
        inventory
            .add_claim(
                DiscoveryClaim::new(first_input, systems(&["arcade"]))
                    .unwrap()
                    .with_preference_key("first-family")
                    .covering(second_input.clone()),
            )
            .unwrap();
        inventory
            .add_claim(
                DiscoveryClaim::new(second_input, systems(&["sms"]))
                    .unwrap()
                    .with_preference_key("second-family")
                    .covering(third_input.clone()),
            )
            .unwrap();
        inventory
            .add_claim(
                DiscoveryClaim::new(third_input, systems(&["gamegear"]))
                    .unwrap()
                    .with_preference_key("third-family"),
            )
            .unwrap();

        let dirty = [first].into_iter().collect();
        assert_eq!(
            inventory.affected_systems(&dirty),
            systems(&["arcade", "sms", "gamegear"])
        );
    }

    fn scan_unit(value: &str) -> ScanUnitId {
        ScanUnitId::parse(value).unwrap()
    }

    fn systems(values: &[&str]) -> BTreeSet<SystemId> {
        values
            .iter()
            .map(|value| SystemId::parse(value).unwrap())
            .collect()
    }
}
