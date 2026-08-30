# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations

import hashlib
import json
import subprocess
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

from scripts.magik_ci import build, bundle, databases, metadata
from scripts.magik_ci.assurance import fast_checks
from scripts.magik_ci.bundle import bundle_id, update_plan
from scripts.magik_ci.cli import parser
from scripts.magik_ci.host import HOST_GROUPS, commands
from scripts.magik_ci.manifest import candidate_id, serialize
from scripts.magik_ci.python_tests import SLOW_TEST
from scripts.magik_ci.python_tests import commands as python_test_commands
from scripts.magik_ci.quality import QUALITY_COMMANDS, execute


class MagikCiTests(unittest.TestCase):
    def test_cli_import_does_not_require_platform_manifest_dependencies(self) -> None:
        command = """
import builtins
real_import = builtins.__import__
def guarded_import(name, globals=None, locals=None, fromlist=(), level=0):
    if name == 'scripts.magik_ci.manifest' or (
        name == 'scripts.magik_ci' and 'manifest' in fromlist
    ):
        raise ModuleNotFoundError(name)
    return real_import(name, globals, locals, fromlist, level)
builtins.__import__ = guarded_import
import scripts.magik_ci.cli
"""
        subprocess.run([sys.executable, "-c", command], check=True)

    def test_failed_platform_run_is_eligible_only_for_verified_components(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory) / "run.json"
            run.write_text(
                '{"headSha":"0123456789012345678901234567890123456789",'
                '"headBranch":"main","status":"completed","conclusion":"failure"}',
                encoding="utf-8",
            )
            head_sha = "0123456789012345678901234567890123456789"
            self.assertFalse(metadata.platform_eligible_run(run, head_sha))
            self.assertTrue(
                metadata.platform_eligible_run(run, head_sha, allow_failed=True)
            )

    def test_cancelled_platform_run_remains_ineligible_for_components(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            run = Path(directory) / "run.json"
            run.write_text(
                '{"headSha":"0123456789012345678901234567890123456789",'
                '"headBranch":"main","status":"completed","conclusion":"cancelled"}',
                encoding="utf-8",
            )
            self.assertFalse(
                metadata.platform_eligible_run(
                    run,
                    "0123456789012345678901234567890123456789",
                    allow_failed=True,
                )
            )

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
        self.assertFalse(
            any(
                "mister-magik-ui-preview" in command
                and command[0:2] == ["cargo", "test"]
                for command in commands("app")
            )
        )
        self.assertFalse(
            any(
                command[0:2] == ["cargo", "check"] and "mister-magik-fb" in command
                for command in commands("app")
            )
        )
        self.assertFalse(
            any("ui,bench-scenes" in command for command in commands("app"))
        )
        self.assertFalse(
            any(
                "media_http::tests" in command
                and not any(
                    "signed-media-manifests" in argument for argument in command
                )
                for command in commands("app")
            )
        )
        self.assertIn(
            [
                "cargo",
                "test",
                "--manifest-path",
                "apps/mister/Cargo.toml",
                "--lib",
                "--no-default-features",
                "--features",
                "ui",
                "visual_platform::tests::cache_preserving_full_raster_refreshes_moved_deleted_and_rotated_content",
                "--",
                "--ignored",
                "--exact",
            ],
            commands("app"),
        )
        self.assertIn(["python3", SLOW_TEST], commands("app"))

    def test_host_group_does_not_shadow_the_top_level_command(self) -> None:
        args = parser().parse_args(["ci", "host-assurance", "--group", "app"])
        self.assertEqual(args.group, "ci")
        self.assertEqual(args.host_group, "app")

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

    def test_legacy_bundle_identity_requires_current_assembly(self) -> None:
        values = ("a" * 64, "b" * 64, "c" * 64)
        current: dict[str, object] = {
            "main_input_sha256": values[0],
            "fpga_input_sha256": values[1],
            "kernel_input_sha256": values[2],
            "bundle_id": bundle_id(*values, assembly_revision=0),
        }
        plan = update_plan(current, 35, *values)
        self.assertTrue(plan["update_needed"])
        self.assertFalse(plan["main_changed"])
        self.assertFalse(plan["fpga_changed"])
        self.assertFalse(plan["kernel_changed"])

    def test_updater_mra_inspection_tolerates_case_variant_rom_closing_tags(
        self,
    ) -> None:
        mra = b"""
            <misterromdescription>
                <name>Space Demon</name>
                <year>1980</year>
                <manufacturer>Nintendo</manufacturer>
                <setname>SpaceDemon</setname>
                <rbf>SpaceFirebird</rbf>
                <rom index="0" zip="spacedem.zip|spacefb.zip">
                    <part name="main.bin" />
                </ROM>
            </misterromdescription>
        """

        header, primary_rom = databases._mra_inspection(mra, "Space Demon.mra")

        self.assertEqual(header["name"], "Space Demon")
        self.assertEqual(header["setname"], "SpaceDemon")
        self.assertEqual(
            primary_rom,
            {"Archive": {"namespace": "Mame", "setname": "spacedem"}},
        )

    def test_updater_mra_inspection_handles_space_firebird_fixture(self) -> None:
        mra = b"""
            <misterromdescription>
                <name>Space Firebird</name>
                <setname>spacefb</setname>
                <rbf>SpaceFirebird</rbf>
                <rom index="0" zip="spacefb.zip">
                    <part name="main.bin" />
                </ROM>
            </misterromdescription>
        """

        header, primary_rom = databases._mra_inspection(mra, "Space Firebird.mra")

        self.assertEqual(header["name"], "Space Firebird")
        self.assertEqual(
            primary_rom,
            {"Archive": {"namespace": "Mame", "setname": "spacefb"}},
        )

    def test_updater_build_validates_sources_integrity_and_precedence(self) -> None:
        mra = b"""
            <misterromdescription>
                <name>Fixture Game</name>
                <setname>fixture</setname>
                <rom zip="fixture.zip"><part /></ROM>
            </misterromdescription>
        """
        source_order = (
            "distribution",
            "alternatives",
            "jtcores",
            "coinop",
            "arcade-offset",
        )

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            sources = []
            for position, source_id in enumerate(source_order):
                source_root = root / source_id
                mra_path = source_root / "_Arcade" / "Fixture Game.mra"
                mra_path.parent.mkdir(parents=True)
                mra_path.write_bytes(mra)
                database_path = root / f"{source_id}.json"
                database_path.write_text(
                    json.dumps(
                        {
                            "files": {
                                "_Arcade/Fixture Game.mra": {
                                    "hash": hashlib.md5(
                                        mra, usedforsecurity=False
                                    ).hexdigest(),
                                    "size": len(mra),
                                }
                            }
                        }
                    ),
                    encoding="utf-8",
                )
                sources.append(
                    {
                        "id": source_id,
                        "revision": f"{position + 1:040x}",
                        "database": str(database_path),
                        "roots": [str(source_root)],
                    }
                )

            inputs = root / "inputs.json"
            inputs.write_text(
                json.dumps(
                    {
                        "format": "mister-magik-arcade-updater-inputs-v1",
                        "sources": sources,
                    }
                ),
                encoding="utf-8",
            )
            output = root / "arcade-updater-index-v1.lz4b"

            summary = databases.build_updater(inputs, output)

            self.assertGreater(output.stat().st_size, 0)
            self.assertEqual(summary["compressed_bytes"], output.stat().st_size)
            self.assertEqual(summary["rows"], 1)
            self.assertEqual(summary["source_rows"], dict.fromkeys(source_order, 1))

            bad_database = root / "distribution.json"
            bad_database.write_text(
                json.dumps(
                    {
                        "files": {
                            "_Arcade/Fixture Game.mra": {
                                "hash": "0" * 32,
                                "size": len(mra),
                            }
                        }
                    }
                ),
                encoding="utf-8",
            )
            with self.assertRaisesRegex(ValueError, "updater MD5 mismatch"):
                databases.build_updater(inputs, root / "bad-index.lz4b")

    def test_updater_build_rejects_noncanonical_sources(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            inputs = Path(directory) / "inputs.json"
            inputs.write_text(
                json.dumps(
                    {
                        "format": "mister-magik-arcade-updater-inputs-v1",
                        "sources": [],
                    }
                ),
                encoding="utf-8",
            )

            with self.assertRaisesRegex(ValueError, "five canonical sources"):
                databases.build_updater(inputs, Path(directory) / "index.lz4b")

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
            ).write_text(
                "platform_contract_sha256="
                + "1" * 64
                + "\nrbf_sha256="
                + "2" * 64
                + "\nlatch_protocol_sha256="
                + "3" * 64
                + "\nlatch_protocol_version=5"
                + "\ndiagnostic_architecture=scaler-output-scheduler-gates-v1\n"
            )
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
            payload = verify(archive)
            self.assertEqual(payload["release_version"], 1)
            self.assertEqual(payload["assembly_revision"], 1)
            self.assertEqual(payload["latch_rbf_sha256"], "2" * 64)
            self.assertEqual(payload["latch_protocol_sha256"], "3" * 64)
            self.assertEqual(payload["latch_protocol_version"], 5)
            self.assertEqual(
                payload["diagnostic_architecture"],
                "scaler-output-scheduler-gates-v1",
            )

    def test_platform_bundle_historical_baseline_architectures_are_bounded(
        self,
    ) -> None:
        from scripts.magik_ci.bundle import (
            HISTORICAL_DIAGNOSTIC_ARCHITECTURES,
            PATCHED_DIAGNOSTIC_ARCHITECTURE,
            _validate_diagnostic_architecture,
        )

        self.assertEqual(
            HISTORICAL_DIAGNOSTIC_ARCHITECTURES,
            {
                "scaler-fetch-no-request-gates-v1",
                PATCHED_DIAGNOSTIC_ARCHITECTURE,
            },
        )
        arguments = parser().parse_args(
            [
                "ci",
                "platform-bundle",
                "verify",
                "platform.zip",
                "--historical-baseline",
            ]
        )
        self.assertTrue(arguments.historical_baseline)
        extract_arguments = parser().parse_args(
            [
                "ci",
                "platform-bundle",
                "extract-component",
                "platform.zip",
                "--manifest",
                "platform.json",
                "--component",
                "main",
                "--component-id",
                "a" * 64,
                "--output",
                "main",
                "--historical-baseline",
            ]
        )
        self.assertTrue(extract_arguments.historical_baseline)
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            archive = root / "platform.zip"
            manifest = root / "platform.json"
            output = root / "main"
            with zipfile.ZipFile(archive, "w") as stream:
                stream.writestr("main/component.bin", b"component")
            with patch.object(
                bundle,
                "verify",
                return_value={
                    "main_input_sha256": "a" * 64,
                    "components": {"main": {}},
                    "release_version": 37,
                },
            ) as verify_bundle:
                bundle.extract_component(
                    archive,
                    manifest,
                    "main",
                    "a" * 64,
                    output,
                    historical_baseline=True,
                )
            verify_bundle.assert_called_once_with(
                archive,
                manifest,
                historical_baseline=True,
            )
            self.assertEqual((output / "component.bin").read_bytes(), b"component")
        _validate_diagnostic_architecture(
            "scaler-fetch-no-request-gates-v1",
            "scaler-fetch-no-request-gates-v1",
            historical_baseline=True,
        )
        with self.assertRaisesRegex(ValueError, "fpga_diagnostic_architecture"):
            _validate_diagnostic_architecture(
                "scaler-fetch-no-request-gates-v1",
                "scaler-fetch-no-request-gates-v1",
                historical_baseline=False,
            )
        with self.assertRaisesRegex(ValueError, "fpga_diagnostic_architecture"):
            _validate_diagnostic_architecture(
                "unknown-diagnostic-v1",
                "unknown-diagnostic-v1",
                historical_baseline=True,
            )
        with self.assertRaisesRegex(ValueError, "fpga_diagnostic_architecture"):
            _validate_diagnostic_architecture(
                "scaler-fetch-no-request-gates-v1",
                PATCHED_DIAGNOSTIC_ARCHITECTURE,
                historical_baseline=True,
            )

    def test_platform_assembly_requires_successful_planning(self) -> None:
        workflow = (
            Path(__file__).resolve().parents[2]
            / ".github/workflows/platform-bundle.yml"
        ).read_text(encoding="utf-8")

        self.assertIn(
            "always() && needs.plan.result == 'success' &&",
            workflow,
        )
        self.assertIn("--historical-baseline >/dev/null", workflow)
        extraction_lines = [
            line for line in workflow.splitlines() if "extract-component" in line
        ]
        self.assertEqual(len(extraction_lines), 3)
        self.assertTrue(
            all("--historical-baseline" in line for line in extraction_lines)
        )

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
