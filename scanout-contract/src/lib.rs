// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Checked userspace representation of the kernel UAPI and qualified platform.

pub const DEVICE: &str = "/dev/mister-magik-scanout-slots";
pub const ABI_VERSION: u32 = 1;
pub const SLOT_COUNT: usize = 2;
pub const REGION_OFFSET_BYTES: usize = 1024 * 1024;
pub const FRAME_BYTES: usize = 960 * 540 * 2;
pub const MAP_BYTES: usize = 1_040_384;
pub const LAYOUT_WRITE_COMBINE: u32 = 1;
pub const GET_LAYOUT: usize = 0x8040_4d01;
pub const UAPI_SHA256: &str = "51b6f4a53efb76abbe1f5f1f1c7c248007aeb443f3a337ba86981940748c8fd6";

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
    pub width: u32,
    pub height: u32,
    pub stride_bytes: u32,
    pub frame_bytes: u32,
    pub map_bytes: u32,
    pub flags: u32,
    pub slots: [ScanoutSlotLayout; SLOT_COUNT],
    pub reserved: [u32; 4],
}

pub const EXPECTED_LAYOUT: ScanoutSlotsLayout = ScanoutSlotsLayout {
    abi_version: ABI_VERSION,
    slot_count: SLOT_COUNT as u32,
    width: 960,
    height: 540,
    stride_bytes: 1920,
    frame_bytes: FRAME_BYTES as u32,
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
    }
}
