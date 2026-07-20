// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use crate::redact::redact_args;
use std::ffi::OsString;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RawRequest {
    pub id: String,
    pub args: Vec<String>,
}

impl RawRequest {
    #[must_use]
    pub fn capture(args: impl IntoIterator<Item = OsString>) -> Self {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            id: format!("{now:x}-{:x}", std::process::id()),
            args: redact_args(args),
        }
    }
}
