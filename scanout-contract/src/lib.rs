// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checked userspace representation of the kernel UAPI and qualified platform.

pub const DEVICE: &str = "/dev/mister-magik-scanout-slots";
pub const QUALIFIED_KERNEL_RELEASE: &str = "5.15.1-MiSTer";
pub const PLATFORM_CONTRACT_ID: &str = "mister-5.15.1-scanout-v2";
pub const ABI_VERSION: u32 = 2;
pub const SLOT_COUNT: usize = 2;
pub const REGION_OFFSET_BYTES: usize = 8_294_400;
pub const MAX_WIDTH: usize = 1280;
pub const MAX_HEIGHT: usize = 720;
pub const MAX_STRIDE_BYTES: usize = MAX_WIDTH * 2;
pub const SLOT_CAPACITY_BYTES: usize = 1_843_200;
pub const MAP_BYTES: usize = SLOT_CAPACITY_BYTES;
pub const LAYOUT_WRITE_COMBINE: u32 = 1;
pub const GET_LAYOUT: usize = 0x8040_4d01;
pub const UAPI_SHA256: &str = "ee9c6ef38adc995dc5b182371a13e2db59c1edb5b118a68ac8bdbc555c0e0e11";

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
        assert_eq!(EXPECTED_LAYOUT.slot_capacity_bytes, 1280 * 720 * 2);
        assert_eq!(EXPECTED_LAYOUT.slots[1].mmap_offset_bytes, 1920 * 1080 * 4);
    }
}
