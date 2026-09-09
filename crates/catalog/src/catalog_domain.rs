// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Validated scan-unit identifiers stored in catalog manifests.

use serde::{Deserialize, Serialize};
use std::error::Error;
use std::fmt;

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ScanUnitId(String);

impl ScanUnitId {
    pub fn parse(value: &str) -> Result<Self, CatalogDomainError> {
        let value = value.trim().to_ascii_lowercase().replace('_', "-");
        if value.is_empty()
            || value.len() > 64
            || value.starts_with('-')
            || value.ends_with('-')
            || value.contains("--")
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        {
            return Err(CatalogDomainError::new("invalid scan-unit ID"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogDomainError {
    message: &'static str,
}

impl CatalogDomainError {
    fn new(message: &'static str) -> Self {
        Self { message }
    }
}

impl fmt::Display for CatalogDomainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl Error for CatalogDomainError {}
