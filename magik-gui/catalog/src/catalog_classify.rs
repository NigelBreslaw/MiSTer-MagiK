//! Canonical product taxonomy for launcher systems.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

const SYSTEM_TAXONOMY_JSON: &str = include_str!("../data/system_taxonomy.json");
pub const SYSTEM_TAXONOMY_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformKind {
    Arcade,
    Console,
    Handheld,
    Computer,
    #[default]
    Unknown,
}

impl PlatformKind {
    pub fn from_category(category: &str) -> Self {
        match category.trim().to_ascii_lowercase().as_str() {
            "arcade" => Self::Arcade,
            "console" => Self::Console,
            "handheld" => Self::Handheld,
            "computer" => Self::Computer,
            _ => Self::Unknown,
        }
    }

    pub fn inferred_for_system_id(system_id: &str) -> Self {
        platform_kind_for_system(system_id)
    }

    pub(crate) const fn encoded(self) -> u8 {
        match self {
            Self::Unknown => 0,
            Self::Arcade => 1,
            Self::Console => 2,
            Self::Handheld => 3,
            Self::Computer => 4,
        }
    }

    pub(crate) fn from_encoded(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Unknown),
            1 => Some(Self::Arcade),
            2 => Some(Self::Console),
            3 => Some(Self::Handheld),
            4 => Some(Self::Computer),
            _ => None,
        }
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Arcade => "arcade",
            Self::Console => "console",
            Self::Handheld => "handheld",
            Self::Computer => "computer",
            Self::Unknown => "unknown",
        }
    }

    pub const fn category_label(self) -> &'static str {
        match self {
            Self::Arcade => "Arcade",
            Self::Console => "Console",
            Self::Handheld => "Handheld",
            Self::Computer => "Computer",
            Self::Unknown => "Unknown",
        }
    }

    pub fn from_stored(value: &str) -> Result<Self, String> {
        match value {
            "arcade" => Ok(Self::Arcade),
            "console" => Ok(Self::Console),
            "handheld" => Ok(Self::Handheld),
            "computer" => Ok(Self::Computer),
            "unknown" => Ok(Self::Unknown),
            _ => Err(format!("invalid stored platform kind {value:?}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LauncherSection {
    Arcade,
    SnkNeogeo,
    Consoles,
    Handhelds,
    Computers,
    #[default]
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemDefinition {
    pub id: String,
    pub title: String,
    pub platform_kind: PlatformKind,
    pub section: LauncherSection,
    #[serde(default)]
    pub family: String,
    #[serde(default = "default_order")]
    pub order: u16,
    #[serde(default)]
    pub aliases: Vec<String>,
}

fn default_order() -> u16 {
    1000
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemClassification {
    pub system_id: String,
    pub platform_kind: PlatformKind,
    pub source: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemClassificationDiagnostic {
    pub system_id: String,
    pub accepted_kind: PlatformKind,
    pub accepted_source: String,
    pub rejected_kind: PlatformKind,
    pub rejected_source: String,
    pub reason: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemClassificationResolution {
    pub classification: SystemClassification,
    pub diagnostic: Option<SystemClassificationDiagnostic>,
}

#[derive(Debug, Deserialize)]
struct SystemTaxonomyDocument {
    schema: u32,
    systems: Vec<SystemDefinition>,
}

#[derive(Debug)]
struct SystemTaxonomy {
    systems: Vec<SystemDefinition>,
    by_id: HashMap<String, usize>,
}

impl SystemTaxonomy {
    fn parse(text: &str) -> Result<Self, String> {
        let document: SystemTaxonomyDocument = serde_json::from_str(text)
            .map_err(|error| format!("parse system taxonomy: {error}"))?;
        if document.schema != SYSTEM_TAXONOMY_VERSION {
            return Err(format!(
                "system taxonomy schema {} does not match expected {SYSTEM_TAXONOMY_VERSION}",
                document.schema
            ));
        }
        let mut by_id = HashMap::new();
        let mut aliases = HashSet::new();
        for (index, system) in document.systems.iter().enumerate() {
            let id = normalize_system_id(&system.id);
            if id.is_empty() || id != system.id {
                return Err(format!(
                    "system taxonomy id {:?} is not normalized",
                    system.id
                ));
            }
            if system.title.trim().is_empty() {
                return Err(format!("system taxonomy {id} has an empty title"));
            }
            if by_id.insert(id.clone(), index).is_some() {
                return Err(format!("duplicate system taxonomy id {id}"));
            }
            for alias in &system.aliases {
                let alias = normalize_system_id(alias);
                if alias.is_empty() || by_id.contains_key(&alias) || !aliases.insert(alias.clone())
                {
                    return Err(format!("duplicate system taxonomy alias {alias}"));
                }
            }
        }
        for (index, system) in document.systems.iter().enumerate() {
            for alias in &system.aliases {
                by_id.insert(normalize_system_id(alias), index);
            }
        }
        Ok(Self {
            systems: document.systems,
            by_id,
        })
    }

    fn definition(&self, id: &str) -> Option<&SystemDefinition> {
        self.by_id
            .get(&normalize_system_id(id))
            .and_then(|index| self.systems.get(*index))
    }
}

static SYSTEM_TAXONOMY: OnceLock<Result<SystemTaxonomy, String>> = OnceLock::new();

fn taxonomy() -> Result<&'static SystemTaxonomy, String> {
    match SYSTEM_TAXONOMY.get_or_init(|| SystemTaxonomy::parse(SYSTEM_TAXONOMY_JSON)) {
        Ok(taxonomy) => Ok(taxonomy),
        Err(error) => Err(error.clone()),
    }
}

pub fn validate_system_taxonomy() -> Result<(), String> {
    taxonomy().map(|_| ())
}

pub fn system_definition(system_id: &str) -> Option<&'static SystemDefinition> {
    taxonomy().ok()?.definition(system_id)
}

pub fn system_definitions() -> Result<&'static [SystemDefinition], String> {
    taxonomy().map(|taxonomy| taxonomy.systems.as_slice())
}

pub fn platform_kind_for_system(system_id: &str) -> PlatformKind {
    if matches!(system_id, "menu:arcade" | "menu:snk-arcade") {
        return PlatformKind::Arcade;
    }
    system_definition(system_id)
        .map(|definition| definition.platform_kind)
        .unwrap_or(PlatformKind::Unknown)
}

/// Resolve product classification from the canonical taxonomy. Observed category
/// is deliberately diagnostic-only: a core path, manifest row, or discovery can
/// never override the taxonomy attached to the associated system id.
pub fn classify_system(
    system_id: &str,
    observed_category: Option<&str>,
    observed_source: &str,
) -> SystemClassificationResolution {
    let normalized_id = normalize_system_id(system_id);
    let (platform_kind, source) = match system_definition(&normalized_id) {
        Some(definition) => (definition.platform_kind, "system-taxonomy-v1"),
        None => (PlatformKind::Unknown, "runtime-unknown-fallback"),
    };
    let classification = SystemClassification {
        system_id: normalized_id.clone(),
        platform_kind,
        source: source.to_string(),
    };
    let diagnostic = observed_category.and_then(|category| {
        let rejected_kind = PlatformKind::from_category(category);
        (rejected_kind != PlatformKind::Unknown && rejected_kind != platform_kind).then(|| {
            SystemClassificationDiagnostic {
                system_id: normalized_id,
                accepted_kind: platform_kind,
                accepted_source: source.to_string(),
                rejected_kind,
                rejected_source: observed_source.to_string(),
                reason: "physical or legacy category disagrees with canonical system taxonomy"
                    .to_string(),
            }
        })
    });
    SystemClassificationResolution {
        classification,
        diagnostic,
    }
}

pub fn system_title(system_id: &str) -> String {
    system_definition(system_id)
        .map(|definition| definition.title.clone())
        .unwrap_or_else(|| fallback_title(system_id))
}

pub fn normalize_system_id(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('_', "-")
}

fn fallback_title(id: &str) -> String {
    id.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checked_in_taxonomy_is_valid_and_unique() {
        validate_system_taxonomy().expect("valid taxonomy");
        let systems = system_definitions().expect("taxonomy systems");
        let ids = systems
            .iter()
            .map(|system| &system.id)
            .collect::<HashSet<_>>();
        assert_eq!(ids.len(), systems.len());
    }

    #[test]
    fn critical_systems_have_explicit_product_classification() {
        for (id, kind, section, family) in [
            (
                "sms",
                PlatformKind::Console,
                LauncherSection::Consoles,
                "sega",
            ),
            (
                "gamegear",
                PlatformKind::Handheld,
                LauncherSection::Handhelds,
                "sega",
            ),
            (
                "astrocade",
                PlatformKind::Console,
                LauncherSection::Consoles,
                "other",
            ),
        ] {
            let definition = system_definition(id).unwrap_or_else(|| panic!("missing {id}"));
            assert_eq!(definition.platform_kind, kind, "{id}");
            assert_eq!(definition.section, section, "{id}");
            assert_eq!(definition.family, family, "{id}");
        }
    }

    #[test]
    fn invalid_stored_platform_kinds_are_rejected() {
        assert_eq!(
            PlatformKind::from_stored("console"),
            Ok(PlatformKind::Console)
        );
        assert!(PlatformKind::from_stored("Arcade").is_err());
        assert!(PlatformKind::from_stored("cabinet").is_err());
    }

    #[test]
    fn physical_arcade_location_cannot_reclassify_console_or_handheld_systems() {
        for (id, expected) in [
            ("sms", PlatformKind::Console),
            ("gamegear", PlatformKind::Handheld),
            ("astrocade", PlatformKind::Console),
        ] {
            let resolution = classify_system(id, Some("Arcade"), "core-location:_Arcade/cores");
            assert_eq!(resolution.classification.platform_kind, expected, "{id}");
            assert!(resolution.diagnostic.is_some(), "{id}");
        }
    }
}
