// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Compile-time identity of the MiSTer MagiK executable producing diagnostics.

use serde::Serialize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct BuildIdentity {
    pub package_version: &'static str,
    pub version: &'static str,
    pub build_number: &'static str,
    pub source_revision: &'static str,
    pub source_dirty: Option<bool>,
    pub build_time: &'static str,
    pub arch: &'static str,
}

impl BuildIdentity {
    #[must_use]
    pub const fn current() -> Self {
        Self {
            package_version: env!("CARGO_PKG_VERSION"),
            version: env!("MISTER_MAGIK_VERSION"),
            build_number: env!("MISTER_MAGIK_BUILD_NUMBER"),
            source_revision: env!("MISTER_MAGIK_SOURCE_REVISION"),
            source_dirty: parse_dirty(env!("MISTER_MAGIK_SOURCE_DIRTY")),
            build_time: env!("MISTER_MAGIK_BUILD_TIME"),
            arch: std::env::consts::ARCH,
        }
    }

    #[must_use]
    pub const fn source_dirty_label(self) -> &'static str {
        match self.source_dirty {
            Some(false) => "0",
            Some(true) => "1",
            None => "unknown",
        }
    }

    #[must_use]
    pub fn log_detail(self) -> String {
        format!(
            "version={} build_number={} source_revision={} source_dirty={} build_time={} arch={}",
            self.version,
            self.build_number,
            self.source_revision,
            self.source_dirty_label(),
            self.build_time,
            self.arch
        )
    }
}

const fn parse_dirty(value: &str) -> Option<bool> {
    match value.as_bytes() {
        b"0" | b"false" => Some(false),
        b"1" | b"true" => Some(true),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_dirty_metadata_supports_unknown_development_builds() {
        assert_eq!(parse_dirty("0"), Some(false));
        assert_eq!(parse_dirty("true"), Some(true));
        assert_eq!(parse_dirty("unknown"), None);
    }

    #[test]
    fn current_identity_contains_all_embedded_fields() {
        let identity = BuildIdentity::current();

        assert!(!identity.package_version.is_empty());
        assert!(!identity.version.is_empty());
        assert!(!identity.build_number.is_empty());
        assert!(!identity.source_revision.is_empty());
        assert!(!identity.build_time.is_empty());
        assert!(!identity.arch.is_empty());
    }
}
