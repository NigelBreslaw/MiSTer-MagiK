#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import importlib.util
from pathlib import Path
import shutil
import tempfile


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "ci_cache_identity", ROOT / "scripts/checks/ci-cache-identity.py"
)
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def copy_inputs(destination: Path) -> None:
    patterns = tuple(pattern for group in MODULE.GROUPS.values() for pattern in group)
    for source in MODULE.files_for(ROOT, patterns):
        relative = source.relative_to(ROOT)
        target = destination / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target)


def main() -> int:
    baseline = MODULE.identities(ROOT)
    assert baseline["schema"] == "v2"
    assert baseline["cross_image"].startswith("ghcr.io/")

    with tempfile.TemporaryDirectory() as raw_tmp:
        fixture = Path(raw_tmp)
        copy_inputs(fixture)
        initial = MODULE.identities(fixture)
        assert initial == baseline

        (fixture / "irrelevant.txt").write_text("ignored\n", encoding="utf-8")
        assert MODULE.identities(fixture) == initial

        ignored_ui = fixture / "apps/mister/ui/.DS_Store"
        ignored_ui.write_bytes(b"ignored")
        assert MODULE.identities(fixture) == initial

        ui = next(iter(MODULE.files_for(fixture, ("apps/mister/ui/**/*.slint",))))
        ui.write_text(ui.read_text(encoding="utf-8") + "\n// cache identity test\n", encoding="utf-8")
        changed_ui = MODULE.identities(fixture)
        assert changed_ui["host_target"] != initial["host_target"]
        assert changed_ui["arm_target"] != initial["arm_target"]
        assert changed_ui["arm_build_cache"] == initial["arm_build_cache"]
        assert changed_ui["agent_cli"] == initial["agent_cli"]

        cross = fixture / "apps/mister/Cross.toml"
        cross.write_text(cross.read_text(encoding="utf-8").replace("d047ace4d737", "changedimage1"), encoding="utf-8")
        changed_cross = MODULE.identities(fixture)
        assert changed_cross["cross_abi"] != changed_ui["cross_abi"]
        assert changed_cross["arm_build_cache"] == changed_ui["arm_build_cache"]
        assert changed_cross["arm_target"] != changed_ui["arm_target"]
        assert changed_cross["agent_target"] != changed_ui["agent_target"]
        assert changed_cross["ffmpeg"] != changed_ui["ffmpeg"]

        arm_lock = fixture / "apps/mister/Cargo.lock"
        arm_lock.write_text(
            arm_lock.read_text(encoding="utf-8") + "\n# cache identity test\n",
            encoding="utf-8",
        )
        changed_arm_lock = MODULE.identities(fixture)
        assert changed_arm_lock["arm_build_cache"] != changed_cross["arm_build_cache"]
        assert changed_arm_lock["cargo_arm"] != changed_cross["cargo_arm"]

        lock = fixture / "crates/catalog/Cargo.lock"
        lock.write_text(lock.read_text(encoding="utf-8") + "\n# cache identity test\n", encoding="utf-8")
        changed_lock = MODULE.identities(fixture)
        assert changed_lock["arm_build_cache"] == changed_arm_lock["arm_build_cache"]
        assert changed_lock["cargo_host"] != changed_arm_lock["cargo_host"]
        assert changed_lock["host_target"] != changed_arm_lock["host_target"]
        assert changed_lock["agent_cli"] == changed_arm_lock["agent_cli"]
        assert changed_lock["agent_cli_deps"] == changed_arm_lock["agent_cli_deps"]

        toolchain = fixture / "apps/mister/rust-toolchain.toml"
        toolchain.write_text(
            toolchain.read_text(encoding="utf-8") + "\n# cache identity test\n",
            encoding="utf-8",
        )
        changed_toolchain = MODULE.identities(fixture)
        assert changed_toolchain["rust_abi"] != changed_lock["rust_abi"]
        assert changed_toolchain["cross_abi"] != changed_lock["cross_abi"]
        assert changed_toolchain["arm_build_cache"] == changed_lock["arm_build_cache"]
        assert changed_toolchain["agent_cli"] != changed_lock["agent_cli"]
        assert changed_toolchain["agent_cli_deps"] != changed_lock["agent_cli_deps"]
        assert changed_toolchain["host_target"] != changed_lock["host_target"]
        assert changed_toolchain["arm_target"] != changed_lock["arm_target"]
        assert changed_toolchain["agent_target"] != changed_lock["agent_target"]

        manifest = fixture / "apps/mister/Cargo.toml"
        manifest.write_text(manifest.read_text(encoding="utf-8") + "\n# cache identity test\n", encoding="utf-8")
        changed_manifest = MODULE.identities(fixture)
        assert (
            changed_manifest["arm_build_cache"]
            != changed_toolchain["arm_build_cache"]
        )
        assert changed_manifest["host_target"] != changed_toolchain["host_target"]
        assert changed_manifest["arm_target"] != changed_toolchain["arm_target"]
        assert changed_manifest["agent_cli_deps"] == changed_toolchain["agent_cli_deps"]

        desktop_lock = fixture / "apps/desktop/Cargo.lock"
        desktop_lock.write_text(
            desktop_lock.read_text(encoding="utf-8") + "\n# cache identity test\n",
            encoding="utf-8",
        )
        changed_desktop_lock = MODULE.identities(fixture)
        assert changed_desktop_lock["cargo_host"] != changed_manifest["cargo_host"]
        assert changed_desktop_lock["host_target"] != changed_manifest["host_target"]

        desktop_source = next(
            iter(MODULE.files_for(fixture, ("apps/desktop/src/**/*.rs",)))
        )
        desktop_source.write_text(
            desktop_source.read_text(encoding="utf-8") + "\n// cache identity test\n",
            encoding="utf-8",
        )
        changed_desktop_source = MODULE.identities(fixture)
        assert (
            changed_desktop_source["host_target"]
            != changed_desktop_lock["host_target"]
        )

        stream_source = next(
            iter(MODULE.files_for(fixture, ("crates/framebuffer-stream/src/**/*.rs",)))
        )
        stream_source.write_text(
            stream_source.read_text(encoding="utf-8") + "\n// cache identity test\n",
            encoding="utf-8",
        )
        changed_stream_source = MODULE.identities(fixture)
        assert (
            changed_stream_source["host_target"]
            != changed_desktop_source["host_target"]
        )
        assert (
            changed_stream_source["arm_target"]
            != changed_desktop_source["arm_target"]
        )

        source_expectations = (
            ("apps/mister/src/**/*.rs", ("host_target", "arm_target")),
            ("crates/magik-core/src/**/*.rs", ("host_target", "arm_target")),
            (
                "crates/catalog/src/**/*.rs",
                ("host_target", "arm_target", "agent_cli"),
            ),
            ("mister/platform/runtime/src/**/*.rs", ("host_target", "arm_target")),
            ("mister/tools/agent/src/**/*.rs", ("host_target", "agent_target")),
            (
                "mister/platform/contracts/video-diagnostics/src/**/*.rs",
                ("cargo_agent", "host_target", "agent_target"),
            ),
            ("agent-cli/src/**/*.rs", ("host_target", "agent_cli")),
            ("crates/media-contract/src/**/*.rs", ("agent_cli",)),
            ("crates/agent-protocol/src/**/*.rs", ("agent_cli",)),
        )
        previous = changed_stream_source
        for pattern, changed_groups in source_expectations:
            previous_agent_cli_deps = previous["agent_cli_deps"]
            previous_arm_build_cache = previous["arm_build_cache"]
            source = next(iter(MODULE.files_for(fixture, (pattern,))))
            source.write_text(
                source.read_text(encoding="utf-8") + "\n// cache identity test\n",
                encoding="utf-8",
            )
            current = MODULE.identities(fixture)
            for group in changed_groups:
                assert current[group] != previous[group], (pattern, group)
            assert current["agent_cli_deps"] == previous_agent_cli_deps, pattern
            assert current["arm_build_cache"] == previous_arm_build_cache, pattern
            previous = current

        compiled_input_expectations = (
            (
                "crates/catalog/data/**/*.json",
                ("host_target", "arm_target", "agent_cli"),
            ),
            ("crates/catalog/tests/**/*.rs", ("host_target",)),
            ("apps/mister/ui/fonts/*.ttf", ("host_target", "arm_target")),
            ("apps/mister/ui/icons/*.svg", ("host_target", "arm_target")),
        )
        for pattern, changed_groups in compiled_input_expectations:
            previous_agent_cli = previous["agent_cli"]
            previous_arm_build_cache = previous["arm_build_cache"]
            source = next(iter(MODULE.files_for(fixture, (pattern,))))
            source.write_bytes(source.read_bytes() + b"\n")
            current = MODULE.identities(fixture)
            for group in changed_groups:
                assert current[group] != previous[group], (pattern, group)
            if "agent_cli" not in changed_groups:
                assert current["agent_cli"] == previous_agent_cli, pattern
            assert current["arm_build_cache"] == previous_arm_build_cache, pattern
            previous = current

    print("cache identity tests ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
