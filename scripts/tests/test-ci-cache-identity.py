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

        ignored_ui = fixture / "magik-gui/ui/.DS_Store"
        ignored_ui.write_bytes(b"ignored")
        assert MODULE.identities(fixture) == initial

        ui = next(iter(MODULE.files_for(fixture, ("magik-gui/ui/**/*.slint",))))
        ui.write_text(ui.read_text(encoding="utf-8") + "\n// cache identity test\n", encoding="utf-8")
        changed_ui = MODULE.identities(fixture)
        assert changed_ui["host_target"] != initial["host_target"]
        assert changed_ui["arm_target"] != initial["arm_target"]

        cross = fixture / "magik-gui/Cross.toml"
        cross.write_text(cross.read_text(encoding="utf-8").replace("d047ace4d737", "changedimage1"), encoding="utf-8")
        changed_cross = MODULE.identities(fixture)
        assert changed_cross["cross_abi"] != changed_ui["cross_abi"]
        assert changed_cross["arm_target"] != changed_ui["arm_target"]
        assert changed_cross["agent_target"] != changed_ui["agent_target"]
        assert changed_cross["ffmpeg"] != changed_ui["ffmpeg"]

        lock = fixture / "magik-gui/catalog/Cargo.lock"
        lock.write_text(lock.read_text(encoding="utf-8") + "\n# cache identity test\n", encoding="utf-8")
        changed_lock = MODULE.identities(fixture)
        assert changed_lock["cargo_host"] != changed_cross["cargo_host"]
        assert changed_lock["host_target"] != changed_cross["host_target"]

        toolchain = fixture / "magik-gui/rust-toolchain.toml"
        toolchain.write_text(toolchain.read_text(encoding="utf-8").replace("1.97.0", "1.98.0"), encoding="utf-8")
        changed_toolchain = MODULE.identities(fixture)
        assert changed_toolchain["rust_abi"] != changed_lock["rust_abi"]
        assert changed_toolchain["host_target"] != changed_lock["host_target"]
        assert changed_toolchain["arm_target"] != changed_lock["arm_target"]
        assert changed_toolchain["agent_target"] != changed_lock["agent_target"]

        manifest = fixture / "magik-gui/Cargo.toml"
        manifest.write_text(manifest.read_text(encoding="utf-8") + "\n# cache identity test\n", encoding="utf-8")
        changed_manifest = MODULE.identities(fixture)
        assert changed_manifest["host_target"] != changed_toolchain["host_target"]
        assert changed_manifest["arm_target"] != changed_toolchain["arm_target"]

        desktop_lock = fixture / "desktop/Cargo.lock"
        desktop_lock.write_text(
            desktop_lock.read_text(encoding="utf-8") + "\n# cache identity test\n",
            encoding="utf-8",
        )
        changed_desktop_lock = MODULE.identities(fixture)
        assert changed_desktop_lock["cargo_host"] != changed_manifest["cargo_host"]
        assert changed_desktop_lock["host_target"] != changed_manifest["host_target"]

        desktop_source = next(
            iter(MODULE.files_for(fixture, ("desktop/src/**/*.rs",)))
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
            iter(MODULE.files_for(fixture, ("framebuffer-stream/src/**/*.rs",)))
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
            ("magik-gui/src/**/*.rs", ("host_target", "arm_target")),
            ("magik-gui/catalog/src/**/*.rs", ("host_target", "arm_target")),
            ("tools/mister/src/**/*.rs", ("host_target",)),
            ("tools/magik-agent/src/**/*.rs", ("host_target", "agent_target")),
        )
        previous = changed_stream_source
        for pattern, changed_groups in source_expectations:
            source = next(iter(MODULE.files_for(fixture, (pattern,))))
            source.write_text(
                source.read_text(encoding="utf-8") + "\n// cache identity test\n",
                encoding="utf-8",
            )
            current = MODULE.identities(fixture)
            for group in changed_groups:
                assert current[group] != previous[group], (pattern, group)
            previous = current

        compiled_input_expectations = (
            ("magik-gui/catalog/data/**/*.json", ("host_target", "arm_target")),
            ("magik-gui/catalog/tests/**/*.rs", ("host_target",)),
            ("magik-gui/ui/fonts/*.ttf", ("host_target", "arm_target")),
            ("magik-gui/ui/icons/*.svg", ("host_target", "arm_target")),
        )
        for pattern, changed_groups in compiled_input_expectations:
            source = next(iter(MODULE.files_for(fixture, (pattern,))))
            source.write_bytes(source.read_bytes() + b"\n")
            current = MODULE.identities(fixture)
            for group in changed_groups:
                assert current[group] != previous[group], (pattern, group)
            previous = current

    print("cache identity tests ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
