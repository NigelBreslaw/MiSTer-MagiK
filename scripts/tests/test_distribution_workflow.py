# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import unittest

from scripts.magik_ci.distribution import ROOT


class DistributionWorkflowTests(unittest.TestCase):
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
        self.assertIn("scripts/tests/test-mister-magik-installer.sh", action)
        self.assertIn("update-binfmts --enable qemu-arm", action)


if __name__ == "__main__":
    unittest.main()
