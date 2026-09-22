# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

import importlib.util
import tempfile
import tomllib
import unittest
from pathlib import Path

SPEC = importlib.util.spec_from_file_location(
    "runtime_environment_reference",
    Path(__file__).resolve().parents[1]
    / "checks/generate-runtime-environment-reference.py",
)
assert SPEC and SPEC.loader
REFERENCE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(REFERENCE)


class RuntimeEnvironmentReferenceTests(unittest.TestCase):
    def test_checked_in_registry_and_reference_are_current(self):
        registry = tomllib.loads(REFERENCE.DEFAULT_REGISTRY.read_text())
        self.assertEqual(REFERENCE.validate(registry, REFERENCE.ROOT), [])
        self.assertEqual(
            REFERENCE.render(registry), REFERENCE.DEFAULT_OUTPUT.read_text()
        )

    def test_owner_and_exact_source_name_are_required(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "src").mkdir()
            (root / "src/config.rs").write_text('"MISTER_LIVE_LONG"')
            registry = {
                "source_roots": ["src"],
                "control": [
                    {"name": "MISTER_LIVE_LONG", "owner": "src/config.rs"},
                    {"name": "MISTER_LIVE", "owner": "src/config.rs"},
                    {"name": "MISTER_LIVE_LONG", "owner": "src/removed.rs"},
                ],
            }
            self.assertEqual(
                REFERENCE.validate(registry, root),
                [
                    "MISTER_LIVE: no source reference",
                    "MISTER_LIVE_LONG: missing owner src/removed.rs",
                ],
            )


if __name__ == "__main__":
    unittest.main()
