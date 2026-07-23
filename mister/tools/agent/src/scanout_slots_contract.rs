// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

pub(crate) use mister_magik_scanout_contract::{
    DEVICE, EXPECTED_LAYOUT, GET_LAYOUT, ScanoutSlotsLayout,
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_layout_matches_the_shared_contract() {
        assert_eq!(std::mem::size_of::<ScanoutSlotsLayout>(), 64);
        assert_eq!(GET_LAYOUT, 0x8040_4d01);
    }
}
