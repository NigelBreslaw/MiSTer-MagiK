# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import subprocess
import tempfile
import unittest
import zipfile
from contextlib import ExitStack
from pathlib import Path
from unittest.mock import patch

from scripts.magik_ci import distribution as dist
from scripts.magik_ci import manifest
from scripts.tests.distribution_fixture import CandidateFixture


class DistributionTests(unittest.TestCase):
    def setUp(self):
        self.stack = ExitStack()
        self.addCleanup(self.stack.close)
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        self.fixture = CandidateFixture(self.root)
        self.stack.enter_context(patch.dict(dist.ARTWORK, self.fixture.artwork()))
        self.contract = self.stack.enter_context(
            patch.object(
                manifest,
                "verify",
                side_effect=lambda path, *_args, **_kw: manifest.parse_fields(
                    path.read_text()
                ),
            )
        )
        # Host execution is a separate, mandatory CI integration test.
        self.manager = self.stack.enter_context(
            patch.object(
                dist.subprocess,
                "run",
                return_value=subprocess.CompletedProcess([], 0, "verified", ""),
            )
        )
        self.stack.enter_context(
            patch.dict("os.environ", {"MISTER_MAGIK_HOST_MANAGER": __file__})
        )

    def test_real_packaging_and_downloader_formats_agree_and_receipt_is_stable(self):
        candidate = self.fixture.package()
        result = dist.verify(candidate, channel="beta", write_receipt=True)
        self.assertEqual(result, dist.verify(candidate, channel="beta"))
        self.assertEqual(self.contract.call_count, 4)
        self.assertEqual(self.manager.call_count, 4)
        self.assertEqual(
            result["source_revision"], self.fixture.fields["magik_revision"]
        )

    def test_broken_beta_layout_is_passed_unmodified_to_strict_contract(self):
        for name, value in manifest.LAYOUTS["dev"].items():
            if name != "root":
                self.fixture.fields[name + "_path"] = value
        self.fixture.refresh()
        candidate = self.fixture.package()

        def reject(path, *_args, **kwargs):
            self.assertEqual(kwargs["layout"], "public")
            self.assertIn("manager_path=/media/fat/mister-magik-dev/", path.read_text())
            raise ValueError("platform_path_mismatch: manager")

        self.contract.side_effect = reject
        with self.assertRaisesRegex(ValueError, "platform_path_mismatch"):
            dist.verify(candidate, channel="beta", write_receipt=True)
        self.assertFalse((candidate / dist.RECEIPT).exists())

    def test_missing_required_file(self):
        (self.fixture.stage / dist.LAUNCHER).unlink()
        with self.assertRaisesRegex(ValueError, "nonexecutable|missing"):
            dist.verify(self.fixture.package(), channel="beta", write_receipt=True)

    def test_extra_helper_is_rejected(self):
        (self.fixture.stage / dist.LEGACY_HELPER).write_text("exit 99")
        with self.assertRaisesRegex(ValueError, "exactly one"):
            dist.verify(self.fixture.package(), channel="beta", write_receipt=True)

    def test_component_metadata_rejected_by_real_manager_boundary(self):
        candidate = self.fixture.package()
        self.manager.return_value = subprocess.CompletedProcess(
            [], 1, "metadata contract mismatch", ""
        )
        with self.assertRaisesRegex(ValueError, "metadata contract mismatch"):
            dist.verify(candidate, channel="beta", write_receipt=True)

    def test_corrupt_asset_even_with_updated_transport_hash(self):
        candidate = self.fixture.package()
        (candidate / dist.asset_name(dist.LAUNCHER)).write_bytes(b"corrupt")
        dist.write_checksums(candidate)
        with self.assertRaisesRegex(ValueError, "receipt mismatch"):
            dist.verify(candidate, channel="beta", write_receipt=True)

    def test_zip_payload_difference(self):
        candidate = self.fixture.package()
        receipt = dist.read_json(candidate / "release-assets.json")
        archive = candidate / receipt["archive"]
        with zipfile.ZipFile(archive, "a") as output:
            output.writestr("mister-magik/extra", "wrong")
        receipt["archive_sha256"] = dist.sha256_file(archive)
        (candidate / "release-assets.json").write_bytes(dist.canonical_json(receipt))
        dist.write_checksums(candidate)
        with self.assertRaisesRegex(ValueError, "ZIP/receipt"):
            dist.verify(candidate, channel="beta", write_receipt=True)

    def test_changed_candidate_receipt_is_rejected(self):
        candidate = self.fixture.package()
        dist.verify(candidate, channel="beta", write_receipt=True)
        result = dist.read_json(candidate / dist.RECEIPT)
        result["source_revision"] = "0" * 40
        (candidate / dist.RECEIPT).write_bytes(dist.canonical_json(result))
        dist.write_checksums(candidate)
        with self.assertRaisesRegex(ValueError, "validated candidate changed"):
            dist.verify(candidate, channel="beta")

    def test_unsafe_archive_entries(self):
        for name in ["../escape", "/absolute", "a/./b", "a//b", "a\\b"]:
            with (
                self.subTest(name=name),
                zipfile.ZipFile(self.root / "unsafe.zip", "w") as output,
            ):
                output.writestr(name, b"no")
            with self.assertRaisesRegex(ValueError, "unsafe"):
                dist.extract_package(self.root / "unsafe.zip", self.root / "out")


if __name__ == "__main__":
    unittest.main()
