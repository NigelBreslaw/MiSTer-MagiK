# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.magik_ci import delivery_tests as delivery
from scripts.magik_ci import distribution as dist


class DeliveryEvidenceTests(unittest.TestCase):
    @staticmethod
    def _executed(value):
        value["execution"] = {
            "status": "passed",
            "candidate_id": value["candidate_id"],
            "result_digest": delivery._results_digest(value["results"]),
        }
        return value

    def test_evidence_must_match_exact_candidate_and_test_suite(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = self._executed(
                delivery.evidence_for_candidate({"candidate_id": "a" * 64})
            )
            (root / dist.EVIDENCE).write_bytes(dist.canonical_json(value))
            delivery.require_evidence(root, {"candidate_id": "a" * 64})
            with self.assertRaisesRegex(ValueError, "exact candidate"):
                delivery.require_evidence(root, {"candidate_id": "b" * 64})
            value["cases"] = ["fresh"]
            (root / dist.EVIDENCE).write_bytes(dist.canonical_json(value))
            with self.assertRaisesRegex(ValueError, "complete passing"):
                delivery.require_evidence(root, {"candidate_id": "a" * 64})

            value = self._executed(
                delivery.evidence_for_candidate({"candidate_id": "a" * 64})
            )
            value["format"] = "mister-magik-delivery-evidence-v1"
            (root / dist.EVIDENCE).write_bytes(dist.canonical_json(value))
            with self.assertRaisesRegex(ValueError, "v1"):
                delivery.require_evidence(root, {"candidate_id": "a" * 64})

            value = self._executed(
                delivery.evidence_for_candidate({"candidate_id": "a" * 64})
            )
            value["results"]["update_all"] = value["results"]["update_all"][:-1]
            (root / dist.EVIDENCE).write_bytes(dist.canonical_json(value))
            with self.assertRaisesRegex(ValueError, "complete passing"):
                delivery.require_evidence(root, {"candidate_id": "a" * 64})

            value = self._executed(
                delivery.evidence_for_candidate({"candidate_id": "a" * 64})
            )
            value["dependency_pins"]["update_all"] = "stale"
            (root / dist.EVIDENCE).write_bytes(dist.canonical_json(value))
            with self.assertRaisesRegex(ValueError, "complete passing"):
                delivery.require_evidence(root, {"candidate_id": "a" * 64})

            value = self._executed(
                delivery.evidence_for_candidate({"candidate_id": "a" * 64})
            )
            (root / dist.EVIDENCE).write_bytes(dist.canonical_json(value))
            with (
                patch.object(
                    delivery, "SUITE_SOURCE_FILES", ("scripts/magik_ci/cli.py",)
                ),
                self.assertRaisesRegex(ValueError, "complete passing"),
            ):
                delivery.require_evidence(root, {"candidate_id": "a" * 64})

    def test_smoke_refuses_a_native_or_stub_manager(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manager = root / dist.PUBLIC["manager"].removeprefix("/media/fat/")
            manager.parent.mkdir(parents=True)
            manager.write_bytes(b"#!/bin/sh\necho verified platform\n")
            with self.assertRaisesRegex(ValueError, "ARM ELF"):
                delivery.smoke(root)

    def test_downloader_archive_is_directly_executable(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            source = root / "source"
            (source / "src").mkdir(parents=True)
            (source / "src/__main__.py").write_text("print('2.4.0 test fixture')\n")
            archive = root / "downloader_latest.zip"
            delivery._make_downloader_archive(source, archive)
            self.assertEqual(archive.stat().st_mode & 0o777, 0o755)
            delivery._assert_direct_pyz(archive, "fixture")


if __name__ == "__main__":
    unittest.main()
