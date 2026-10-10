# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Exercise CI selection against real Git history, including deleted inputs."""

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from scripts.magik_ci.changes import PATHS, execute, select


class ChangesTests(unittest.TestCase):
    def setUp(self) -> None:
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.name", "Fixture")
        self.git("config", "user.email", "fixture@example.invalid")
        self.write("README.md")
        self.base = self.commit()

    def git(self, *arguments: str) -> str:
        return subprocess.check_output(
            ["git", *arguments], cwd=self.root, text=True
        ).strip()

    def write(self, path: str) -> None:
        destination = self.root / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text("fixture\n")

    def commit(self) -> str:
        self.git("add", "--", ".")
        self.git("commit", "-qm", "fixture")
        return self.git("rev-parse", "HEAD")

    def push(self, base: str | None = None) -> dict[str, bool]:
        return select(
            self.root,
            "push",
            {"before": base or self.base, "after": self.git("rev-parse", "HEAD")},
        )

    def test_unrelated_changes_skip_optional_checks(self) -> None:
        self.write("docs/notes.md")
        self.commit()
        self.assertEqual(self.push(), dict.fromkeys(PATHS, False))

    def test_each_owner_selects_its_checks(self) -> None:
        for path, expected in (
            ("magik/agent/src/device.rs", {"tooling"}),
            ("magik/scenarios/journey.py", {"tooling"}),
            ("magik/agent/src/publication.rs", {"tooling", "scanout"}),
            ("mister/platform/contracts/latch/src/lib.rs", {"tooling", "fpga"}),
            ("mister/platform/kernel/scanout-slots/policy.c", {"kernel"}),
            ("scripts/build-new-fpga-latch.sh", {"fpga"}),
            ("scripts/nested/build-fpga-latch.sh", set()),
            ("scripts/checks/check-scanout-slots-contract.sh", {"scanout", "kernel"}),
            (".github/workflows/platform-bundle.yml", {"fpga", "kernel"}),
            (".github/workflows/ci.yml", set(PATHS)),
            ("scripts/magik_ci/changes.py", set(PATHS)),
        ):
            with self.subTest(path=path):
                before = self.git("rev-parse", "HEAD")
                self.write(path)
                self.commit()
                self.assertEqual(
                    {key for key, selected in self.push(before).items() if selected},
                    expected,
                )

    def test_pr_uses_merge_base_and_excludes_new_base_changes(self) -> None:
        self.git("switch", "-qc", "feature")
        self.write("apps/mister/src/lib.rs")
        head = self.commit()
        self.git("switch", "-q", "main")
        self.write("mister/platform/fpga/menu-vblank-latch/core.sv")
        base = self.commit()
        values = select(
            self.root,
            "pull_request",
            {"pull_request": {"base": {"sha": base}, "head": {"sha": head}}},
        )
        self.assertEqual(values, {**dict.fromkeys(PATHS, False), "tooling": True})

    def test_renaming_an_input_out_of_its_owner_still_selects_checks(self) -> None:
        self.write("mister/platform/kernel/scanout-slots/policy.c")
        before = self.commit()
        self.git("mv", "mister/platform/kernel/scanout-slots/policy.c", "policy.c")
        self.commit()
        self.assertTrue(self.push(before)["kernel"])
        before = self.git("rev-parse", "HEAD")
        self.git("rm", "--", "policy.c")
        self.commit()
        self.assertFalse(self.push(before)["kernel"])

    def test_deleted_contract_input_selects_both_owners(self) -> None:
        path = "scripts/checks/check-scanout-slots-contract.sh"
        self.write(path)
        before = self.commit()
        self.git("rm", "--", path)
        self.commit()
        values = self.push(before)
        self.assertTrue(values["kernel"])
        self.assertTrue(values["scanout"])

    def test_large_diffs_do_not_truncate_changed_inputs(self) -> None:
        for index in range(301):
            self.write(f"docs/{index:03}.md")
        self.write("mister/platform/kernel/scanout-slots/policy.c")
        self.commit()
        self.assertTrue(self.push()["kernel"])

    def test_manual_and_new_branch_runs_select_all_checks(self) -> None:
        self.assertEqual(
            select(self.root, "workflow_dispatch", {}), dict.fromkeys(PATHS, True)
        )
        self.assertEqual(self.push("0" * 40), dict.fromkeys(PATHS, True))

    def test_unknown_or_unavailable_revisions_fail_instead_of_skipping(self) -> None:
        with self.assertRaises(ValueError):
            select(self.root, "push", {"before": "--bad", "after": self.base})
        with self.assertRaises(subprocess.CalledProcessError):
            self.push("f" * 40)
        with self.assertRaises(ValueError):
            select(self.root, "schedule", {})

    def test_cli_writes_github_boolean_outputs(self) -> None:
        from scripts.magik_ci.cli import parser

        event = self.root / "event.json"
        output = self.root / "github-output"
        event.write_text(json.dumps({}))
        args = parser().parse_args(
            [
                "ci",
                "changes",
                "--event-name",
                "workflow_dispatch",
                "--event",
                str(event),
                "--github-output",
                str(output),
            ]
        )
        execute(self.root, args.event_name, args.event, args.github_output)
        self.assertEqual(
            output.read_text().splitlines(), [f"{key}=true" for key in PATHS]
        )


if __name__ == "__main__":
    unittest.main()
