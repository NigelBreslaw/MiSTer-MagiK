// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

/// Empty MiSTer mount directories and USB input devices are not storage mounts.
pub fn connected() -> bool {
    std::fs::read_to_string("/proc/self/mountinfo")
        .is_ok_and(|mounts| contains_usb_storage_mount(&mounts))
}

fn contains_usb_storage_mount(mounts: &str) -> bool {
    mounts.lines().any(|line| {
        let Some(mountpoint) = line.split_ascii_whitespace().nth(4) else {
            return false;
        };
        matches!(
            mountpoint,
            "/media/usb0"
                | "/media/usb1"
                | "/media/usb2"
                | "/media/usb3"
                | "/media/usb4"
                | "/media/usb5"
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_mounts_exclude_sd_network_and_unmounted_directories() {
        let sd = "12 1 179:1 / /media/fat rw - exfat /dev/mmcblk0p1 rw";
        assert!(!contains_usb_storage_mount(sd));
        assert!(!contains_usb_storage_mount(""));
        assert!(!contains_usb_storage_mount("malformed"));
        assert!(!contains_usb_storage_mount(
            "13 1 0:1 / /media/network rw - cifs server rw"
        ));
        for index in 0..6 {
            assert!(contains_usb_storage_mount(&format!(
                "{sd}\n13 1 8:1 / /media/usb{index} rw - exfat /dev/sda1 rw"
            )));
        }
        assert!(!contains_usb_storage_mount(
            "13 1 8:1 / /media/usb1/games rw - ext4 /dev/sda1 rw"
        ));
    }
}
