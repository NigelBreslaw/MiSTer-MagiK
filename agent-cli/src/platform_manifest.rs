// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::error::{AgentError, AgentResult};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

const FORMAT: &str = "mister-magik-platform-v2";
const FIELDS: &[&str] = &[
    "format",
    "main_path",
    "gui_path",
    "manager_path",
    "scanout_module_path",
    "scanout_metadata_path",
    "latch_rbf_path",
    "latch_metadata_path",
    "main_sha256",
    "gui_sha256",
    "manager_sha256",
    "scanout_module_sha256",
    "scanout_metadata_sha256",
    "latch_rbf_sha256",
    "latch_metadata_sha256",
    "platform_contract_sha256",
    "main_revision",
    "magik_revision",
    "menu_revision",
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Layout {
    Public,
    Development,
}

impl Layout {
    pub fn parse(value: &str) -> AgentResult<Self> {
        match value {
            "public" => Ok(Self::Public),
            "dev" => Ok(Self::Development),
            _ => Err(AgentError::Classified {
                code: "invalid_platform_layout",
                detail: value.into(),
            }),
        }
    }

    fn paths(self) -> [(&'static str, &'static str); 7] {
        let main = match self {
            Self::Public => "/media/fat/MiSTer_MagiK",
            Self::Development => "/media/fat/MiSTer_MagiKDev",
        };
        [
            ("main", main),
            (
                "gui",
                if self == Self::Public {
                    "/media/fat/mister-magik/mister-magik-fb"
                } else {
                    "/media/fat/mister-magik-dev/mister-magik-fb"
                },
            ),
            (
                "manager",
                if self == Self::Public {
                    "/media/fat/mister-magik/mister-magik-manager"
                } else {
                    "/media/fat/mister-magik-dev/mister-magik-manager"
                },
            ),
            (
                "scanout_module",
                if self == Self::Public {
                    "/media/fat/mister-magik/mister_magik_scanout_slots.ko"
                } else {
                    "/media/fat/mister-magik-dev/mister_magik_scanout_slots.ko"
                },
            ),
            (
                "scanout_metadata",
                if self == Self::Public {
                    "/media/fat/mister-magik/mister_magik_scanout_slots.metadata.txt"
                } else {
                    "/media/fat/mister-magik-dev/mister_magik_scanout_slots.metadata.txt"
                },
            ),
            (
                "latch_rbf",
                if self == Self::Public {
                    "/media/fat/mister-magik/fpga/menu-magik-vblank-latch.rbf"
                } else {
                    "/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.rbf"
                },
            ),
            (
                "latch_metadata",
                if self == Self::Public {
                    "/media/fat/mister-magik/fpga/menu-magik-vblank-latch.metadata.txt"
                } else {
                    "/media/fat/mister-magik-dev/fpga/menu-magik-vblank-latch.metadata.txt"
                },
            ),
        ]
    }
}

pub struct Artifacts {
    pub main: PathBuf,
    pub gui: PathBuf,
    pub manager: PathBuf,
    pub scanout_module: PathBuf,
    pub scanout_metadata: PathBuf,
    pub latch_rbf: PathBuf,
    pub latch_metadata: PathBuf,
}

impl Artifacts {
    fn values(&self) -> [(&'static str, &Path); 7] {
        [
            ("main", &self.main),
            ("gui", &self.gui),
            ("manager", &self.manager),
            ("scanout_module", &self.scanout_module),
            ("scanout_metadata", &self.scanout_metadata),
            ("latch_rbf", &self.latch_rbf),
            ("latch_metadata", &self.latch_metadata),
        ]
    }
}

pub fn generate(
    output: &Path,
    artifacts: &Artifacts,
    main_revision: &str,
    magik_revision: &str,
    layout: Layout,
) -> AgentResult<()> {
    require_hex("main_revision", main_revision, 40)?;
    require_hex("magik_revision", magik_revision, 40)?;
    for (name, path) in artifacts.values() {
        if !path.is_file() {
            return classified(
                "platform_artifact_missing",
                format!("{name}: {}", path.display()),
            );
        }
    }
    let (contract, menu_revision) = validate_metadata(artifacts)?;
    let mut values: BTreeMap<String, String> = BTreeMap::new();
    values.insert("format".into(), FORMAT.to_owned());
    for (name, path) in layout.paths() {
        values.insert(format!("{name}_path"), path.to_owned());
    }
    for (name, path) in artifacts.values() {
        values.insert(format!("{name}_sha256"), digest(path)?);
    }
    values.insert("platform_contract_sha256".into(), contract);
    values.insert("main_revision".into(), main_revision.into());
    values.insert("magik_revision".into(), magik_revision.into());
    values.insert("menu_revision".into(), menu_revision);
    let text = FIELDS
        .iter()
        .map(|field| format!("{field}={}\n", values[*field]))
        .collect::<String>();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(output, text)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    verify(output, None, layout)
}

pub fn update_runtime(
    output: &Path,
    installed: &str,
    gui: &Path,
    magik_revision: &str,
) -> AgentResult<()> {
    require_hex("magik_revision", magik_revision, 40)?;
    let mut values = BTreeMap::new();
    for line in installed.lines() {
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("invalid platform manifest line: {line}"))?;
        if values.insert(key.to_owned(), value.to_owned()).is_some() {
            return classified("duplicate_platform_manifest_field", key);
        }
    }
    if values.len() != FIELDS.len() || FIELDS.iter().any(|field| !values.contains_key(*field)) {
        return classified(
            "platform_manifest_fields",
            "installed manifest shape is not canonical",
        );
    }
    if values["format"] != FORMAT {
        return classified("unsupported_platform_manifest", values["format"].clone());
    }
    for name in [
        "main",
        "gui",
        "manager",
        "scanout_module",
        "scanout_metadata",
        "latch_rbf",
        "latch_metadata",
        "platform_contract",
    ] {
        require_hex(
            &format!("{name}_sha256"),
            &values[&format!("{name}_sha256")],
            64,
        )?;
    }
    for name in ["main_revision", "magik_revision", "menu_revision"] {
        require_hex(name, &values[name], 40)?;
    }
    values.insert("gui_sha256".into(), digest(gui)?);
    values.insert("magik_revision".into(), magik_revision.into());
    let text = FIELDS
        .iter()
        .map(|field| format!("{field}={}\n", values[*field]))
        .collect::<String>();
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    fs::write(output, text)
        .map_err(|error| format!("cannot write {}: {error}", output.display()))?;
    verify(output, None, Layout::Development)
}

pub fn verify(manifest: &Path, artifact_root: Option<&Path>, layout: Layout) -> AgentResult<()> {
    let fields = parse_fields(manifest, Some(FIELDS))?;
    if fields["format"] != FORMAT {
        return classified("unsupported_platform_manifest", fields["format"].clone());
    }
    for (name, expected) in layout.paths() {
        if fields[&format!("{name}_path")] != expected {
            return classified("platform_path_mismatch", name);
        }
        require_hex(
            &format!("{name}_sha256"),
            &fields[&format!("{name}_sha256")],
            64,
        )?;
    }
    require_hex(
        "platform_contract_sha256",
        &fields["platform_contract_sha256"],
        64,
    )?;
    for name in ["main_revision", "magik_revision", "menu_revision"] {
        require_hex(name, &fields[name], 40)?;
    }
    let Some(root) = artifact_root else {
        return Ok(());
    };
    let paths = layout.paths();
    let artifact = |name: &str| -> AgentResult<PathBuf> {
        let device = paths.iter().find(|(field, _)| *field == name).unwrap().1;
        let relative = device
            .strip_prefix("/media/fat/")
            .ok_or("platform path is outside /media/fat")?;
        Ok(root.join(relative))
    };
    let artifacts = Artifacts {
        main: artifact("main")?,
        gui: artifact("gui")?,
        manager: artifact("manager")?,
        scanout_module: artifact("scanout_module")?,
        scanout_metadata: artifact("scanout_metadata")?,
        latch_rbf: artifact("latch_rbf")?,
        latch_metadata: artifact("latch_metadata")?,
    };
    for (name, path) in artifacts.values() {
        if digest(path)? != fields[&format!("{name}_sha256")] {
            return classified("installed_artifact_mismatch", name);
        }
    }
    let (contract, menu) = validate_metadata(&artifacts)?;
    if contract != fields["platform_contract_sha256"] || menu != fields["menu_revision"] {
        return classified(
            "platform_metadata_mismatch",
            "manifest does not match component metadata",
        );
    }
    Ok(())
}

fn validate_metadata(artifacts: &Artifacts) -> AgentResult<(String, String)> {
    let module = parse_fields(&artifacts.scanout_metadata, None)?;
    let latch = parse_fields(&artifacts.latch_metadata, None)?;
    if module.get("module_sha256") != Some(&digest(&artifacts.scanout_module)?) {
        return classified("scanout_metadata_mismatch", "module hash");
    }
    if latch.get("rbf_sha256") != Some(&digest(&artifacts.latch_rbf)?) {
        return classified("latch_metadata_mismatch", "RBF hash");
    }
    let contract = module
        .get("platform_contract_sha256")
        .ok_or("scanout metadata has no platform contract")?;
    if latch.get("platform_contract_sha256") != Some(contract) {
        return classified(
            "platform_contract_mismatch",
            "component metadata uses mixed contracts",
        );
    }
    require_hex("platform_contract_sha256", contract, 64)?;
    let menu = latch
        .get("source_commit")
        .ok_or("latch metadata has no source commit")?;
    require_hex("menu_revision", menu, 40)?;
    Ok((contract.clone(), menu.clone()))
}

fn parse_fields(path: &Path, exact: Option<&[&str]>) -> AgentResult<BTreeMap<String, String>> {
    let text = fs::read_to_string(path)
        .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut fields = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("{}:{}: malformed field", path.display(), index + 1))?;
        if key.is_empty() || value.is_empty() || fields.insert(key.into(), value.into()).is_some() {
            return classified(
                "invalid_platform_manifest",
                format!("{}:{}", path.display(), index + 1),
            );
        }
    }
    if let Some(exact) = exact {
        let actual: Vec<_> = fields.keys().map(String::as_str).collect();
        if !exact.iter().all(|field| fields.contains_key(*field)) || actual.len() != exact.len() {
            return classified("invalid_platform_manifest_fields", actual.join(","));
        }
    }
    Ok(fields)
}

fn digest(path: &Path) -> AgentResult<String> {
    let mut file =
        File::open(path).map_err(|error| format!("cannot open {}: {error}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn require_hex(name: &str, value: &str, length: usize) -> AgentResult<()> {
    if value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        classified("invalid_platform_identity", format!("{name}: {value}"))
    }
}

fn classified<T>(code: &'static str, detail: impl Into<String>) -> AgentResult<T> {
    Err(AgentError::Classified {
        code,
        detail: detail.into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_are_closed_and_use_canonical_paths() {
        assert_eq!(Layout::parse("public").unwrap(), Layout::Public);
        assert_eq!(Layout::parse("dev").unwrap(), Layout::Development);
        assert!(Layout::parse("custom").is_err());
        assert_eq!(Layout::Public.paths()[0].1, "/media/fat/MiSTer_MagiK");
    }

    #[test]
    fn identities_require_lowercase_hex() {
        assert!(require_hex("sha", &"a".repeat(40), 40).is_ok());
        assert!(require_hex("sha", &"A".repeat(40), 40).is_err());
        assert!(require_hex("sha", "abc", 40).is_err());
    }

    #[test]
    fn runtime_update_changes_only_gui_identity_and_magik_revision() {
        let root = std::env::temp_dir().join(format!("runtime-manifest-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let gui = root.join("mister-magik-fb");
        let output = root.join("platform-v2.manifest");
        fs::write(&gui, b"new-gui").unwrap();
        let paths = Layout::Development.paths();
        let mut values = BTreeMap::new();
        values.insert("format".to_owned(), FORMAT.to_owned());
        for (name, path) in paths {
            values.insert(format!("{name}_path"), path.into());
        }
        for name in [
            "main",
            "gui",
            "manager",
            "scanout_module",
            "scanout_metadata",
            "latch_rbf",
            "latch_metadata",
            "platform_contract",
        ] {
            values.insert(format!("{name}_sha256"), "a".repeat(64));
        }
        for name in ["main_revision", "magik_revision", "menu_revision"] {
            values.insert(name.to_owned(), "b".repeat(40));
        }
        let installed = FIELDS
            .iter()
            .map(|field| format!("{field}={}\n", values[*field]))
            .collect::<String>();

        update_runtime(&output, &installed, &gui, &"c".repeat(40)).unwrap();

        let updated = parse_fields(&output, Some(FIELDS)).unwrap();
        assert_eq!(updated["magik_revision"], "c".repeat(40));
        assert_eq!(updated["gui_sha256"], digest(&gui).unwrap());
        assert_eq!(updated["main_sha256"], "a".repeat(64));
        let _ = fs::remove_dir_all(root);
    }
}
