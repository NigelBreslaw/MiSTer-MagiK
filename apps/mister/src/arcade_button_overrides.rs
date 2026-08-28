// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use quick_xml::Reader as XmlReader;
use quick_xml::XmlVersion;
use quick_xml::events::{BytesStart, Event};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const BUTTON_OVERRIDES_PATH: &str = "/tmp/mister-magik/button-overrides";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ButtonOverrideValue {
    Button(&'static str),
    Unmap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ButtonOverride {
    pub index: usize,
    pub value: ButtonOverrideValue,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MraButtons {
    labels: Vec<String>,
    defaults: Vec<Option<String>>,
}

pub fn button_overrides_for_mra(path: &Path) -> Result<Vec<ButtonOverride>, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read MRA {}: {e}", path.display()))?;
    Ok(button_overrides_from_mra_text(&text))
}

pub fn write_button_overrides_for_mra(path: &Path) -> Result<(), String> {
    let mut fault_control =
        mister_magik_mister_runtime::direct_reset_fault::process_fault_control();
    write_button_overrides_for_mra_with_fault_control(path, &mut fault_control)
}

pub fn write_button_overrides_for_mra_with_fault_control(
    path: &Path,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    let overrides = button_overrides_for_mra(path)?;
    write_button_overrides_with_fault_control(&overrides, fault_control)
}

pub fn write_button_overrides(overrides: &[ButtonOverride]) -> Result<(), String> {
    let mut fault_control =
        mister_magik_mister_runtime::direct_reset_fault::process_fault_control();
    write_button_overrides_with_fault_control(overrides, &mut fault_control)
}

pub fn write_button_overrides_with_fault_control(
    overrides: &[ButtonOverride],
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    write_button_overrides_to_path(overrides, Path::new(BUTTON_OVERRIDES_PATH), fault_control)
}

fn write_button_overrides_to_path(
    overrides: &[ButtonOverride],
    path: &Path,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    if overrides.is_empty() {
        remove_button_overrides_at(path, fault_control)?;
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "failed to create button override dir {}: {e}",
                parent.display()
            )
        })?;
    }

    let tmp = temp_path(path);
    let mut file =
        fs::File::create(&tmp).map_err(|e| format!("failed to create {}: {e}", tmp.display()))?;
    writeln!(file, "schema=1").map_err(|e| format!("failed to write {}: {e}", tmp.display()))?;
    for override_entry in overrides {
        let value = match override_entry.value {
            ButtonOverrideValue::Button(button) => button,
            ButtonOverrideValue::Unmap => "unmap",
        };
        writeln!(file, "{}={value}", override_entry.index)
            .map_err(|e| format!("failed to write {}: {e}", tmp.display()))?;
    }
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "button_overrides.after_temp_write",
        path,
        fault_control,
    );
    file.sync_all()
        .map_err(|e| format!("failed to sync {}: {e}", tmp.display()))?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "button_overrides.after_temp_sync",
        path,
        fault_control,
    );
    drop(file);
    fs::rename(&tmp, path).map_err(|e| {
        format!(
            "failed to rename {} to {}: {e}",
            tmp.display(),
            path.display()
        )
    })?;
    mister_magik_catalog::fs_fault::maybe_fault_with_control(
        "button_overrides.after_rename",
        path,
        fault_control,
    );
    Ok(())
}

pub fn remove_button_overrides() -> Result<(), String> {
    let mut fault_control =
        mister_magik_mister_runtime::direct_reset_fault::process_fault_control();
    remove_button_overrides_with_fault_control(&mut fault_control)
}

pub fn remove_button_overrides_with_fault_control(
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    remove_button_overrides_at(Path::new(BUTTON_OVERRIDES_PATH), fault_control)
}

fn remove_button_overrides_at(
    path: &Path,
    fault_control: &mut dyn mister_magik_catalog::fs_fault::DirectResetFaultControl,
) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => {
            mister_magik_catalog::fs_fault::maybe_fault_with_control(
                "button_overrides.after_remove",
                path,
                fault_control,
            );
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("failed to remove {}: {e}", path.display())),
    }
}

fn temp_path(path: &Path) -> PathBuf {
    let mut tmp = path.to_path_buf();
    tmp.set_extension(format!("tmp.{}", std::process::id()));
    tmp
}

fn button_overrides_from_mra_text(text: &str) -> Vec<ButtonOverride> {
    let Some(buttons) = parse_mra_buttons(text) else {
        return Vec::new();
    };
    button_overrides_for_buttons(&buttons)
}

fn parse_mra_buttons(text: &str) -> Option<MraButtons> {
    let mut reader = XmlReader::from_str(text);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) | Ok(Event::Empty(e))
                if e.name()
                    .as_ref()
                    .as_bytes()
                    .eq_ignore_ascii_case(b"buttons") =>
            {
                let names = xml_attr_value(&e, b"names")?;
                let defaults = xml_attr_value(&e, b"default").unwrap_or_default();
                let labels = split_csv(&names);
                let default_tokens = split_csv(&defaults);
                return Some(MraButtons {
                    defaults: defaults_by_label_index(&labels, &default_tokens),
                    labels,
                });
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
    }
}

fn split_csv(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .map(ToOwned::to_owned)
        .collect()
}

fn defaults_by_label_index(labels: &[String], defaults: &[String]) -> Vec<Option<String>> {
    let mut out = Vec::with_capacity(labels.len());
    let mut default_index = 0;
    for label in labels {
        if label == "-" {
            out.push(None);
        } else {
            out.push(defaults.get(default_index).cloned());
            default_index += 1;
        }
    }
    out
}

fn button_overrides_for_buttons(buttons: &MraButtons) -> Vec<ButtonOverride> {
    let mut out = Vec::new();
    let coin_count = buttons
        .labels
        .iter()
        .filter(|label| is_coin_like(label))
        .count();
    let start_count = buttons
        .labels
        .iter()
        .filter(|label| is_start_like(label))
        .count();
    let l_used_by_gameplay = l_used_by_gameplay(buttons);

    for (index, label) in buttons.labels.iter().enumerate() {
        if label == "-" {
            continue;
        }
        if is_core_credits(label) || is_service_or_test(label) {
            out.push(unmap(index));
        } else if is_coin_like(label) {
            if is_p2_label(label) {
                out.push(unmap(index));
            } else if is_ambiguous_compound_admin(label) {
                continue;
            } else if coin_count == 1 || is_p1_label(label) {
                out.push(button(index, "Select"));
            } else {
                out.push(unmap(index));
            }
        } else if is_start_like(label) {
            if is_p2_label(label) {
                out.push(unmap(index));
            } else if is_ambiguous_compound_admin(label) {
                continue;
            } else if start_count == 1 || is_p1_label(label) {
                out.push(button(index, "Start"));
            } else {
                out.push(unmap(index));
            }
        } else if is_pause(label) && !l_used_by_gameplay {
            out.push(button(index, "L"));
        }
    }
    out
}

fn l_used_by_gameplay(buttons: &MraButtons) -> bool {
    buttons
        .labels
        .iter()
        .zip(buttons.defaults.iter())
        .any(|(label, default)| {
            label != "-"
                && !is_admin_label(label)
                && default.as_deref().is_some_and(|value| {
                    value.eq_ignore_ascii_case("L") || value.eq_ignore_ascii_case("LT")
                })
        })
}

fn button(index: usize, value: &'static str) -> ButtonOverride {
    ButtonOverride {
        index,
        value: ButtonOverrideValue::Button(value),
    }
}

fn unmap(index: usize) -> ButtonOverride {
    ButtonOverride {
        index,
        value: ButtonOverrideValue::Unmap,
    }
}

fn compact(label: &str) -> String {
    label
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn is_coin_like(label: &str) -> bool {
    let compact = compact(label);
    compact.contains("coin") || compact.contains("credit")
}

fn is_start_like(label: &str) -> bool {
    compact(label).contains("start")
}

fn is_core_credits(label: &str) -> bool {
    compact(label) == "corecredits"
}

fn is_service_or_test(label: &str) -> bool {
    let compact = compact(label);
    compact.contains("service") || compact == "test" || compact == "testcredit"
}

fn is_pause(label: &str) -> bool {
    compact(label) == "pause"
}

fn is_admin_label(label: &str) -> bool {
    is_core_credits(label)
        || is_service_or_test(label)
        || is_coin_like(label)
        || is_start_like(label)
        || is_pause(label)
}

fn is_p2_label(label: &str) -> bool {
    let compact = compact(label);
    compact == "coinb"
        || compact == "startb"
        || compact == "coin2"
        || compact == "start2"
        || compact == "start2p"
        || compact.contains("player2")
        || compact.contains("p2")
        || compact.contains("2p")
}

fn is_p1_label(label: &str) -> bool {
    let compact = compact(label);
    compact == "coin"
        || compact == "start"
        || compact == "coina"
        || compact == "starta"
        || compact == "coin1"
        || compact == "coint1"
        || compact == "start1"
        || compact == "start1p"
        || compact.contains("player1")
        || compact.contains("p1")
        || compact.contains("1p")
}

fn is_ambiguous_compound_admin(label: &str) -> bool {
    (label.contains('/') || label.contains('+')) && (is_coin_like(label) || is_start_like(label))
}

fn xml_attr_value(e: &BytesStart<'_>, key: &[u8]) -> Option<String> {
    e.attributes()
        .with_checks(false)
        .flatten()
        .find(|attr| attr.key.as_ref().as_bytes().eq_ignore_ascii_case(key))
        .and_then(|attr| {
            attr.normalized_value(XmlVersion::Implicit1_0)
                .ok()
                .map(|value| value.into_owned())
        })
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Default)]
    struct RecordingFaultControl {
        points: Vec<String>,
    }

    impl mister_magik_catalog::fs_fault::DirectResetFaultControl for RecordingFaultControl {
        fn request_direct_reset(
            &mut self,
            request: &mister_magik_catalog::fs_fault::DirectResetFaultRequest,
        ) -> mister_magik_catalog::fs_fault::DirectResetFaultOutcome {
            self.points.push(request.point().to_string());
            mister_magik_catalog::fs_fault::DirectResetFaultOutcome::Noop
        }
    }

    fn overrides(names: &str, defaults: &str) -> Vec<ButtonOverride> {
        button_overrides_from_mra_text(&format!(
            r#"<misterromdescription><buttons names="{names}" default="{defaults}" /></misterromdescription>"#
        ))
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "mister-magik-button-overrides-{name}-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    #[test]
    fn air_gallet_maps_coin_start_pause() {
        assert_eq!(
            overrides("Shot,Bomb,-,-,Start,Coin,Pause", "A,B,X,R,L,Start"),
            vec![button(4, "Start"), button(5, "Select"), button(6, "L")]
        );
    }

    #[test]
    fn p1_p2_coin_and_start_split() {
        assert_eq!(
            overrides("Button1,Button2,Start1P,Start2P,CoinA,CoinB", "A,B,X,Y,R,L"),
            vec![button(2, "Start"), unmap(3), button(4, "Select"), unmap(5)]
        );
    }

    #[test]
    fn spaced_p1_p2_labels_split() {
        assert_eq!(
            overrides("P1 Start,P2 Start,Coin 1,Coin 2", "X,Y,R,L"),
            vec![button(0, "Start"), unmap(1), button(2, "Select"), unmap(3)]
        );
    }

    #[test]
    fn admin_metadata_unmaps() {
        assert_eq!(
            overrides("Start,Coin,Core credits,Service,Test", "Start,Select,-,L,R"),
            vec![
                button(0, "Start"),
                button(1, "Select"),
                unmap(2),
                unmap(3),
                unmap(4)
            ]
        );
    }

    #[test]
    fn pause_does_not_steal_gameplay_l() {
        assert_eq!(
            overrides("Shot,Grenade,Rotate Left,Rotate Right,Pause", "A,B,L,R,X"),
            Vec::<ButtonOverride>::new()
        );
    }

    #[test]
    fn p2_admin_l_does_not_block_pause() {
        assert_eq!(
            overrides(
                "Shot,P1 Start,P2 Start,Coin A,Coin B,Pause",
                "A,R,L,Start,L,X"
            ),
            vec![
                button(1, "Start"),
                unmap(2),
                button(3, "Select"),
                unmap(4),
                button(5, "L")
            ]
        );
    }

    #[test]
    fn ambiguous_compound_admin_does_not_override() {
        assert_eq!(
            overrides("Magic/Start,Fire,-,-,Coin,-", "B,A,-,-,R,-"),
            vec![button(4, "Select")]
        );
    }

    #[test]
    fn writes_expected_button_override_file_for_mra_mapping() {
        let dir = temp_test_dir("write");
        let output = dir.join("nested").join("button-overrides");
        let mra = dir.join("mapped.mra");
        fs::write(
            &mra,
            r#"<misterromdescription><buttons names="P1 Start,P2 Start,Coin 1,Coin 2,Pause" default="X,Y,R,L,A" /></misterromdescription>"#,
        )
        .expect("write mra fixture");

        let overrides = button_overrides_for_mra(&mra).expect("parse mra overrides");
        let mut fault_control = RecordingFaultControl::default();
        write_button_overrides_to_path(&overrides, &output, &mut fault_control)
            .expect("write overrides");

        assert_eq!(
            fs::read_to_string(&output).expect("read override file"),
            "schema=1\n0=Start\n1=unmap\n2=Select\n3=unmap\n4=L\n"
        );
        assert!(
            !temp_path(&output).exists(),
            "temporary override file should be atomically renamed away"
        );
        assert_eq!(
            fault_control.points,
            vec![
                "button_overrides.after_temp_write",
                "button_overrides.after_temp_sync",
                "button_overrides.after_rename",
            ]
        );

        fs::remove_dir_all(dir).expect("remove temp test dir");
    }

    #[test]
    fn empty_override_set_removes_stale_file() {
        let dir = temp_test_dir("remove");
        let output = dir.join("button-overrides");
        fs::write(&output, "schema=1\n0=Start\n").expect("write stale override file");

        let mut fault_control = RecordingFaultControl::default();
        write_button_overrides_to_path(&[], &output, &mut fault_control)
            .expect("remove stale overrides");

        assert!(!output.exists());
        assert_eq!(fault_control.points, vec!["button_overrides.after_remove"]);

        fs::remove_dir_all(dir).expect("remove temp test dir");
    }
}
