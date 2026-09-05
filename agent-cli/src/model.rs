// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum BenchmarkScenario {
    InputIntegrity,
}

impl BenchmarkScenario {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::InputIntegrity => "input-integrity",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Rejected,
    NoOp,
    ExternalRequired,
}
