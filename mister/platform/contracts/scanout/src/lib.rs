// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checked userspace representation of the kernel UAPI and qualified platform.

pub const DEVICE: &str = "/dev/mister-magik-scanout-slots";
pub const LEGACY_KERNEL_RELEASE: &str = "5.15.1-MiSTer";
pub const DEVELOPMENT_KERNEL_RELEASE: &str = "6.18.38-MiSTer";
pub const LEGACY_PLATFORM_CONTRACT_ID: &str = "mister-5.15.1-scanout-v3";
pub const DEVELOPMENT_PLATFORM_CONTRACT_ID: &str = "stock-6.18-latch-reuse-v3";
pub const DEVELOPMENT_KERNEL_REVISION: &str = "6a581bac47c32dfd2525f9874fd263cf08058610";
pub const DEVELOPMENT_PROVIDER_IDENTITY: &str = "stock-6.18-latch-reuse-v3";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformProfile {
    Legacy515,
    Development618,
}

impl PlatformProfile {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Legacy515 => LEGACY_PLATFORM_CONTRACT_ID,
            Self::Development618 => DEVELOPMENT_PLATFORM_CONTRACT_ID,
        }
    }

    pub const fn kernel_release(self) -> &'static str {
        match self {
            Self::Legacy515 => LEGACY_KERNEL_RELEASE,
            Self::Development618 => DEVELOPMENT_KERNEL_RELEASE,
        }
    }

    pub const fn kernel_revision(self) -> Option<&'static str> {
        match self {
            Self::Legacy515 => None,
            Self::Development618 => Some(DEVELOPMENT_KERNEL_REVISION),
        }
    }

    pub const fn provider_identity(self) -> Option<&'static str> {
        match self {
            Self::Legacy515 => None,
            Self::Development618 => Some(DEVELOPMENT_PROVIDER_IDENTITY),
        }
    }

    pub const fn development_only(self) -> bool {
        matches!(self, Self::Development618)
    }
}

pub const LEGACY_PROFILE: PlatformProfile = PlatformProfile::Legacy515;
pub const DEVELOPMENT_PROFILE: PlatformProfile = PlatformProfile::Development618;

pub fn resolve_profile(
    kernel_release: &str,
    platform_contract_id: Option<&str>,
    provider_identity: Option<&str>,
    development_layout: bool,
) -> Option<PlatformProfile> {
    if kernel_release == LEGACY_KERNEL_RELEASE
        && platform_contract_id.is_none_or(|value| value == LEGACY_PLATFORM_CONTRACT_ID)
        && provider_identity.is_none()
    {
        return Some(LEGACY_PROFILE);
    }
    if development_layout
        && kernel_release == DEVELOPMENT_KERNEL_RELEASE
        && platform_contract_id == Some(DEVELOPMENT_PLATFORM_CONTRACT_ID)
        && provider_identity == Some(DEVELOPMENT_PROVIDER_IDENTITY)
    {
        return Some(DEVELOPMENT_PROFILE);
    }
    None
}
pub const ABI_VERSION: u32 = 3;
pub const SLOT_COUNT: usize = 2;
pub const REGION_OFFSET_BYTES: usize = 8_294_400;
pub const MAX_WIDTH: usize = 1366;
pub const MAX_HEIGHT: usize = 768;
pub const MAX_STRIDE_BYTES: usize = 2736;
pub const MIN_QUALIFIED_UI_WIDTH: usize = 320;
pub const MIN_QUALIFIED_UI_HEIGHT: usize = 240;
pub const SLOT_CAPACITY_BYTES: usize = MAX_STRIDE_BYTES * MAX_HEIGHT;
pub const MAP_BYTES: usize = SLOT_CAPACITY_BYTES;
pub const LAYOUT_WRITE_COMBINE: u32 = 1;
pub const GET_LAYOUT: usize = 0x8040_4d01;
pub const UAPI_SHA256: &str = "1657c992fc863d173773685ece775be9e8cf51db4647e8db64c133fefc8dfaba";

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanoutSlotLayout {
    pub physical_address: u32,
    pub mmap_offset_bytes: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ScanoutSlotsLayout {
    pub abi_version: u32,
    pub slot_count: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub max_stride_bytes: u32,
    pub slot_capacity_bytes: u32,
    pub map_bytes: u32,
    pub flags: u32,
    pub slots: [ScanoutSlotLayout; SLOT_COUNT],
    pub reserved: [u32; 4],
}

pub const EXPECTED_LAYOUT: ScanoutSlotsLayout = ScanoutSlotsLayout {
    abi_version: ABI_VERSION,
    slot_count: SLOT_COUNT as u32,
    max_width: MAX_WIDTH as u32,
    max_height: MAX_HEIGHT as u32,
    max_stride_bytes: MAX_STRIDE_BYTES as u32,
    slot_capacity_bytes: SLOT_CAPACITY_BYTES as u32,
    map_bytes: MAP_BYTES as u32,
    flags: LAYOUT_WRITE_COMBINE,
    slots: [
        ScanoutSlotLayout {
            physical_address: 0x227e_9000,
            mmap_offset_bytes: 0,
        },
        ScanoutSlotLayout {
            physical_address: 0x22fd_2000,
            mmap_offset_bytes: REGION_OFFSET_BYTES as u32,
        },
    ],
    reserved: [0; 4],
};

const _: [(); 64] = [(); std::mem::size_of::<ScanoutSlotsLayout>()];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qualified_layout_and_ioctl_are_stable() {
        assert_eq!(std::mem::size_of::<ScanoutSlotsLayout>(), 64);
        assert_eq!(GET_LAYOUT, 0x8040_4d01);
        assert_eq!(EXPECTED_LAYOUT.slots[1].physical_address, 0x22fd_2000);
        assert_eq!(EXPECTED_LAYOUT.max_width, 1366);
        assert_eq!(EXPECTED_LAYOUT.max_height, 768);
        assert_eq!(EXPECTED_LAYOUT.max_stride_bytes, 2736);
        assert_eq!(EXPECTED_LAYOUT.slot_capacity_bytes, 2_101_248);
        assert_eq!(EXPECTED_LAYOUT.slot_capacity_bytes % 4096, 0);
        assert_eq!(
            (MIN_QUALIFIED_UI_WIDTH, MIN_QUALIFIED_UI_HEIGHT),
            (320, 240)
        );
        assert_eq!(EXPECTED_LAYOUT.slots[1].mmap_offset_bytes, 1920 * 1080 * 4);
    }

    #[test]
    fn legacy_profile_remains_compatible_with_historical_metadata() {
        assert_eq!(
            resolve_profile(LEGACY_KERNEL_RELEASE, None, None, false),
            Some(LEGACY_PROFILE)
        );
        assert_eq!(
            resolve_profile(
                LEGACY_KERNEL_RELEASE,
                Some(LEGACY_PLATFORM_CONTRACT_ID),
                None,
                true,
            ),
            Some(LEGACY_PROFILE)
        );
    }

    #[test]
    fn development_profile_requires_exact_identity_and_dev_layout() {
        assert_eq!(
            resolve_profile(
                DEVELOPMENT_KERNEL_RELEASE,
                Some(DEVELOPMENT_PLATFORM_CONTRACT_ID),
                Some(DEVELOPMENT_PROVIDER_IDENTITY),
                true,
            ),
            Some(DEVELOPMENT_PROFILE)
        );
        for (contract, provider, development) in [
            (
                Some(DEVELOPMENT_PLATFORM_CONTRACT_ID),
                Some(DEVELOPMENT_PROVIDER_IDENTITY),
                false,
            ),
            (Some(DEVELOPMENT_PLATFORM_CONTRACT_ID), None, true),
            (
                Some(LEGACY_PLATFORM_CONTRACT_ID),
                Some(DEVELOPMENT_PROVIDER_IDENTITY),
                true,
            ),
        ] {
            assert_eq!(
                resolve_profile(DEVELOPMENT_KERNEL_RELEASE, contract, provider, development),
                None
            );
        }
    }

    #[test]
    fn mixed_kernel_profiles_fail_closed() {
        assert_eq!(
            resolve_profile(
                LEGACY_KERNEL_RELEASE,
                Some(DEVELOPMENT_PLATFORM_CONTRACT_ID),
                Some(DEVELOPMENT_PROVIDER_IDENTITY),
                true,
            ),
            None
        );
        assert_eq!(
            resolve_profile(
                DEVELOPMENT_KERNEL_RELEASE,
                Some(LEGACY_PLATFORM_CONTRACT_ID),
                None,
                true,
            ),
            None
        );
        assert_eq!(resolve_profile("6.18.39-MiSTer", None, None, true), None);
    }
}
