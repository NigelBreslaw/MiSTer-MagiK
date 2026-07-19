#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

from __future__ import annotations
import importlib.util, json, os, shutil, subprocess, tempfile, unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts/release/platform/recover-platform-component.sh"
SPEC = importlib.util.spec_from_file_location("bundle_fixture", ROOT / "scripts/tests/test-platform-bundle.py")
assert SPEC and SPEC.loader
fixtures = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(fixtures)


class DurableRecoveryTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temp = tempfile.TemporaryDirectory(prefix="mister-magik-recovery-")
        self.root = Path(self.temp.name)
        self.releases = self.root / "releases"
        self.releases.mkdir()
        self.fixture = fixtures.PlatformBundleTests(methodName="test_round_trip")
        self.fixture.setUp()
        archive = self.fixture.create()
        good = self.releases / "platform-v0.2"
        good.mkdir()
        shutil.copy2(archive, good / archive.name)
        shutil.copy2(self.fixture.root / "output" / fixtures.bundle.MANIFEST_NAME, good / fixtures.bundle.MANIFEST_NAME)
        binary = self.root / "bin"
        binary.mkdir()
        gh = binary / "gh"
        gh.write_text("""#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" = api ]]; then cat "$MOCK_RELEASES_JSON"; exit 0; fi
if [[ "$1 $2" = "release download" ]]; then
  tag="$3"; shift 3; destination=""; pattern=""
  while [[ $# -gt 0 ]]; do
    case "$1" in --repo) shift 2;; --dir) destination="$2"; shift 2;; --pattern) pattern="$2"; shift 2;; *) exit 2;; esac
  done
  mkdir -p "$destination"; found=0
  for source in "$MOCK_RELEASE_ROOT/$tag"/$pattern; do
    [[ -f "$source" ]] || continue
    cp "$source" "$destination/"; found=1
  done
  [[ "$found" = 1 ]]; exit
fi
exit 2
""")
        gh.chmod(0o755)
        self.path = f"{binary}:{os.environ['PATH']}"

    def tearDown(self) -> None:
        self.fixture.tearDown()
        self.temp.cleanup()

    @staticmethod
    def release(tag: str, draft: bool = False) -> dict[str, object]:
        return {"tag_name": tag, "published_at": "2026-07-19T00:00:00Z", "draft": draft}

    def recover(self, component: str, identity: str, releases: list[dict[str, object]]) -> subprocess.CompletedProcess[str]:
        listing = self.root / "releases.json"
        listing.write_text(json.dumps(releases))
        env = dict(os.environ, PATH=self.path, GITHUB_REPOSITORY="NigelBreslaw/MiSTer-MagiK",
                   MOCK_RELEASES_JSON=str(listing), MOCK_RELEASE_ROOT=str(self.releases))
        return subprocess.run(
            [str(SCRIPT), "--component", component, "--component-id", identity,
             "--output", str(self.root / f"output-{component}"),
             "--github-output", str(self.root / f"github-output-{component}")],
            cwd=ROOT, env=env, text=True, capture_output=True, check=True,
        )

    def test_corrupt_newest_falls_back_to_older_verified_release(self) -> None:
        corrupt = self.releases / "platform-v0.3"
        corrupt.mkdir()
        shutil.copy2(self.fixture.root / "output" / fixtures.bundle.MANIFEST_NAME, corrupt / fixtures.bundle.MANIFEST_NAME)
        (corrupt / "mister-magik-platform-v0.3.zip").write_bytes(b"not a zip")
        result = self.recover("main", self.fixture.main_id, [self.release("platform-v0.2"), self.release("platform-v0.3")])
        self.assertIn("from platform-v0.2", result.stdout)
        self.assertTrue((self.root / "output-main/MiSTer_MagiK").is_file())
        self.assertIn("run_id=100", (self.root / "github-output-main").read_text())

    def test_v1_recovers_fpga_but_not_main(self) -> None:
        legacy = self.fixture.legacy_archive()
        release = self.releases / "platform-v0.1"
        release.mkdir()
        shutil.copy2(legacy, release / "mister-magik-platform-v0.1.zip")
        shutil.copy2(self.fixture.root / fixtures.bundle.MANIFEST_V1, release / fixtures.bundle.MANIFEST_V1)
        selected = [self.release("platform-v0.1")]
        self.assertIn("from platform-v0.1", self.recover("fpga", self.fixture.fpga_id, selected).stdout)
        self.assertIn("No published platform contains main", self.recover("main", self.fixture.main_id, selected).stdout)
        self.assertIn("hit=false", (self.root / "github-output-main").read_text())

    def test_draft_and_mismatch_produce_durable_miss(self) -> None:
        result = self.recover("kernel", "d" * 64, [self.release("platform-v0.2"), self.release("platform-v0.9", True)])
        self.assertIn("No published platform contains kernel", result.stdout)
        self.assertIn("hit=false", (self.root / "github-output-kernel").read_text())
        self.assertFalse((self.root / "output-kernel").exists())


if __name__ == "__main__":
    unittest.main()
