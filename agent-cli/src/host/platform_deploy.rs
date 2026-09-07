// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use super::installed_layout::{arming_paths, paths};
use super::{Layout, sh};

const SNES_ARTWORK_SHA256: &str =
    "7a76993e7e1b0063832b94e9d2ad588549587cf09a14ac2ced72d349ed12f766";
const SETTINGS_ARTWORK_SHA256: &str =
    "44d657ff706a49fd8c8999b7c02ea4cdb7e4a8488a54dc68e0b79235dc40e8ec";

pub(super) fn installed_platform_verify_command(layout: Layout) -> String {
    let installed = paths(layout);
    let root = installed.root;
    let main = installed.main;
    format!(
        r#"set -eu
root={root}
manifest="$root/platform-v3.manifest"
fail() {{ printf 'platform verification: %s\n' "$1" >&2; exit 1; }}
require() {{
    label="$1"
    path="$2"
    mode="$3"
    if [ "$mode" = x ] && [ ! -x "$path" ]; then fail "$label is missing or not executable: $path"; fi
    if [ "$mode" = r ] && [ ! -r "$path" ]; then fail "$label is missing or unreadable: $path"; fi
}}
check_hash() {{
    label="$1"
    path="$2"
    expected="$3"
    actual=$(sha256sum "$path" | awk '{{print $1}}')
    if [ "$actual" != "$expected" ]; then
        fail "$label hash mismatch path=$path expected=$expected actual=$actual"
    fi
}}
test -s "$manifest" || fail "manifest is missing or empty: $manifest"
require "Main" {main} x
require "GUI" "$root/mister-magik-fb" x
require "manager" "$root/mister-magik-manager" x
require "scanout module" "$root/mister_magik_scanout_slots.ko" r
require "scanout metadata" "$root/mister_magik_scanout_slots.metadata.txt" r
require "FPGA RBF" "$root/fpga/menu-magik-vblank-latch.rbf" r
require "FPGA metadata" "$root/fpga/menu-magik-vblank-latch.metadata.txt" r
require "SNES artwork" "$root/assets/snes/snes-small-v1.rgb565a" r
require "settings artwork" "$root/assets/ui/settings-v1.rgb565a" r
grep -qx 'format={manifest_format}' "$manifest" || fail "manifest format is not {manifest_format}"
get() {{
    values=$(sed -n "s/^$1=//p" "$manifest")
    [ -n "$values" ] || fail "manifest key is missing or empty: $1"
    count=$(printf '%s\n' "$values" | wc -l | tr -d ' ')
    [ "$count" -eq 1 ] || fail "manifest key is duplicated: $1"
    printf '%s' "$values"
}}
check_hash "Main" {main} "$(get main_sha256)"
check_hash "GUI" "$root/mister-magik-fb" "$(get gui_sha256)"
check_hash "manager" "$root/mister-magik-manager" "$(get manager_sha256)"
check_hash "scanout module" "$root/mister_magik_scanout_slots.ko" "$(get scanout_module_sha256)"
check_hash "scanout metadata" "$root/mister_magik_scanout_slots.metadata.txt" "$(get scanout_metadata_sha256)"
check_hash "FPGA RBF" "$root/fpga/menu-magik-vblank-latch.rbf" "$(get latch_rbf_sha256)"
check_hash "FPGA metadata" "$root/fpga/menu-magik-vblank-latch.metadata.txt" "$(get latch_metadata_sha256)"
check_hash "SNES artwork" "$root/assets/snes/snes-small-v1.rgb565a" "{SNES_ARTWORK_SHA256}"
check_hash "settings artwork" "$root/assets/ui/settings-v1.rgb565a" "{SETTINGS_ARTWORK_SHA256}""#,
        root = sh(root),
        main = sh(main),
        manifest_format = crate::platform_manifest::FORMAT,
    )
}

pub(super) fn platform_safety_script() -> String {
    let paths = arming_paths()
        .iter()
        .map(|path| sh(path))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "for path in {paths}; do if test -e \"$path\"; then printf 'platform safety blocked: %s\\n' \"$path\" >&2; exit 1; fi; done",
    )
}
