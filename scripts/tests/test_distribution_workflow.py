# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import os
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts.magik_ci.distribution import ROOT


class DistributionWorkflowTests(unittest.TestCase):
    def test_packaging_delegates_artwork_staging_to_cli(self):
        packaging = (ROOT / "scripts/package-distribution.sh").read_text()
        self.assertIn(
            '"$ROOT/scripts/magik-ci" ci distribution stage-artwork '
            '"$ROOT/apps/mister/assets/ui/launcher-cards" '
            '"$STAGE/$PUBLIC_ROOT_RELATIVE/assets/ui/launcher-cards"',
            " ".join(packaging.replace("\\\n", " ").split()),
        )

    def test_only_alpha_builds_and_promotion_reuses_the_versioned_artifact(self):
        workflow = (ROOT / ".github/workflows/distribution.yml").read_text()
        build = workflow.split("\n  distribution:\n", 1)[1].split(
            "\n  promotion:\n", 1
        )[0]
        promotion = workflow.split("\n  promotion:\n", 1)[1].split("\n  publish:\n", 1)[
            0
        ]
        publish = workflow.split("\n  publish:\n", 1)[1]
        self.assertIn("if: inputs.release_channel == 'alpha'", build)
        self.assertIn('gh release download "v$VERSION"', promotion)
        self.assertIn("distribution prepare-promotion", promotion)
        for block in (promotion, publish):
            self.assertNotIn("runtime-device", block)
            self.assertNotIn("package-distribution.sh", block)
            self.assertNotIn("select-published-release.py", block)
        self.assertIn("needs.distribution.result == 'success'", publish)
        self.assertIn("needs.promotion.result == 'success'", publish)
        self.assertIn("needs: [release-metadata, distribution, promotion]", publish)
        for forbidden in ("--clobber", "release delete", "require-alpha-promotion"):
            self.assertNotIn(forbidden, workflow)

    def test_scanout_source_offer_accepts_current_and_legacy_provenance(self):
        packaging = (ROOT / "scripts/package-distribution.sh").read_text()
        block = (
            "SCANOUT_SOURCE_REVISION="
            + packaging.split("\nSCANOUT_SOURCE_REVISION=", 1)[1].split(
                "\nLATCH_SOURCE_REVISION=", 1
            )[0]
        )
        with tempfile.TemporaryDirectory() as directory:
            metadata = Path(directory) / "provenance.txt"
            current, legacy = "a" * 40, "b" * 40
            for text, expected in (
                (f"component_revision={current}\n", current),
                (f"source_revision={legacy}\n", legacy),
                (f"component_revision={current}\nsource_revision={legacy}\n", current),
                ("component_revision=invalid\n", None),
                ("builder_revision=" + current + "\n", None),
            ):
                with self.subTest(metadata=text):
                    metadata.write_text(text)
                    result = subprocess.run(
                        [
                            "bash",
                            "-ec",
                            block + '\nprintf "%s" "$SCANOUT_SOURCE_REVISION"',
                        ],
                        env={**os.environ, "SCANOUT_METADATA": str(metadata)},
                        capture_output=True,
                        text=True,
                        check=False,
                    )
                    if expected is None:
                        self.assertNotEqual(result.returncode, 0)
                    else:
                        self.assertEqual(result.returncode, 0, result.stderr)
                        self.assertEqual(result.stdout, expected)

    def test_shipped_installer_gate_runs_before_candidate_upload(self):
        workflow = (ROOT / ".github/workflows/distribution.yml").read_text()
        for job in ("distribution", "promotion"):
            block = workflow.split(f"\n  {job}:\n", 1)[1].split("\n  publish:\n", 1)[0]
            self.assertLess(
                block.index("uses: ./.github/actions/verify-distribution"),
                block.index("uses: actions/upload-artifact"),
            )
        action = (ROOT / ".github/actions/verify-distribution/action.yml").read_text()
        self.assertIn("ci distribution test-delivery", action)
        self.assertIn("scripts/tests/test-start-magik.sh", action)
        self.assertIn("update-binfmts --enable qemu-arm", action)


if __name__ == "__main__":
    unittest.main()
