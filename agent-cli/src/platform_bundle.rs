// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::archive::{MemberLayout, read_zip};
use crate::error::{AgentError, AgentResult};
use crate::platform_manifest::{
    CURRENT_FPGA_SOURCE_STATUSES, LATCH_CAPABILITY_MASK, LATCH_PROTOCOL_VERSION,
    LEGACY_FPGA_SOURCE_STATUSES,
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

pub const FORMAT: &str = "mister-magik-platform-bundle-v0.2";
pub const MANIFEST: &str = "platform-bundle-v0.2.json";
const ASSEMBLY_REVISION: u64 = 1;
const ORIGIN: &str = "platform-component-origin-v1.json";
const COMPONENT_CHECKSUMS: &str = "platform-component-SHA256SUMS";
const LEGACY_SCHEMA14_RBF_SHA256: &str =
    "ef1920500c925d35b23808792f0930954446a6030b33d3e92c0f4feccd23106e";
const PATCHED_DIAGNOSTIC_ARCHITECTURE: &str = "scaler-off-domain-scheduler-terminal-v3";
const STOCK_DIAGNOSTIC_ARCHITECTURE: &str = "stock-uninstrumented-v1";

pub struct Create<'a> {
    pub main: &'a Path,
    pub fpga: &'a Path,
    pub scanout: &'a Path,
    pub main_id: &'a str,
    pub fpga_id: &'a str,
    pub kernel_id: &'a str,
    pub main_run_id: &'a str,
    pub fpga_run_id: &'a str,
    pub kernel_run_id: &'a str,
    pub main_head_sha: &'a str,
    pub fpga_head_sha: &'a str,
    pub kernel_head_sha: &'a str,
    pub release_version: u64,
    pub output: &'a Path,
    pub main_source: &'a str,
    pub fpga_source: &'a str,
    pub kernel_source: &'a str,
}

pub fn bundle_id(main: &str, fpga: &str, kernel: &str) -> AgentResult<String> {
    bundle_id_for_revision(main, fpga, kernel, ASSEMBLY_REVISION)
}

fn bundle_id_for_revision(
    main: &str,
    fpga: &str,
    kernel: &str,
    assembly_revision: u64,
) -> AgentResult<String> {
    for (name, value) in [("main", main), ("fpga", fpga), ("kernel", kernel)] {
        require_hex(name, value, 64)?;
    }
    if !matches!(assembly_revision, 0 | ASSEMBLY_REVISION) {
        return classified("platform_assembly_revision", assembly_revision.to_string());
    }
    let revision = if assembly_revision == 0 {
        String::new()
    } else {
        format!("assembly_revision={assembly_revision}\n")
    };
    Ok(digest_bytes(
        format!("format={FORMAT}\nmain={main}\nfpga={fpga}\nkernel={kernel}\n{revision}")
            .as_bytes(),
    ))
}

fn assembly_revision(payload: &Value) -> AgentResult<u64> {
    match payload.get("assembly_revision") {
        None => Ok(0),
        Some(value) if value.as_u64() == Some(ASSEMBLY_REVISION) => Ok(ASSEMBLY_REVISION),
        Some(value) => classified("platform_assembly_revision", value.to_string()),
    }
}

pub fn update_plan(
    current: Option<&Value>,
    current_version: u64,
    main: &str,
    fpga: &str,
    kernel: &str,
) -> AgentResult<Value> {
    let identity = bundle_id(main, fpga, kernel)?;
    let Some(current) = current else {
        if current_version != 0 {
            return classified(
                "platform_manifest_missing",
                "current version requires a manifest",
            );
        }
        return Ok(
            json!({"current_version":0,"next_version":1,"current_bundle_id":"","bundle_id":identity,"update_needed":true,"main_changed":true,"fpga_changed":true,"kernel_changed":true,"release_tag":"platform-v0.1"}),
        );
    };
    validate_manifest(current, Some(current_version))?;
    let old_main = current["main_input_sha256"].as_str().unwrap_or_default();
    let old_fpga = current["fpga_input_sha256"].as_str().unwrap_or_default();
    let old_kernel = current["kernel_input_sha256"].as_str().unwrap_or_default();
    let old_identity =
        bundle_id_for_revision(old_main, old_fpga, old_kernel, assembly_revision(current)?)?;
    if current["bundle_id"] != old_identity {
        return classified(
            "platform_bundle_identity",
            "current identity does not match components",
        );
    }
    Ok(
        json!({"current_version":current_version,"next_version":current_version+1,"current_bundle_id":old_identity,"bundle_id":identity,"update_needed":old_identity!=identity,"main_changed":old_main!=main,"fpga_changed":old_fpga!=fpga,"kernel_changed":old_kernel!=kernel,"release_tag":format!("platform-v0.{}",current_version+1)}),
    )
}

pub fn write_component_cache(
    component: &str,
    artifact: &Path,
    component_id: &str,
    run_id: &str,
    head_sha: &str,
) -> AgentResult<()> {
    validate_component_name(component)?;
    require_hex("component_id", component_id, 64)?;
    require_run_id(run_id)?;
    require_hex("head_sha", head_sha, 40)?;
    let branch = if component == "main" {
        "mister-magik"
    } else {
        "main"
    };
    let origin = json!({"format":"mister-magik-platform-component-origin-v1","component":component,"component_id":component_id,"workflow":"platform-bundle.yml","run_id":run_id,"head_sha":head_sha,"head_branch":branch});
    fs::write(
        artifact.join(ORIGIN),
        serde_json::to_string_pretty(&origin).unwrap() + "\n",
    )
    .map_err(|error| error.to_string())?;
    let mut text = String::new();
    for path in component_files(artifact)? {
        text.push_str(&format!(
            "{}  {}\n",
            digest(&path)?,
            relative(artifact, &path)?
        ));
    }
    fs::write(artifact.join(COMPONENT_CHECKSUMS), text).map_err(|error| error.to_string())?;
    Ok(())
}

pub fn verify_component(
    component: &str,
    artifact: &Path,
    component_id: &str,
    revision: Option<&str>,
) -> AgentResult<Value> {
    validate_component_name(component)?;
    require_hex("component_id", component_id, 64)?;
    let contract = match component {
        "main" => {
            verify_main(artifact, component_id, revision)?;
            None
        }
        "fpga" => Some(verify_fpga(artifact, component_id)?),
        "kernel" => Some(verify_kernel(artifact, component_id)?),
        _ => unreachable!(),
    };
    let origin = verify_component_cache(component, artifact, component_id)?;
    let mut result = Map::new();
    result.insert("component".into(), component.into());
    result.insert("component_id".into(), component_id.into());
    result.insert("origin".into(), origin);
    if let Some(contract) = contract {
        result.insert("platform_contract_sha256".into(), contract.into());
    }
    Ok(Value::Object(result))
}

pub fn compact_component(
    component: &str,
    artifact: &Path,
    output: &Path,
    component_id: &str,
) -> AgentResult<PathBuf> {
    if component != "fpga" {
        return classified("component_compaction_unsupported", component);
    }
    verify_component(component, artifact, component_id, None)?;
    if output.exists() {
        return classified("component_output_exists", output.display().to_string());
    }

    let result = (|| {
        fs::create_dir_all(output).map_err(|error| error.to_string())?;
        copy_component_file(artifact, output, Path::new("quartus-delta-signoff.tsv"))?;
        for flavour in ["stock", "patched"] {
            let root = artifact.join(flavour);
            let metadata_name =
                PathBuf::from(format!("{flavour}/menu-magik-vblank-latch.metadata.txt"));
            for name in [
                "menu-magik-vblank-latch.rbf",
                "menu-magik-vblank-latch.metadata.txt",
                "menu-magik-vblank-latch.build.log",
            ] {
                copy_component_file(
                    artifact,
                    output,
                    &PathBuf::from(format!("{flavour}/{name}")),
                )?;
            }
            for report in declared_reports(&root.join("menu-magik-vblank-latch.metadata.txt"))? {
                copy_component_file(artifact, output, &PathBuf::from(flavour).join(report))?;
            }
            if !output.join(metadata_name).is_file() {
                return classified("fpga_component_compaction", flavour);
            }
        }
        copy_component_file(artifact, output, Path::new(ORIGIN))?;
        let origin: Value = serde_json::from_slice(
            &fs::read(output.join(ORIGIN)).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        write_component_cache(
            component,
            output,
            component_id,
            origin["run_id"].as_str().unwrap_or_default(),
            origin["head_sha"].as_str().unwrap_or_default(),
        )?;
        verify_component(component, output, component_id, None)?;
        Ok(output.to_path_buf())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(output);
    }
    result
}

fn copy_component_file(root: &Path, output: &Path, relative: &Path) -> AgentResult<()> {
    if relative.is_absolute()
        || relative
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return classified("fpga_report_path", relative.display().to_string());
    }
    let source = root.join(relative);
    let destination = output.join(relative);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    fs::copy(&source, &destination).map_err(|error| error.to_string())?;
    Ok(())
}

fn declared_reports(metadata: &Path) -> AgentResult<Vec<PathBuf>> {
    let metadata = fields(metadata)?;
    let mut reports = Vec::new();
    for name in metadata
        .keys()
        .filter_map(|key| key.strip_prefix("report_sha256."))
    {
        let path = PathBuf::from(name);
        if path.is_absolute()
            || path
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
            || !name.starts_with("reports/")
        {
            return classified("fpga_report_path", name);
        }
        reports.push(path);
    }
    if reports.is_empty() {
        return classified("fpga_reports_missing", metadata.len().to_string());
    }
    reports.sort();
    Ok(reports)
}

pub fn create(request: &Create<'_>) -> AgentResult<PathBuf> {
    if request.release_version == 0 {
        return classified(
            "invalid_platform_release",
            "release version must be positive",
        );
    }
    for id in [request.main_id, request.fpga_id, request.kernel_id] {
        require_hex("component_id", id, 64)?;
    }
    verify_main(request.main, request.main_id, Some(request.main_head_sha))?;
    let fpga_contract = verify_fpga(request.fpga, request.fpga_id)?;
    let kernel_contract = verify_kernel(request.scanout, request.kernel_id)?;
    if fpga_contract != kernel_contract {
        return classified(
            "platform_contract_mismatch",
            "FPGA and kernel contracts differ",
        );
    }
    let fpga_metadata = fields(
        &request
            .fpga
            .join("patched/menu-magik-vblank-latch.metadata.txt"),
    )?;
    let fpga_architecture = diagnostic_architecture(&fpga_metadata, "patched")?;
    let mut files = Vec::new();
    for (prefix, root) in [
        ("main", request.main),
        ("fpga", request.fpga),
        ("scanout", request.scanout),
    ] {
        for path in all_files(root)? {
            let name = format!("{prefix}/{}", relative(root, &path)?);
            files.push((name, fs::read(path).map_err(|error| error.to_string())?));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let identity = bundle_id(request.main_id, request.fpga_id, request.kernel_id)?;
    let file_entries: Vec<_> = files
        .iter()
        .map(|(name, bytes)| json!({"path":name,"size":bytes.len(),"sha256":digest_bytes(bytes)}))
        .collect();
    let payload = json!({
        "format":FORMAT,"assembly_revision":ASSEMBLY_REVISION,"release_version":request.release_version,"bundle_id":identity,
        "main_input_sha256":request.main_id,"fpga_input_sha256":request.fpga_id,"kernel_input_sha256":request.kernel_id,
        "platform_contract_sha256":fpga_contract,
        "latch_protocol_sha256":fpga_metadata.get("latch_protocol_sha256").cloned().unwrap_or_default(),
        "latch_protocol_version":fpga_metadata.get("latch_protocol_version").and_then(|value|value.parse::<u64>().ok()).unwrap_or(0),
        "latch_rbf_sha256":fpga_metadata.get("rbf_sha256").cloned().unwrap_or_default(),
        "diagnostic_architecture":fpga_architecture,
        "components":{
            "main":origin("main",request.main_run_id,request.main_head_sha,"mister-magik",request.main_source),
            "fpga":origin("fpga",request.fpga_run_id,request.fpga_head_sha,"main",request.fpga_source),
            "kernel":origin("kernel",request.kernel_run_id,request.kernel_head_sha,"main",request.kernel_source)},
        "files":file_entries
    });
    validate_manifest(&payload, Some(request.release_version))?;
    let manifest =
        serde_json::to_string_pretty(&payload).map_err(|error| error.to_string())? + "\n";
    let mut checksums = files
        .iter()
        .map(|(name, bytes)| format!("{}  {name}\n", digest_bytes(bytes)))
        .collect::<String>();
    checksums.push_str(&format!(
        "{}  {MANIFEST}\n",
        digest_bytes(manifest.as_bytes())
    ));
    fs::create_dir_all(request.output).map_err(|error| error.to_string())?;
    let archive_path = request.output.join(format!(
        "mister-magik-platform-v0.{}.zip",
        request.release_version
    ));
    let mut archive =
        ZipWriter::new(File::create(&archive_path).map_err(|error| error.to_string())?);
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in files.iter().chain(
        [
            (MANIFEST.to_owned(), manifest.as_bytes().to_vec()),
            ("SHA256SUMS".into(), checksums.as_bytes().to_vec()),
        ]
        .iter(),
    ) {
        archive
            .start_file(name, options)
            .map_err(|error| error.to_string())?;
        archive
            .write_all(bytes)
            .map_err(|error| error.to_string())?;
    }
    archive.finish().map_err(|error| error.to_string())?;
    fs::write(request.output.join(MANIFEST), manifest).map_err(|error| error.to_string())?;
    fs::write(request.output.join("SHA256SUMS"), checksums).map_err(|error| error.to_string())?;
    verify(
        &archive_path,
        Some(&request.output.join(MANIFEST)),
        Some(request.release_version),
    )?;
    Ok(archive_path)
}

pub fn verify(
    archive: &Path,
    release_manifest: Option<&Path>,
    release_version: Option<u64>,
) -> AgentResult<Value> {
    let files = read_zip(archive, MemberLayout::Nested)?;
    let manifest_bytes = files
        .get(MANIFEST)
        .ok_or("platform bundle manifest is missing")?;
    let payload: Value =
        serde_json::from_slice(manifest_bytes).map_err(|error| error.to_string())?;
    validate_manifest(&payload, release_version)?;
    if release_manifest.is_some_and(|path| fs::read(path).ok().as_deref() != Some(manifest_bytes)) {
        return classified(
            "platform_release_manifest_mismatch",
            "release manifest differs from archive",
        );
    }
    let expected_id = bundle_id_for_revision(
        payload["main_input_sha256"].as_str().unwrap_or_default(),
        payload["fpga_input_sha256"].as_str().unwrap_or_default(),
        payload["kernel_input_sha256"].as_str().unwrap_or_default(),
        assembly_revision(&payload)?,
    )?;
    if payload["bundle_id"] != expected_id {
        return classified(
            "platform_bundle_identity",
            "bundle identity does not match components",
        );
    }
    let entries = payload["files"]
        .as_array()
        .ok_or("platform file manifest is missing")?;
    let actual: BTreeMap<_, _> = files
        .iter()
        .filter(|(name, _)| name.as_str() != MANIFEST && name.as_str() != "SHA256SUMS")
        .map(|(name, bytes)| (name.clone(), (bytes.len() as u64, digest_bytes(bytes))))
        .collect();
    if entries.len() != actual.len() {
        return classified("platform_file_manifest", "file count mismatch");
    }
    for entry in entries {
        let name = entry["path"].as_str().unwrap_or_default();
        if actual.get(name)
            != Some(&(
                entry["size"].as_u64().unwrap_or_default(),
                entry["sha256"].as_str().unwrap_or_default().to_owned(),
            ))
        {
            return classified("platform_file_manifest", name);
        }
    }
    verify_archive_checksums(&files)?;
    verify_embedded_components(&payload, &files)?;
    Ok(payload)
}

pub fn extract_component(
    archive: &Path,
    manifest: &Path,
    component: &str,
    component_id: &str,
    output: &Path,
) -> AgentResult<Value> {
    validate_component_name(component)?;
    require_hex("component_id", component_id, 64)?;
    let payload = verify(archive, Some(manifest), None)?;
    let (key, prefix) = match component {
        "main" => ("main_input_sha256", "main/"),
        "fpga" => ("fpga_input_sha256", "fpga/"),
        "kernel" => ("kernel_input_sha256", "scanout/"),
        _ => unreachable!(),
    };
    if payload[key] != component_id {
        return classified("component_identity_mismatch", component);
    }
    if output.exists() {
        return classified("component_output_exists", output.display().to_string());
    }
    fs::create_dir_all(output).map_err(|e| e.to_string())?;
    for (name, bytes) in read_zip(archive, MemberLayout::Nested)? {
        if let Some(relative) = name.strip_prefix(prefix) {
            let path = output.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| e.to_string())?;
            }
            fs::write(path, bytes).map_err(|e| e.to_string())?;
        }
    }
    let origin = payload
        .pointer(&format!("/components/{component}"))
        .cloned()
        .ok_or("component origin missing")?;
    Ok(
        json!({"component":component,"component_id":component_id,"run_id":origin["run_id"],"head_sha":origin["head_sha"],"workflow":origin["workflow"],"head_branch":origin["head_branch"],"release_version":payload["release_version"]}),
    )
}

fn verify_embedded_components(
    payload: &Value,
    files: &BTreeMap<String, Vec<u8>>,
) -> AgentResult<()> {
    let fpga = payload["fpga_input_sha256"].as_str().unwrap_or_default();
    let kernel = payload["kernel_input_sha256"].as_str().unwrap_or_default();
    let patched = archive_fields(files, "fpga/patched/menu-magik-vblank-latch.metadata.txt")?;
    let provenance = archive_fields(files, "scanout/provenance.txt")?;
    if patched.get("component_input_sha256").map(String::as_str) != Some(fpga)
        || provenance.get("component_input_sha256").map(String::as_str) != Some(kernel)
    {
        return classified("embedded_component_identity", "metadata mismatch");
    }
    let embedded_architecture = diagnostic_architecture(&patched, "patched")?;
    if payload["latch_rbf_sha256"] != LEGACY_SCHEMA14_RBF_SHA256
        && payload
            .get("diagnostic_architecture")
            .and_then(Value::as_str)
            != Some(embedded_architecture.as_str())
    {
        return classified("fpga_diagnostic_architecture", "bundle metadata mismatch");
    }
    if patched.get("platform_contract_sha256") != provenance.get("platform_contract_sha256")
        || payload["platform_contract_sha256"]
            != patched
                .get("platform_contract_sha256")
                .cloned()
                .unwrap_or_default()
    {
        return classified("platform_contract_mismatch", "embedded components");
    }
    Ok(())
}
fn validate_manifest(payload: &Value, version: Option<u64>) -> AgentResult<()> {
    if payload["format"] != FORMAT || payload["release_version"].as_u64().is_none_or(|v| v == 0) {
        return classified("invalid_platform_manifest", "format or version");
    }
    let _ = assembly_revision(payload)?;
    if version.is_some_and(|v| payload["release_version"] != v) {
        return classified("platform_release_version", "tag and manifest differ");
    }
    for name in ["main", "fpga", "kernel"] {
        require_hex(
            name,
            payload[format!("{name}_input_sha256")]
                .as_str()
                .unwrap_or_default(),
            64,
        )?;
        let origin = &payload["components"][name];
        require_hex(
            "head_sha",
            origin["head_sha"].as_str().unwrap_or_default(),
            40,
        )?;
        require_run_id(origin["run_id"].as_str().unwrap_or_default())?;
    }
    match payload
        .get("diagnostic_architecture")
        .and_then(Value::as_str)
    {
        Some(architecture)
            if architecture == PATCHED_DIAGNOSTIC_ARCHITECTURE
                || architecture == STOCK_DIAGNOSTIC_ARCHITECTURE => {}
        Some(architecture) => {
            return classified("fpga_diagnostic_architecture", architecture);
        }
        None if payload["latch_rbf_sha256"] == LEGACY_SCHEMA14_RBF_SHA256 => {}
        None => {
            return classified("fpga_diagnostic_architecture", "missing from new bundle");
        }
    }
    Ok(())
}
fn verify_main(root: &Path, id: &str, revision: Option<&str>) -> AgentResult<()> {
    let payload: Value = serde_json::from_slice(
        &fs::read(root.join("main-component-v0.1.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    if payload["component_id"] != id || revision.is_some_and(|v| payload["source_revision"] != v) {
        return classified("main_component_identity", "receipt mismatch");
    }
    let binary = root.join("MiSTer_MagiK");
    if payload.pointer("/binary/sha256").and_then(Value::as_str) != Some(&digest(&binary)?) {
        return classified("main_component_hash", "binary mismatch");
    }
    Ok(())
}

fn diagnostic_architecture(
    metadata: &BTreeMap<String, String>,
    flavour: &str,
) -> AgentResult<String> {
    let expected = match flavour {
        "patched" => PATCHED_DIAGNOSTIC_ARCHITECTURE,
        "stock" => STOCK_DIAGNOSTIC_ARCHITECTURE,
        _ => return classified("fpga_diagnostic_architecture", flavour),
    };
    if let Some(architecture) = metadata.get("diagnostic_architecture") {
        if architecture == expected {
            return Ok(architecture.clone());
        }
        return classified(
            "fpga_diagnostic_architecture",
            format!("{flavour}: unsupported architecture {architecture}"),
        );
    }
    if flavour == "patched"
        && metadata.get("rbf_sha256").map(String::as_str) == Some(LEGACY_SCHEMA14_RBF_SHA256)
    {
        return Ok(PATCHED_DIAGNOSTIC_ARCHITECTURE.into());
    }
    if flavour == "stock" && metadata.get("apply_patch").map(String::as_str) == Some("0") {
        return Ok(STOCK_DIAGNOSTIC_ARCHITECTURE.into());
    }
    classified(
        "fpga_diagnostic_architecture",
        format!("{flavour}: missing architecture metadata"),
    )
}

fn verify_fpga(root: &Path, id: &str) -> AgentResult<String> {
    let mut contract = None;
    for flavour in ["stock", "patched"] {
        let directory = root.join(flavour);
        let metadata = fields(&directory.join("menu-magik-vblank-latch.metadata.txt"))?;
        if metadata.get("component_input_sha256").map(String::as_str) != Some(id)
            || metadata.get("rbf_sha256")
                != Some(&digest(&directory.join("menu-magik-vblank-latch.rbf"))?)
        {
            return classified("fpga_component_identity", flavour);
        }
        if metadata.get("format").map(String::as_str) != Some("mister-magik-fpga-release-v2") {
            return classified("fpga_metadata_format", "release v2 required");
        }
        if metadata.get("latch_protocol_version").map(String::as_str)
            != Some(LATCH_PROTOCOL_VERSION)
        {
            return classified(
                "fpga_protocol",
                format!("version {LATCH_PROTOCOL_VERSION} required"),
            );
        }
        if metadata.get("latch_capability_mask").map(String::as_str) != Some(LATCH_CAPABILITY_MASK)
        {
            return classified(
                "fpga_capabilities",
                format!("{LATCH_CAPABILITY_MASK} required"),
            );
        }
        let _ = diagnostic_architecture(&metadata, flavour)?;
        for field in ["latch_protocol_sha256", "latch_bridge_sha256"] {
            require_hex(
                field,
                metadata.get(field).map(String::as_str).unwrap_or_default(),
                64,
            )?;
        }
        for report in declared_reports(&directory.join("menu-magik-vblank-latch.metadata.txt"))? {
            let key = format!("report_sha256.{}", report.to_string_lossy());
            if metadata.get(&key) != Some(&digest(&directory.join(&report))?) {
                return classified(
                    "fpga_report_hash",
                    format!("{flavour}/{}", report.display()),
                );
            }
        }
        match &contract {
            None => contract = metadata.get("platform_contract_sha256").cloned(),
            Some(value) if metadata.get("platform_contract_sha256") != Some(value) => {
                return classified("platform_contract_mismatch", "stock/patched FPGA");
            }
            _ => {}
        }
    }
    contract.ok_or_else(|| AgentError::Classified {
        code: "fpga_contract_missing",
        detail: "metadata".into(),
    })
}
fn verify_kernel(root: &Path, id: &str) -> AgentResult<String> {
    let metadata = fields(&root.join("provenance.txt"))?;
    if metadata.get("component_input_sha256").map(String::as_str) != Some(id)
        || metadata.get("module_sha256")
            != Some(&digest(&root.join("mister_magik_scanout_slots.ko"))?)
    {
        return classified("kernel_component_identity", "provenance mismatch");
    }
    verify_checksum_file(root, &root.join("SHA256SUMS"))?;
    metadata
        .get("platform_contract_sha256")
        .cloned()
        .ok_or_else(|| AgentError::Classified {
            code: "kernel_contract_missing",
            detail: "provenance".into(),
        })
}
fn verify_component_cache(component: &str, root: &Path, id: &str) -> AgentResult<Value> {
    let payload: Value =
        serde_json::from_slice(&fs::read(root.join(ORIGIN)).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let branch = if component == "main" {
        "mister-magik"
    } else {
        "main"
    };
    if payload["format"] != "mister-magik-platform-component-origin-v1"
        || payload["component"] != component
        || payload["component_id"] != id
        || payload["workflow"] != "platform-bundle.yml"
        || payload["head_branch"] != branch
    {
        return classified("component_origin", "invalid immutable origin");
    }
    require_run_id(payload["run_id"].as_str().unwrap_or_default())?;
    require_hex(
        "head_sha",
        payload["head_sha"].as_str().unwrap_or_default(),
        40,
    )?;
    verify_checksum_file(root, &root.join(COMPONENT_CHECKSUMS))?;
    Ok(payload)
}
fn verify_checksum_file(root: &Path, path: &Path) -> AgentResult<()> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    for line in text.lines() {
        let (hash, name) = line.split_once("  ").ok_or("malformed checksum")?;
        if digest(&root.join(name))? != hash {
            return classified("component_checksum", name);
        }
    }
    Ok(())
}
fn verify_archive_checksums(files: &BTreeMap<String, Vec<u8>>) -> AgentResult<()> {
    let text = std::str::from_utf8(files.get("SHA256SUMS").ok_or("checksums missing")?)
        .map_err(|e| e.to_string())?;
    let mut seen = BTreeSet::new();
    for line in text.lines() {
        let (hash, name) = line.split_once("  ").ok_or("malformed checksums")?;
        if files
            .get(name)
            .is_none_or(|bytes| digest_bytes(bytes) != hash)
            || !seen.insert(name)
        {
            return classified("platform_checksum", name);
        }
    }
    let unchecked: Vec<_> = files
        .keys()
        .filter(|name| name.as_str() != "SHA256SUMS" && !seen.contains(name.as_str()))
        .collect();
    if unchecked.iter().any(|name| !name.ends_with("/SHA256SUMS")) {
        return classified("platform_checksum_shape", "incomplete checksum set");
    }
    Ok(())
}
fn all_files(root: &Path) -> AgentResult<Vec<PathBuf>> {
    let mut output = Vec::new();
    collect(root, &mut output)?;
    output.sort();
    Ok(output)
}
fn collect(path: &Path, out: &mut Vec<PathBuf>) -> AgentResult<()> {
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let path = entry.map_err(|e| e.to_string())?.path();
        if path.is_dir() {
            collect(&path, out)?;
        } else {
            out.push(path);
        }
    }
    Ok(())
}
fn component_files(root: &Path) -> AgentResult<Vec<PathBuf>> {
    Ok(all_files(root)?
        .into_iter()
        .filter(|path| {
            path.file_name().and_then(|n| n.to_str()) != Some(COMPONENT_CHECKSUMS)
                && !Path::new(&relative(root, path).unwrap_or_default())
                    .components()
                    .any(|part| part.as_os_str().to_string_lossy().starts_with('.'))
        })
        .collect())
}
fn relative(root: &Path, path: &Path) -> AgentResult<String> {
    Ok(path
        .strip_prefix(root)
        .map_err(|e| e.to_string())
        .map(|p| p.to_string_lossy().replace('\\', "/"))?)
}
fn fields(path: &Path) -> AgentResult<BTreeMap<String, String>> {
    let text = fs::read_to_string(path).map_err(|e| e.to_string())?;
    parse_fields(&text)
}
fn archive_fields(
    files: &BTreeMap<String, Vec<u8>>,
    name: &str,
) -> AgentResult<BTreeMap<String, String>> {
    parse_fields(
        std::str::from_utf8(files.get(name).ok_or("metadata missing")?)
            .map_err(|e| e.to_string())?,
    )
}
fn parse_fields(text: &str) -> AgentResult<BTreeMap<String, String>> {
    let mut result = BTreeMap::new();
    let mut source_statuses = Vec::new();
    for line in text.lines() {
        let (k, v) = line.split_once('=').ok_or("malformed metadata")?;
        if k == "source_status" {
            source_statuses.push(v);
            continue;
        }
        if k.is_empty() || v.is_empty() || result.insert(k.into(), v.into()).is_some() {
            return classified("malformed_metadata", line);
        }
    }
    if !source_statuses.is_empty()
        && source_statuses != LEGACY_FPGA_SOURCE_STATUSES
        && source_statuses != CURRENT_FPGA_SOURCE_STATUSES
    {
        return classified("malformed_metadata", source_statuses.join(","));
    }
    Ok(result)
}
fn origin(component: &str, run: &str, sha: &str, branch: &str, source: &str) -> Value {
    json!({"workflow":"platform-bundle.yml","run_id":run,"head_sha":sha,"head_branch":branch,"source":source,"component":component})
}
fn validate_component_name(value: &str) -> AgentResult<()> {
    if matches!(value, "main" | "fpga" | "kernel") {
        Ok(())
    } else {
        classified("invalid_platform_component", value)
    }
}
fn require_run_id(value: &str) -> AgentResult<()> {
    if value.parse::<u64>().is_ok_and(|v| v > 0) {
        Ok(())
    } else {
        classified("invalid_platform_run_id", value)
    }
}
fn require_hex(name: &str, value: &str, length: usize) -> AgentResult<()> {
    if value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
    {
        Ok(())
    } else {
        classified("invalid_platform_identity", format!("{name}: {value}"))
    }
}
fn digest(path: &Path) -> AgentResult<String> {
    Ok(digest_bytes(&fs::read(path).map_err(|e| e.to_string())?))
}
fn digest_bytes(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn checksum_fixture(
        entries: &[(&str, &[u8])],
        checksummed: &[&str],
    ) -> BTreeMap<String, Vec<u8>> {
        let mut files: BTreeMap<_, _> = entries
            .iter()
            .map(|(name, bytes)| ((*name).to_owned(), bytes.to_vec()))
            .collect();
        let checksums = checksummed
            .iter()
            .map(|name| format!("{}  {name}\n", digest_bytes(&files[*name])))
            .collect::<String>();
        files.insert("SHA256SUMS".to_owned(), checksums.into_bytes());
        files
    }

    fn temp_root(label: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "agent-cli-platform-{label}-{}-{nonce}",
            std::process::id()
        ))
    }

    fn fpga_component(root: &Path, component_id: &str) {
        let contract = "d".repeat(64);
        fs::create_dir_all(root).unwrap();
        fs::write(root.join("quartus-delta-signoff.tsv"), "valid=1\n").unwrap();
        for flavour in ["stock", "patched"] {
            let directory = root.join(flavour);
            fs::create_dir_all(directory.join("reports")).unwrap();
            let rbf = format!("{flavour}-rbf");
            let report = format!("{flavour}-report");
            fs::write(directory.join("menu-magik-vblank-latch.rbf"), &rbf).unwrap();
            fs::write(
                directory.join("menu-magik-vblank-latch.build.log"),
                format!("{flavour}-log"),
            )
            .unwrap();
            fs::write(directory.join("reports/menu.fit.rpt"), &report).unwrap();
            fs::write(
                directory.join("menu-magik-vblank-latch.metadata.txt"),
                format!(
                    "format=mister-magik-fpga-release-v2\nsource_status= M menu.qsf\nsource_status= M sys/sys_top.sdc\ncomponent_input_sha256={component_id}\nplatform_contract_sha256={contract}\nlatch_protocol_sha256={}\nlatch_bridge_sha256={}\nlatch_protocol_version={LATCH_PROTOCOL_VERSION}\nlatch_capability_mask={LATCH_CAPABILITY_MASK}\ndiagnostic_architecture={}\nrbf_sha256={}\nreport_sha256.reports/menu.fit.rpt={}\n",
                    "1".repeat(64),
                    "2".repeat(64),
                    if flavour == "patched" {
                        PATCHED_DIAGNOSTIC_ARCHITECTURE
                    } else {
                        STOCK_DIAGNOSTIC_ARCHITECTURE
                    },
                    digest_bytes(rbf.as_bytes()),
                    digest_bytes(report.as_bytes())
                ),
            )
            .unwrap();
        }
        write_component_cache("fpga", root, component_id, "123", &"e".repeat(40)).unwrap();
    }

    #[test]
    fn identity_is_stable() {
        let main = "a".repeat(64);
        let fpga = "b".repeat(64);
        let kernel = "c".repeat(64);
        let id = bundle_id(&main, &fpga, &kernel).unwrap();
        assert_eq!(id.len(), 64);
        assert_eq!(id, bundle_id(&main, &fpga, &kernel).unwrap());
        assert_ne!(
            id,
            bundle_id_for_revision(&main, &fpga, &kernel, 0).unwrap()
        );
    }

    #[test]
    fn metadata_accepts_only_audited_repeated_source_statuses() {
        assert!(
            parse_fields(
                "format=current\nsource_status= M menu.qsf\nsource_status= M sys/sys_top.sdc\n"
            )
            .is_ok()
        );
        assert!(parse_fields("format=legacy\nsource_status= M sys/sys_top.sdc\n").is_ok());
        for malformed in [
            "format=missing-sdc\nsource_status= M menu.qsf\n",
            "format=duplicate\nsource_status= M menu.qsf\nsource_status= M menu.qsf\n",
            "format=unexpected\nsource_status= M sys/sys_top.v\n",
        ] {
            assert!(matches!(
                parse_fields(malformed),
                Err(AgentError::Classified {
                    code: "malformed_metadata",
                    ..
                })
            ));
        }
    }
    #[test]
    fn new_plan_is_closed() {
        let value =
            update_plan(None, 0, &"a".repeat(64), &"b".repeat(64), &"c".repeat(64)).unwrap();
        assert_eq!(value["next_version"], 1);
        assert_eq!(value["update_needed"], true);
    }

    #[test]
    fn legacy_component_checksum_files_may_be_absent_from_root_checksums() {
        let files = checksum_fixture(
            &[
                ("main/MiSTer_MagiK", b"binary"),
                ("main/SHA256SUMS", b"nested"),
            ],
            &["main/MiSTer_MagiK"],
        );
        verify_archive_checksums(&files).unwrap();
    }

    #[test]
    fn root_checksums_must_cover_other_archive_files() {
        let files = checksum_fixture(
            &[
                ("main/MiSTer_MagiK", b"binary"),
                ("main/provenance.txt", b"origin"),
            ],
            &["main/MiSTer_MagiK"],
        );
        assert!(matches!(
            verify_archive_checksums(&files),
            Err(AgentError::Classified {
                code: "platform_checksum_shape",
                ..
            })
        ));
    }

    #[test]
    fn compaction_removes_quartus_workspaces_and_preserves_identity() {
        let root = temp_root("compact");
        let source = root.join("source");
        let output = root.join("output");
        let component_id = "a".repeat(64);
        fpga_component(&source, &component_id);
        let workspace = source.join("patched/Menu-work/db");
        fs::create_dir_all(&workspace).unwrap();
        let large = File::create(workspace.join("quartus-state.bin")).unwrap();
        large.set_len(400 * 1024 * 1024).unwrap();

        compact_component("fpga", &source, &output, &component_id).unwrap();

        assert!(!output.join("patched/Menu-work").exists());
        assert!(!output.join("stock/Menu-work").exists());
        assert!(output.join("patched/reports/menu.fit.rpt").is_file());
        let verified = verify_component("fpga", &output, &component_id, None).unwrap();
        assert_eq!(verified["origin"]["run_id"], "123");
        assert_eq!(verified["component_id"], component_id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fpga_verification_rejects_tampered_declared_report() {
        let root = temp_root("report-hash");
        let component_id = "a".repeat(64);
        fpga_component(&root, &component_id);
        fs::write(root.join("patched/reports/menu.fit.rpt"), "tampered").unwrap();
        assert!(matches!(
            verify_component("fpga", &root, &component_id, None),
            Err(AgentError::Classified {
                code: "fpga_report_hash",
                ..
            })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fpga_verification_requires_exact_v5_identity() {
        for (label, original, replacement, expected_code) in [
            (
                "protocol",
                format!("latch_protocol_version={LATCH_PROTOCOL_VERSION}"),
                "latch_protocol_version=3".to_owned(),
                "fpga_protocol",
            ),
            (
                "capabilities",
                format!("latch_capability_mask={LATCH_CAPABILITY_MASK}"),
                "latch_capability_mask=0x01fe".to_owned(),
                "fpga_capabilities",
            ),
        ] {
            let root = temp_root(label);
            let component_id = "a".repeat(64);
            fpga_component(&root, &component_id);
            let metadata = root.join("patched/menu-magik-vblank-latch.metadata.txt");
            let source = fs::read_to_string(&metadata).unwrap();
            fs::write(&metadata, source.replace(&original, &replacement)).unwrap();
            match verify_component("fpga", &root, &component_id, None) {
                Err(AgentError::Classified { code, .. }) => {
                    assert_eq!(code, expected_code);
                }
                result => panic!("unexpected verification result: {result:?}"),
            }
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn compaction_rejects_existing_output_and_non_fpga_components() {
        let root = temp_root("compact-errors");
        let source = root.join("source");
        let output = root.join("output");
        let component_id = "a".repeat(64);
        fpga_component(&source, &component_id);
        fs::create_dir_all(&output).unwrap();
        assert!(matches!(
            compact_component("fpga", &source, &output, &component_id),
            Err(AgentError::Classified {
                code: "component_output_exists",
                ..
            })
        ));
        assert!(matches!(
            compact_component("kernel", &root, &root.join("out"), &"a".repeat(64)),
            Err(AgentError::Classified {
                code: "component_compaction_unsupported",
                ..
            })
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn compaction_rejects_missing_evidence_and_unsafe_report_paths() {
        let root = temp_root("compact-evidence");
        let source = root.join("source");
        let component_id = "a".repeat(64);
        fpga_component(&source, &component_id);
        fs::remove_file(source.join("stock/menu-magik-vblank-latch.build.log")).unwrap();
        assert!(
            compact_component("fpga", &source, &root.join("missing-output"), &component_id)
                .is_err()
        );
        assert!(!root.join("missing-output").exists());

        let metadata = source.join("patched/menu-magik-vblank-latch.metadata.txt");
        let mut text = fs::read_to_string(&metadata).unwrap();
        text.push_str(&format!(
            "report_sha256.reports/../escape={}\n",
            "f".repeat(64)
        ));
        fs::write(metadata, text).unwrap();
        assert!(matches!(
            verify_fpga(&source, &component_id),
            Err(AgentError::Classified {
                code: "fpga_report_path",
                ..
            })
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
