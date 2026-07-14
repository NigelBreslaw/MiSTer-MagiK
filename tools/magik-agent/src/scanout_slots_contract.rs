//! Userspace view of the qualified scanout-slots kernel ABI.

pub(crate) const DEVICE: &str = "/dev/mister-magik-scanout-slots";
pub(crate) const ABI_VERSION: u32 = 1;
pub(crate) const GET_LAYOUT: libc::c_ulong = 0x8040_4d01;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ScanoutSlotLayout {
    pub(crate) physical_address: u32,
    pub(crate) mmap_offset_bytes: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ScanoutSlotsLayout {
    pub(crate) abi_version: u32,
    pub(crate) slot_count: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) stride_bytes: u32,
    pub(crate) frame_bytes: u32,
    pub(crate) map_bytes: u32,
    pub(crate) flags: u32,
    pub(crate) slots: [ScanoutSlotLayout; 2],
    pub(crate) reserved: [u32; 4],
}

pub(crate) const EXPECTED_LAYOUT: ScanoutSlotsLayout = ScanoutSlotsLayout {
    abi_version: ABI_VERSION,
    slot_count: 2,
    width: 960,
    height: 540,
    stride_bytes: 1920,
    frame_bytes: 1_036_800,
    map_bytes: 1_040_384,
    flags: 1,
    slots: [
        ScanoutSlotLayout {
            physical_address: 0x227e_9000,
            mmap_offset_bytes: 0,
        },
        ScanoutSlotLayout {
            physical_address: 0x22fd_2000,
            mmap_offset_bytes: 1_048_576,
        },
    ],
    reserved: [0; 4],
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_layout_matches_the_64_byte_uapi_structure() {
        assert_eq!(std::mem::size_of::<ScanoutSlotsLayout>(), 64);
        assert_eq!(GET_LAYOUT, 0x8040_4d01);
    }
}
