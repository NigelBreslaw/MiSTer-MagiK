from __future__ import annotations

import unittest

from scripts.magik_ci.bundle import bundle_id, update_plan
from scripts.magik_ci.manifest import candidate_id, serialize


class MagikCiTests(unittest.TestCase):
    def test_bundle_identity_is_deterministic(self) -> None:
        values = ("a" * 64, "b" * 64, "c" * 64)
        self.assertEqual(bundle_id(*values), bundle_id(*values))

    def test_platform_update_plan_starts_at_one(self) -> None:
        plan = update_plan(None, 0, "a" * 64, "b" * 64, "c" * 64)
        self.assertEqual(plan["next_version"], 1)
        self.assertTrue(plan["update_needed"])

    def test_manifest_candidate_is_ordered(self) -> None:
        values = {
            field: "x"
            for field in __import__(
                "scripts.magik_ci.manifest", fromlist=["FIELDS"]
            ).FIELDS
        }
        values["qualification_candidate_id"] = candidate_id(values)
        self.assertEqual(len(serialize(values).splitlines()), 25)


if __name__ == "__main__":
    unittest.main()
