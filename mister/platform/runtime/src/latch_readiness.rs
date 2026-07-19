// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stable latch-readiness vocabulary shared by runtime policy and diagnostics.

use serde::{Deserialize, Serialize};
use std::fs::{create_dir_all, rename, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub const REPORT_PATH: &str = "/tmp/mister-magik/latch-readiness.json";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatchReadinessState {
    Ready,
    InstallationFault,
    PlatformIncompatible,
    RuntimeFault,
}

impl LatchReadinessState {
    pub const fn code(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::InstallationFault => "installation-fault",
            Self::PlatformIncompatible => "platform-incompatible",
            Self::RuntimeFault => "runtime-fault",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatchFailureStage {
    FrontendIntegrity,
    Manifest,
    Kernel,
    ModuleOpen,
    ModuleLayout,
    BufferMap,
    FpgaStatus,
    FpgaCapabilities,
    FrameCopy,
    OverlayCompose,
    LatchPost,
    RouteArm,
    PostVerification,
}

impl LatchFailureStage {
    pub const fn code(self) -> &'static str {
        match self {
            Self::FrontendIntegrity => "frontend-integrity",
            Self::Manifest => "manifest",
            Self::Kernel => "kernel",
            Self::ModuleOpen => "module-open",
            Self::ModuleLayout => "module-layout",
            Self::BufferMap => "buffer-map",
            Self::FpgaStatus => "fpga-status",
            Self::FpgaCapabilities => "fpga-capabilities",
            Self::FrameCopy => "frame-copy",
            Self::OverlayCompose => "overlay-compose",
            Self::LatchPost => "latch-post",
            Self::RouteArm => "route-arm",
            Self::PostVerification => "post-verification",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatchFailureReason {
    FrontendHashMismatch,
    ManifestInvalid,
    KernelReleaseUnsupported,
    ScanoutDeviceMissing,
    ScanoutAbiMismatch,
    ScanoutLayoutMismatch,
    ScanoutGeometryUnsupported,
    ScanoutMapFailed,
    FpgaStatusUnsupported,
    FpgaProtocolUnsupported,
    FpgaCapabilitiesInsufficient,
    FpgaTransportFailed,
    NoWritableHiddenBuffer,
    FrameCopyFailed,
    OverlayComposeFailed,
    LatchPostFailed,
    RouteArmFailed,
    ActiveGeometryMismatch,
    PostedSequenceUnverified,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LatchFailure {
    pub state: LatchReadinessState,
    pub stage: LatchFailureStage,
    pub reason: LatchFailureReason,
    pub detail: String,
}

impl LatchFailure {
    pub fn runtime(
        stage: LatchFailureStage,
        reason: LatchFailureReason,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            state: LatchReadinessState::RuntimeFault,
            stage,
            reason,
            detail: detail.into(),
        }
    }

    pub fn incompatible(
        stage: LatchFailureStage,
        reason: LatchFailureReason,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            state: LatchReadinessState::PlatformIncompatible,
            stage,
            reason,
            detail: detail.into(),
        }
    }

    pub const fn reason_code(&self) -> &'static str {
        match self.reason {
            LatchFailureReason::FrontendHashMismatch => "frontend-hash-mismatch",
            LatchFailureReason::ManifestInvalid => "manifest-invalid",
            LatchFailureReason::KernelReleaseUnsupported => "kernel-release-unsupported",
            LatchFailureReason::ScanoutDeviceMissing => "scanout-device-missing",
            LatchFailureReason::ScanoutAbiMismatch => "scanout-abi-mismatch",
            LatchFailureReason::ScanoutLayoutMismatch => "scanout-layout-mismatch",
            LatchFailureReason::ScanoutGeometryUnsupported => "scanout-geometry-unsupported",
            LatchFailureReason::ScanoutMapFailed => "scanout-map-failed",
            LatchFailureReason::FpgaStatusUnsupported => "fpga-status-unsupported",
            LatchFailureReason::FpgaProtocolUnsupported => "fpga-protocol-unsupported",
            LatchFailureReason::FpgaCapabilitiesInsufficient => "fpga-capabilities-insufficient",
            LatchFailureReason::FpgaTransportFailed => "fpga-transport-failed",
            LatchFailureReason::NoWritableHiddenBuffer => "no-writable-hidden-buffer",
            LatchFailureReason::FrameCopyFailed => "frame-copy-failed",
            LatchFailureReason::OverlayComposeFailed => "overlay-compose-failed",
            LatchFailureReason::LatchPostFailed => "latch-post-failed",
            LatchFailureReason::RouteArmFailed => "route-arm-failed",
            LatchFailureReason::ActiveGeometryMismatch => "active-geometry-mismatch",
            LatchFailureReason::PostedSequenceUnverified => "posted-sequence-unverified",
        }
    }
}

impl std::fmt::Display for LatchFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(output, "{}: {}", self.reason_code(), self.detail)
    }
}

impl std::error::Error for LatchFailure {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LatchReadinessReport {
    pub schema: &'static str,
    pub state: LatchReadinessState,
    pub stage: Option<LatchFailureStage>,
    pub reason_code: Option<String>,
    pub detail: String,
    pub kernel_release: String,
    pub expected_kernel_release: &'static str,
    pub platform_contract_id: &'static str,
    pub scanout_abi_version: Option<u32>,
    pub scanout_slot_capacity_bytes: Option<u32>,
    pub latch_protocol_version: Option<u16>,
    pub latch_capability_flags: Option<u16>,
    pub latch_max_width: Option<u16>,
    pub latch_max_height: Option<u16>,
    pub latch_max_stride_bytes: Option<u16>,
}

impl LatchReadinessReport {
    pub fn ready(kernel_release: String) -> Self {
        Self {
            schema: "mister-magik-latch-readiness-v1",
            state: LatchReadinessState::Ready,
            stage: None,
            reason_code: None,
            detail: "live platform ready".to_string(),
            kernel_release,
            expected_kernel_release: mister_magik_scanout_contract::QUALIFIED_KERNEL_RELEASE,
            platform_contract_id: mister_magik_scanout_contract::PLATFORM_CONTRACT_ID,
            scanout_abi_version: None,
            scanout_slot_capacity_bytes: None,
            latch_protocol_version: None,
            latch_capability_flags: None,
            latch_max_width: None,
            latch_max_height: None,
            latch_max_stride_bytes: None,
        }
    }

    pub fn failed(kernel_release: String, failure: &LatchFailure) -> Self {
        let mut report = Self::ready(kernel_release);
        report.state = failure.state;
        report.stage = Some(failure.stage);
        report.reason_code = Some(failure.reason_code().to_string());
        report.detail.clone_from(&failure.detail);
        report
    }

    pub fn write_atomic(&self, path: impl AsRef<Path>) -> io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            create_dir_all(parent)?;
        }
        let temporary = path.with_extension(format!("json.tmp.{}", std::process::id()));
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&temporary)?;
        serde_json::to_writer(&mut output, self).map_err(io::Error::other)?;
        output.write_all(b"\n")?;
        output.sync_all()?;
        drop(output);
        rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reason_codes_are_stable_and_machine_readable() {
        let failure = LatchFailure::incompatible(
            LatchFailureStage::Kernel,
            LatchFailureReason::KernelReleaseUnsupported,
            "got 6.1, expected 5.15.1-MiSTer",
        );
        assert_eq!(failure.reason_code(), "kernel-release-unsupported");
        assert_eq!(
            serde_json::to_value(&failure).unwrap()["state"],
            "platform-incompatible"
        );
    }
}
