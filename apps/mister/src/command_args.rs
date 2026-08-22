// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommandKind {
    PreFpga,
    Fpga,
    ListOnly,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CommandSpec {
    pub name: &'static str,
    pub kind: CommandKind,
}

impl CommandSpec {
    const fn new(name: &'static str, kind: CommandKind) -> Self {
        Self { name, kind }
    }
}

pub const CATALOG_INSPECT_COMMAND: &str = "catalog-v3-inspect";
pub const CATALOG_ROM_AUDIT_COMMAND: &str = "catalog-arcade-rom-audit";
pub const CATALOG_CORPUS_INVENTORY_COMMAND: &str = "catalog-corpus-inventory";
pub const CATALOG_REGISTRY_REPORT_COMMAND: &str = "catalog-v3-registry-report";

pub const COMMANDS: &[CommandSpec] = &[
    CommandSpec::new("read", CommandKind::Fpga),
    CommandSpec::new("early-black", CommandKind::Fpga),
    CommandSpec::new("ui", CommandKind::Fpga),
    #[cfg(mister_bench_scenes)]
    CommandSpec::new("scenes", CommandKind::Fpga),
    #[cfg(mister_experiments)]
    CommandSpec::new("experiment-capabilities", CommandKind::ListOnly),
    #[cfg(mister_experiments)]
    CommandSpec::new("preview-transitions", CommandKind::ListOnly),
    #[cfg(mister_experiments)]
    CommandSpec::new("effects", CommandKind::Fpga),
    #[cfg(mister_experiments)]
    CommandSpec::new("camera-effects", CommandKind::ListOnly),
    #[cfg(mister_experiments)]
    CommandSpec::new("sprite-effects", CommandKind::ListOnly),
    #[cfg(mister_experiments)]
    CommandSpec::new("text-effects", CommandKind::ListOnly),
    #[cfg(mister_experiments)]
    CommandSpec::new("raster-effects", CommandKind::ListOnly),
    #[cfg(mister_experiments)]
    CommandSpec::new("transition-effects", CommandKind::ListOnly),
    #[cfg(mister_experiments)]
    CommandSpec::new("effect-bench", CommandKind::Fpga),
    #[cfg(feature = "diagnostics")]
    CommandSpec::new("vsync-probe", CommandKind::PreFpga),
    #[cfg(feature = "diagnostics")]
    CommandSpec::new("cpu-profile-smoke", CommandKind::PreFpga),
    #[cfg(feature = "diagnostics")]
    CommandSpec::new("fb-map-report", CommandKind::PreFpga),
    #[cfg(feature = "diagnostics")]
    CommandSpec::new("fb-map-bandwidth", CommandKind::PreFpga),
    #[cfg(feature = "diagnostics")]
    CommandSpec::new("scanout-slots-map-report", CommandKind::PreFpga),
    CommandSpec::new("fpga-latch-report", CommandKind::Fpga),
    CommandSpec::new("latch-readiness-report", CommandKind::Fpga),
    #[cfg(all(feature = "diagnostics", feature = "ui"))]
    CommandSpec::new("fpga-latch-post-report", CommandKind::Fpga),
    #[cfg(all(feature = "diagnostics", feature = "ui"))]
    CommandSpec::new("fpga-latch-pattern", CommandKind::Fpga),
    #[cfg(feature = "diagnostics")]
    CommandSpec::new("input", CommandKind::Fpga),
    CommandSpec::new("library-refresh", CommandKind::PreFpga),
    CommandSpec::new("request-library-rebuild", CommandKind::PreFpga),
    CommandSpec::new("toggle-simple-joystick-setting", CommandKind::PreFpga),
    CommandSpec::new("display-persist", CommandKind::PreFpga),
    CommandSpec::new("purge-library-data", CommandKind::PreFpga),
    CommandSpec::new("reset-delete-screenshot-packs", CommandKind::PreFpga),
    CommandSpec::new("benchmark-capabilities", CommandKind::PreFpga),
    CommandSpec::new("input-integrity-driver", CommandKind::PreFpga),
    CommandSpec::new("pmu-probe", CommandKind::PreFpga),
    CommandSpec::new("pmu-profile", CommandKind::PreFpga),
    CommandSpec::new("search-bench", CommandKind::PreFpga),
    CommandSpec::new("rom-identity-bench", CommandKind::PreFpga),
    CommandSpec::new(CATALOG_CORPUS_INVENTORY_COMMAND, CommandKind::PreFpga),
    CommandSpec::new("media-bench-download", CommandKind::PreFpga),
    #[cfg(feature = "bench-tools")]
    CommandSpec::new("media-bench-save", CommandKind::PreFpga),
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    CommandSpec::new("preview-pack-bench", CommandKind::PreFpga),
    #[cfg(any(feature = "bench-tools", feature = "diagnostics"))]
    CommandSpec::new("preview-index-refresh-bench", CommandKind::PreFpga),
    CommandSpec::new(CATALOG_INSPECT_COMMAND, CommandKind::PreFpga),
    CommandSpec::new(CATALOG_ROM_AUDIT_COMMAND, CommandKind::PreFpga),
    CommandSpec::new(CATALOG_REGISTRY_REPORT_COMMAND, CommandKind::PreFpga),
    #[cfg(feature = "diagnostics")]
    CommandSpec::new("hbmame-metadata-from-library", CommandKind::PreFpga),
    #[cfg(feature = "diagnostics")]
    CommandSpec::new("library-scan-bench", CommandKind::Fpga),
    #[cfg(feature = "bench-tools")]
    CommandSpec::new("launch-prep-bench", CommandKind::PreFpga),
    #[cfg(feature = "bench-tools")]
    CommandSpec::new("framebuffer-stream-scalar-bench", CommandKind::PreFpga),
];

pub fn find_command(name: &str) -> Option<&'static CommandSpec> {
    COMMANDS.iter().find(|command| command.name == name)
}

pub fn is_known_command(name: &str) -> bool {
    find_command(name).is_some()
}

pub fn command_names() -> impl Iterator<Item = &'static str> {
    COMMANDS.iter().map(|command| command.name)
}

pub fn command_usage() -> String {
    command_names().collect::<Vec<_>>().join(" | ")
}

pub fn needs_explicit_command(args: &[String]) -> bool {
    matches!(args.get(1).map(|s| s.as_str()), None | Some(""))
}

pub fn resolve_command(args: &[String]) -> String {
    match args.get(1).map(|s| s.as_str()) {
        None | Some("") => "".into(),
        Some(arg1) if is_launcher_boot(arg1) => "ui".into(),
        Some(arg1) => arg1.to_string(),
    }
}

pub fn is_launcher_boot(arg: &str) -> bool {
    arg.ends_with("menu.rbf") || arg.ends_with("/menu.rbf")
}

pub fn should_handoff_to_mister(arg: &str) -> bool {
    if is_known_command(arg) || is_launcher_boot(arg) {
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

pub fn requires_display_owner(command: &str) -> bool {
    matches!(command, "early-black" | "ui" | "effect-bench")
}

pub fn requires_process_exclusive(command: &str) -> bool {
    matches!(
        command,
        "early-black"
            | "ui"
            | "effect-bench"
            | "library-refresh"
            | "request-library-rebuild"
            | "toggle-simple-joystick-setting"
            | "purge-library-data"
            | "reset-delete-screenshot-packs"
            | "media-bench-download"
            | "media-bench-save"
            | "preview-pack-bench"
            | "preview-index-refresh-bench"
            | "hbmame-metadata-from-library"
            | "library-scan-bench"
            | "launch-prep-bench"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn bare_binary_needs_explicit_command() {
        assert!(needs_explicit_command(&args(&["mister-magik-fb"])));
        assert!(needs_explicit_command(&args(&["mister-magik-fb", ""])));
        assert_eq!(resolve_command(&args(&["mister-magik-fb"])), "");
        assert_eq!(resolve_command(&args(&["mister-magik-fb", ""])), "");
    }

    #[test]
    fn menu_rbf_boot_resolves_to_launcher_ui() {
        assert_eq!(
            resolve_command(&args(&["mister-magik-fb", "/media/fat/menu.rbf"])),
            "ui"
        );
    }

    #[test]
    fn recognizes_explicit_commands() {
        assert_command_kind("library-refresh", CommandKind::PreFpga);
        assert_command_kind("purge-library-data", CommandKind::PreFpga);
        assert_command_kind("reset-delete-screenshot-packs", CommandKind::PreFpga);
        assert_command_kind("ui", CommandKind::Fpga);
        for command in command_names() {
            assert_eq!(
                resolve_command(&args(&["mister-magik-fb", command])),
                command
            );
            assert!(!should_handoff_to_mister(command));
        }
    }

    #[test]
    fn command_table_has_unique_names() {
        let names = command_names().collect::<Vec<_>>();
        for (index, name) in names.iter().enumerate() {
            assert!(
                !names[..index].contains(name),
                "duplicate command entry: {name}"
            );
        }
    }

    #[test]
    fn display_owner_guard_is_scoped_to_framebuffer_renderers() {
        assert!(requires_display_owner("ui"));
        assert!(requires_display_owner("early-black"));
        assert!(requires_display_owner("effect-bench"));
        assert!(!requires_display_owner("library-refresh"));
        assert!(!requires_display_owner("catalog-v3-inspect"));
        assert!(!requires_display_owner("catalog-v3-registry-report"));
        assert!(!requires_display_owner("read"));
    }

    #[test]
    fn process_exclusive_guard_blocks_managed_commands_only() {
        assert!(requires_process_exclusive("ui"));
        assert!(requires_process_exclusive("early-black"));
        assert!(requires_process_exclusive("library-refresh"));
        assert!(requires_process_exclusive("purge-library-data"));
        assert!(!requires_process_exclusive("catalog-v3-inspect"));
        assert!(!requires_process_exclusive("catalog-v3-registry-report"));
        assert!(!requires_process_exclusive("read"));
        assert!(!requires_process_exclusive("vsync-probe"));
        assert!(!requires_process_exclusive("cpu-profile-smoke"));
    }

    #[test]
    #[cfg(all(not(feature = "diagnostics"), not(feature = "bench-tools")))]
    fn production_command_list_hides_diagnostics() {
        assert!(is_known_command("catalog-v3-inspect"));
        assert!(is_known_command("catalog-v3-registry-report"));
        assert!(is_known_command("benchmark-capabilities"));
        assert!(is_known_command("pmu-probe"));
        assert!(is_known_command("pmu-profile"));
        assert!(is_known_command("search-bench"));
        assert!(is_known_command("rom-identity-bench"));
        assert!(is_known_command("read"));
        assert!(is_known_command("fpga-latch-report"));
        assert_command_kind("fpga-latch-report", CommandKind::Fpga);
        for command in [
            "vsync-probe",
            "cpu-profile-smoke",
            "input",
            "hbmame-metadata-from-library",
            "library-scan-bench",
            "preview-pack-bench",
            "preview-index-refresh-bench",
            "framebuffer-stream-scalar-bench",
        ] {
            assert!(!is_known_command(command), "{command}");
        }
        for command in [
            "media-bench-download",
            "media-bench-save",
            "launch-prep-bench",
            "audio-tone",
        ] {
            assert!(!is_known_command(command), "{command}");
        }
    }

    #[test]
    #[cfg(feature = "diagnostics")]
    fn diagnostics_command_list_exposes_diagnostics() {
        for command in [
            "read",
            "vsync-probe",
            "cpu-profile-smoke",
            "fb-map-report",
            "fb-map-bandwidth",
            "scanout-slots-map-report",
            "fpga-latch-report",
            "input",
            "catalog-v3-inspect",
            "hbmame-metadata-from-library",
            "library-scan-bench",
            "preview-pack-bench",
            "preview-index-refresh-bench",
        ] {
            assert!(is_known_command(command), "{command}");
        }
        #[cfg(feature = "ui")]
        for command in ["fpga-latch-post-report", "fpga-latch-pattern"] {
            assert!(is_known_command(command), "{command}");
        }
        #[cfg(not(feature = "ui"))]
        for command in ["fpga-latch-post-report", "fpga-latch-pattern"] {
            assert!(!is_known_command(command), "{command}");
        }
        #[cfg(feature = "bench-tools")]
        assert!(is_known_command("framebuffer-stream-scalar-bench"));
        #[cfg(not(feature = "bench-tools"))]
        assert!(!is_known_command("framebuffer-stream-scalar-bench"));
        assert!(!is_known_command("audio-tone"));
        assert_command_kind("read", CommandKind::Fpga);
        assert_command_kind("vsync-probe", CommandKind::PreFpga);
        assert_command_kind("cpu-profile-smoke", CommandKind::PreFpga);
        assert_command_kind("fb-map-report", CommandKind::PreFpga);
        assert_command_kind("fb-map-bandwidth", CommandKind::PreFpga);
        assert_command_kind("scanout-slots-map-report", CommandKind::PreFpga);
        assert_command_kind("fpga-latch-report", CommandKind::Fpga);
        #[cfg(feature = "ui")]
        assert_command_kind("fpga-latch-post-report", CommandKind::Fpga);
        #[cfg(feature = "ui")]
        assert_command_kind("fpga-latch-pattern", CommandKind::Fpga);
        assert_command_kind("input", CommandKind::Fpga);
    }

    #[test]
    #[cfg(all(feature = "bench-tools", not(feature = "diagnostics")))]
    fn bench_tool_command_list_exposes_benchmarks() {
        for command in [
            "media-bench-download",
            "media-bench-save",
            "launch-prep-bench",
            "fpga-latch-report",
            "preview-pack-bench",
            "preview-index-refresh-bench",
        ] {
            assert!(is_known_command(command), "{command}");
        }
        assert!(!is_known_command("audio-tone"));
        assert_command_kind("fpga-latch-report", CommandKind::Fpga);
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
            assert!(!is_known_command(command), "{command}");
        }
        #[cfg(not(mister_bench_scenes))]
        assert!(!is_known_command("scenes"), "scenes");
        #[cfg(mister_bench_scenes)]
        assert!(is_known_command("scenes"), "scenes");
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
            assert!(is_known_command(command), "{command}");
        }
        assert_command_kind("experiment-capabilities", CommandKind::ListOnly);
        assert_command_kind("preview-transitions", CommandKind::ListOnly);
        assert_command_kind("camera-effects", CommandKind::ListOnly);
        assert_command_kind("effects", CommandKind::Fpga);
        assert_command_kind("effect-bench", CommandKind::Fpga);
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

    fn assert_command_kind(command: &str, kind: CommandKind) {
        assert_eq!(find_command(command).map(|spec| spec.kind), Some(kind));
    }
}
