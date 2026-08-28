from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

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

    def test_bundle_round_trip(self) -> None:
        from scripts.magik_ci.bundle import create, verify

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("main", "fpga", "scanout"):
                (root / name).mkdir()
                (root / name / "payload").write_bytes(name.encode())
            (root / "fpga" / "patched").mkdir()
            (
                root / "fpga" / "patched" / "menu-magik-vblank-latch.metadata.txt"
            ).write_text("platform_contract_sha256=" + "1" * 64 + "\n")
            archive = create(
                main=root / "main",
                fpga=root / "fpga",
                scanout=root / "scanout",
                main_id="a" * 64,
                fpga_id="b" * 64,
                kernel_id="c" * 64,
                main_run_id="1",
                fpga_run_id="2",
                kernel_run_id="3",
                main_head_sha="d" * 40,
                fpga_head_sha="e" * 40,
                kernel_head_sha="f" * 40,
                main_source="main",
                fpga_source="fpga",
                kernel_source="kernel",
                release_version=1,
                output=root / "out",
            )
            self.assertEqual(verify(archive)["release_version"], 1)


if __name__ == "__main__":
    unittest.main()
