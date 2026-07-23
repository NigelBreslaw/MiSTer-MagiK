// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Exact identity-based reconciliation planning for sharded catalogs.

use crate::catalog_classify::SystemId;
use crate::catalog_domain::{ScanUnitId, ScanUnitInventory};
use crate::incremental_inputs::{InputChangeKind, InputProbe};
use crate::sharded_catalog::{
    CatalogError, PlannedInput, PlannedInputChange, PlannedScanUnit, PlannedSystem,
    PlannedSystemAction, ReconcilePlan, ReconcileReason,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CatalogInvalidation {
    Metadata { systems: BTreeSet<SystemId> },
    SemanticVersion,
    PresentationTaxonomy,
    ExplicitSystem { system_id: SystemId },
}

pub struct PlanningRequest<'a> {
    pub current_generation: Option<u64>,
    pub published_systems: &'a BTreeSet<SystemId>,
    pub inventory: &'a ScanUnitInventory,
    pub probes: &'a BTreeMap<ScanUnitId, InputProbe>,
    pub invalidations: &'a [CatalogInvalidation],
}

pub fn plan_reconciliation(request: PlanningRequest<'_>) -> Result<ReconcilePlan, CatalogError> {
    let all_systems = request.inventory.all_systems();
    let dirty_units = request
        .probes
        .iter()
        .filter(|(_, probe)| !probe.changes.is_empty())
        .map(|(scan_unit_id, _)| scan_unit_id.clone())
        .collect::<BTreeSet<_>>();
    let mut reasons = BTreeMap::<SystemId, BTreeSet<ReconcileReason>>::new();

    let directly_affected = dirty_units
        .iter()
        .filter_map(|unit| request.inventory.produced_systems(unit))
        .flat_map(|systems| systems.iter().cloned())
        .collect::<BTreeSet<_>>();
    let globally_affected = request.inventory.affected_systems(&dirty_units);
    add_reason(
        &mut reasons,
        directly_affected.iter().cloned(),
        ReconcileReason::SourceChanged,
    );
    add_reason(
        &mut reasons,
        globally_affected.difference(&directly_affected).cloned(),
        ReconcileReason::SharedClaimChanged,
    );

    add_reason(
        &mut reasons,
        all_systems.difference(request.published_systems).cloned(),
        ReconcileReason::MissingCatalog,
    );
    add_reason(
        &mut reasons,
        request.published_systems.difference(&all_systems).cloned(),
        ReconcileReason::RemovedSystem,
    );

    let mut global_rebuild = false;
    let mut manifest_only = request.current_generation.is_none();
    for invalidation in request.invalidations {
        match invalidation {
            CatalogInvalidation::Metadata { systems } => add_reason(
                &mut reasons,
                systems.intersection(&all_systems).cloned(),
                ReconcileReason::MetadataChanged,
            ),
            CatalogInvalidation::SemanticVersion => {
                global_rebuild = true;
                add_reason(
                    &mut reasons,
                    all_systems.iter().cloned(),
                    ReconcileReason::SemanticVersionChanged,
                );
            }
            CatalogInvalidation::PresentationTaxonomy => manifest_only = true,
            CatalogInvalidation::ExplicitSystem { system_id } => {
                if all_systems.contains(system_id) {
                    add_reason(
                        &mut reasons,
                        std::iter::once(system_id.clone()),
                        ReconcileReason::ExplicitRequest,
                    );
                }
            }
        }
    }

    let scan_units = request
        .probes
        .iter()
        .filter(|(_, probe)| !probe.changes.is_empty())
        .map(|(scan_unit_id, probe)| PlannedScanUnit {
            scan_unit_id: scan_unit_id.clone(),
            inputs: probe
                .changes
                .iter()
                .map(|change| PlannedInput {
                    input_id: change.input_id.clone(),
                    change: match change.kind {
                        InputChangeKind::Added => PlannedInputChange::Added,
                        InputChangeKind::Modified => PlannedInputChange::Modified,
                        InputChangeKind::Removed => PlannedInputChange::Removed,
                    },
                })
                .collect(),
        })
        .collect::<Vec<_>>();
    let systems = reasons
        .into_iter()
        .map(|(system_id, reasons)| {
            let action = if reasons.contains(&ReconcileReason::RemovedSystem) {
                PlannedSystemAction::Remove
            } else {
                PlannedSystemAction::Rebuild
            };
            PlannedSystem {
                system_id,
                action,
                reasons: reasons.into_iter().collect(),
            }
        })
        .collect::<Vec<_>>();
    let changed = global_rebuild || manifest_only || !systems.is_empty();
    let current = request.current_generation.unwrap_or(0);
    let intended_generation = if changed {
        current
            .checked_add(1)
            .ok_or_else(|| CatalogError::new("plan", "manifest generation overflow"))?
    } else {
        current
    };
    Ok(ReconcilePlan {
        current_generation: request.current_generation,
        intended_generation,
        scan_units,
        systems,
        global_rebuild,
        manifest_only,
    })
}

fn add_reason(
    reasons: &mut BTreeMap<SystemId, BTreeSet<ReconcileReason>>,
    systems: impl Iterator<Item = SystemId>,
    reason: ReconcileReason,
) {
    for system_id in systems {
        reasons.entry(system_id).or_default().insert(reason.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog_domain::{DiscoveryClaim, InputId};
    use crate::incremental_inputs::{InputChange, InputSnapshot};
    use std::path::PathBuf;

    #[test]
    fn no_op_plan_keeps_the_current_generation() {
        let (inventory, _, _) = inventory();
        let published = inventory.all_systems();
        let probes = BTreeMap::new();
        let plan = plan_reconciliation(PlanningRequest {
            current_generation: Some(4),
            published_systems: &published,
            inventory: &inventory,
            probes: &probes,
            invalidations: &[],
        })
        .unwrap();
        assert!(plan.is_unchanged());
        assert_eq!(plan.intended_generation, 4);
    }

    #[test]
    fn dirty_input_reports_exact_identity_and_cross_unit_systems() {
        let (inventory, arcade, payload) = inventory();
        let published = inventory.all_systems();
        let dirty_input = InputId::new(arcade.clone(), PathBuf::from("Game.mgl")).unwrap();
        let probes = BTreeMap::from([(
            arcade.clone(),
            probe(dirty_input.clone(), InputChangeKind::Modified),
        )]);
        let plan = plan_reconciliation(PlanningRequest {
            current_generation: Some(4),
            published_systems: &published,
            inventory: &inventory,
            probes: &probes,
            invalidations: &[],
        })
        .unwrap();
        assert_eq!(plan.intended_generation, 5);
        assert_eq!(plan.scan_units[0].scan_unit_id, arcade);
        assert_eq!(plan.scan_units[0].inputs[0].input_id, dirty_input);
        assert_eq!(
            plan.systems
                .iter()
                .map(|system| system.system_id.as_str())
                .collect::<Vec<_>>(),
            vec!["arcade", "sms"]
        );
        assert!(plan.systems.iter().any(|system| {
            system.system_id.as_str() == "sms"
                && system
                    .reasons
                    .contains(&ReconcileReason::SharedClaimChanged)
        }));
        assert_eq!(payload.as_str(), "payload-root");
    }

    #[test]
    fn presentation_change_publishes_only_the_manifest() {
        let (inventory, _, _) = inventory();
        let published = inventory.all_systems();
        let probes = BTreeMap::new();
        let plan = plan_reconciliation(PlanningRequest {
            current_generation: Some(7),
            published_systems: &published,
            inventory: &inventory,
            probes: &probes,
            invalidations: &[CatalogInvalidation::PresentationTaxonomy],
        })
        .unwrap();
        assert!(plan.manifest_only);
        assert!(plan.systems.is_empty());
        assert_eq!(plan.intended_generation, 8);
    }

    #[test]
    fn semantic_change_rebuilds_every_known_system() {
        let (inventory, _, _) = inventory();
        let published = inventory.all_systems();
        let probes = BTreeMap::new();
        let plan = plan_reconciliation(PlanningRequest {
            current_generation: Some(2),
            published_systems: &published,
            inventory: &inventory,
            probes: &probes,
            invalidations: &[CatalogInvalidation::SemanticVersion],
        })
        .unwrap();
        assert!(plan.global_rebuild);
        assert_eq!(plan.systems.len(), 2);
        assert!(plan.systems.iter().all(|system| {
            system
                .reasons
                .contains(&ReconcileReason::SemanticVersionChanged)
        }));
    }

    #[test]
    fn system_absent_from_current_ownership_gets_a_removal_action() {
        let (inventory, _, _) = inventory();
        let published = systems(&["arcade", "sms", "obsolete"]);
        let probes = BTreeMap::new();
        let plan = plan_reconciliation(PlanningRequest {
            current_generation: Some(9),
            published_systems: &published,
            inventory: &inventory,
            probes: &probes,
            invalidations: &[],
        })
        .unwrap();
        let removed = plan
            .systems
            .iter()
            .find(|system| system.system_id.as_str() == "obsolete")
            .unwrap();
        assert_eq!(removed.action, PlannedSystemAction::Remove);
        assert_eq!(removed.reasons, vec![ReconcileReason::RemovedSystem]);
        assert_eq!(plan.intended_generation, 10);
    }

    #[test]
    fn maximum_generation_is_rejected_instead_of_wrapping() {
        let (inventory, _, _) = inventory();
        let published = BTreeSet::new();
        let probes = BTreeMap::new();
        let error = plan_reconciliation(PlanningRequest {
            current_generation: Some(u64::MAX),
            published_systems: &published,
            inventory: &inventory,
            probes: &probes,
            invalidations: &[],
        })
        .unwrap_err();
        assert_eq!(error.message(), "manifest generation overflow");
    }

    fn inventory() -> (ScanUnitInventory, ScanUnitId, ScanUnitId) {
        let arcade = ScanUnitId::parse("arcade-root").unwrap();
        let payload = ScanUnitId::parse("payload-root").unwrap();
        let mut inventory = ScanUnitInventory::default();
        inventory
            .register_scan_unit(arcade.clone(), systems(&["arcade"]))
            .unwrap();
        inventory
            .register_scan_unit(payload.clone(), systems(&["sms"]))
            .unwrap();
        let payload_input = InputId::new(payload.clone(), PathBuf::from("Game.sms")).unwrap();
        inventory
            .add_claim(
                DiscoveryClaim::new(payload_input.clone(), systems(&["sms"]))
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
                .covering(payload_input),
            )
            .unwrap();
        (inventory, arcade, payload)
    }

    fn systems(values: &[&str]) -> BTreeSet<SystemId> {
        values
            .iter()
            .map(|value| SystemId::parse(value).unwrap())
            .collect()
    }

    fn probe(input_id: InputId, kind: InputChangeKind) -> InputProbe {
        InputProbe {
            snapshot: InputSnapshot::default(),
            changes: vec![InputChange { input_id, kind }],
            statted_files: 1,
            enumerated_directories: 1,
        }
    }
}
