pub const COMMANDS: &[&str] = &[
    "read",
    "route",
    "fb",
    "fb-current",
    "fb-format-smoke",
    "early-black",
    "ui",
    "scenes",
    "effects",
    "camera-effects",
    "sprite-effects",
    "text-effects",
    "raster-effects",
    "transition-effects",
    "preview-transitions",
    "effect-bench",
    "vsync-probe",
    "cpu-profile-smoke",
    "input",
    "library-refresh",
    "library-sql",
    "library-scan-bench",
    "audio-tone",
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
        assert!(COMMANDS.contains(&"library-sql"));
        for command in COMMANDS {
            assert_eq!(
                resolve_command(&args(&["mister-magik-fb", command])),
                *command
            );
            assert!(!should_handoff_to_mister(command));
        }
    }

    #[test]
    fn hands_launchable_files_back_to_main() {
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
            assert!(should_handoff_to_mister(path), "{path}");
        }
    }

    #[test]
    fn keeps_menu_boot_in_launcher() {
        assert!(!should_handoff_to_mister("menu.rbf"));
        assert!(!should_handoff_to_mister("/media/fat/menu.rbf"));
    }
}
