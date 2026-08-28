// Copyright (C) 2026 Nigel Breslaw
// SPDX-License-Identifier: GPL-3.0-or-later

use std::path::{Component, Path, PathBuf};

use crate::error::AgentResult;

#[derive(Clone, Copy)]
struct Authority {
    classification: &'static str,
    regeneration: &'static str,
}

pub fn report(repository: &Path, requested: &Path) -> AgentResult<String> {
    let path = repository_path(repository, requested)?;
    let display = path.to_string_lossy().replace('\\', "/");
    let authority = authority(&display);
    let mut output = String::new();
    output.push_str(&format!("path: {display}\n"));
    output.push_str("guidance: AGENTS.md");
    if let Some(scoped) = scoped_guidance(&display) {
        output.push_str(&format!(", {scoped}"));
    }
    output.push('\n');
    output.push_str(&format!("authority: {}\n", authority.classification));
    output.push_str(&format!("regeneration: {}\n", authority.regeneration));
    output.push_str(&format!("canonical: {}\n", canonical_document(&display)));
    output.push_str(&format!("extra-assurance: {}\n", extra_assurance(&display)));
    Ok(output)
}

fn repository_path(repository: &Path, requested: &Path) -> AgentResult<PathBuf> {
    if requested.starts_with("/media/fat") || requested.starts_with("/tmp/mister-magik") {
        return Ok(requested.to_path_buf());
    }
    let relative = if requested.is_absolute() {
        requested
            .strip_prefix(repository)
            .map_err(|_| format!("guidance_path_outside_repository: {}", requested.display()))?
    } else {
        requested
    };
    if relative
        .components()
        .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(format!("guidance_path_escapes_repository: {}", requested.display()).into());
    }
    Ok(relative
        .components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect())
}

fn scoped_guidance(path: &str) -> Option<&'static str> {
    if path.starts_with("apps/mister/src/ui_runner/") {
        Some("apps/mister/src/ui_runner/AGENTS.md")
    } else if path.starts_with("apps/mister/") {
        Some("apps/mister/AGENTS.md")
    } else if path.starts_with("apps/desktop/") {
        Some("apps/desktop/AGENTS.md")
    } else if path.starts_with("apps/") {
        Some("apps/AGENTS.md")
    } else if path.starts_with("crates/") {
        Some("crates/AGENTS.md")
    } else if path.starts_with("mister/platform/fpga/") {
        Some("mister/platform/fpga/AGENTS.md")
    } else if path.starts_with("mister/tools/agent/") {
        Some("mister/tools/agent/AGENTS.md")
    } else if path.starts_with("mister/") {
        Some("mister/AGENTS.md")
    } else if path.starts_with("scripts/") {
        Some("scripts/AGENTS.md")
    } else if path.starts_with("private/magik-cloud/") {
        Some("private/magik-cloud/AGENTS.md")
    } else if path.starts_with("private/magik-assets/") {
        Some("private/magik-assets/AGENTS.md")
    } else {
        None
    }
}

fn authority(path: &str) -> Authority {
    let file_name = Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    if path.starts_with("/media/fat/") || path.starts_with("/tmp/mister-magik/") {
        return Authority {
            classification: "device-owned runtime state; never copy into Git",
            regeneration: "scripts/agent deliver or an attended typed scripts/agent device command",
        };
    }
    if file_name.starts_with(".env") || path.contains("/.wrangler/") {
        return Authority {
            classification: "ignored secret; never stage or print",
            regeneration: "none",
        };
    }
    if path.starts_with("build/")
        || path.starts_with("dist/")
        || path.starts_with("outputs/")
        || path.contains("/target/")
        || path.starts_with("documentation/dist/")
    {
        return Authority {
            classification: "ignored disposable output; never stage",
            regeneration: "rerun the owning typed workflow",
        };
    }
    if path == "docs/reference/mister-runtime-environment.md" {
        return Authority {
            classification: "checked-in generated reference",
            regeneration: "python3 scripts/checks/generate-runtime-environment-reference.py",
        };
    }
    if path == "crates/catalog/data/core_launch_manifest.json" {
        return Authority {
            classification: "checked-in generated core-launch manifest",
            regeneration: "python3 scripts/media/harvest-core-launch-manifest.py --help",
        };
    }
    if path == "apps/mister/licenses/RUST-LIBRARIES.txt" {
        return Authority {
            classification: "checked-in generated legal inventory",
            regeneration: "python3 scripts/release/packaging/generate-third-party-licenses.py",
        };
    }
    if path.starts_with("mister/platform/contracts/generated/") {
        return Authority {
            classification: "checked-in generated platform-v3 consumer; never hand-edit",
            regeneration: "python3 scripts/checks/generate-platform-v3-consumers.py",
        };
    }
    if path.ends_with("generated_hdmi_evidence.rs")
        || path.ends_with("mister_magik_video_diagnostics_protocol.svh")
    {
        return Authority {
            classification: "checked-in generated HDMI diagnostics consumer",
            regeneration: "python3 scripts/checks/generate-hdmi-evidence-protocol.py followed by generate-video-diagnostics-protocol.py",
        };
    }
    if path.contains("/visual-baselines/launcher/") {
        return Authority {
            classification: "checked-in reviewed visual baseline; never update in place",
            regeneration: "render a fresh 18-scene matrix directory and review every output",
        };
    }
    if path.ends_with(".mmbf") {
        return Authority {
            classification: "checked-in generated bitmap font",
            regeneration: "apps/mister/scripts/generate-bitmap-fonts.sh",
        };
    }
    if path.ends_with(".rgb565a") {
        return Authority {
            classification: "checked-in generated RGB565A artwork",
            regeneration: "python3 scripts/media/convert-rgba-to-rgb565a.py SOURCE OUTPUT",
        };
    }
    if path.ends_with("magik-alpha-mask.bin") {
        return Authority {
            classification: "checked-in generated particle target",
            regeneration: "use the command recorded in adjacent provenance",
        };
    }
    if path.ends_with("arcade-cabinet.pcloud") || path.ends_with("arcade-cabinet.pcolor") {
        return Authority {
            classification: "checked-in generated cabinet particle data",
            regeneration: "scripts/particle-model compile with the adjacent notice parameters",
        };
    }
    if path.starts_with("crates/particles/assets/intro/") {
        return Authority {
            classification: "checked-in generated intro asset",
            regeneration: "scripts/agent scene-lab generate-intro-assets --output crates/particles/assets/intro",
        };
    }
    if path.starts_with("apps/desktop/vendor/") {
        return Authority {
            classification: "public submodule; parent owns only the gitlink",
            regeneration: "git submodule update --init for the selected vendor",
        };
    }
    if path.starts_with("private/magik-cloud/") || path.starts_with("private/magik-assets/") {
        return Authority {
            classification: "private submodule; parent owns only the gitlink",
            regeneration: "commit and push the private repository before staging the parent gitlink",
        };
    }
    if path.starts_with("history/") {
        return Authority {
            classification: "hand-edited dated evidence",
            regeneration: "experiment-specific; preserve provenance",
        };
    }
    if file_name == "Cargo.lock" {
        return Authority {
            classification: "checked-in dependency resolution",
            regeneration: "scripts/agent dependencies sync PATH/Cargo.toml",
        };
    }
    if path.ends_with(".slint") || path.ends_with(".rs") || path.ends_with(".toml") {
        return Authority {
            classification: "hand-edited source unless a more specific rule above applies",
            regeneration: "none",
        };
    }
    Authority {
        classification: "unclassified; inspect source ownership and Git history before editing",
        regeneration: "none",
    }
}

fn canonical_document(path: &str) -> &'static str {
    if path.starts_with("apps/mister/src/ui_runner/") {
        "matching heading in docs/architecture.md"
    } else if path.starts_with("apps/mister/ui/") {
        "docs/architecture.md#launcher-composition"
    } else if path.starts_with("crates/catalog/") || path.contains("media_update") {
        "matching heading in docs/catalog.md"
    } else if path.starts_with("crates/framebuffer-scenes/")
        || path.starts_with("crates/particles/")
        || path.starts_with("apps/framebuffer-scene-lab/")
    {
        "matching heading in docs/startup-particles.md"
    } else if path.starts_with("mister/platform/fpga/") {
        "docs/fpga-development.md; release work uses the matching docs/fpga-latch-release.md heading"
    } else if path.starts_with("mister/platform/kernel/") {
        "docs/kernel-scanout-plugin-assurance.md"
    } else if path.starts_with("mister/tools/agent/") {
        "docs/magik-agent.md"
    } else if path.starts_with("mister/tools/manager/") {
        "docs/installer.md"
    } else if path.starts_with("scripts/release/") || path.contains("package-distribution") {
        "docs/releases.md"
    } else if path.starts_with("agent-cli/") {
        "matching workflow heading in docs/device.md"
    } else {
        "none; start with source and tests"
    }
}

fn extra_assurance(path: &str) -> &'static str {
    if path.starts_with("mister/platform/fpga/") {
        "typed FPGA signoff and attended physical qualification"
    } else if path.starts_with("mister/platform/kernel/") {
        "kernel build and attended device qualification"
    } else if path.starts_with("mister/platform/runtime/src/framebuffer/") {
        "attended HDMI proof for scan-out claims"
    } else if path.starts_with("apps/mister/ui/") {
        "visual matrix; attended capture only for physical HDMI/CRT claims"
    } else if path.starts_with("apps/desktop/ui/") {
        "live Slint visual verification"
    } else if path.starts_with("private/magik-cloud/") {
        "explicit authorization before public Cloudflare or GitHub mutation"
    } else {
        "none beyond selected hooks and CI"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_only_root_and_nearest_scoped_guidance() {
        let report = report(
            Path::new("/repo"),
            Path::new("apps/mister/src/ui_runner/launcher_loop.rs"),
        )
        .unwrap();
        assert!(report.contains("AGENTS.md, apps/mister/src/ui_runner/AGENTS.md"));
        assert!(!report.contains("apps/mister/AGENTS.md"));
    }

    #[test]
    fn identifies_generated_runtime_reference() {
        let report = report(
            Path::new("/repo"),
            Path::new("docs/reference/mister-runtime-environment.md"),
        )
        .unwrap();
        assert!(report.contains("checked-in generated reference"));
        assert!(report.contains("generate-runtime-environment-reference.py"));
    }

    #[test]
    fn rejects_paths_outside_repository() {
        assert!(report(Path::new("/repo"), Path::new("/elsewhere/file.rs")).is_err());
        assert!(report(Path::new("/repo"), Path::new("../file.rs")).is_err());
    }
}
