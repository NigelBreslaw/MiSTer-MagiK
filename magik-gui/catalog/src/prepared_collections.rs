//! Shared metadata for collections that provide their own one-click launch artifacts.

use std::fmt;

pub const PREPARED_COLLECTION_ADAPTER_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreparedCollectionId {
    AmigaVision,
    ZeroMhz,
    Neon68k,
    OneLoad64,
}

impl PreparedCollectionId {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AmigaVision => "amigavision",
            Self::ZeroMhz => "0mhz",
            Self::Neon68k => "neon68k",
            Self::OneLoad64 => "oneload64",
        }
    }
}

impl fmt::Display for PreparedCollectionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchQuality {
    Prepared,
    Generic,
}

impl LaunchQuality {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Prepared => "prepared",
            Self::Generic => "generic",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PreparedLaunchProvenance {
    pub collection_id: PreparedCollectionId,
    pub launch_quality: LaunchQuality,
    pub adapter_version: u32,
}

impl PreparedLaunchProvenance {
    pub const fn prepared(collection_id: PreparedCollectionId) -> Self {
        Self {
            collection_id,
            launch_quality: LaunchQuality::Prepared,
            adapter_version: PREPARED_COLLECTION_ADAPTER_VERSION,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepared_provenance_uses_stable_storage_values() {
        let provenance = PreparedLaunchProvenance::prepared(PreparedCollectionId::ZeroMhz);
        assert_eq!(provenance.collection_id.as_str(), "0mhz");
        assert_eq!(provenance.launch_quality.as_str(), "prepared");
        assert_eq!(
            provenance.adapter_version,
            PREPARED_COLLECTION_ADAPTER_VERSION
        );
    }
}
