// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::Deserialize;
use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::PathBuf;

#[derive(Deserialize)]
struct Schema {
    format: String,
    manifest_format: String,
    file_name: String,
    latch_protocol_version: String,
    latch_capability_mask: String,
    fields: Vec<String>,
    layouts: BTreeMap<String, Layout>,
}

#[derive(Deserialize)]
struct Layout {
    root: String,
    main: String,
    gui: String,
    manager: String,
    scanout_module: String,
    scanout_metadata: String,
    latch_rbf: String,
    latch_metadata: String,
}

fn main() {
    let manifest_dir = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let schema_path = manifest_dir.join("../platform-v3.schema.toml");
    println!("cargo:rerun-if-changed={}", schema_path.display());
    let schema_text = fs::read_to_string(&schema_path).expect("read platform-v3 schema");
    let schema: Schema = toml::from_str(&schema_text).expect("parse platform-v3 schema");
    assert_eq!(schema.format, "mister-magik-platform-v3-schema-v1");
    let public = schema.layouts.get("public").expect("public layout");
    let development = schema.layouts.get("dev").expect("development layout");
    let generated = format!(
        "// @generated from mister/platform/contracts/platform-v3.schema.toml\n\
         pub const FORMAT: &str = {format:?};\n\
         pub const FILE_NAME: &str = {file_name:?};\n\
         pub const LATCH_PROTOCOL_VERSION: &str = {latch_version:?};\n\
         pub const LATCH_CAPABILITY_MASK: &str = {latch_mask:?};\n\
         pub const FIELDS: &[&str] = &{fields:?};\n\
         pub const PUBLIC_PATHS: InstalledPaths = {public};\n\
         pub const DEVELOPMENT_PATHS: InstalledPaths = {development};\n",
        format = schema.manifest_format,
        file_name = schema.file_name,
        latch_version = schema.latch_protocol_version,
        latch_mask = schema.latch_capability_mask,
        fields = schema.fields,
        public = rust_layout(public, &schema.file_name),
        development = rust_layout(development, &schema.file_name),
    );
    let output = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("generated.rs");
    fs::write(output, generated).expect("write generated platform-v3 constants");
}

fn rust_layout(layout: &Layout, file_name: &str) -> String {
    let manifest = format!("{}/{file_name}", layout.root);
    format!(
        "InstalledPaths {{ root: {root:?}, manifest: {manifest:?}, main: {main:?}, gui: {gui:?}, manager: {manager:?}, scanout_module: {module:?}, scanout_metadata: {module_metadata:?}, latch_rbf: {rbf:?}, latch_metadata: {rbf_metadata:?} }}",
        root = layout.root,
        manifest = manifest,
        main = layout.main,
        gui = layout.gui,
        manager = layout.manager,
        module = layout.scanout_module,
        module_metadata = layout.scanout_metadata,
        rbf = layout.latch_rbf,
        rbf_metadata = layout.latch_metadata,
    )
}
