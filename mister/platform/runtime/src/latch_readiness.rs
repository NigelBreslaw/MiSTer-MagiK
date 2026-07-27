// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Stable latch-readiness vocabulary shared by runtime policy and diagnostics.

use serde::{Deserialize, Serialize};
use std::fs::{OpenOptions, create_dir_all, rename};
use std::io::{self, Write};
use std::path::Path;

pub const REPORT_PATH: &str = "/tmp/mister-magik/latch-readiness.json";
pub const RUNTIME_FAILURE_PATH: &str = "/tmp/mister-magik/latch-failure.json";

pub const MAX_LATCH_WIRE_WORDS: usize = 14;
pub const MAX_LATCH_WIRE_ATTEMPTS: usize = 6;

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatchWireErrorPhase {
    #[default]
    None,
    AckHigh,
    AckLow,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatchWireResult {
    #[default]
    Empty,
    Decoded,
    TransportError,
    DecodeError,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LatchWireDecision {
    #[default]
    None,
    Decoded,
    ReadFailed,
    TransportRetryRecovered,
    TransportRetryFailed,
    Corroborated,
    Rejected,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct LatchWireWord {
    pub index: u8,
    pub transmitted: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_high: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ack_low: Option<u16>,
    #[serde(default)]
    pub error_phase: LatchWireErrorPhase,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LatchWireAttempt {
    pub command: u16,
    pub elapsed_us: u64,
    pub command_word: LatchWireWord,
    pub response_words: [LatchWireWord; MAX_LATCH_WIRE_WORDS],
    pub response_word_count: u8,
    pub result: LatchWireResult,
}

impl Default for LatchWireAttempt {
    fn default() -> Self {
        Self {
            command: 0,
            elapsed_us: 0,
            command_word: LatchWireWord::default(),
            response_words: [LatchWireWord::default(); MAX_LATCH_WIRE_WORDS],
            response_word_count: 0,
            result: LatchWireResult::Empty,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LatchWireDiagnostics {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_flags: Option<u16>,
    pub attempts: [LatchWireAttempt; MAX_LATCH_WIRE_ATTEMPTS],
    pub attempt_count: u8,
    #[serde(default)]
    pub decision: LatchWireDecision,
    #[serde(default)]
    pub suppressed_similar_episodes: u32,
}

impl Default for LatchWireDiagnostics {
    fn default() -> Self {
        Self {
            protocol_version: None,
            capability_flags: None,
            attempts: [LatchWireAttempt::default(); MAX_LATCH_WIRE_ATTEMPTS],
            attempt_count: 0,
            decision: LatchWireDecision::None,
            suppressed_similar_episodes: 0,
        }
    }
}

impl LatchWireDiagnostics {
    pub fn push_attempt(&mut self, attempt: LatchWireAttempt) {
        let index = usize::from(self.attempt_count);
        if index < MAX_LATCH_WIRE_ATTEMPTS {
            self.attempts[index] = attempt;
            self.attempt_count += 1;
        } else {
            self.suppressed_similar_episodes = self.suppressed_similar_episodes.saturating_add(1);
        }
    }

    pub fn append(&mut self, other: &Self) {
        if self.protocol_version.is_none() {
            self.protocol_version = other.protocol_version;
        }
        if self.capability_flags.is_none() {
            self.capability_flags = other.capability_flags;
        }
        for attempt in other
            .attempts
            .iter()
            .take(usize::from(other.attempt_count))
            .copied()
        {
            self.push_attempt(attempt);
        }
        self.suppressed_similar_episodes = self
            .suppressed_similar_episodes
            .saturating_add(other.suppressed_similar_episodes);
    }
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_diagnostics: Option<LatchWireDiagnostics>,
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
            wire_diagnostics: None,
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
            wire_diagnostics: None,
        }
    }

    pub fn with_wire_diagnostics(mut self, diagnostics: LatchWireDiagnostics) -> Self {
        self.wire_diagnostics = Some(diagnostics);
        self
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

    pub const fn is_transient_runtime_failure(&self) -> bool {
        matches!(self.state, LatchReadinessState::RuntimeFault)
            && matches!(
                self.reason,
                LatchFailureReason::FpgaTransportFailed
                    | LatchFailureReason::NoWritableHiddenBuffer
                    | LatchFailureReason::LatchPostFailed
                    | LatchFailureReason::RouteArmFailed
                    | LatchFailureReason::ActiveGeometryMismatch
                    | LatchFailureReason::PostedSequenceUnverified
            )
    }
}

impl std::fmt::Display for LatchFailure {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(output, "{}: {}", self.reason_code(), self.detail)
    }
}

impl std::error::Error for LatchFailure {}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct LatchFailureEvidence {
    pub schema: &'static str,
    pub state: String,
    pub stage: String,
    pub reason: String,
    pub detail: String,
    #[serde(default)]
    pub latest_state: String,
    #[serde(default)]
    pub latest_stage: String,
    #[serde(default)]
    pub latest_reason: String,
    #[serde(default)]
    pub latest_detail: String,
    #[serde(default)]
    pub attempt_count: u8,
    #[serde(default)]
    pub latest_result: String,
    #[serde(default)]
    pub recovery_state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_wire_diagnostics: Option<LatchWireDiagnostics>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wire_diagnostics: Option<LatchWireDiagnostics>,
}

impl From<&LatchFailure> for LatchFailureEvidence {
    fn from(failure: &LatchFailure) -> Self {
        Self {
            schema: if failure.wire_diagnostics.is_some() {
                "mister-magik-latch-failure-v3"
            } else {
                "mister-magik-latch-failure-v1"
            },
            state: failure.state.code().to_string(),
            stage: failure.stage.code().to_string(),
            reason: failure.reason_code().to_string(),
            detail: failure.detail.clone(),
            latest_state: failure.state.code().to_string(),
            latest_stage: failure.stage.code().to_string(),
            latest_reason: failure.reason_code().to_string(),
            latest_detail: failure.detail.clone(),
            attempt_count: 0,
            latest_result: "not-attempted".to_string(),
            recovery_state: "compatibility-prompt".to_string(),
            first_wire_diagnostics: failure.wire_diagnostics.clone(),
            wire_diagnostics: failure.wire_diagnostics.clone(),
        }
    }
}

impl LatchFailureEvidence {
    pub fn for_recovery(
        first: &LatchFailure,
        latest: &LatchFailure,
        attempt_count: u8,
        latest_result: impl Into<String>,
        recovery_state: impl Into<String>,
    ) -> Self {
        Self {
            schema: if latest.wire_diagnostics.is_some() || first.wire_diagnostics.is_some() {
                "mister-magik-latch-failure-v3"
            } else {
                "mister-magik-latch-failure-v2"
            },
            state: first.state.code().to_string(),
            stage: first.stage.code().to_string(),
            reason: first.reason_code().to_string(),
            detail: first.detail.clone(),
            latest_state: latest.state.code().to_string(),
            latest_stage: latest.stage.code().to_string(),
            latest_reason: latest.reason_code().to_string(),
            latest_detail: latest.detail.clone(),
            attempt_count,
            latest_result: latest_result.into(),
            recovery_state: recovery_state.into(),
            first_wire_diagnostics: first.wire_diagnostics.clone(),
            wire_diagnostics: latest.wire_diagnostics.clone(),
        }
    }

    pub fn write_atomic(&self, path: impl AsRef<Path>) -> io::Result<()> {
        write_json_atomic(self, path)
    }
}

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
        write_json_atomic(self, path)
    }
}

fn write_json_atomic(value: &impl Serialize, path: impl AsRef<Path>) -> io::Result<()> {
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
    serde_json::to_writer(&mut output, value).map_err(io::Error::other)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    drop(output);
    rename(temporary, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

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

    #[test]
    fn every_state_stage_and_reason_has_a_stable_code() {
        let states = [
            (LatchReadinessState::Ready, "ready"),
            (LatchReadinessState::InstallationFault, "installation-fault"),
            (
                LatchReadinessState::PlatformIncompatible,
                "platform-incompatible",
            ),
            (LatchReadinessState::RuntimeFault, "runtime-fault"),
        ];
        for (state, expected) in states {
            assert_eq!(state.code(), expected);
        }

        let stages = [
            (LatchFailureStage::FrontendIntegrity, "frontend-integrity"),
            (LatchFailureStage::Manifest, "manifest"),
            (LatchFailureStage::Kernel, "kernel"),
            (LatchFailureStage::ModuleOpen, "module-open"),
            (LatchFailureStage::ModuleLayout, "module-layout"),
            (LatchFailureStage::BufferMap, "buffer-map"),
            (LatchFailureStage::FpgaStatus, "fpga-status"),
            (LatchFailureStage::FpgaCapabilities, "fpga-capabilities"),
            (LatchFailureStage::FrameCopy, "frame-copy"),
            (LatchFailureStage::OverlayCompose, "overlay-compose"),
            (LatchFailureStage::LatchPost, "latch-post"),
            (LatchFailureStage::RouteArm, "route-arm"),
            (LatchFailureStage::PostVerification, "post-verification"),
        ];
        for (stage, expected) in stages {
            assert_eq!(stage.code(), expected);
        }

        let reasons = [
            (
                LatchFailureReason::FrontendHashMismatch,
                "frontend-hash-mismatch",
            ),
            (LatchFailureReason::ManifestInvalid, "manifest-invalid"),
            (
                LatchFailureReason::KernelReleaseUnsupported,
                "kernel-release-unsupported",
            ),
            (
                LatchFailureReason::ScanoutDeviceMissing,
                "scanout-device-missing",
            ),
            (
                LatchFailureReason::ScanoutAbiMismatch,
                "scanout-abi-mismatch",
            ),
            (
                LatchFailureReason::ScanoutLayoutMismatch,
                "scanout-layout-mismatch",
            ),
            (
                LatchFailureReason::ScanoutGeometryUnsupported,
                "scanout-geometry-unsupported",
            ),
            (LatchFailureReason::ScanoutMapFailed, "scanout-map-failed"),
            (
                LatchFailureReason::FpgaStatusUnsupported,
                "fpga-status-unsupported",
            ),
            (
                LatchFailureReason::FpgaProtocolUnsupported,
                "fpga-protocol-unsupported",
            ),
            (
                LatchFailureReason::FpgaCapabilitiesInsufficient,
                "fpga-capabilities-insufficient",
            ),
            (
                LatchFailureReason::FpgaTransportFailed,
                "fpga-transport-failed",
            ),
            (
                LatchFailureReason::NoWritableHiddenBuffer,
                "no-writable-hidden-buffer",
            ),
            (LatchFailureReason::FrameCopyFailed, "frame-copy-failed"),
            (
                LatchFailureReason::OverlayComposeFailed,
                "overlay-compose-failed",
            ),
            (LatchFailureReason::LatchPostFailed, "latch-post-failed"),
            (LatchFailureReason::RouteArmFailed, "route-arm-failed"),
            (
                LatchFailureReason::ActiveGeometryMismatch,
                "active-geometry-mismatch",
            ),
            (
                LatchFailureReason::PostedSequenceUnverified,
                "posted-sequence-unverified",
            ),
        ];
        for (reason, expected) in reasons {
            let failure = LatchFailure::runtime(LatchFailureStage::LatchPost, reason, "detail");
            assert_eq!(failure.reason_code(), expected);
            assert_eq!(failure.to_string(), format!("{expected}: detail"));
        }
    }

    #[test]
    fn ready_and_failed_reports_preserve_their_contracts() {
        let ready = LatchReadinessReport::ready("kernel".to_string());
        assert_eq!(ready.state, LatchReadinessState::Ready);
        assert_eq!(ready.stage, None);
        assert_eq!(ready.reason_code, None);
        assert_eq!(ready.detail, "live platform ready");

        let failure = LatchFailure::incompatible(
            LatchFailureStage::ModuleLayout,
            LatchFailureReason::ScanoutLayoutMismatch,
            "unexpected slot layout",
        );
        let failed = LatchReadinessReport::failed("kernel".to_string(), &failure);
        assert_eq!(failed.state, LatchReadinessState::PlatformIncompatible);
        assert_eq!(failed.stage, Some(LatchFailureStage::ModuleLayout));
        assert_eq!(
            failed.reason_code.as_deref(),
            Some("scanout-layout-mismatch")
        );
        assert_eq!(failed.detail, "unexpected slot layout");
    }

    #[test]
    fn failed_report_publishes_and_removes_the_temporary_file() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-latch-readiness-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("nested/readiness.json");
        let failure = LatchFailure::incompatible(
            LatchFailureStage::ModuleLayout,
            LatchFailureReason::ScanoutLayoutMismatch,
            "unexpected slot layout",
        );
        let report = LatchReadinessReport::failed("kernel".to_string(), &failure);

        report.write_atomic(&path).unwrap();

        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted["schema"], "mister-magik-latch-readiness-v1");
        assert_eq!(persisted["state"], "platform-incompatible");
        assert_eq!(persisted["kernel_release"], "kernel");
        assert_eq!(
            persisted["expected_kernel_release"],
            mister_magik_scanout_contract::QUALIFIED_KERNEL_RELEASE
        );
        assert!(
            !root
                .join(format!("nested/readiness.json.tmp.{}", std::process::id()))
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn runtime_failure_evidence_publishes_stable_codes_atomically() {
        let root = std::env::temp_dir().join(format!(
            "mister-magik-latch-failure-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = root.join("nested/failure.json");
        let failure = LatchFailure::runtime(
            LatchFailureStage::PostVerification,
            LatchFailureReason::PostedSequenceUnverified,
            "posted=219 final_active=218",
        );

        LatchFailureEvidence::from(&failure)
            .write_atomic(&path)
            .unwrap();

        let persisted: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(persisted["schema"], "mister-magik-latch-failure-v1");
        assert_eq!(persisted["state"], "runtime-fault");
        assert_eq!(persisted["stage"], "post-verification");
        assert_eq!(persisted["reason"], "posted-sequence-unverified");
        assert_eq!(persisted["detail"], "posted=219 final_active=218");
        assert!(
            !root
                .join(format!("nested/failure.json.tmp.{}", std::process::id()))
                .exists()
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn wire_diagnostics_round_trip_without_losing_ack_phases() {
        let diagnostics = LatchWireDiagnostics {
            protocol_version: Some(2),
            capability_flags: Some(7),
            attempts: [LatchWireAttempt {
                command: 0x58,
                elapsed_us: 42,
                command_word: LatchWireWord {
                    index: 0,
                    transmitted: 0x58,
                    ack_high: Some(0x4d48),
                    ack_low: Some(0),
                    error_phase: LatchWireErrorPhase::None,
                },
                response_words: {
                    let mut words = [LatchWireWord::default(); MAX_LATCH_WIRE_WORDS];
                    words[0] = LatchWireWord {
                        index: 0,
                        transmitted: 0,
                        ack_high: Some(0),
                        ack_low: Some(0x1234),
                        error_phase: LatchWireErrorPhase::None,
                    };
                    words
                },
                response_word_count: 1,
                result: LatchWireResult::Decoded,
            }; MAX_LATCH_WIRE_ATTEMPTS],
            attempt_count: 1,
            decision: LatchWireDecision::Corroborated,
            suppressed_similar_episodes: 3,
        };
        let failure = LatchFailure::runtime(
            LatchFailureStage::FpgaStatus,
            LatchFailureReason::ActiveGeometryMismatch,
            "diagnostic fixture",
        )
        .with_wire_diagnostics(diagnostics.clone());
        let evidence = LatchFailureEvidence::from(&failure);
        assert_eq!(evidence.schema, "mister-magik-latch-failure-v3");
        let encoded = serde_json::to_vec(&evidence).unwrap();
        assert!(encoded.len() < 32 * 1024);
        let decoded: serde_json::Value = serde_json::from_slice(&encoded).unwrap();
        let decoded_diagnostics: LatchWireDiagnostics =
            serde_json::from_value(decoded["wire_diagnostics"].clone()).unwrap();
        assert_eq!(decoded_diagnostics, diagnostics);
    }
}
