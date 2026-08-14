// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Canonical physical-frame evidence and return-qualification aggregation.

use crate::error::{AgentError, AgentResult};
use crate::platform_manifest::{self, Layout};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

pub const FRAME_SCHEMA: &str = "mister-magik-return-frame-evidence-v1";
pub const CLASSIFIER_FRAME_SCHEMA: &str = "mister-magik-physical-frame-classification-v1";
pub const BOARD_SCHEMA: &str = "mister-magik-return-qualification-board-v1";
pub const AGGREGATE_SCHEMA: &str = "mister-magik-return-qualification-aggregate-v1";
pub const DEFAULT_AGGREGATE_CERTIFICATE: &str =
    "build/release-qualification/return-qualification/aggregate-certificate.json";
pub const MINIMUM_BOARDS: usize = 3;
pub const MINIMUM_SINKS: usize = 2;
pub const MINIMUM_SINK_CHIPSETS: usize = 2;
pub const MINIMUM_TRANSITIONS: u64 = 300_000;
pub const MINIMUM_TRANSITIONS_PER_BOARD: u64 = 100_000;
pub const MINIMUM_ARCADE_RETURNS: u64 = 150_000;
pub const MINIMUM_OTHER_TRANSITION_KIND: u64 = 10_000;
pub const MINIMUM_TRANSITIONS_PER_MODE: u64 = 1_000;

const MODES: [&str; 11] = [
    "auto",
    "hdmi-1280x720p60",
    "hdmi-1366x768p60",
    "hdmi-1920x1080p60",
    "hdmi-1920x1200p60",
    "hdmi-2048x1536p60",
    "hdmi-2560x1440p60",
    "crt-240p60",
    "crt-288p50",
    "crt-480p60",
    "crt-576p50",
];

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateIdentity {
    pub qualification_candidate_id: String,
    pub platform_bundle_id: String,
    pub main_sha256: String,
    pub gui_sha256: String,
    pub manager_sha256: String,
    pub scanout_module_sha256: String,
    pub scanout_metadata_sha256: String,
    pub latch_rbf_sha256: String,
    pub latch_metadata_sha256: String,
    pub main_revision: String,
    pub magik_revision: String,
}

impl CandidateIdentity {
    pub fn from_manifest(text: &str, layout: Layout) -> AgentResult<Self> {
        let manifest = platform_manifest::parse_installed(text, layout)?;
        Ok(Self {
            qualification_candidate_id: manifest.qualification_candidate_id().into(),
            platform_bundle_id: manifest.platform_bundle_id().into(),
            main_sha256: manifest.main_sha256().into(),
            gui_sha256: manifest.gui_sha256().into(),
            manager_sha256: manifest.manager_sha256().into(),
            scanout_module_sha256: manifest.scanout_module_sha256().into(),
            scanout_metadata_sha256: manifest.scanout_metadata_sha256().into(),
            latch_rbf_sha256: manifest.latch_rbf_sha256().into(),
            latch_metadata_sha256: manifest.latch_metadata_sha256().into(),
            main_revision: manifest.main_revision().into(),
            magik_revision: manifest.magik_revision().into(),
        })
    }

    fn validate(&self) -> AgentResult<()> {
        for (name, value, length) in [
            (
                "qualification_candidate_id",
                self.qualification_candidate_id.as_str(),
                64,
            ),
            ("platform_bundle_id", self.platform_bundle_id.as_str(), 64),
            ("main_sha256", self.main_sha256.as_str(), 64),
            ("gui_sha256", self.gui_sha256.as_str(), 64),
            ("manager_sha256", self.manager_sha256.as_str(), 64),
            (
                "scanout_module_sha256",
                self.scanout_module_sha256.as_str(),
                64,
            ),
            (
                "scanout_metadata_sha256",
                self.scanout_metadata_sha256.as_str(),
                64,
            ),
            ("latch_rbf_sha256", self.latch_rbf_sha256.as_str(), 64),
            (
                "latch_metadata_sha256",
                self.latch_metadata_sha256.as_str(),
                64,
            ),
            ("main_revision", self.main_revision.as_str(), 40),
            ("magik_revision", self.magik_revision.as_str(), 40),
        ] {
            require_hex(name, value, length)?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransitionKind {
    ArcadeReturn,
    CoreReturn,
    ActiveRestart,
    CrashRespawn,
    DisplayReconfigure,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CaptureSource {
    PhysicalHdmiRx,
    PhysicalCrtAdc,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum FrameClassification {
    Correct,
    Black,
    Stale,
    Partial,
    Banded,
    Corrupt,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ClassifierFrame {
    schema: String,
    frame_sequence: u64,
    source_nonblack: bool,
    classification: FrameClassification,
}

const TRANSITION_KINDS: [TransitionKind; 5] = [
    TransitionKind::ArcadeReturn,
    TransitionKind::CoreReturn,
    TransitionKind::ActiveRestart,
    TransitionKind::CrashRespawn,
    TransitionKind::DisplayReconfigure,
];

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrameClassifications {
    pub black: u64,
    pub stale: u64,
    pub partial: u64,
    pub banded: u64,
    pub corrupt: u64,
}

impl FrameClassifications {
    fn total(&self) -> AgentResult<u64> {
        [
            self.black,
            self.stale,
            self.partial,
            self.banded,
            self.corrupt,
        ]
        .into_iter()
        .try_fold(0_u64, checked_add)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FrameEvidenceV1 {
    pub schema: String,
    pub candidate: CandidateIdentity,
    pub board_id: String,
    pub sink_id: String,
    pub sink_chipset_id: String,
    pub capture_id: String,
    pub attended: bool,
    pub capture_source: CaptureSource,
    pub frame_complete_capture: bool,
    pub transition_kind: TransitionKind,
    pub video_mode: String,
    pub expected_refresh_millihz: u64,
    pub capture_rate_millihz: u64,
    pub first_frame_sequence: u64,
    pub last_frame_sequence: u64,
    pub frames_observed: u64,
    pub correct_frames: u64,
    pub source_nonblack_frames: u64,
    pub transitions_observed: u64,
    pub classifications: FrameClassifications,
    pub capture_sha256: String,
    pub classifier_sha256: String,
    pub classifier_report_path: PathBuf,
    pub classifier_report_sha256: String,
}

pub fn verify_frame_evidence(evidence: &FrameEvidenceV1) -> AgentResult<()> {
    if evidence.schema != FRAME_SCHEMA {
        return classified("unsupported_frame_evidence", evidence.schema.clone());
    }
    evidence.candidate.validate()?;
    for (name, value) in [
        ("board_id", evidence.board_id.as_str()),
        ("sink_id", evidence.sink_id.as_str()),
        ("sink_chipset_id", evidence.sink_chipset_id.as_str()),
        ("capture_id", evidence.capture_id.as_str()),
    ] {
        require_identifier(name, value)?;
    }
    if !evidence.attended {
        return classified("unattended_frame_evidence", &evidence.capture_id);
    }
    if !evidence.frame_complete_capture {
        return classified(
            "non_physical_frame_evidence",
            "evidence must come from frame-complete HDMI RX or CRT ADC capture",
        );
    }
    if !MODES.contains(&evidence.video_mode.as_str()) {
        return classified("unsupported_frame_video_mode", &evidence.video_mode);
    }
    if (evidence.video_mode.starts_with("hdmi-")
        && evidence.capture_source != CaptureSource::PhysicalHdmiRx)
        || (evidence.video_mode.starts_with("crt-")
            && evidence.capture_source != CaptureSource::PhysicalCrtAdc)
    {
        return classified(
            "capture_route_mismatch",
            format!(
                "mode={} source={:?}",
                evidence.video_mode, evidence.capture_source
            ),
        );
    }
    if evidence.expected_refresh_millihz == 0
        || evidence.capture_rate_millihz < evidence.expected_refresh_millihz
    {
        return classified(
            "below_refresh_frame_capture",
            format!(
                "capture={} expected={}",
                evidence.capture_rate_millihz, evidence.expected_refresh_millihz
            ),
        );
    }
    if evidence.frames_observed == 0 || evidence.transitions_observed == 0 {
        return classified("empty_frame_evidence", &evidence.capture_id);
    }
    let expected_frames = evidence
        .last_frame_sequence
        .checked_sub(evidence.first_frame_sequence)
        .and_then(|span| span.checked_add(1))
        .ok_or_else(|| AgentError::Classified {
            code: "frame_sequence_gap",
            detail: evidence.capture_id.clone(),
        })?;
    if expected_frames != evidence.frames_observed {
        return classified(
            "frame_sequence_gap",
            format!(
                "capture={} expected={} observed={}",
                evidence.capture_id, expected_frames, evidence.frames_observed
            ),
        );
    }
    if evidence.frames_observed < evidence.transitions_observed {
        return classified(
            "insufficient_transition_frames",
            format!(
                "capture={} frames={} transitions={}",
                evidence.capture_id, evidence.frames_observed, evidence.transitions_observed
            ),
        );
    }
    let failures = evidence.classifications.total()?;
    if failures != 0 {
        return classified(
            "physical_frame_failure",
            format!("capture={} classifications={failures}", evidence.capture_id),
        );
    }
    if evidence.correct_frames != evidence.frames_observed
        || evidence.source_nonblack_frames != evidence.frames_observed
    {
        return classified(
            "incomplete_correct_frame_evidence",
            format!(
                "capture={} frames={} correct={} nonblack_source={}",
                evidence.capture_id,
                evidence.frames_observed,
                evidence.correct_frames,
                evidence.source_nonblack_frames
            ),
        );
    }
    require_hex("capture_sha256", &evidence.capture_sha256, 64)?;
    require_hex("classifier_sha256", &evidence.classifier_sha256, 64)?;
    require_relative_path("classifier_report_path", &evidence.classifier_report_path)?;
    require_hex(
        "classifier_report_sha256",
        &evidence.classifier_report_sha256,
        64,
    )
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EvidenceReference {
    pub sha256: String,
    pub evidence: FrameEvidenceV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardCertificateV1 {
    pub schema: String,
    pub candidate: CandidateIdentity,
    pub board_id: String,
    pub attended: bool,
    pub sink_ids: Vec<String>,
    pub sink_chipset_ids: Vec<String>,
    pub total_transitions: u64,
    pub transitions: BTreeMap<TransitionKind, u64>,
    pub modes: BTreeMap<String, u64>,
    pub frame_evidence: Vec<EvidenceReference>,
}

pub fn create_board_certificate(
    manifest: &str,
    layout: Layout,
    attended: bool,
    paths: &[PathBuf],
) -> AgentResult<BoardCertificateV1> {
    if !attended {
        return classified("attendance_required", "record-board requires --attended");
    }
    if paths.is_empty() {
        return classified("frame_evidence_missing", "no frame evidence supplied");
    }
    let candidate = CandidateIdentity::from_manifest(manifest, layout)?;
    let mut references = Vec::with_capacity(paths.len());
    for path in paths {
        let evidence = read_frame_evidence(path)?;
        if evidence.candidate != candidate {
            return classified(
                "frame_candidate_identity_mismatch",
                path.display().to_string(),
            );
        }
        references.push(EvidenceReference {
            sha256: digest_json(&evidence)?,
            evidence,
        });
    }
    let board_id = references[0].evidence.board_id.clone();
    if references
        .iter()
        .any(|entry| entry.evidence.board_id != board_id)
    {
        return classified("mixed_board_evidence", board_id);
    }
    let certificate = summarize_board(candidate, board_id, references)?;
    verify_board_certificate(&certificate)?;
    Ok(certificate)
}

fn summarize_board(
    candidate: CandidateIdentity,
    board_id: String,
    frame_evidence: Vec<EvidenceReference>,
) -> AgentResult<BoardCertificateV1> {
    let mut sinks = BTreeSet::new();
    let mut sink_chipsets = BTreeSet::new();
    let mut transitions = BTreeMap::new();
    let mut modes = BTreeMap::new();
    let mut total = 0_u64;
    for entry in &frame_evidence {
        let evidence = &entry.evidence;
        sinks.insert(evidence.sink_id.clone());
        sink_chipsets.insert(evidence.sink_chipset_id.clone());
        total = checked_add(total, evidence.transitions_observed)?;
        add_count(
            &mut transitions,
            evidence.transition_kind,
            evidence.transitions_observed,
        )?;
        add_count(
            &mut modes,
            evidence.video_mode.clone(),
            evidence.transitions_observed,
        )?;
    }
    Ok(BoardCertificateV1 {
        schema: BOARD_SCHEMA.into(),
        candidate,
        board_id,
        attended: true,
        sink_ids: sinks.into_iter().collect(),
        sink_chipset_ids: sink_chipsets.into_iter().collect(),
        total_transitions: total,
        transitions,
        modes,
        frame_evidence,
    })
}

pub fn verify_board_certificate(certificate: &BoardCertificateV1) -> AgentResult<()> {
    if certificate.schema != BOARD_SCHEMA {
        return classified("unsupported_board_certificate", &certificate.schema);
    }
    certificate.candidate.validate()?;
    require_identifier("board_id", &certificate.board_id)?;
    if !certificate.attended || certificate.frame_evidence.is_empty() {
        return classified("incomplete_board_certificate", &certificate.board_id);
    }
    let mut seen_captures = BTreeSet::new();
    for entry in &certificate.frame_evidence {
        require_hex("frame_evidence_sha256", &entry.sha256, 64)?;
        if entry.sha256 != digest_json(&entry.evidence)? {
            return classified("frame_evidence_digest_mismatch", &entry.evidence.capture_id);
        }
        verify_frame_evidence(&entry.evidence)?;
        if entry.evidence.candidate != certificate.candidate
            || entry.evidence.board_id != certificate.board_id
        {
            return classified("board_certificate_identity_mismatch", &certificate.board_id);
        }
        if !seen_captures.insert(entry.evidence.capture_id.as_str()) {
            return classified("duplicate_frame_capture", &entry.evidence.capture_id);
        }
    }
    let expected = summarize_board(
        certificate.candidate.clone(),
        certificate.board_id.clone(),
        certificate.frame_evidence.clone(),
    )?;
    if certificate.sink_ids != expected.sink_ids
        || certificate.sink_chipset_ids != expected.sink_chipset_ids
        || certificate.total_transitions != expected.total_transitions
        || certificate.transitions != expected.transitions
        || certificate.modes != expected.modes
    {
        return classified("board_certificate_summary_mismatch", &certificate.board_id);
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BoardReference {
    pub sha256: String,
    pub certificate: BoardCertificateV1,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AggregateCertificateV1 {
    pub schema: String,
    pub candidate: CandidateIdentity,
    pub total_transitions: u64,
    pub distinct_boards: u64,
    pub distinct_sinks: u64,
    pub distinct_sink_chipsets: u64,
    pub transitions: BTreeMap<TransitionKind, u64>,
    pub modes: BTreeMap<String, u64>,
    pub boards: Vec<BoardReference>,
}

pub fn create_aggregate_certificate(
    manifest: &str,
    layout: Layout,
    paths: &[PathBuf],
) -> AgentResult<AggregateCertificateV1> {
    if paths.is_empty() {
        return classified("board_evidence_missing", "no board certificates supplied");
    }
    let candidate = CandidateIdentity::from_manifest(manifest, layout)?;
    let mut boards = Vec::with_capacity(paths.len());
    for path in paths {
        let bytes = read(path)?;
        let certificate: BoardCertificateV1 = parse_json(path, &bytes)?;
        verify_board_certificate(&certificate)?;
        if certificate.candidate != candidate {
            return classified(
                "board_candidate_identity_mismatch",
                path.display().to_string(),
            );
        }
        boards.push(BoardReference {
            sha256: digest_json(&certificate)?,
            certificate,
        });
    }
    let aggregate = summarize_aggregate(candidate, boards)?;
    verify_aggregate_certificate(&aggregate)?;
    Ok(aggregate)
}

fn summarize_aggregate(
    candidate: CandidateIdentity,
    boards: Vec<BoardReference>,
) -> AgentResult<AggregateCertificateV1> {
    let mut total = 0_u64;
    let mut board_ids = BTreeSet::new();
    let mut sink_ids = BTreeSet::new();
    let mut sink_chipset_ids = BTreeSet::new();
    let mut transitions = BTreeMap::new();
    let mut modes = BTreeMap::new();
    for board in &boards {
        let certificate = &board.certificate;
        board_ids.insert(certificate.board_id.as_str());
        sink_ids.extend(certificate.sink_ids.iter().map(String::as_str));
        sink_chipset_ids.extend(certificate.sink_chipset_ids.iter().map(String::as_str));
        total = checked_add(total, certificate.total_transitions)?;
        merge_counts(&mut transitions, &certificate.transitions)?;
        merge_counts(&mut modes, &certificate.modes)?;
    }
    Ok(AggregateCertificateV1 {
        schema: AGGREGATE_SCHEMA.into(),
        candidate,
        total_transitions: total,
        distinct_boards: usize_to_u64(board_ids.len())?,
        distinct_sinks: usize_to_u64(sink_ids.len())?,
        distinct_sink_chipsets: usize_to_u64(sink_chipset_ids.len())?,
        transitions,
        modes,
        boards,
    })
}

pub fn verify_aggregate_certificate(certificate: &AggregateCertificateV1) -> AgentResult<()> {
    if certificate.schema != AGGREGATE_SCHEMA {
        return classified("unsupported_return_certificate", &certificate.schema);
    }
    certificate.candidate.validate()?;
    let mut board_ids = BTreeSet::new();
    for board in &certificate.boards {
        require_hex("board_certificate_sha256", &board.sha256, 64)?;
        if board.sha256 != digest_json(&board.certificate)? {
            return classified(
                "board_certificate_digest_mismatch",
                &board.certificate.board_id,
            );
        }
        verify_board_certificate(&board.certificate)?;
        if board.certificate.candidate != certificate.candidate {
            return classified(
                "aggregate_candidate_identity_mismatch",
                &board.certificate.board_id,
            );
        }
        if !board_ids.insert(board.certificate.board_id.as_str()) {
            return classified("duplicate_qualification_board", &board.certificate.board_id);
        }
        if board.certificate.total_transitions < MINIMUM_TRANSITIONS_PER_BOARD {
            return classified(
                "board_transition_minimum",
                format!(
                    "board={} observed={} minimum={MINIMUM_TRANSITIONS_PER_BOARD}",
                    board.certificate.board_id, board.certificate.total_transitions
                ),
            );
        }
    }
    let expected = summarize_aggregate(certificate.candidate.clone(), certificate.boards.clone())?;
    if certificate.total_transitions != expected.total_transitions
        || certificate.distinct_boards != expected.distinct_boards
        || certificate.distinct_sinks != expected.distinct_sinks
        || certificate.distinct_sink_chipsets != expected.distinct_sink_chipsets
        || certificate.transitions != expected.transitions
        || certificate.modes != expected.modes
    {
        return classified(
            "aggregate_certificate_summary_mismatch",
            "aggregate counters",
        );
    }
    if certificate.distinct_boards < usize_to_u64(MINIMUM_BOARDS)? {
        return classified(
            "board_count_minimum",
            format!(
                "observed={} minimum={MINIMUM_BOARDS}",
                certificate.distinct_boards
            ),
        );
    }
    if certificate.distinct_sinks < usize_to_u64(MINIMUM_SINKS)? {
        return classified(
            "sink_count_minimum",
            format!(
                "observed={} minimum={MINIMUM_SINKS}",
                certificate.distinct_sinks
            ),
        );
    }
    if certificate.distinct_sink_chipsets < usize_to_u64(MINIMUM_SINK_CHIPSETS)? {
        return classified(
            "sink_chipset_count_minimum",
            format!(
                "observed={} minimum={MINIMUM_SINK_CHIPSETS}",
                certificate.distinct_sink_chipsets
            ),
        );
    }
    if certificate.total_transitions < MINIMUM_TRANSITIONS {
        return classified(
            "transition_count_minimum",
            format!(
                "observed={} minimum={MINIMUM_TRANSITIONS}",
                certificate.total_transitions
            ),
        );
    }
    for kind in TRANSITION_KINDS {
        let count = certificate.transitions.get(&kind).copied().unwrap_or(0);
        let minimum = if kind == TransitionKind::ArcadeReturn {
            MINIMUM_ARCADE_RETURNS
        } else {
            MINIMUM_OTHER_TRANSITION_KIND
        };
        if count < minimum {
            return classified(
                "transition_kind_minimum",
                format!("kind={kind:?} observed={count} minimum={minimum}"),
            );
        }
    }
    for mode in MODES {
        let count = certificate.modes.get(mode).copied().unwrap_or(0);
        if count < MINIMUM_TRANSITIONS_PER_MODE {
            return classified(
                "video_mode_minimum",
                format!("mode={mode} observed={count} minimum={MINIMUM_TRANSITIONS_PER_MODE}"),
            );
        }
    }
    Ok(())
}

pub fn verify_aggregate_for_manifest(
    certificate_path: &Path,
    manifest: &str,
    layout: Layout,
) -> AgentResult<AggregateCertificateV1> {
    let bytes = read(certificate_path)?;
    let certificate: AggregateCertificateV1 = parse_json(certificate_path, &bytes)?;
    verify_aggregate_certificate(&certificate)?;
    let candidate = CandidateIdentity::from_manifest(manifest, layout)?;
    if certificate.candidate != candidate {
        return classified(
            "return_certificate_candidate_mismatch",
            certificate_path.display().to_string(),
        );
    }
    Ok(certificate)
}

pub fn read_frame_evidence(path: &Path) -> AgentResult<FrameEvidenceV1> {
    let bytes = read(path)?;
    let evidence = parse_json(path, &bytes)?;
    verify_frame_evidence(&evidence)?;
    verify_classifier_report(path, &evidence)?;
    Ok(evidence)
}

fn verify_classifier_report(evidence_path: &Path, evidence: &FrameEvidenceV1) -> AgentResult<()> {
    let base = evidence_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let report_path = base.join(&evidence.classifier_report_path);
    let canonical_base = fs::canonicalize(base)
        .map_err(|error| format!("cannot resolve {}: {error}", base.display()))?;
    let canonical_report = fs::canonicalize(&report_path)
        .map_err(|error| format!("cannot resolve {}: {error}", report_path.display()))?;
    if !canonical_report.starts_with(&canonical_base) {
        return classified(
            "invalid_evidence_path",
            format!("classifier_report_path: {}", report_path.display()),
        );
    }
    let bytes = read(&canonical_report)?;
    if digest(&bytes) != evidence.classifier_report_sha256 {
        return classified(
            "classifier_report_digest_mismatch",
            report_path.display().to_string(),
        );
    }
    let text = std::str::from_utf8(&bytes).map_err(|error| {
        AgentError::from(format!(
            "cannot decode {} as UTF-8: {error}",
            report_path.display()
        ))
    })?;
    let mut previous: Option<u64> = None;
    let mut frames = 0_u64;
    let mut correct = 0_u64;
    let mut source_nonblack = 0_u64;
    let mut classifications = FrameClassifications::default();
    for (index, line) in text.lines().enumerate() {
        let frame: ClassifierFrame = serde_json::from_str(line).map_err(|error| {
            AgentError::from(format!(
                "cannot parse {} line {}: {error}",
                report_path.display(),
                index + 1
            ))
        })?;
        if frame.schema != CLASSIFIER_FRAME_SCHEMA {
            return classified("unsupported_classifier_report", frame.schema);
        }
        if let Some(previous) = previous {
            let expected = previous
                .checked_add(1)
                .ok_or_else(|| AgentError::Classified {
                    code: "frame_sequence_gap",
                    detail: evidence.capture_id.clone(),
                })?;
            if frame.frame_sequence != expected {
                return classified(
                    "frame_sequence_gap",
                    format!(
                        "capture={} expected={} observed={}",
                        evidence.capture_id, expected, frame.frame_sequence
                    ),
                );
            }
        } else if frame.frame_sequence != evidence.first_frame_sequence {
            return classified("frame_sequence_gap", &evidence.capture_id);
        }
        previous = Some(frame.frame_sequence);
        frames = checked_add(frames, 1)?;
        if frame.source_nonblack {
            source_nonblack = checked_add(source_nonblack, 1)?;
        }
        match frame.classification {
            FrameClassification::Correct => correct = checked_add(correct, 1)?,
            FrameClassification::Black => {
                classifications.black = checked_add(classifications.black, 1)?
            }
            FrameClassification::Stale => {
                classifications.stale = checked_add(classifications.stale, 1)?
            }
            FrameClassification::Partial => {
                classifications.partial = checked_add(classifications.partial, 1)?
            }
            FrameClassification::Banded => {
                classifications.banded = checked_add(classifications.banded, 1)?
            }
            FrameClassification::Corrupt => {
                classifications.corrupt = checked_add(classifications.corrupt, 1)?
            }
        }
    }
    if previous != Some(evidence.last_frame_sequence)
        || frames != evidence.frames_observed
        || correct != evidence.correct_frames
        || source_nonblack != evidence.source_nonblack_frames
        || classifications != evidence.classifications
    {
        return classified(
            "classifier_report_summary_mismatch",
            format!(
                "capture={} report={}",
                evidence.capture_id,
                report_path.display()
            ),
        );
    }
    Ok(())
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> AgentResult<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    }
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("cannot encode {}: {error}", path.display()))?;
    bytes.push(b'\n');
    fs::write(path, bytes)
        .map_err(|error| format!("cannot write {}: {error}", path.display()).into())
}

fn read(path: &Path) -> AgentResult<Vec<u8>> {
    fs::read(path).map_err(|error| format!("cannot read {}: {error}", path.display()).into())
}

fn parse_json<T: for<'de> Deserialize<'de>>(path: &Path, bytes: &[u8]) -> AgentResult<T> {
    serde_json::from_slice(bytes)
        .map_err(|error| format!("cannot parse {}: {error}", path.display()).into())
}

fn digest(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn digest_json<T: Serialize>(value: &T) -> AgentResult<String> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("cannot encode canonical evidence: {error}"))?;
    Ok(digest(&bytes))
}

fn require_identifier(name: &str, value: &str) -> AgentResult<()> {
    if !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
    {
        Ok(())
    } else {
        classified("invalid_evidence_identifier", format!("{name}: {value}"))
    }
}

fn require_relative_path(name: &str, value: &Path) -> AgentResult<()> {
    if value.as_os_str().is_empty()
        || value.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return classified(
            "invalid_evidence_path",
            format!("{name}: {}", value.display()),
        );
    }
    Ok(())
}

fn require_hex(name: &str, value: &str, length: usize) -> AgentResult<()> {
    if value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        classified("invalid_evidence_hash", format!("{name}: {value}"))
    }
}

fn checked_add(left: u64, right: u64) -> AgentResult<u64> {
    left.checked_add(right)
        .ok_or_else(|| AgentError::Classified {
            code: "evidence_counter_overflow",
            detail: format!("{left}+{right}"),
        })
}

fn add_count<K: Ord>(map: &mut BTreeMap<K, u64>, key: K, count: u64) -> AgentResult<()> {
    let next = checked_add(map.get(&key).copied().unwrap_or(0), count)?;
    map.insert(key, next);
    Ok(())
}

fn merge_counts<K: Clone + Ord>(
    destination: &mut BTreeMap<K, u64>,
    source: &BTreeMap<K, u64>,
) -> AgentResult<()> {
    for (key, count) in source {
        add_count(destination, key.clone(), *count)?;
    }
    Ok(())
}

fn usize_to_u64(value: usize) -> AgentResult<u64> {
    u64::try_from(value).map_err(|_| AgentError::Classified {
        code: "evidence_counter_overflow",
        detail: value.to_string(),
    })
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

    fn assert_code<T: std::fmt::Debug>(result: AgentResult<T>, code: &str) {
        let error = result.unwrap_err().to_string();
        assert!(error.starts_with(code), "expected {code}, found {error}");
    }

    fn rebuild_board(reference: &mut BoardReference) {
        let board_id = reference.certificate.board_id.clone();
        reference.certificate = summarize_board(
            reference.certificate.candidate.clone(),
            board_id,
            reference.certificate.frame_evidence.clone(),
        )
        .unwrap();
        reference.sha256 = digest_json(&reference.certificate).unwrap();
    }

    fn rehash_frames(reference: &mut BoardReference) {
        for frame in &mut reference.certificate.frame_evidence {
            frame.sha256 = digest_json(&frame.evidence).unwrap();
        }
        rebuild_board(reference);
    }

    fn identity(seed: char) -> CandidateIdentity {
        CandidateIdentity {
            qualification_candidate_id: seed.to_string().repeat(64),
            platform_bundle_id: seed.to_string().repeat(64),
            main_sha256: seed.to_string().repeat(64),
            gui_sha256: seed.to_string().repeat(64),
            manager_sha256: seed.to_string().repeat(64),
            scanout_module_sha256: seed.to_string().repeat(64),
            scanout_metadata_sha256: seed.to_string().repeat(64),
            latch_rbf_sha256: seed.to_string().repeat(64),
            latch_metadata_sha256: seed.to_string().repeat(64),
            main_revision: seed.to_string().repeat(40),
            magik_revision: seed.to_string().repeat(40),
        }
    }

    fn frame(board: &str, sink: &str, kind: TransitionKind, mode: &str) -> FrameEvidenceV1 {
        FrameEvidenceV1 {
            schema: FRAME_SCHEMA.into(),
            candidate: identity('a'),
            board_id: board.into(),
            sink_id: sink.into(),
            sink_chipset_id: format!("{sink}-chipset"),
            capture_id: format!("{board}-{sink}-{mode}-{kind:?}"),
            attended: true,
            capture_source: if mode.starts_with("crt-") {
                CaptureSource::PhysicalCrtAdc
            } else {
                CaptureSource::PhysicalHdmiRx
            },
            frame_complete_capture: true,
            transition_kind: kind,
            video_mode: mode.into(),
            expected_refresh_millihz: 60_000,
            capture_rate_millihz: 60_000,
            first_frame_sequence: 1,
            last_frame_sequence: 100_000,
            frames_observed: 100_000,
            correct_frames: 100_000,
            source_nonblack_frames: 100_000,
            transitions_observed: 10_000,
            classifications: FrameClassifications::default(),
            capture_sha256: "b".repeat(64),
            classifier_sha256: "c".repeat(64),
            classifier_report_path: "frames.ndjson".into(),
            classifier_report_sha256: "f".repeat(64),
        }
    }

    fn board(board_id: &str, sink: &str) -> BoardCertificateV1 {
        let mut evidence = Vec::new();
        for (index, kind) in TRANSITION_KINDS.into_iter().enumerate() {
            for mode in MODES {
                let mut item = frame(board_id, sink, kind, mode);
                item.capture_id = format!("{board_id}-{index}-{mode}");
                item.transitions_observed = if kind == TransitionKind::ArcadeReturn {
                    10_000
                } else {
                    2_000
                };
                evidence.push(EvidenceReference {
                    sha256: digest_json(&item).unwrap(),
                    evidence: item,
                });
            }
        }
        summarize_board(identity('a'), board_id.into(), evidence).unwrap()
    }

    fn aggregate() -> AggregateCertificateV1 {
        let boards = [
            ("board-1", "sink-a"),
            ("board-2", "sink-b"),
            ("board-3", "sink-a"),
        ]
        .into_iter()
        .map(|(board_id, sink)| {
            let certificate = board(board_id, sink);
            BoardReference {
                sha256: digest_json(&certificate).unwrap(),
                certificate,
            }
        })
        .collect();
        summarize_aggregate(identity('a'), boards).unwrap()
    }

    #[test]
    fn frame_evidence_rejects_sequence_gaps_and_below_refresh_capture() {
        let mut evidence = frame(
            "board-1",
            "sink-a",
            TransitionKind::ArcadeReturn,
            "hdmi-1920x1080p60",
        );
        evidence.last_frame_sequence += 1;
        assert!(verify_frame_evidence(&evidence).is_err());
        evidence.last_frame_sequence -= 1;
        evidence.capture_rate_millihz = 59_999;
        assert!(verify_frame_evidence(&evidence).is_err());

        evidence.capture_rate_millihz = 60_000;
        evidence.capture_source = CaptureSource::PhysicalCrtAdc;
        evidence.video_mode = "crt-480p60".into();
        verify_frame_evidence(&evidence).unwrap();
        evidence.frame_complete_capture = false;
        assert_code(
            verify_frame_evidence(&evidence),
            "non_physical_frame_evidence",
        );
    }

    #[test]
    fn frame_summary_is_bound_to_ingested_per_frame_classifier_report() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let directory = std::env::temp_dir().join(format!(
            "mister-magik-frame-evidence-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let report_path = directory.join("frames.ndjson");
        let evidence_path = directory.join("evidence.json");
        let correct = concat!(
            "{\"schema\":\"mister-magik-physical-frame-classification-v1\",\"frame_sequence\":1,\"source_nonblack\":true,\"classification\":\"correct\"}\n",
            "{\"schema\":\"mister-magik-physical-frame-classification-v1\",\"frame_sequence\":2,\"source_nonblack\":true,\"classification\":\"correct\"}\n"
        );
        fs::write(&report_path, correct).unwrap();
        let mut evidence = frame(
            "board-1",
            "sink-a",
            TransitionKind::ArcadeReturn,
            "hdmi-1920x1080p60",
        );
        evidence.last_frame_sequence = 2;
        evidence.frames_observed = 2;
        evidence.correct_frames = 2;
        evidence.source_nonblack_frames = 2;
        evidence.transitions_observed = 1;
        evidence.classifier_report_sha256 = digest(correct.as_bytes());
        write_json(&evidence_path, &evidence).unwrap();
        read_frame_evidence(&evidence_path).unwrap();

        let black = concat!(
            "{\"schema\":\"mister-magik-physical-frame-classification-v1\",\"frame_sequence\":1,\"source_nonblack\":true,\"classification\":\"correct\"}\n",
            "{\"schema\":\"mister-magik-physical-frame-classification-v1\",\"frame_sequence\":2,\"source_nonblack\":true,\"classification\":\"black\"}\n"
        );
        fs::write(&report_path, black).unwrap();
        evidence.classifier_report_sha256 = digest(black.as_bytes());
        write_json(&evidence_path, &evidence).unwrap();
        assert_code(
            read_frame_evidence(&evidence_path),
            "classifier_report_summary_mismatch",
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn every_nonzero_failure_classification_is_rejected() {
        for field in ["black", "stale", "partial", "banded", "corrupt"] {
            let mut evidence = frame(
                "board-1",
                "sink-a",
                TransitionKind::ArcadeReturn,
                "hdmi-1920x1080p60",
            );
            match field {
                "black" => evidence.classifications.black = 1,
                "stale" => evidence.classifications.stale = 1,
                "partial" => evidence.classifications.partial = 1,
                "banded" => evidence.classifications.banded = 1,
                "corrupt" => evidence.classifications.corrupt = 1,
                _ => unreachable!(),
            }
            assert!(verify_frame_evidence(&evidence).is_err(), "{field}");
        }
    }

    #[test]
    fn board_and_aggregate_reject_identity_mismatch() {
        let mut certificate = board("board-1", "sink-a");
        certificate.frame_evidence[0].evidence.candidate = identity('b');
        certificate.frame_evidence[0].sha256 =
            digest_json(&certificate.frame_evidence[0].evidence).unwrap();
        assert_code(
            verify_board_certificate(&certificate),
            "board_certificate_identity_mismatch",
        );

        let mut certificate = aggregate();
        certificate.boards[0].certificate.candidate = identity('b');
        for frame in &mut certificate.boards[0].certificate.frame_evidence {
            frame.evidence.candidate = identity('b');
            frame.sha256 = digest_json(&frame.evidence).unwrap();
        }
        rebuild_board(&mut certificate.boards[0]);
        assert_code(
            verify_aggregate_certificate(&certificate),
            "aggregate_candidate_identity_mismatch",
        );
    }

    #[test]
    fn aggregate_enforces_board_sink_and_transition_minimums() {
        let certificate = aggregate();
        verify_aggregate_certificate(&certificate).unwrap();

        let mut too_few_boards = certificate.clone();
        too_few_boards.boards.pop();
        too_few_boards = summarize_aggregate(identity('a'), too_few_boards.boards).unwrap();
        assert_code(
            verify_aggregate_certificate(&too_few_boards),
            "board_count_minimum",
        );

        let mut one_sink = certificate.clone();
        for board in &mut one_sink.boards {
            for frame in &mut board.certificate.frame_evidence {
                frame.evidence.sink_id = "sink-a".into();
                frame.evidence.sink_chipset_id = "chipset-a".into();
            }
            rehash_frames(board);
        }
        one_sink = summarize_aggregate(identity('a'), one_sink.boards).unwrap();
        assert_code(
            verify_aggregate_certificate(&one_sink),
            "sink_count_minimum",
        );

        let mut one_chipset = certificate.clone();
        for board in &mut one_chipset.boards {
            for frame in &mut board.certificate.frame_evidence {
                frame.evidence.sink_chipset_id = "chipset-a".into();
            }
            rehash_frames(board);
        }
        one_chipset = summarize_aggregate(identity('a'), one_chipset.boards).unwrap();
        assert_code(
            verify_aggregate_certificate(&one_chipset),
            "sink_chipset_count_minimum",
        );

        let mut too_few = certificate.clone();
        for frame in &mut too_few.boards[0].certificate.frame_evidence {
            frame.evidence.transitions_observed =
                if frame.evidence.transition_kind == TransitionKind::ArcadeReturn {
                    1_000
                } else {
                    500
                };
        }
        rehash_frames(&mut too_few.boards[0]);
        too_few = summarize_aggregate(identity('a'), too_few.boards).unwrap();
        assert_code(
            verify_aggregate_certificate(&too_few),
            "board_transition_minimum",
        );

        let mut weak_arcade = certificate.clone();
        for board in &mut weak_arcade.boards {
            for frame in &mut board.certificate.frame_evidence {
                if frame.evidence.transition_kind == TransitionKind::ArcadeReturn {
                    frame.evidence.transitions_observed = 4_000;
                }
            }
            rehash_frames(board);
        }
        weak_arcade = summarize_aggregate(identity('a'), weak_arcade.boards).unwrap();
        assert_code(
            verify_aggregate_certificate(&weak_arcade),
            "transition_kind_minimum",
        );

        let mut weak_mode = certificate;
        for board in &mut weak_mode.boards {
            for index in 0..TRANSITION_KINDS.len() {
                let weak_index = index * MODES.len();
                let moved = board.certificate.frame_evidence[weak_index]
                    .evidence
                    .transitions_observed
                    - 1;
                board.certificate.frame_evidence[weak_index]
                    .evidence
                    .transitions_observed = 1;
                board.certificate.frame_evidence[weak_index + 3]
                    .evidence
                    .transitions_observed += moved;
            }
            rehash_frames(board);
        }
        weak_mode = summarize_aggregate(identity('a'), weak_mode.boards).unwrap();
        assert_code(
            verify_aggregate_certificate(&weak_mode),
            "video_mode_minimum",
        );
    }
}
