// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::redact::redact_args;
use std::ffi::OsString;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawRequest {
    pub id: String,
    pub args: Vec<String>,
    pub started_ms: i64,
    pub started: Instant,
}

impl RawRequest {
    #[must_use]
    pub fn capture(args: impl IntoIterator<Item = OsString>) -> Self {
        let started = Instant::now();
        let duration = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        Self {
            id: format!("{:x}-{:x}", duration.as_nanos(), std::process::id()),
            args: redact_args(args),
            started_ms: duration.as_millis().try_into().unwrap_or(i64::MAX),
            started,
        }
    }
}
