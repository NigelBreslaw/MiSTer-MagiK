#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "release/platform/main-component.py"
SPEC = importlib.util.spec_from_file_location("main_component", SCRIPT)
assert SPEC and SPEC.loader
component = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(component)


class MainComponentTests(unittest.TestCase):
    revision = "1" * 40

    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="main-component-consumer-")
        self.root = Path(self.temp.name)
        binary = self.root / "MiSTer_MagiK"
        binary.write_bytes(b"main-binary\n")
        payload = {
            "format": component.FORMAT,
            "component_id": component.component_id(self.revision),
            "repository": component.REPOSITORY,
            "branch": component.BRANCH,
            "source_revision": self.revision,
            "toolchain": component.TOOLCHAIN,
            "binary": {
                "path": "MiSTer_MagiK",
                "size": binary.stat().st_size,
                "sha256": component.digest(binary),
            },
        }
        receipt = self.root / "main-component-v0.1.json"
        receipt.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
        (self.root / "SHA256SUMS").write_text(
            f"{component.digest(binary)}  MiSTer_MagiK\n{component.digest(receipt)}  main-component-v0.1.json\n"
        )

    def tearDown(self):
        self.temp.cleanup()

    def test_shared_known_identity_vector(self):
        self.assertEqual(
            component.component_id(self.revision),
            "a9ead23864010064528bde4fa70a84567058ea6009026089af7f6783a7fad36d",
        )

    def test_verify_accepts_exact_artifact(self):
        self.assertEqual(
            component.verify(self.root, self.revision)["source_revision"], self.revision
        )

    def test_verify_rejects_wrong_revision_and_corrupt_binary(self):
        with self.assertRaisesRegex(
            component.MainComponentError, "source revision mismatch"
        ):
            component.verify(self.root, "2" * 40)
        (self.root / "MiSTer_MagiK").write_bytes(b"corrupt")
        with self.assertRaisesRegex(
            component.MainComponentError, "binary identity mismatch"
        ):
            component.verify(self.root)

    def test_verify_rejects_wrong_toolchain_and_malformed_receipt(self):
        receipt = self.root / "main-component-v0.1.json"
        payload = json.loads(receipt.read_text())
        payload["toolchain"] = "other"
        receipt.write_text(json.dumps(payload))
        with self.assertRaisesRegex(
            component.MainComponentError, "unsupported toolchain"
        ):
            component.verify(self.root)
        receipt.write_text("[]")
        with self.assertRaisesRegex(component.MainComponentError, "must be an object"):
            component.verify(self.root)


if __name__ == "__main__":
    unittest.main()
