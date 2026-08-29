# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.magik_ci import build, metadata
from scripts.magik_ci.assurance import fast_checks
from scripts.magik_ci.bundle import bundle_id, update_plan
from scripts.magik_ci.host import HOST_GROUPS, commands
from scripts.magik_ci.manifest import candidate_id, serialize
from scripts.magik_ci.python_tests import SLOW_TEST
from scripts.magik_ci.python_tests import commands as python_test_commands
from scripts.magik_ci.quality import QUALITY_COMMANDS, execute


class MagikCiTests(unittest.TestCase):
    @patch("scripts.magik_ci.metadata.subprocess.run")
    def test_visual_host_assurance_runs_the_real_matrix(self, run) -> None:
        metadata.host_assurance(["apps/mister/tests/visual-baselines/launcher"])
        self.assertEqual(
            run.call_args_list[-1].args[0],
            [
                "cargo",
                "run",
                "--manifest-path",
                "apps/mister/Cargo.toml",
                "--bin",
                "mister-magik-ui-preview",
                "--no-default-features",
                "--features",
                "ui-preview",
                "--",
                "--check-baselines",
                "apps/mister/tests/visual-baselines/launcher",
            ],
        )

    def test_arm_library_check_is_a_no_default_feature_check(self) -> None:
        self.assertEqual(
            build.CHECKS["runtime-library-ci"],
            ("apps/mister/Cargo.toml", "all", ""),
        )

    def test_host_groups_have_unique_commands_and_no_preview_build(self) -> None:
        self.assertEqual(
            HOST_GROUPS, ("static", "agent", "domain", "catalog", "app", "tools")
        )
        all_commands = [command for group in HOST_GROUPS for command in commands(group)]
        self.assertEqual(
            len(all_commands), len({tuple(command) for command in all_commands})
        )
        self.assertFalse(
            any(
                "ui-preview" in command[1:] and command[1] == "build"
                for command in all_commands
            )
        )
        self.assertIn(["python3", SLOW_TEST], commands("app"))

    def test_python_tests_skip_unrelated_paths(self) -> None:
        self.assertEqual(python_test_commands(["agent-cli/src/main.rs"]), [])

    def test_python_changes_run_fast_tests_only(self) -> None:
        self.assertEqual(
            python_test_commands(["scripts/magik_ci/assurance.py"]),
            [
                [
                    "uv",
                    "run",
                    "pytest",
                    "scripts/tests",
                    "-q",
                    "--ignore",
                    SLOW_TEST,
                ],
            ],
        )

    def test_ui_changes_run_only_the_slint_contract(self) -> None:
        self.assertEqual(
            python_test_commands(["apps/mister/ui/components/combo_box.slint"]),
            [["python3", SLOW_TEST]],
        )

    def test_combined_python_and_ui_changes_run_the_full_suite_once(self) -> None:
        self.assertEqual(
            python_test_commands(
                [
                    "scripts/magik_ci/assurance.py",
                    "apps/mister/ui/components/combo_box.slint",
                ]
            ),
            [["uv", "run", "pytest", "scripts/tests", "-q"]],
        )

    def test_fast_assurance_selects_only_static_checks_for_slint(self) -> None:
        commands = fast_checks(
            Path("/repository"), ["apps/mister/ui/components/launcher.slint"]
        )
        self.assertEqual(
            commands,
            [
                ["scripts/checks/check-repository-layout.py"],
                ["scripts/checks/check-unified-agent-surface.py"],
                [
                    "scripts/checks/check-font-text-contract.py",
                    "--repository",
                    ".",
                    "--all",
                ],
                [
                    "scripts/checks/check-launcher-contract.py",
                    "--repository",
                    ".",
                    "--all",
                ],
            ],
        )
        self.assertFalse(any(command[0] in {"cargo", "cross"} for command in commands))

    def test_fast_assurance_handles_deleted_shell_paths(self) -> None:
        self.assertEqual(
            fast_checks(Path("/repository"), ["scripts/removed-helper.sh"]),
            [
                ["scripts/checks/check-repository-layout.py"],
                ["scripts/checks/check-unified-agent-surface.py"],
                [
                    "scripts/checks/check-font-text-contract.py",
                    "--repository",
                    ".",
                    "--all",
                ],
            ],
        )

    def test_quality_commands_match_ci_scopes(self) -> None:
        self.assertEqual(
            QUALITY_COMMANDS["format"],
            (
                "uv",
                "run",
                "ruff",
                "format",
                "--check",
                "scripts",
                "apps/mister/ui_tests",
            ),
        )
        self.assertEqual(
            QUALITY_COMMANDS["lint"],
            ("uv", "run", "ruff", "check", "scripts", "apps/mister/ui_tests"),
        )
        self.assertEqual(QUALITY_COMMANDS["typecheck"], ("uv", "run", "ty", "check"))

    @patch("scripts.magik_ci.quality.subprocess.run")
    def test_quality_all_runs_every_check_and_aggregates_failures(self, run) -> None:
        run.side_effect = [
            subprocess.CompletedProcess([], 1),
            subprocess.CompletedProcess([], 0),
            subprocess.CompletedProcess([], 2),
        ]
        with self.assertRaisesRegex(RuntimeError, "format.*typecheck"):
            execute(Path("/repository"), ["all"])
        self.assertEqual(run.call_count, 3)
        self.assertEqual(
            [call.args[0] for call in run.call_args_list],
            [list(QUALITY_COMMANDS[name]) for name in ("format", "lint", "typecheck")],
        )

    def test_bundle_identity_is_deterministic(self) -> None:
        values = ("a" * 64, "b" * 64, "c" * 64)
        self.assertEqual(bundle_id(*values), bundle_id(*values))

    def test_platform_update_plan_starts_at_one(self) -> None:
        plan = update_plan(None, 0, "a" * 64, "b" * 64, "c" * 64)
        self.assertEqual(plan["next_version"], 1)
        self.assertTrue(plan["update_needed"])

    def test_manifest_candidate_is_ordered(self) -> None:
        values = {
            field: "x"
            for field in __import__(
                "scripts.magik_ci.manifest", fromlist=["FIELDS"]
            ).FIELDS
        }
        values["qualification_candidate_id"] = candidate_id(values)
        self.assertEqual(len(serialize(values).splitlines()), 25)

    def test_bundle_round_trip(self) -> None:
        from scripts.magik_ci.bundle import create, verify

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("main", "fpga", "scanout"):
                (root / name).mkdir()
                (root / name / "payload").write_bytes(name.encode())
            (root / "fpga" / "patched").mkdir()
            (
                root / "fpga" / "patched" / "menu-magik-vblank-latch.metadata.txt"
            ).write_text("platform_contract_sha256=" + "1" * 64 + "\n")
            archive = create(
                main=root / "main",
                fpga=root / "fpga",
                scanout=root / "scanout",
                main_id="a" * 64,
                fpga_id="b" * 64,
                kernel_id="c" * 64,
                main_run_id="1",
                fpga_run_id="2",
                kernel_run_id="3",
                main_head_sha="d" * 40,
                fpga_head_sha="e" * 40,
                kernel_head_sha="f" * 40,
                main_source="main",
                fpga_source="fpga",
                kernel_source="kernel",
                release_version=1,
                output=root / "out",
            )
            self.assertEqual(verify(archive)["release_version"], 1)

    def test_database_round_trip(self) -> None:
        from scripts.magik_ci.databases import create, verify

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            mame = root / "mame.sqlite3"
            hbmame = root / "hbmame.sqlite3"
            csv = root / "ArcadeDatabase.csv"
            license_file = root / "ArcadeDatabase-LICENSE.txt"
            index = root / "arcade-updater-index-v1.lz4b"
            for path, data in (
                (mame, b"mame"),
                (hbmame, b"hbmame"),
                (csv, b"name\n"),
                (license_file, b"license"),
                (index, b"index"),
            ):
                path.write_bytes(data)
            archive = create(
                mame=mame,
                hbmame=hbmame,
                release_version=1,
                mame_tag="mame0288",
                mame_sha="a" * 40,
                listxml_asset="listxml.zip",
                listxml_sha256="b" * 64,
                hbmame_tag="hbmame",
                hbmame_sha="c" * 40,
                mame_builder_sha="d" * 40,
                hbmame_builder_sha="e" * 40,
                arcade_database_csv=csv,
                arcade_database_license=license_file,
                arcade_database_sha="f" * 40,
                arcade_database_builder_sha="1" * 40,
                arcade_updater_builder_sha="2" * 40,
                arcade_updater_index=index,
                output=root / "release",
            )
            self.assertEqual(verify(archive)["release_version"], 1)


if __name__ == "__main__":
    unittest.main()
