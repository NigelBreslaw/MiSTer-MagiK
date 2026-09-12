// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Private development-trial extension; the production layout ABI is unchanged.

pub const ABI_VERSION: u32 = 2;
pub const GET_DIAGNOSTIC: usize = 0x8040_4d03;
pub const UAPI_SHA256: &str = "4742c44803550a75f9e0519aa2c7aaddd259d877c3283ad2b194c40fea8fe9ad";
pub const NO_PAGE: u32 = u32::MAX;
pub const NOT_ATTEMPTED: u32 = 0;
pub const IN_PROGRESS: u32 = 1;
pub const PASSED: u32 = 2;
pub const FAILED: u32 = 3;
pub const FAILURE_REMAP: u32 = 1;
pub const FAILURE_LOOKUP: u32 = 1 << 1;
pub const FAILURE_PFN: u32 = 1 << 2;
pub const FAILURE_WRITABLE: u32 = 1 << 3;
pub const FAILURE_WC: u32 = 1 << 4;
pub const FAILURE_XN: u32 = 1 << 5;
pub const FAILURE_READONLY: u32 = 1 << 6;
pub const FAILURE_SHARED: u32 = 1 << 7;
pub const VERIFIED_VMA_FLAGS: u32 = 1;
pub const VERIFIED_WC: u32 = 1 << 1;
pub const VERIFIED_XN: u32 = 1 << 2;
pub const VERIFIED_WRITABLE_PROTECTION: u32 = 1 << 3;
pub const VERIFIED_SHARED: u32 = 1 << 4;
pub const VERIFIED_PROTECTION_SUPPLIED: u32 = 1 << 5;
pub const VERIFIED_PFNS: u32 = 1 << 6;
pub const VERIFIED_WRITABILITY: u32 = 1 << 7;
pub const PROTECTION_READBACK_UNAVAILABLE: u32 = 1 << 8;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MappingDiagnostic {
    pub abi_version: u32,
    pub record_bytes: u32,
    pub state: u32,
    pub failure_flags: u32,
    pub page_index: u32,
    pub expected_pfn: u32,
    pub observed_pfn: u32,
    pub writable: u32,
    pub protection_mask: u32,
    pub expected_protection: u32,
    pub observed_protection: u32,
    pub error_code: i32,
    pub physical_base: u32,
    pub map_bytes: u32,
    pub verification_flags: u32,
    pub reserved: u32,
}

const _: [(); 64] = [(); std::mem::size_of::<MappingDiagnostic>()];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_layout_and_flags_match_the_trial_uapi() {
        assert_eq!(std::mem::size_of::<MappingDiagnostic>(), 64);
        assert_eq!(ABI_VERSION, 2);
        assert_eq!(GET_DIAGNOSTIC, 0x8040_4d03);
        assert_eq!(NO_PAGE, u32::MAX);
        assert_eq!(FAILURE_REMAP | FAILURE_LOOKUP | FAILURE_PFN, 0x07);
        assert_eq!(FAILURE_WRITABLE | FAILURE_WC | FAILURE_XN, 0x38);
        assert_eq!(FAILURE_READONLY | FAILURE_SHARED, 0xc0);
        assert_eq!(PROTECTION_READBACK_UNAVAILABLE, 0x100);
    }
}
