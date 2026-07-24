// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

//! Supported launcher resolutions and comment-preserving MiSTer.ini persistence.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::Path;

pub const MISTER_INI_PATH: &str = "/media/fat/MiSTer.ini";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DisplayResolution {
    pub id: &'static str,
    pub label: &'static str,
    pub output_w: u16,
    pub output_h: u16,
    pub video_mode: Option<&'static str>,
    pub direct_video: u8,
    pub menu_pal: u8,
    pub forced_scandoubler: u8,
}

const AUTOMATIC_DISPLAY_RESOLUTION: DisplayResolution = DisplayResolution {
    id: "auto",
    label: "Automatic (HDMI / VGA DAC)",
    output_w: 0,
    output_h: 0,
    video_mode: None,
    direct_video: 2,
    menu_pal: 0,
    forced_scandoubler: 0,
};

pub const DISPLAY_RESOLUTIONS: &[DisplayResolution] = &[
    DisplayResolution {
        id: "hdmi-1280x720p60",
        label: "1280×720 (16:9)",
        output_w: 1280,
        output_h: 720,
        video_mode: Some("0"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "hdmi-1366x768p60",
        label: "1366×768 (16:9)",
        output_w: 1366,
        output_h: 768,
        video_mode: Some("10"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "hdmi-1920x1080p60",
        label: "1920×1080 (16:9)",
        output_w: 1920,
        output_h: 1080,
        video_mode: Some("8"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "hdmi-1920x1200p60",
        label: "1920×1200 (16:10)",
        output_w: 1920,
        output_h: 1200,
        video_mode: Some("1920,1200,60"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "hdmi-2048x1536p60",
        label: "2048×1536 (4:3)",
        output_w: 2048,
        output_h: 1536,
        video_mode: Some("13"),
        direct_video: 0,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "crt-240p60",
        label: "CRT 240p 60hz NTSC",
        output_w: 640,
        output_h: 240,
        video_mode: None,
        direct_video: 1,
        menu_pal: 0,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "crt-480p60",
        label: "CRT 480p 60hz NTSC",
        output_w: 640,
        output_h: 480,
        video_mode: None,
        direct_video: 1,
        menu_pal: 0,
        forced_scandoubler: 1,
    },
    DisplayResolution {
        id: "crt-288p50",
        label: "CRT 288p 50hz PAL",
        output_w: 640,
        output_h: 288,
        video_mode: None,
        direct_video: 1,
        menu_pal: 1,
        forced_scandoubler: 0,
    },
    DisplayResolution {
        id: "crt-576p50",
        label: "CRT 576p 50hz PAL",
        output_w: 640,
        output_h: 576,
        video_mode: None,
        direct_video: 1,
        menu_pal: 1,
        forced_scandoubler: 1,
    },
];

pub fn find(id: &str) -> Option<&'static DisplayResolution> {
    if id == AUTOMATIC_DISPLAY_RESOLUTION.id {
        Some(&AUTOMATIC_DISPLAY_RESOLUTION)
    } else {
        DISPLAY_RESOLUTIONS.iter().find(|mode| mode.id == id)
    }
}

pub fn persist(id: &str) -> io::Result<()> {
    persist_to(MISTER_INI_PATH, id)
}

pub fn persist_to(path: impl AsRef<Path>, id: &str) -> io::Result<()> {
    let mode = find(id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "unsupported display mode"))?;
    let path = path.as_ref();
    let text = fs::read_to_string(path)?;
    let mut lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
    set_ini_key(
        &mut lines,
        "Menu",
        "direct_video",
        &mode.direct_video.to_string(),
    );
    set_ini_key(&mut lines, "Menu", "menu_pal", &mode.menu_pal.to_string());
    set_ini_key(
        &mut lines,
        "Menu",
        "forced_scandoubler",
        &mode.forced_scandoubler.to_string(),
    );
    if let Some(video_mode) = mode.video_mode {
        set_ini_key(&mut lines, "Menu", "video_mode", video_mode);
    }
    let tmp = path.with_extension("ini.mister-magik-display-new");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&tmp)?;
    file.write_all(lines.join("\n").as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(tmp, path)?;
    if let Some(parent) = path.parent() {
        OpenOptions::new().read(true).open(parent)?.sync_all()?;
    }
    Ok(())
}

fn set_ini_key(lines: &mut Vec<String>, wanted_section: &str, wanted_key: &str, value: &str) {
    let mut section = String::new();
    let mut last_match = None;
    let mut section_end = None;
    for (index, line) in lines.iter().enumerate() {
        let content = line.split(';').next().unwrap_or("").trim();
        if content.starts_with('[') && content.ends_with(']') {
            if section.eq_ignore_ascii_case(wanted_section) && section_end.is_none() {
                section_end = Some(index);
            }
            section = content[1..content.len() - 1].trim().to_owned();
        } else if section.eq_ignore_ascii_case(wanted_section)
            && content
                .split_once('=')
                .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case(wanted_key))
        {
            last_match = Some(index);
        }
    }
    if section.eq_ignore_ascii_case(wanted_section) && section_end.is_none() {
        section_end = Some(lines.len());
    }
    if let Some(index) = last_match {
        let suffix = lines[index]
            .find(';')
            .map(|at| format!(" {}", &lines[index][at..]))
            .unwrap_or_default();
        lines[index] = format!("{wanted_key}={value}{suffix}");
    } else if let Some(index) = section_end {
        lines.insert(index, format!("{wanted_key}={value}"));
    } else {
        lines.push(format!("[{wanted_section}]"));
        lines.push(format!("{wanted_key}={value}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_has_unique_stable_ids() {
        assert_eq!(
            DISPLAY_RESOLUTIONS
                .iter()
                .map(|mode| mode.id)
                .collect::<Vec<_>>(),
            vec![
                "hdmi-1280x720p60",
                "hdmi-1366x768p60",
                "hdmi-1920x1080p60",
                "hdmi-1920x1200p60",
                "hdmi-2048x1536p60",
                "crt-240p60",
                "crt-480p60",
                "crt-288p50",
                "crt-576p50",
            ]
        );
        assert_eq!(
            DISPLAY_RESOLUTIONS[5..]
                .iter()
                .map(|mode| (mode.id, mode.label))
                .collect::<Vec<_>>(),
            vec![
                ("crt-240p60", "CRT 240p 60hz NTSC"),
                ("crt-480p60", "CRT 480p 60hz NTSC"),
                ("crt-288p50", "CRT 288p 50hz PAL"),
                ("crt-576p50", "CRT 576p 50hz PAL"),
            ]
        );
        for (index, mode) in DISPLAY_RESOLUTIONS.iter().enumerate() {
            assert!(
                DISPLAY_RESOLUTIONS[..index]
                    .iter()
                    .all(|other| other.id != mode.id)
            );
            assert!(!mode.label.is_empty());
        }
        assert!(DISPLAY_RESOLUTIONS.iter().all(|mode| mode.id != "auto"));
        assert_eq!(find("auto"), Some(&AUTOMATIC_DISPLAY_RESOLUTION));
    }

    #[test]
    fn persistence_preserves_comments_and_unrelated_keys() {
        let path =
            std::env::temp_dir().join(format!("mister-magik-display-{}.ini", std::process::id()));
        fs::write(
            &path,
            "[MiSTer]\nmain=MiSTer_MagiK\n[Menu]\nvideo_mode=8 ; keep\nfoo=bar\n",
        )
        .unwrap();
        persist_to(&path, "crt-576p50").unwrap();
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.contains("video_mode=8 ; keep"));
        assert!(text.contains("direct_video=1"));
        assert!(text.contains("menu_pal=1"));
        assert!(text.contains("forced_scandoubler=1"));
        assert!(text.contains("foo=bar"));
        let _ = fs::remove_file(path);
    }
}
