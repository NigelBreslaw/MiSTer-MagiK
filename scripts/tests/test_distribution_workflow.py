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
        self.assertIn("docker buildx build --platform linux/arm/v7", action)
        self.assertIn("unshare --mount --net --pid --fork --mount-proc", action)
        self.assertIn("MISTER_REQUIRE_MEDIA_FAT_MOUNT=1", action)
        self.assertIn("dependency_pins.json", action)
        self.assertLess(
            action.index("scripts/cargo build"),
            action.index("unshare --mount"),
        )
        self.assertNotIn("cat release_patch", action)
        self.assertIn('docker cp "$runtime_container:/usr/local/lib"', action)
        self.assertIn('export QEMU_LD_PREFIX="$runtime_root"', action)

    def test_branch_qualification_builds_but_never_publishes(self):
        workflow = (ROOT / ".github/workflows/distribution.yml").read_text()
        self.assertIn("qualification_only", workflow)
        self.assertIn("inputs.qualification_only != true", workflow)
        publish = workflow.split("\n  publish:\n", 1)[1]
        self.assertIn("inputs.qualification_only != true", publish)
        self.assertIn('test "$GITHUB_REF" = refs/heads/main', publish)
        self.assertIn(
            "if: inputs.release_channel == 'alpha' || inputs.qualification_only == true",
            workflow,
        )

    def test_downloader_runtime_is_cached_independently_of_candidate_tests(self):
        action = (ROOT / ".github/actions/verify-distribution/action.yml").read_text()
        restore = action.index("uses: actions/cache/restore@v6")
        build = action.index("- name: Build pinned Downloader runtime")
        save = action.index("uses: actions/cache/save@v6")
        unpack = action.index("- name: Check and unpack pinned Downloader runtime")
        verify = action.index("- name: Verify actual delivered bytes")
        self.assertLess(restore, build)
        self.assertLess(build, save)
        self.assertLess(save, unpack)
        self.assertLess(unpack, verify)
        self.assertIn(
            "hashFiles('scripts/magik_ci/dependency_pins.json', "
            "'.github/actions/verify-distribution/action.yml')",
            action[restore:build],
        )
        self.assertNotIn("restore-keys:", action)
        self.assertEqual(
            action.count("if: steps.downloader-runtime.outputs.cache-hit != 'true'"),
            2,
        )
        self.assertIn("steps.downloader-runtime.outputs.cache-primary-key", action)
        self.assertIn(
            "docker create --platform linux/arm/v7 downloader_bin_builder",
            action[build:save],
        )
        self.assertIn("sha256sum --check SHA256SUMS", action[unpack:verify])
        self.assertLess(
            action.index("sha256sum --check SHA256SUMS"),
            action.index('tar -C "$RUNNER_TEMP" -xzf'),
        )
        self.assertIn("ci distribution test-delivery", action[verify:])


if __name__ == "__main__":
    unittest.main()
