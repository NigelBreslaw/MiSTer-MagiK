# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Repository instruction discovery and source ownership, without a Rust bootstrap."""

from __future__ import annotations

import argparse
import json
import sys
import tomllib
from pathlib import Path

DEVICE_ROOTS = (Path("/media/fat"), Path("/tmp/mister-magik"))


def authority(path: str) -> tuple[str, str]:
    file_name = Path(path).name
    if path.startswith(("/media/fat/", "/tmp/mister-magik/")):
        return (
            "device-owned runtime state; never copy into Git",
            "scripts/agent deliver or an attended typed scripts/agent device command",
        )
    if file_name.startswith(".env") or "/.wrangler/" in path:
        return "ignored secret; never stage or print", "none"
    if (
        path.startswith(("build/", "dist/", "outputs/", "documentation/dist/"))
        or "/target/" in path
    ):
        return (
            "ignored disposable output; never stage",
            "rerun the owning typed workflow",
        )
    if path == "docs/reference/mister-runtime-environment.md":
        return (
            "checked-in generated reference",
            "python3 scripts/checks/generate-runtime-environment-reference.py",
        )
    if path == "crates/catalog/data/core_launch_manifest.json":
        return (
            "checked-in generated core-launch manifest",
            "python3 scripts/media/harvest-core-launch-manifest.py --help",
        )
    if path == "apps/mister/licenses/RUST-LIBRARIES.txt":
        return (
            "checked-in generated legal inventory",
            "python3 scripts/release/packaging/generate-third-party-licenses.py",
        )
    if path.startswith("mister/platform/contracts/generated/"):
        return (
            "checked-in generated platform-v3 consumer; never hand-edit",
            "python3 scripts/checks/generate-platform-v3-consumers.py",
        )
    if path.endswith(
        ("generated_hdmi_evidence.rs", "mister_magik_video_diagnostics_protocol.svh")
    ):
        return (
            "checked-in generated HDMI diagnostics consumer",
            "python3 scripts/checks/generate-hdmi-evidence-protocol.py followed by generate-video-diagnostics-protocol.py",
        )
    if "/visual-baselines/launcher/" in path:
        return (
            "checked-in reviewed visual baseline; never update in place",
            "render a fresh 18-scene matrix directory and review every output",
        )
    if path.endswith(".mmbf"):
        return (
            "checked-in generated bitmap font",
            "apps/mister/scripts/generate-bitmap-fonts.sh",
        )
    if path.endswith(".rgb565a"):
        return (
            "checked-in generated RGB565A artwork",
            "python3 scripts/media/convert-rgba-to-rgb565a.py SOURCE OUTPUT",
        )
    if path.endswith("magik-alpha-mask.bin"):
        return (
            "checked-in generated particle target",
            "use the command recorded in adjacent provenance",
        )
    if path.endswith(("arcade-cabinet.pcloud", "arcade-cabinet.pcolor")):
        return (
            "checked-in generated cabinet particle data",
            "scripts/particle-model compile with the adjacent notice parameters",
        )
    if path.startswith("crates/particles/assets/intro/"):
        return (
            "checked-in generated intro asset",
            "scripts/agent scene-lab generate-intro-assets --output crates/particles/assets/intro",
        )
    if path.startswith("apps/desktop/vendor/"):
        return (
            "public submodule; parent owns only the gitlink",
            "git submodule update --init for the selected vendor",
        )
    if path.startswith(("private/magik-cloud/", "private/magik-assets/")):
        return (
            "private submodule; parent owns only the gitlink",
            "commit and push the private repository before staging the parent gitlink",
        )
    if path.startswith("history/"):
        return "hand-edited dated evidence", "experiment-specific; preserve provenance"
    if file_name == "Cargo.lock":
        return (
            "checked-in dependency resolution",
            "scripts/agent dependencies sync PATH/Cargo.toml",
        )
    if path.endswith((".slint", ".rs", ".toml")):
        return "hand-edited source unless a more specific rule above applies", "none"
    return (
        "unclassified; inspect source ownership and Git history before editing",
        "none",
    )


def canonical_document(path: str) -> str:
    if path.startswith("apps/mister/src/ui_runner/"):
        return "matching heading in docs/architecture.md"
    if path.startswith("apps/mister/ui/"):
        return "docs/architecture.md#launcher-composition"
    if path.startswith("crates/catalog/") or "media_update" in path:
        return "matching heading in docs/catalog.md"
    if path.startswith(
        (
            "crates/framebuffer-scenes/",
            "crates/particles/",
            "apps/framebuffer-scene-lab/",
        )
    ):
        return "matching heading in docs/startup-particles.md"
    if path.startswith("mister/platform/fpga/"):
        return "docs/fpga-development.md; release work uses the matching docs/fpga-latch-release.md heading"
    if path.startswith("mister/platform/kernel/"):
        return "docs/kernel-scanout-plugin-assurance.md"
    if path.startswith("mister/tools/agent/"):
        return "docs/magik-agent.md"
    if path.startswith("mister/tools/manager/"):
        return "docs/installer.md"
    if path.startswith("scripts/release/") or "package-distribution" in path:
        return "docs/releases.md"
    if path.startswith("agent-cli/"):
        return "matching workflow heading in docs/device.md"
    return "none; start with source and tests"


def extra_assurance(path: str) -> str:
    if path.startswith("mister/platform/fpga/"):
        return "typed FPGA signoff and attended physical qualification"
    if path.startswith("mister/platform/kernel/"):
        return "kernel build and attended device qualification"
    if path.startswith("mister/platform/runtime/src/framebuffer/"):
        return "attended HDMI proof for scan-out claims"
    if path.startswith("apps/mister/ui/"):
        return "visual matrix; attended capture only for physical HDMI/CRT claims"
    if path.startswith("apps/desktop/ui/"):
        return "live Slint visual verification"
    if path.startswith("private/magik-cloud/"):
        return "explicit authorization before public Cloudflare or GitHub mutation"
    return "none beyond selected hooks and CI"


def repository_path(repository: Path, requested: Path) -> tuple[Path, bool]:
    if ".." in requested.parts:
        raise ValueError(f"guidance_path_escapes_repository: {requested}")
    if requested.is_absolute() and any(
        requested.is_relative_to(p) for p in DEVICE_ROOTS
    ):
        return requested, True
    candidate = requested if requested.is_absolute() else repository / requested
    # Resolve symlinks, including those in existing parents of a new file.
    candidate = candidate.resolve()
    if not candidate.is_relative_to(repository):
        raise ValueError(f"guidance_path_outside_repository: {requested}")
    return candidate.relative_to(repository), False


def instruction_chain(repository: Path, target: Path) -> list[str]:
    config = repository / ".codex/config.toml"
    fallbacks = (
        tomllib.loads(config.read_text()).get("project_doc_fallback_filenames", [])
        if config.is_file()
        else []
    )
    if not isinstance(fallbacks, list) or any(
        not isinstance(n, str) or Path(n).name != n or n in {"", ".", ".."}
        for n in fallbacks
    ):
        raise ValueError("guidance_invalid_fallback_filenames")
    names = ["AGENTS.override.md", "AGENTS.md", *fallbacks]
    directory = target if (repository / target).is_dir() else target.parent
    chain = []
    for parent in [*reversed(directory.parents), directory]:
        for name in names:
            candidate = repository / parent / name
            if (
                candidate.is_file()
                and candidate.resolve().is_relative_to(repository)
                and candidate.read_text().strip()
            ):
                chain.append((parent / name).as_posix())
                break
    return chain


def report(repository: Path, requested: Path) -> dict[str, object]:
    repository = repository.resolve()
    path, device = repository_path(repository, requested)
    display = path.as_posix()
    classification, regeneration = authority(
        display + "/" if device and path in DEVICE_ROOTS else display
    )
    return {
        "schema_version": 1,
        "path": display,
        "guidance": instruction_chain(repository, Path("."))
        if device
        else instruction_chain(repository, path),
        "guidance_scope": "repository; global and runtime instructions are supplied by the client",
        "authority": classification,
        "regeneration": regeneration,
        "canonical": canonical_document(display),
        "extra-assurance": extra_assurance(display),
    }


def render(record: dict[str, object]) -> str:
    return (
        "\n".join(
            f"{key}: {', '.join(value) if isinstance(value, list) else value}"
            for key, value in record.items()
            if key not in {"schema_version", "guidance_scope"}
        )
        + "\n"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repository", type=Path, default=Path(__file__).resolve().parents[2]
    )
    parser.add_argument("path", type=Path)
    parser.add_argument("--json", action="store_true", dest="json_output")
    args = parser.parse_args()
    try:
        record = report(args.repository, args.path)
    except (OSError, ValueError) as error:
        print(f"guidance_error: {error}", file=sys.stderr)
        return 2
    print(
        json.dumps(record) if args.json_output else render(record),
        end="\n" if args.json_output else "",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
