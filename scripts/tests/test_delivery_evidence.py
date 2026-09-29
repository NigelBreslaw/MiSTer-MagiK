# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import os
import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

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

    def test_downloaded_manager_is_prepared_by_launcher_before_direct_verification(
        self,
    ):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            manager = root / dist.PUBLIC["manager"].removeprefix("/media/fat/")
            manager.parent.mkdir(parents=True)
            header = bytearray(20)
            header[:6] = b"\x7fELF\x01\x01"
            header[18:20] = b"\x28\x00"
            manager.write_bytes(header)
            manager.chmod(0o644)
            metadata = root / dist.PUBLIC["scanout_metadata"].removeprefix(
                "/media/fat/"
            )
            metadata.write_text("vermagic=6.18.38-MiSTer SMP\n")
            launcher = root / dist.LAUNCHER
            launcher.parent.mkdir(parents=True)
            launcher.write_text(
                '#!/bin/sh\nchmod +x "$MISTER_MAGIK_FAT/mister-magik/mister-magik-manager"\n'
                'echo "start cancelled" >&2\nexit 1\n'
            )
            run = subprocess.run

            def execute(command, **kwargs):
                if command[0] == "/bin/sh":
                    return run(command, **kwargs)
                self.assertTrue(os.access(manager, os.X_OK))
                self.assertEqual(command, [str(manager), "verify-platform", "public"])
                return subprocess.CompletedProcess(command, 0, "verified platform", "")

            with patch.object(delivery.subprocess, "run", side_effect=execute):
                delivery.smoke(root)

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
