#!/usr/bin/env python3
"""Offline safety tests for local synthesis orchestration; no containers run."""

import json
import sys
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from scripts.magik_ci import fpga_local


class LocalSignoff(unittest.TestCase):
    def test_ref_mismatch_fails_before_any_container_call(self):
        with patch.object(
            fpga_local, "run", side_effect=["candidate", "other-main"]
        ) as run:
            with self.assertRaisesRegex(ValueError, "local main"):
                fpga_local.identity(Path("/fixture"))
            self.assertEqual(run.call_count, 2)

    def test_dirty_candidate_fails_before_any_container_call(self):
        with patch.object(
            fpga_local, "run", side_effect=["candidate", "candidate", " M source.sv"]
        ):
            with self.assertRaisesRegex(ValueError, "clean committed"):
                fpga_local.identity(Path("/fixture"))

    def test_missing_runtime_preserves_completed_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            old = root / "runs" / "old"
            old.mkdir(parents=True)
            report = old / "completed.json"
            report.write_text('{"saved":true}')
            with (
                patch.object(fpga_local, "identity", return_value={}),
                patch.object(fpga_local, "run") as run,
            ):
                with self.assertRaisesRegex(ValueError, "runtime missing"):
                    fpga_local.signoff(root, root)
                run.assert_not_called()
            self.assertEqual(json.loads(report.read_text()), {"saved": True})

    def comparison_fixture(self, root):
        completed = root / "baseline"
        completed.mkdir()
        driver = root / "driver"
        driver.write_text("driver")
        files = {}
        for name in (
            "menu-magik-vblank-latch.rbf",
            "menu-magik-vblank-latch.metadata.txt",
        ):
            (completed / name).write_text(name)
            files[name] = fpga_local.sha(completed / name)
        frozen = {
            key: "fixed"
            for key in (
                "menu",
                "baseline",
                "seed",
                "date",
                "prepare_sha256",
                "quartus_version",
                "container_image",
            )
        }
        manifest = {
            "variant": "baseline",
            "inputs": frozen,
            "files": files,
            "driver_sha256": fpga_local.sha(driver),
        }
        (completed / "completed.json").write_text(json.dumps(manifest))
        return frozen, driver, completed

    def test_verified_reuse_preserves_original_evidence(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            frozen, driver, completed = self.comparison_fixture(root)
            before = (completed / "completed.json").read_bytes()
            self.assertEqual(
                fpga_local.verify_comparison(root, "baseline", frozen, root, driver),
                completed.resolve(),
            )
            self.assertEqual((completed / "completed.json").read_bytes(), before)

    def test_reuse_rejects_changed_seed_driver_and_artifact(self):
        for change in ("seed", "driver", "artifact"):
            with (
                self.subTest(change=change),
                tempfile.TemporaryDirectory() as directory,
            ):
                root = Path(directory)
                frozen, driver, completed = self.comparison_fixture(root)
                if change == "seed":
                    frozen = dict(frozen, seed="other")
                elif change == "driver":
                    driver.write_text("changed")
                else:
                    (completed / "menu-magik-vblank-latch.rbf").write_text("changed")
                with self.assertRaisesRegex(ValueError, "mismatch"):
                    fpga_local.verify_comparison(root, "baseline", frozen, root, driver)

    def test_stock_reuse_rejects_changed_source(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            frozen, driver, completed = self.comparison_fixture(root)
            manifest = json.loads((completed / "completed.json").read_text())
            manifest["variant"] = "stock"
            manifest["inputs"]["commit"] = "old"
            (completed / "completed.json").write_text(json.dumps(manifest))
            completed.rename(root / "stock")
            frozen = dict(frozen, commit="new")
            with patch.object(fpga_local, "run", side_effect=["old-blob", "new-blob"]):
                with self.assertRaisesRegex(
                    ValueError, "stock synthesis input changed"
                ):
                    fpga_local.verify_comparison(root, "stock", frozen, root, driver)

    def test_candidate_cannot_be_reused(self):
        with self.assertRaisesRegex(ValueError, "only comparison"):
            fpga_local.verify_comparison(
                Path("/fixture"),
                "patched",
                {},
                Path("/fixture"),
                Path("/fixture/driver"),
            )

    def test_relative_cache_override_is_rejected(self):
        with self.assertRaisesRegex(ValueError, "absolute"):
            fpga_local.local_root(Path("/fixture"), Path("relative"))

    def test_shared_default_cache_uses_primary_checkout(self):
        with (
            patch.object(fpga_local, "run", return_value="/primary/.git"),
            patch.dict("os.environ", {}, clear=True),
        ):
            self.assertEqual(
                fpga_local.local_root(Path("/other-worktree")),
                Path("/primary/build/fpga-local-apple"),
            )


if __name__ == "__main__":
    unittest.main()
