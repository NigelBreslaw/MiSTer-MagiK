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

    def test_compact_release_can_be_reused_by_next_promotion(self) -> None:
        create_prior_release(self.fixture, compact=True)
        self.assert_step_succeeds(RESTORE)
        for database in ("MAME", "HBMAME"):
            self.assert_step_succeeds(f"Reuse unchanged {database} database")
        for name, data in OLD_DATABASES.items():
            self.assertEqual((self.workspace / "build" / name).read_bytes(), data)


if __name__ == "__main__":
    unittest.main()
