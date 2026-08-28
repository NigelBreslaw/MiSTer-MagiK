#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for the bootstrap-free pre-commit gate."""

from __future__ import annotations

import importlib.util
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GATE = ROOT / "scripts/checks/pre-commit.py"
WATCHDOG = ROOT / "scripts/checks/run-with-deadline.py"
UNIFIED_AGENT_CHECK = ROOT / "scripts/checks/check-unified-agent-surface.py"
HOOK = ROOT / ".githooks/pre-commit"

SPEC = importlib.util.spec_from_file_location("pre_commit_gate", GATE)
assert SPEC and SPEC.loader
PRE_COMMIT = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(PRE_COMMIT)


class Repository:
    def __init__(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="pre-commit-fixture-")
        self.root = Path(self.temporary.name)
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.command_log = self.root / "commands.log"
        self.run("git", "init", "-q")
        self.configure_identity()
        self.run("git", "commit", "--allow-empty", "-qm", "baseline")
        unified_agent_check = (
            self.root / "scripts/checks/check-unified-agent-surface.py"
        )
        unified_agent_check.parent.mkdir(parents=True)
        unified_agent_check.write_text(UNIFIED_AGENT_CHECK.read_text())
        cargo = self.bin / "cargo"
        cargo.write_text(
            "#!/bin/sh\n"
            'printf "%s\\n" "$*" >> "$PRE_COMMIT_TEST_LOG"\n'
            'exit "${PRE_COMMIT_TEST_CARGO_EXIT:-0}"\n'
        )
        cargo.chmod(0o755)

    def close(self) -> None:
        self.temporary.cleanup()

    def configure_identity(
        self,
        name: str = PRE_COMMIT.EXPECTED_NAME,
        email: str = PRE_COMMIT.EXPECTED_EMAIL,
    ) -> None:
        self.run("git", "config", "user.name", name)
        self.run("git", "config", "user.email", email)

    def run(
        self,
        *args: str,
        cwd: Path | None = None,
        allowed: tuple[int, ...] = (0,),
    ) -> subprocess.CompletedProcess[str]:
        result = subprocess.run(
            args,
            cwd=cwd or self.root,
            text=True,
            capture_output=True,
            check=False,
        )
        if result.returncode not in allowed:
            raise AssertionError(
                f"{' '.join(args)} exited {result.returncode}\n{result.stdout}\n{result.stderr}"
            )
        return result

    def stage(self, path: str, contents: str, *, force: bool = False) -> None:
        destination = self.root / path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(contents)
        args = ["git", "add"]
        if force:
            args.append("-f")
        args.extend(["--", path])
        self.run(*args)

    def gate(self, **extra_environment: str) -> subprocess.CompletedProcess[str]:
        environment = os.environ.copy()
        environment["PATH"] = f"{self.bin}{os.pathsep}{environment['PATH']}"
        environment["PRE_COMMIT_TEST_LOG"] = str(self.command_log)
        environment.update(extra_environment)
        return subprocess.run(
            [sys.executable, str(GATE), "--repository", str(self.root)],
            cwd=self.root,
            env=environment,
            text=True,
            capture_output=True,
            check=False,
        )


class PreCommitTests(unittest.TestCase):
    def setUp(self) -> None:
        self.repository = Repository()

    def tearDown(self) -> None:
        self.repository.close()

    def test_empty_index_does_not_bootstrap_agent_cli(self) -> None:
        started = time.monotonic()
        result = self.repository.gate(
            MISTER_AGENT_CLI_BINARY="/definitely/missing/agent-cli"
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertLess(time.monotonic() - started, 5)
        self.assertFalse(self.repository.command_log.exists())

    def test_identity_must_match_repository_policy(self) -> None:
        self.repository.configure_identity(email="wrong@example.invalid")
        result = self.repository.gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("git_identity_mismatch", result.stderr)

    def test_forbidden_and_force_added_ignored_paths_are_rejected(self) -> None:
        self.repository.stage(".env", "SECRET=fixture\n", force=True)
        result = self.repository.gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("staged_git_forbidden: .env", result.stderr)

        self.repository.run("git", "reset", "-q")
        self.repository.stage(".gitignore", "ignored.txt\n")
        self.repository.stage("ignored.txt", "ignored\n", force=True)
        result = self.repository.gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("staged_git_ignored: ignored.txt", result.stderr)

    def test_unclassified_paths_are_rejected(self) -> None:
        self.repository.stage("unknown/new.txt", "new\n")
        result = self.repository.gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("unclassified changed paths: unknown/new.txt", result.stderr)

    def test_deprecated_dropped_frame_metrics_are_rejected(self) -> None:
        self.repository.stage(
            "apps/mister/src/lib.rs",
            "// dropped-frame-legacy-fixture: rejection coverage\n"
            "fn probe() { let repeated_refreshes = 1; }\n",
        )
        result = self.repository.gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("deprecated_dropped_frame_term", result.stderr)
        deprecated_term = "repeated_" + "refreshes"
        self.assertIn(deprecated_term, result.stderr)

        self.repository.run("git", "reset", "-q")
        self.repository.stage(
            "apps/mister/src/lib.rs",
            "// dropped-frame-legacy-fixture: rejection coverage\n"
            'const LEGACY: &str = "repeated_'
            'refreshes";\n',
        )
        marked = self.repository.gate()
        self.assertEqual(marked.returncode, 0, marked.stderr)

    def test_software_and_latch_counters_cannot_populate_dropped_frames(self) -> None:
        # dropped-frame-legacy-fixture: rejection coverage
        latch_assignment = "fn invalid(latch_drop_count: u64) { let dropped_frames = latch_drop_count; }\n"
        self.repository.stage(
            "apps/mister/src/lib.rs",
            latch_assignment,
        )
        result = self.repository.gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("software_frame_drop_authority", result.stderr)

        self.repository.run("git", "reset", "-q")
        # dropped-frame-legacy-fixture: rejection coverage
        software_assignment = "fn invalid(software_estimated: u64) { let dropped_frames = software_estimated; }\n"
        self.repository.stage(
            "apps/mister/src/lib.rs",
            software_assignment,
        )
        result = self.repository.gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("software_frame_drop_authority", result.stderr)

    def test_whitespace_shell_and_formatter_failures_are_actionable(self) -> None:
        self.repository.stage("README.md", "trailing space \n")
        result = self.repository.gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("git diff --cached --check", result.stderr)

        self.repository.run("git", "reset", "-q")
        self.repository.stage("scripts/broken.sh", "#!/bin/bash\nif then\n")
        result = self.repository.gate()
        self.assertEqual(result.returncode, 1)
        self.assertIn("bash -n scripts/broken.sh", result.stderr)

        self.repository.run("git", "reset", "-q")
        self.repository.stage("apps/mister/src/lib.rs", "fn probe() {}\n")
        result = self.repository.gate(PRE_COMMIT_TEST_CARGO_EXIT="7")
        self.assertEqual(result.returncode, 1)
        self.assertIn(
            "cargo fmt --manifest-path apps/mister/Cargo.toml --check", result.stderr
        )

    def test_deleted_shell_files_are_not_syntax_checked(self) -> None:
        self.repository.stage("scripts/probe.sh", "#!/bin/bash\ntrue\n")
        self.repository.run("git", "commit", "-qm", "fixture")
        (self.repository.root / "scripts/probe.sh").unlink()
        self.repository.run("git", "add", "-u", "--", "scripts/probe.sh")
        result = self.repository.gate()
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_dirty_submodule_is_rejected(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pre-commit-submodule-") as child_name:
            child = Path(child_name)
            self.repository.run("git", "init", "-q", cwd=child)
            self.repository.run("git", "config", "user.name", "Fixture", cwd=child)
            self.repository.run(
                "git", "config", "user.email", "fixture@example.invalid", cwd=child
            )
            (child / "tracked.txt").write_text("tracked\n")
            self.repository.run("git", "add", "tracked.txt", cwd=child)
            self.repository.run("git", "commit", "-qm", "fixture", cwd=child)
            self.repository.run(
                "git",
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                str(child),
                "private/sample",
            )
            clean = self.repository.gate(GIT_INDEX_FILE=".git/index")
            self.assertEqual(clean.returncode, 0, clean.stderr)
            clean_from_git_hook = self.repository.gate(
                GIT_DIR=str(self.repository.root / ".git"),
                GIT_WORK_TREE=str(self.repository.root),
            )
            self.assertEqual(
                clean_from_git_hook.returncode, 0, clean_from_git_hook.stderr
            )
            (self.repository.root / "private/sample/dirty.txt").write_text("dirty\n")
            result = self.repository.gate(GIT_INDEX_FILE=".git/index")
            self.assertEqual(result.returncode, 1)
            self.assertIn("staged_git_dirty_submodule: private/sample", result.stderr)

    def test_private_submodule_commit_must_be_published(self) -> None:
        with tempfile.TemporaryDirectory(prefix="pre-commit-private-") as area_name:
            area = Path(area_name)
            remote = area / "remote.git"
            source = area / "source"
            self.repository.run("git", "init", "--bare", "-q", str(remote), cwd=area)
            source.mkdir()
            self.repository.run("git", "init", "-q", cwd=source)
            self.repository.run("git", "config", "user.name", "Fixture", cwd=source)
            self.repository.run(
                "git", "config", "user.email", "fixture@example.invalid", cwd=source
            )
            (source / "tracked.txt").write_text("tracked\n")
            self.repository.run("git", "add", "tracked.txt", cwd=source)
            self.repository.run("git", "commit", "-qm", "fixture", cwd=source)
            self.repository.run("git", "branch", "-M", "main", cwd=source)
            self.repository.run(
                "git", "remote", "add", "origin", str(remote), cwd=source
            )
            self.repository.run("git", "push", "-qu", "origin", "main", cwd=source)
            self.repository.run(
                "git",
                "-c",
                "protocol.file.allow=always",
                "submodule",
                "add",
                "-q",
                str(source),
                "private/magik-cloud",
            )
            submodule = self.repository.root / "private/magik-cloud"
            self.repository.run("git", "checkout", "-q", "main", cwd=submodule)
            self.repository.run(
                "git",
                "branch",
                "--set-upstream-to=origin/main",
                "main",
                cwd=submodule,
            )
            self.repository.run("git", "config", "user.name", "Fixture", cwd=submodule)
            self.repository.run(
                "git", "config", "user.email", "fixture@example.invalid", cwd=submodule
            )
            published = self.repository.gate()
            self.assertEqual(published.returncode, 0, published.stderr)
            (submodule / "tracked.txt").write_text("unpublished\n")
            self.repository.run("git", "add", "tracked.txt", cwd=submodule)
            self.repository.run("git", "commit", "-qm", "unpublished", cwd=submodule)
            self.repository.run("git", "add", "--", "private/magik-cloud")
            result = self.repository.gate()
            self.assertEqual(result.returncode, 1)
            self.assertIn(
                "staged_git_private_submodule_must_be_pushed_first", result.stderr
            )

    def test_formatter_selection_matches_affected_packages(self) -> None:
        paths = [
            "agent-cli/src/main.rs",
            "apps/framebuffer-lab/src/main.rs",
            "apps/framebuffer-scene-lab/src/main.rs",
            "apps/mister/src/main.rs",
            "apps/mister/src/other.rs",
            "crates/catalog/src/lib.rs",
            "crates/framebuffer-scenes/src/lib.rs",
            "crates/magik-core/src/lib.rs",
            "crates/agent-protocol/src/lib.rs",
            "mister/platform/runtime/src/lib.rs",
            "mister/platform/contracts/latch/src/lib.rs",
            "mister/platform/contracts/scanout/src/lib.rs",
            "agent-cli/src/host/mod.rs",
        ]
        self.assertEqual(
            PRE_COMMIT.formatters(paths),
            [
                ("agent-cli.format", "agent-cli/Cargo.toml"),
                ("agent-protocol.format", "crates/agent-protocol/Cargo.toml"),
                ("app.format", "apps/mister/Cargo.toml"),
                ("catalog.format", "crates/catalog/Cargo.toml"),
                ("framebuffer-lab.format", "apps/framebuffer-lab/Cargo.toml"),
                (
                    "framebuffer-scene-lab.format",
                    "apps/framebuffer-scene-lab/Cargo.toml",
                ),
                (
                    "framebuffer-scenes.format",
                    "crates/framebuffer-scenes/Cargo.toml",
                ),
                (
                    "latch-contract.format",
                    "mister/platform/contracts/latch/Cargo.toml",
                ),
                ("magik-core.format", "crates/magik-core/Cargo.toml"),
                ("mister-runtime.format", "mister/platform/runtime/Cargo.toml"),
                (
                    "scanout-contract.format",
                    "mister/platform/contracts/scanout/Cargo.toml",
                ),
            ],
        )

    def test_font_text_contract_selection_is_narrow(self) -> None:
        self.assertTrue(
            PRE_COMMIT.needs_font_text_contract(
                ["apps/mister/ui/views/hdmi/home.slint"]
            )
        )
        self.assertTrue(
            PRE_COMMIT.needs_font_text_contract(
                ["scripts/checks/check-font-text-contract.py"]
            )
        )
        self.assertFalse(
            PRE_COMMIT.needs_font_text_contract(["apps/mister/src/ui_display.rs"])
        )

    def test_mixed_gate_deduplicates_formatters_under_budget(self) -> None:
        for path in [
            "agent-cli/src/main.rs",
            "apps/mister/src/main.rs",
            "apps/mister/src/other.rs",
            "crates/catalog/src/lib.rs",
            "agent-cli/src/host/mod.rs",
        ]:
            self.repository.stage(path, "fn probe() {}\n")
        started = time.monotonic()
        result = self.repository.gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertLess(time.monotonic() - started, 5)
        self.assertEqual(
            self.repository.command_log.read_text().splitlines(),
            [
                "fmt --manifest-path agent-cli/Cargo.toml --check",
                "fmt --manifest-path apps/mister/Cargo.toml --check",
                "fmt --manifest-path crates/catalog/Cargo.toml --check",
            ],
        )

    def test_hook_targets_python_gate_under_existing_deadline(self) -> None:
        hook = HOOK.read_text()
        self.assertIn("--seconds 10", hook)
        self.assertIn("scripts/checks/pre-commit.py", hook)
        self.assertNotIn('"$ROOT/scripts/agent" pre-commit', hook)

    def test_watchdog_propagates_status_and_kills_descendants(self) -> None:
        status = subprocess.run(
            [
                sys.executable,
                str(WATCHDOG),
                "--seconds",
                "2",
                "--label",
                "test",
                "--",
                "sh",
                "-c",
                "exit 7",
            ],
            check=False,
        )
        self.assertEqual(status.returncode, 7)

        survivor = self.repository.root / "survivor"
        timed_out = subprocess.run(
            [
                sys.executable,
                str(WATCHDOG),
                "--seconds",
                "0.05",
                "--label",
                "test",
                "--",
                "sh",
                "-c",
                f"(sleep 0.15; touch {survivor}) & wait",
            ],
            check=False,
        )
        self.assertEqual(timed_out.returncode, 124)
        time.sleep(0.25)
        self.assertFalse(survivor.exists())


if __name__ == "__main__":
    unittest.main()
