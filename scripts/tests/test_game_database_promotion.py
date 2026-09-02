# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Exercise database restoration and reuse from the real workflow, offline."""

from __future__ import annotations

import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
import textwrap
import unittest
import zipfile
from pathlib import Path

from scripts.magik_ci import databases

ROOT = Path(__file__).resolve().parents[2]
WORKFLOW = ROOT / ".github/workflows/game-databases.yml"
RESTORE = "Restore unchanged databases from current release"
OLD_DATABASES = {"mame.sqlite3": b"old mame", "hbmame.sqlite3": b"old hbmame"}


def step_block(workflow: str, name: str) -> str:
    """Read one named assemble step, not a general YAML/Actions interpreter."""
    start = "\n  assemble:\n"
    end = "\n  publish:\n"
    if workflow.count(start) != 1 or workflow.count(end) != 1:
        raise ValueError("expected unique assemble and publish jobs")
    assemble = workflow.split(start, 1)[1].split(end, 1)[0]
    marker = f"      - name: {name}\n"
    if assemble.count(marker) != 1:
        raise ValueError(f"expected one assemble step: {name}")
    return re.split(
        r"^      - ", assemble.split(marker, 1)[1], maxsplit=1, flags=re.MULTILINE
    )[0]


def step_run(workflow: str, name: str) -> str:
    block = step_block(workflow, name)
    marker = "        run: "
    if block.count(marker) != 1:
        raise ValueError(f"expected one run command: {name}")
    run = block.split(marker, 1)[1]
    first, _, rest = run.partition("\n")
    if first == "|":
        if any(
            line.strip() and not line.startswith("          ")
            for line in rest.splitlines()
        ):
            raise ValueError(f"unsupported run block indentation: {name}")
        return textwrap.dedent(rest).strip()
    if rest.strip() or first in {">", "|-", ">-"}:
        raise ValueError(f"unsupported run command: {name}")
    return first


def create_prior_release(root: Path, *, compact: bool) -> None:
    """Use the production producers, with tiny data instead of full databases."""
    root.mkdir(parents=True)
    for name, data in OLD_DATABASES.items():
        (root / name).write_bytes(data)
    csv = root / "ArcadeDatabase.csv"
    csv.write_bytes(b"name\n")
    license_file = root / "ArcadeDatabase-LICENSE.txt"
    license_file.write_bytes(b"fixture license")
    sources = []
    for position, source_id in enumerate(databases.SOURCE_ORDER):
        source = root / f"{source_id}.json"
        source.write_text(json.dumps({"files": {}}), encoding="utf-8")
        sources.append(
            {
                "id": source_id,
                "revision": f"{position + 1:040x}",
                "database": str(source),
                "roots": [str(root)],
            }
        )
    inputs = root / "updater-inputs.json"
    inputs.write_text(
        json.dumps({"format": databases.INPUT_FORMAT, "sources": sources}),
        encoding="utf-8",
    )
    index = root / databases.INDEX
    databases.build_updater(inputs, index)
    metadata = bytearray(96)
    metadata[:8] = b"MMMETA1\0"
    metadata[8:12] = (1).to_bytes(4, "little")
    metadata[16:24] = (96).to_bytes(8, "little")
    metadata[28:36] = (96).to_bytes(8, "little")
    metadata[36:40] = (128).to_bytes(4, "little")
    metadata[44:76] = hashlib.sha256(b"").digest()
    metadata_path = root / databases.RUNTIME_METADATA
    metadata_path.write_bytes(metadata)
    archive = databases.create(
        mame=root / "mame.sqlite3",
        hbmame=root / "hbmame.sqlite3",
        release_version=18,
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
        runtime_metadata=metadata_path if compact else None,
        source_output=root / "source" if compact else None,
        output=root / "release",
    )
    databases.verify(archive)


GH_STUB = """
import json
import os
import shutil
import sys
from pathlib import Path

args = sys.argv[1:]
root = Path(os.environ["PROMOTION_FIXTURE"])
with (root / "gh-calls.jsonl").open("a") as log:
    log.write(json.dumps(args) + "\\n")

def option(name):
    return args[args.index(name) + 1]

assert option("--repo") == "fixture/offline", args
if args[:2] == ["release", "download"]:
    assert args[2] == "game-databases-v18", args
    target = Path(option("--dir"))
    target.mkdir(parents=True, exist_ok=True)
    for offset, argument in enumerate(args):
        if argument == "--pattern":
            name = args[offset + 1]
            assert name in {"mister-magik-game-databases-v18.zip", "game-databases-manifest.json"}, args
            shutil.copyfile(root / "release" / name, target / name)
elif args[:2] == ["run", "list"]:
    assert option("--workflow") == "game-databases.yml", args
    assert option("--branch") == "main", args
    assert option("--status") == "success", args
    print("100")
elif args[:2] == ["run", "download"]:
    assert args[2] == "100", args
    assert option("--name") == "game-databases-source-v18", args
    source = root / "source/mister-magik-game-databases-source-v18.zip"
    if not source.exists():
        sys.exit(1)
    target = Path(option("--dir"))
    target.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, target / source.name)
else:
    raise AssertionError(f"unexpected offline gh call: {args}")
"""


class DatabasePromotionTests(unittest.TestCase):
    def setUp(self) -> None:
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.fixture = self.root / "fixture"
        self.workspace = self.root / "workspace"
        self.workspace.mkdir()
        (self.workspace / "scripts").symlink_to(
            ROOT / "scripts", target_is_directory=True
        )
        self.workflow = WORKFLOW.read_text(encoding="utf-8")
        self.bin = self.root / "bin"
        self.bin.mkdir()
        # No real gh, curl, git, or ssh is reachable by workflow shell commands.
        for command in ("mkdir", "cp", "unzip", "grep"):
            executable = shutil.which(command)
            if executable is None:
                self.fail(f"required offline test tool missing: {command}")
            (self.bin / command).symlink_to(executable)
        (self.bin / "python3").symlink_to(sys.executable)
        gh = self.bin / "gh"
        gh.write_text(f"#!{sys.executable}\n" + GH_STUB, encoding="utf-8")
        gh.chmod(0o755)
        self.environment = {
            "PATH": str(self.bin),
            "PYTHONDONTWRITEBYTECODE": "1",
            "CURRENT_VERSION": "18",
            "GITHUB_REPOSITORY": "fixture/offline",
            "PROMOTION_FIXTURE": str(self.fixture),
        }
        self.outputs = {
            "current-version": "18",
            "mame-changed": "false",
            "hbmame-changed": "false",
        }

    def run_step(self, name: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["/bin/bash", "-e", "-c", step_run(self.workflow, name)],
            cwd=self.workspace,
            env=self.environment,
            capture_output=True,
            text=True,
            timeout=15,
            check=False,
        )

    def assert_step_succeeds(self, name: str) -> None:
        result = self.run_step(name)
        self.assertEqual(
            result.returncode, 0, f"{name}:\n{result.stdout}\n{result.stderr}"
        )

    def step_enabled(self, name: str) -> bool:
        # Only single output comparisons are supported, not arbitrary Actions.
        condition = re.findall(
            r"^        if: (.+)$", step_block(self.workflow, name), re.MULTILINE
        )
        self.assertEqual(len(condition), 1, name)
        match = re.fullmatch(
            r"needs\.inspect\.outputs\.([\w-]+) (==|!=) '([^']+)'", condition[0]
        )
        if match is None:
            self.fail(f"unsupported condition for {name}: {condition[0]}")
        key, operator, value = match.groups()
        equal = self.outputs[key] == value
        return equal if operator == "==" else not equal

    def download_artifact(self, step: str, artifacts: dict[str, Path]) -> None:
        block = step_block(self.workflow, step)
        self.assertRegex(block, r"uses: actions/download-artifact@")
        names = re.findall(r"^          name: (.+)$", block, re.MULTILINE)
        paths = re.findall(r"^          path: (.+)$", block, re.MULTILINE)
        self.assertEqual(len(names), 1, step)
        self.assertEqual(len(paths), 1, step)
        destination = self.workspace / paths[0]
        self.assertTrue(destination.resolve().is_relative_to(self.workspace.resolve()))
        destination.mkdir(parents=True, exist_ok=True)
        source = artifacts[names[0]]
        shutil.copyfile(source, destination / source.name)

    def assert_promotion(
        self, *, compact: bool, mame_changed: bool = False, hbmame_changed: bool = False
    ) -> None:
        create_prior_release(self.fixture, compact=compact)
        self.outputs.update(
            {
                "mame-changed": str(mame_changed).lower(),
                "hbmame-changed": str(hbmame_changed).lower(),
            }
        )
        rebuilt = self.fixture / "rebuilt"
        rebuilt.mkdir()
        new_mame = rebuilt / "mame.sqlite3"
        new_mame.write_bytes(b"new mame")
        listxml = rebuilt / "hbmame-listxml.xml"
        listxml.write_text(
            '<mame build="fixture"><machine name="new-hbmame">'
            "<description>New HBMAME</description><year>2026</year>"
            "<manufacturer>Fixture</manufacturer></machine></mame>",
            encoding="utf-8",
        )
        expected_hbmame = rebuilt / "expected-hbmame.sqlite3"
        databases.build_mame(listxml=listxml, out=expected_hbmame)
        artifacts = {
            "game-databases-mame": new_mame,
            "game-databases-hbmame-listxml": listxml,
        }
        self.assertTrue(self.step_enabled(RESTORE))
        self.assert_step_succeeds(RESTORE)
        for name, data in OLD_DATABASES.items():
            self.assertEqual(
                (self.workspace / "build/current" / name).read_bytes(), data
            )
            self.assertFalse((self.workspace / "build" / name).exists())
        for name in (
            "ArcadeDatabase.csv",
            "ArcadeDatabase-LICENSE.txt",
            databases.INDEX,
        ):
            self.assertEqual(
                (self.workspace / "build/current" / name).read_bytes(),
                (self.fixture / name).read_bytes(),
            )

        steps = (
            "Download rebuilt MAME database",
            "Reuse unchanged MAME database",
            "Download rebuilt HBMAME listxml",
            "Build HBMAME database",
            "Reuse unchanged HBMAME database",
        )
        # Follow workflow ordering and conditions; only artifact transport is fake.
        ordered = sorted(
            steps, key=lambda name: self.workflow.index(f"      - name: {name}\n")
        )
        self.assertLess(
            self.workflow.index(f"      - name: {RESTORE}\n"),
            self.workflow.index(f"      - name: {ordered[0]}\n"),
        )
        for step in ordered:
            if self.step_enabled(step):
                if "        uses:" in step_block(self.workflow, step):
                    self.download_artifact(step, artifacts)
                else:
                    self.assert_step_succeeds(step)
        expected = dict(OLD_DATABASES)
        if mame_changed:
            expected["mame.sqlite3"] = new_mame.read_bytes()
        if hbmame_changed:
            expected["hbmame.sqlite3"] = expected_hbmame.read_bytes()
        for name, data in expected.items():
            self.assertEqual((self.workspace / "build" / name).read_bytes(), data)
        calls = [
            json.loads(line)
            for line in (self.fixture / "gh-calls.jsonl").read_text().splitlines()
        ]
        self.assertEqual(
            [call[:2] for call in calls],
            [["release", "download"], ["run", "list"], ["run", "download"]],
        )

    def test_compact_release_can_be_reused_by_next_promotion(self) -> None:
        self.assert_promotion(compact=True)

    def test_legacy_release_can_be_reused_by_next_promotion(self) -> None:
        self.assert_promotion(compact=False)

    def test_rebuilt_mame_survives_hbmame_reuse(self) -> None:
        self.assert_promotion(compact=True, mame_changed=True)

    def test_rebuilt_hbmame_survives_mame_reuse(self) -> None:
        self.assert_promotion(compact=True, hbmame_changed=True)

    def test_both_rebuilt_databases_survive_restore(self) -> None:
        self.assert_promotion(compact=True, mame_changed=True, hbmame_changed=True)

    def assert_restore_fails(self, member: str) -> None:
        result = self.run_step(RESTORE)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("restore database sources from", result.stderr)
        self.assertIn(member, result.stderr)
        for name in OLD_DATABASES:
            self.assertFalse((self.workspace / "build/current" / name).exists())
            self.assertFalse((self.workspace / "build" / name).exists())

    def test_missing_compact_source_stops_restoration(self) -> None:
        create_prior_release(self.fixture, compact=True)
        source = self.fixture / "source/mister-magik-game-databases-source-v18.zip"
        source.rename(source.with_suffix(".unavailable"))
        self.assert_restore_fails("mame.sqlite3")

    def test_incomplete_compact_source_stops_restoration(self) -> None:
        create_prior_release(self.fixture, compact=True)
        source = self.fixture / "source/mister-magik-game-databases-source-v18.zip"
        with zipfile.ZipFile(source, "w") as archive:
            archive.writestr("mame.sqlite3", OLD_DATABASES["mame.sqlite3"])
        self.assert_restore_fails("hbmame.sqlite3")

    def test_corrupt_compact_source_stops_restoration(self) -> None:
        create_prior_release(self.fixture, compact=True)
        source = self.fixture / "source/mister-magik-game-databases-source-v18.zip"
        source.write_bytes(b"corrupt zip")
        self.assert_restore_fails(source.name)

    def test_source_download_is_scoped_to_assemble(self) -> None:
        # Structural guard only; successful recovery is covered by real execution.
        before_assemble = self.workflow.split("\n  assemble:\n", 1)[0]
        self.assertNotIn("gh run download", before_assemble)

    def test_reuse_commands_reject_stale_paths_for_both_databases(self) -> None:
        create_prior_release(self.fixture, compact=True)
        self.assert_step_succeeds(RESTORE)
        original = self.workflow
        for name, data in OLD_DATABASES.items():
            with self.subTest(database=name):
                old = f"cp build/current/{name} build/{name}"
                self.assertEqual(original.count(old), 1)
                self.workflow = original.replace(
                    old, f"cp build/current/source/{name} build/{name}"
                )
                result = self.run_step(
                    f"Reuse unchanged {name.removesuffix('.sqlite3').upper()} database"
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn(name, result.stderr)
                self.assertEqual(
                    (self.workspace / "build/current" / name).read_bytes(), data
                )
        self.workflow = original

    def test_step_extraction_rejects_missing_or_duplicate_steps(self) -> None:
        with self.assertRaisesRegex(ValueError, "expected one assemble step"):
            step_run(self.workflow, "Nonexistent step")
        marker = f"      - name: {RESTORE}\n"
        with self.assertRaisesRegex(ValueError, "expected one assemble step"):
            step_run(self.workflow.replace(marker, marker + marker), RESTORE)

    def test_offline_path_has_no_network_clients(self) -> None:
        for command in ("curl", "wget", "git", "ssh"):
            self.assertIsNone(shutil.which(command, path=self.environment["PATH"]))
        self.assertEqual(
            shutil.which("gh", path=self.environment["PATH"]), str(self.bin / "gh")
        )


if __name__ == "__main__":
    unittest.main()
