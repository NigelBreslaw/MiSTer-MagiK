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
from typing import Any, cast
from unittest.mock import patch

from scripts.magik_ci import build, bundle, databases, metadata
from scripts.magik_ci.assurance import fast_checks
from scripts.magik_ci.bundle import bundle_id, update_plan
from scripts.magik_ci.cli import parser
from scripts.magik_ci.host import HOST_GROUPS, commands
from scripts.magik_ci.manifest import candidate_id, parse_fields, serialize
from scripts.magik_ci.python_tests import SLOW_TEST
from scripts.magik_ci.python_tests import commands as python_test_commands
from scripts.magik_ci.quality import QUALITY_COMMANDS, execute


class MagikCiTests(unittest.TestCase):
    def test_build_mame_ingests_software_lists_and_hashes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            listxml = root / "listxml.xml"
            listxml.write_text(
                """<mame build="0.289 (mame0289)"><machine name="fixture" sourcefile="src/fixture.cpp">
                  <description>Fixture Machine</description><year>1985</year>
                  <manufacturer>Example</manufacturer>
                  <input players="1" control="joy" />
                  <display type="raster" width="256" height="240" rotate="0" />
                  <driver status="good" emulation="good" savestate="supported" />
                </machine></mame>""",
                encoding="utf-8",
            )
            hash_dir = root / "hash"
            hash_dir.mkdir()
            (hash_dir / "c64_cart.xml").write_text(
                """<softwarelist name="c64_cart" build="0.289 (mame0289)" description="C64 carts">
                  <software name="fixture" cloneof="parent">
                    <description>Fixture Cart</description><year>1985</year>
                    <publisher>Example</publisher>
                    <part name="cart"><dataarea name="rom" size="4">
                      <rom name="fixture.bin" size="4" crc="deadbeef"
                           sha1="0123456789abcdef0123456789abcdef01234567" />
                    </dataarea></part>
                  </software>
                </softwarelist>""",
                encoding="utf-8",
            )
            output = root / "mame.sqlite3"
            databases.build_mame(
                listxml=listxml,
                out=output,
                software_dir=hash_dir,
            )
            connection = databases.sqlite3.connect(output)
            self.assertEqual(
                connection.execute(
                    "SELECT title, parent_setname FROM mame_machines"
                ).fetchone(),
                ("Fixture Machine", None),
            )
            self.assertEqual(
                connection.execute(
                    "SELECT source_version FROM mame_machines"
                ).fetchone(),
                ("0.289 (mame0289)",),
            )
            self.assertEqual(
                connection.execute(
                    "SELECT list_name, software_name, parent_name FROM mame_software_items"
                ).fetchone(),
                ("c64_cart", "fixture", "parent"),
            )
            self.assertEqual(
                connection.execute(
                    "SELECT source_version FROM mame_software_items"
                ).fetchone(),
                ("0.289 (mame0289)",),
            )
            self.assertEqual(
                connection.execute(
                    "SELECT size, crc32, sha1, data_area FROM mame_software_hashes"
                ).fetchone(),
                (
                    4,
                    "deadbeef",
                    "0123456789abcdef0123456789abcdef01234567",
                    "rom",
                ),
            )
            connection.close()

    def test_mame_runtime_coverage_requires_every_supported_system(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / "mame.sqlite3"
            connection = databases.sqlite3.connect(database)
            connection.executescript(
                """
                CREATE TABLE mame_software_items (
                    list_name TEXT NOT NULL,
                    software_name TEXT NOT NULL
                );
                CREATE TABLE mame_software_hashes (list_name TEXT NOT NULL);
                """
            )
            connection.executemany(
                "INSERT INTO mame_software_items VALUES (?, ?)",
                (
                    (source_lists[0], f"fixture-{index}")
                    for index, (_, _, source_lists) in enumerate(
                        databases.MAME_RUNTIME_SOFTWARE_LISTS
                    )
                ),
            )
            connection.execute("INSERT INTO mame_software_hashes VALUES ('nes')")
            connection.commit()
            connection.close()

            report = databases.mame_runtime_coverage(database)
            self.assertEqual(report["required_system_count"], 34)
            self.assertEqual(report["covered_system_count"], 34)
            self.assertEqual(len(cast(list[object], report["systems"])), 34)

            connection = databases.sqlite3.connect(database)
            connection.execute(
                "DELETE FROM mame_software_items WHERE list_name='spectrum_cart'"
            )
            connection.commit()
            connection.close()
            with self.assertRaisesRegex(
                ValueError, "mame_runtime_coverage_missing: zx-spectrum"
            ):
                databases.mame_runtime_coverage(database)

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

    def test_component_metadata_allows_repeatable_source_status_only_when_requested(
        self,
    ) -> None:
        metadata = (
            "format=current\n"
            "source_status= M menu.qsf\n"
            "source_status= M sys/sys_top.sdc\n"
        )
        with self.assertRaises(ValueError):
            parse_fields(metadata)
        self.assertEqual(
            parse_fields(metadata, repeatable_keys=frozenset({"source_status"})),
            {"format": "current"},
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

    def test_tools_assurance_runs_the_local_installer_after_manager_build(self) -> None:
        tools_commands = commands("tools")
        installer = ["scripts/tests/test-mister-magik-installer.sh"]
        manager_test = [
            "cargo",
            "test",
            "--manifest-path",
            "mister/tools/manager/Cargo.toml",
        ]
        manager_build = [
            "cargo",
            "build",
            "--manifest-path",
            "mister/tools/manager/Cargo.toml",
        ]
        self.assertEqual(tools_commands.count(installer), 1)
        self.assertLess(
            tools_commands.index(manager_test), tools_commands.index(installer)
        )
        self.assertLess(
            tools_commands.index(manager_build), tools_commands.index(installer)
        )

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
                + "\ndiagnostic_architecture=scaler-off-domain-scheduler-terminal-v4\n"
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
                "scaler-off-domain-scheduler-terminal-v4",
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
                "scaler-output-scheduler-gates-v1",
                "scaler-pre-read-scheduler-evidence-v1",
                "scaler-off-domain-scheduler-snapshot-v1",
                "scaler-off-domain-scheduler-snapshot-v2",
                "scaler-off-domain-scheduler-terminal-v3",
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
            ):
                path.write_bytes(data)
            mra = (
                b"<misterromdescription><name>Fixture Game</name>"
                b'<rom zip="fixture.zip"/></misterromdescription>'
            )
            source_order = (
                "distribution",
                "alternatives",
                "jtcores",
                "coinop",
                "arcade-offset",
            )
            updater_sources = []
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
                updater_sources.append(
                    {
                        "id": source_id,
                        "revision": f"{position + 1:040x}",
                        "database": str(database_path),
                        "roots": [str(source_root)],
                    }
                )
            inputs = root / "updater-inputs.json"
            inputs.write_text(
                json.dumps(
                    {
                        "format": "mister-magik-arcade-updater-inputs-v1",
                        "sources": updater_sources,
                    }
                ),
                encoding="utf-8",
            )
            databases.build_updater(inputs, index)
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
            payload = cast(dict[str, Any], verify(archive))
            self.assertEqual(payload["release_version"], 1)
            updater = payload["sources"]["arcade_updater"]
            self.assertEqual(updater["format"], "mister-magik-arcade-updater-index-v1")
            self.assertEqual(
                [source["id"] for source in updater["sources"]],
                sorted(source_order),
            )
            self.assertEqual(updater["catalog_metadata_rows"], 0)

            with zipfile.ZipFile(archive) as stream:
                legacy_files = {name: stream.read(name) for name in stream.namelist()}
            legacy_manifest = json.loads(legacy_files[databases.MANIFEST])
            legacy_manifest["sources"]["arcade_updater"] = {
                "builder_sha": updater["builder_sha"],
                "sha256": updater["sha256"],
            }
            legacy_files[databases.MANIFEST] = (
                json.dumps(legacy_manifest, indent=2, sort_keys=True) + "\n"
            ).encode()
            legacy_files[databases.CHECKSUMS] = (
                "".join(
                    f"{databases.sha256_bytes(data)}  {name}\n"
                    for name, data in legacy_files.items()
                    if name not in {databases.MANIFEST, databases.CHECKSUMS}
                ).encode()
                + (
                    f"{databases.sha256_bytes(legacy_files[databases.MANIFEST])}  "
                    f"{databases.MANIFEST}\n"
                ).encode()
            )
            legacy_archive = root / "legacy-v3.zip"
            with zipfile.ZipFile(
                legacy_archive, "w", compression=zipfile.ZIP_DEFLATED
            ) as stream:
                for name, data in legacy_files.items():
                    stream.writestr(name, data)
            self.assertEqual(verify(legacy_archive)["release_version"], 1)


if __name__ == "__main__":
    unittest.main()
