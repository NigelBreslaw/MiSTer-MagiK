// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

const FORCE_FRAME_POINTERS: &str = "force-frame-pointers=yes";

/// Returns whether Cargo passed Rust's force-frame-pointers codegen option.
///
/// Build scripts receive Rust flags in Cargo's unit-separated encoded form.
/// Keep the parser local to the shared build support so every ARM C build
/// applies the same profiling policy without letting `cc` translate unrelated
/// Rust flags into C flags.
#[allow(dead_code)]
pub(crate) fn force_frame_pointers_requested() -> bool {
    std::env::var_os("CARGO_ENCODED_RUSTFLAGS")
        .map(|flags| force_frame_pointers_requested_in(&flags.to_string_lossy()))
        .unwrap_or(false)
}

#[allow(dead_code)]
fn force_frame_pointers_requested_in(flags: &str) -> bool {
    let mut expects_codegen_value = false;

    for flag in flags.split('\u{1f}') {
        if expects_codegen_value {
            expects_codegen_value = false;
            if flag == FORCE_FRAME_POINTERS {
                return true;
            }
        }

        match flag {
            "-C" | "--codegen" => expects_codegen_value = true,
            "-Cforce-frame-pointers=yes" | "--codegen=force-frame-pointers=yes" => {
                return true;
            }
            _ => {}
        }
    }

    false
}

#[cfg(test)]
mod tests {
    use super::force_frame_pointers_requested_in;

    #[test]
    fn no_force_frame_pointer_flag_is_not_profiled() {
        assert!(!force_frame_pointers_requested_in(
            "-C\u{1f}target-cpu=cortex-a9"
        ));
    }

    #[test]
    fn separated_force_frame_pointer_flag_is_detected() {
        assert!(force_frame_pointers_requested_in(
            "-D\u{1f}warnings\u{1f}-C\u{1f}force-frame-pointers=yes"
        ));
    }

    #[test]
    fn combined_force_frame_pointer_flag_is_detected() {
        assert!(force_frame_pointers_requested_in(
            "-Cforce-frame-pointers=yes\u{1f}target-cpu=cortex-a9"
        ));
    }
}
