#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Contract tests for Slint build-script dependency tracking."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
BUILD_SCRIPT = ROOT / "apps/mister/ui-generated/build.rs"
UI_ROOT = ROOT / "apps/mister/ui"
UI_CRATE = ROOT / "apps/mister/ui-generated"

REQUIRED_BUILD_SCRIPT_FRAGMENTS = (
    "cargo:rerun-if-env-changed=MISTER_UI_BUILD_SCOPE",
    'unwrap_or_else(|_| "production".into())',
    'std::env::var_os("CARGO_FEATURE_BENCH_SCENES")',
    '"launcher" | "arcade" | "production" => true',
    "cargo:rustc-cfg=mister_ui_scope_launcher",
    "cargo:rustc-cfg=mister_bench_scenes",
    '"../ui/controller_test.slint"',
    '"../ui/launcher.slint"',
    '"../ui/bench/tear_pattern.slint"',
    '"../ui/bench/video_playback.slint"',
    '"../ui/experiments/effect_hud.slint"',
    "slint_build::EmbedResourcesKind::EmbedFiles",
    "slint_build::compile_with_config(path, config)",
)

FORBIDDEN_BUILD_SCRIPT_FRAGMENTS = (
    '"../ui/mockups/',
    "GENERATOR_CACHE_REVISION",
    "let mut inputs",
    "cargo:rerun-if-changed=",
    "OUT_DIR",
    "slint-inputs.fingerprint",
    "generated_outputs_exist",
    "fn fingerprint(",
    "fn hash_bytes(",
    "return;",
)


def run_cargo(manifest: Path, target: Path) -> subprocess.CompletedProcess[str]:
    environment = os.environ.copy()
    environment["CARGO_TARGET_DIR"] = str(target)
    environment["MISTER_UI_BUILD_SCOPE"] = "launcher"
    return subprocess.run(
        [
            "cargo",
            "check",
            "--manifest-path",
            str(manifest),
            "--quiet",
        ],
        cwd=manifest.parent,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
    )


class SlintBuildContractTests(unittest.TestCase):
    def test_build_script_always_delegates_dependency_tracking_to_slint(self) -> None:
        text = BUILD_SCRIPT.read_text()
        for fragment in REQUIRED_BUILD_SCRIPT_FRAGMENTS:
            with self.subTest(required=fragment):
                self.assertIn(fragment, text)
        for fragment in FORBIDDEN_BUILD_SCRIPT_FRAGMENTS:
            with self.subTest(forbidden=fragment):
                self.assertNotIn(fragment, text)

    def test_imported_component_change_reinvokes_slint_compilation(self) -> None:
        with tempfile.TemporaryDirectory(prefix="slint-build-contract-") as name:
            fixture = Path(name)
            app_root = fixture / "apps/mister"
            ui_root = app_root / "ui"
            ui_crate = app_root / "ui-generated"
            shutil.copytree(UI_ROOT, ui_root)
            shutil.copytree(
                UI_CRATE,
                ui_crate,
                ignore=shutil.ignore_patterns("target", "__pycache__"),
            )

            manifest = ui_crate / "Cargo.toml"
            target = fixture / "target"
            initial = run_cargo(manifest, target)
            self.assertEqual(
                initial.returncode,
                0,
                f"initial compiled-UI fixture failed:\n{initial.stdout}\n{initial.stderr}",
            )

            imported = ui_root / "components/combo_box.slint"
            imported.write_text("this is not valid Slint;\n")
            advanced_mtime = time.time_ns() + 2_000_000_000
            os.utime(imported, ns=(advanced_mtime, advanced_mtime))

            rebuilt = run_cargo(manifest, target)
            combined = f"{rebuilt.stdout}\n{rebuilt.stderr}"
            self.assertNotEqual(
                rebuilt.returncode,
                0,
                "changing a transitive Slint import did not trigger compilation",
            )
            self.assertIn("combo_box.slint", combined)


if __name__ == "__main__":
    unittest.main()
