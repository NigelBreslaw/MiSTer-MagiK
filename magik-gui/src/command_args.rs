pub const COMMANDS: &[&str] = &[
    #[cfg(feature = "diagnostics")]
    "read",
    "early-black",
    "ui",
    #[cfg(mister_bench_scenes)]
    "scenes",
    #[cfg(mister_experiments)]
    "experiment-capabilities",
    #[cfg(mister_experiments)]
    "preview-transitions",
    #[cfg(mister_experiments)]
    "effects",
    #[cfg(mister_experiments)]
    "camera-effects",
    #[cfg(mister_experiments)]
    "sprite-effects",
    #[cfg(mister_experiments)]
    "text-effects",
    #[cfg(mister_experiments)]
    "raster-effects",
    #[cfg(mister_experiments)]
    "transition-effects",
    #[cfg(mister_experiments)]
    "effect-bench",
    #[cfg(feature = "diagnostics")]
    "vsync-probe",
    #[cfg(feature = "diagnostics")]
    "cpu-profile-smoke",
    #[cfg(feature = "diagnostics")]
    "input",
    "library-refresh",
    #[cfg(feature = "bench-tools")]
    "media-bench-download",
    #[cfg(feature = "bench-tools")]
    "media-bench-save",
    #[cfg(feature = "diagnostics")]
    "preview-pack-bench",
    #[cfg(feature = "diagnostics")]
    "preview-index-refresh-bench",
    "library-sql",
    #[cfg(feature = "diagnostics")]
    "hbmame-metadata-from-library",
    #[cfg(feature = "diagnostics")]
    "library-scan-bench",
    #[cfg(feature = "bench-tools")]
    "launch-prep-bench",
];

pub fn resolve_command(args: &[String]) -> String {
    match args.get(1).map(|s| s.as_str()) {
        None => "ui".into(),
        Some("") => "ui".into(),
        Some(arg1) if is_launcher_boot(arg1) => "ui".into(),
        Some(arg1) => arg1.to_string(),
    }
}

pub fn is_launcher_boot(arg: &str) -> bool {
    arg.ends_with("menu.rbf") || arg.ends_with("/menu.rbf")
}

pub fn should_handoff_to_mister(arg: &str) -> bool {
    if COMMANDS.contains(&arg) || is_launcher_boot(arg) {
        return false;
    }
    false
}

pub fn is_launchable_arg(arg: &str) -> bool {
    let arg = arg.to_ascii_lowercase();
    arg.ends_with(".rbf")
        || arg.ends_with(".mra")
        || arg.ends_with(".mgl")
        || arg.ends_with(".zip")
        || arg.ends_with(".7z")
        || arg.ends_with(".lha")
        || arg.ends_with(".lzh")
        || arg.ends_with(".rar")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn defaults_to_launcher_ui() {
        assert_eq!(resolve_command(&args(&["mister-magik-fb"])), "ui");
        assert_eq!(resolve_command(&args(&["mister-magik-fb", ""])), "ui");
        assert_eq!(
            resolve_command(&args(&["mister-magik-fb", "/media/fat/menu.rbf"])),
            "ui"
        );
    }

    #[test]
    fn recognizes_explicit_commands() {
        assert!(COMMANDS.contains(&"library-refresh"));
        for command in COMMANDS {
            assert_eq!(
                resolve_command(&args(&["mister-magik-fb", command])),
                *command
            );
            assert!(!should_handoff_to_mister(command));
        }
    }

    #[test]
    #[cfg(all(not(feature = "diagnostics"), not(feature = "bench-tools")))]
    fn production_command_list_hides_diagnostics() {
        assert!(COMMANDS.contains(&"library-sql"));
        for command in [
            "read",
            "vsync-probe",
            "cpu-profile-smoke",
            "input",
            "hbmame-metadata-from-library",
            "library-scan-bench",
            "preview-pack-bench",
            "preview-index-refresh-bench",
        ] {
            assert!(!COMMANDS.contains(&command), "{command}");
        }
        for command in [
            "media-bench-download",
            "media-bench-save",
            "launch-prep-bench",
            "audio-tone",
        ] {
            assert!(!COMMANDS.contains(&command), "{command}");
        }
    }

    #[test]
    #[cfg(feature = "diagnostics")]
    fn diagnostics_command_list_exposes_diagnostics() {
        for command in [
            "read",
            "vsync-probe",
            "cpu-profile-smoke",
            "input",
            "library-sql",
            "hbmame-metadata-from-library",
            "library-scan-bench",
            "preview-pack-bench",
            "preview-index-refresh-bench",
        ] {
            assert!(COMMANDS.contains(&command), "{command}");
        }
        assert!(!COMMANDS.contains(&"audio-tone"));
    }

    #[test]
    #[cfg(all(feature = "bench-tools", not(feature = "diagnostics")))]
    fn bench_tool_command_list_exposes_benchmarks() {
        for command in [
            "media-bench-download",
            "media-bench-save",
            "launch-prep-bench",
        ] {
            assert!(COMMANDS.contains(&command), "{command}");
        }
        assert!(!COMMANDS.contains(&"preview-pack-bench"));
        assert!(!COMMANDS.contains(&"preview-index-refresh-bench"));
        assert!(!COMMANDS.contains(&"audio-tone"));
    }

    #[test]
    #[cfg(not(mister_experiments))]
    fn production_command_list_hides_experiments() {
        for command in [
            "preview-transitions",
            "camera-effects",
            "sprite-effects",
            "text-effects",
            "raster-effects",
            "transition-effects",
            "effects",
            "effect-bench",
            "experiment-capabilities",
        ] {
            assert!(!COMMANDS.contains(&command), "{command}");
        }
        #[cfg(not(mister_bench_scenes))]
        assert!(!COMMANDS.contains(&"scenes"), "scenes");
        #[cfg(mister_bench_scenes)]
        assert!(COMMANDS.contains(&"scenes"), "scenes");
    }

    #[test]
    #[cfg(mister_experiments)]
    fn experiment_command_list_exposes_experiments() {
        for command in [
            "preview-transitions",
            "camera-effects",
            "sprite-effects",
            "text-effects",
            "raster-effects",
            "transition-effects",
            "effects",
            "effect-bench",
            "experiment-capabilities",
        ] {
            assert!(COMMANDS.contains(&command), "{command}");
        }
    }

    #[test]
    fn detects_launchable_files() {
        for path in [
            "/media/fat/_Arcade/foo.mra",
            "/media/fat/games/foo.rbf",
            "/media/fat/games/foo.mgl",
            "/media/fat/games/foo.zip",
            "/media/fat/games/foo.7z",
            "/media/fat/games/foo.lha",
            "/media/fat/games/foo.lzh",
            "/media/fat/games/foo.rar",
        ] {
            assert!(is_launchable_arg(path), "{path}");
        }
    }

    #[test]
    fn detects_launchable_files_with_uppercase_extensions() {
        for path in [
            "/media/fat/_Arcade/1942.MRA",
            "/media/fat/games/Saturn/NIGHTS.RBF",
            "/media/fat/games/SNES/Mario.ZIP",
            "/media/fat/games/Amiga/Demo.LHA",
        ] {
            assert!(is_launchable_arg(path), "{path}");
        }
    }

    #[test]
    fn launchable_files_are_not_handed_off_by_slint() {
        for path in [
            "/media/fat/_Arcade/foo.mra",
            "/media/fat/games/foo.rbf",
            "/media/fat/games/foo.mgl",
            "/media/fat/games/foo.zip",
        ] {
            assert!(!should_handoff_to_mister(path), "{path}");
        }
    }

    #[test]
    fn keeps_menu_boot_in_launcher() {
        assert!(!should_handoff_to_mister("menu.rbf"));
        assert!(!should_handoff_to_mister("/media/fat/menu.rbf"));
    }
}
