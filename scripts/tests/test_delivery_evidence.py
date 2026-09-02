# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import tempfile
import unittest
from pathlib import Path

from scripts.magik_ci import delivery_tests as delivery
from scripts.magik_ci import distribution as dist


class DeliveryEvidenceTests(unittest.TestCase):
    def test_evidence_must_match_exact_candidate_and_test_suite(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            value = {
                "format": "mister-magik-delivery-evidence-v1",
                "candidate_id": "a" * 64,
                "downloader_revision": delivery.DOWNLOADER_REVISION,
                "installer": "shipped-arm-verify-platform",
                "cases": list(delivery.CASES),
                "validation": "passed",
            }
            (root / dist.EVIDENCE).write_bytes(dist.canonical_json(value))
            delivery.require_evidence(root, {"candidate_id": "a" * 64})
            with self.assertRaisesRegex(ValueError, "exact candidate"):
                delivery.require_evidence(root, {"candidate_id": "b" * 64})
            value["cases"] = ["fresh"]
            (root / dist.EVIDENCE).write_bytes(dist.canonical_json(value))
            with self.assertRaisesRegex(ValueError, "exact candidate"):
                delivery.require_evidence(root, {"candidate_id": "a" * 64})

    def test_smoke_refuses_a_native_or_stub_manager(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manager = root / dist.PUBLIC["manager"].removeprefix("/media/fat/")
            manager.parent.mkdir(parents=True)
            manager.write_bytes(b"#!/bin/sh\necho verified platform\n")
            with self.assertRaisesRegex(ValueError, "ARM ELF"):
                delivery.smoke(root)


if __name__ == "__main__":
    unittest.main()
