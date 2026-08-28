#!/usr/bin/env python3
# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Fixture tests for the privacy-safe Codex context report."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REPORTER = ROOT / "scripts/codex-context-report.py"
SECRET = "DO_NOT_PRINT_SESSION_CONTENT"


def record(record_type: str, payload: dict[str, object]) -> str:
    return json.dumps({"type": record_type, "payload": payload})


def fixture_lines(*, large: bool = True) -> list[str]:
    output = SECRET + ("x" * 12_000 if large else "")
    return [
        record(
            "session_meta",
            {"context_window": 258_400, "base_instructions": SECRET},
        ),
        record(
            "response_item",
            {
                "type": "function_call",
                "call_id": "large",
                "name": "exec_command",
                "arguments": SECRET,
            },
        ),
        record(
            "response_item",
            {"type": "function_call_output", "call_id": "large", "output": output},
        ),
        record(
            "response_item",
            {
                "type": "custom_tool_call",
                "call_id": "small",
                "name": "exec",
                "input": "const r = await tools.web__run({}); text(r);",
            },
        ),
        record(
            "response_item",
            {
                "type": "custom_tool_call_output",
                "call_id": "small",
                "output": [{"type": "input_text", "text": "tiny"}],
            },
        ),
        record(
            "event_msg",
            {
                "type": "token_count",
                "info": {
                    "model_context_window": 258_400,
                    "last_token_usage": {"input_tokens": 100},
                },
            },
        ),
        record(
            "event_msg",
            {
                "type": "token_count",
                "info": {
                    "model_context_window": 258_400,
                    "last_token_usage": {"input_tokens": 800},
                },
            },
        ),
        record(
            "event_msg",
            {
                "type": "token_count",
                "info": {
                    "model_context_window": 258_400,
                    "last_token_usage": {"input_tokens": 300},
                },
            },
        ),
        record("compacted", {"message": SECRET}),
        json.dumps({"type": "future_record", "payload": {"text": SECRET}}),
        "malformed " + SECRET,
    ]


class ContextReportTests(unittest.TestCase):
    def run_report(
        self, *args: str, env: dict[str, str] | None = None
    ) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            [sys.executable, str(REPORTER), *args],
            cwd=ROOT,
            env=env,
            text=True,
            capture_output=True,
            check=False,
        )

    def write_session(self, directory: Path, name: str, lines: list[str]) -> Path:
        path = directory / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        return path

    def test_json_is_sorted_aggregate_only_and_tracks_context(self) -> None:
        with tempfile.TemporaryDirectory(prefix="codex-context-fixture-") as name:
            session = self.write_session(
                Path(name), "private-session.jsonl", fixture_lines()
            )
            result = self.run_report(str(session), "--json")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn(SECRET, result.stdout)
        self.assertNotIn("private-session", result.stdout)
        report = json.loads(result.stdout)
        self.assertEqual(report["version"], 1)
        self.assertEqual(report["sessions"], 1)
        self.assertEqual(report["context"]["first_input_tokens"], 100)
        self.assertEqual(report["context"]["maximum_input_tokens"], 800)
        self.assertEqual(report["context"]["final_input_tokens"], 300)
        self.assertEqual(report["context"]["compactions"], 1)
        self.assertEqual(
            [row["tool"] for row in report["tools"]], ["exec_command", "web__run"]
        )
        self.assertEqual(report["tools"][0]["over_10_kib"], 1)
        self.assertEqual(
            {warning["code"] for warning in report["warnings"]},
            {"malformed_lines", "unknown_records"},
        )

    def test_human_output_contains_metrics_but_no_content_or_paths(self) -> None:
        with tempfile.TemporaryDirectory(prefix="codex-context-fixture-") as name:
            session = self.write_session(
                Path(name), "secret-name.jsonl", fixture_lines()
            )
            result = self.run_report(str(session))
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("Tool | Calls | Output bytes | Est. tokens", result.stdout)
        self.assertIn("first=100 maximum=800 final=300", result.stdout)
        self.assertNotIn(SECRET, result.stdout)
        self.assertNotIn("secret-name", result.stdout)

    def test_recent_defaults_to_only_the_newest_session(self) -> None:
        with tempfile.TemporaryDirectory(prefix="codex-home-fixture-") as name:
            root = Path(name)
            sessions = root / "sessions"
            old = self.write_session(sessions, "old.jsonl", fixture_lines(large=False))
            time.sleep(0.01)
            self.write_session(sessions, "new.jsonl", fixture_lines())
            old.touch()
            time.sleep(0.01)
            newest = sessions / "new.jsonl"
            newest.touch()
            environment = dict(os.environ)
            environment["CODEX_HOME"] = str(root)
            result = self.run_report("--json", env=environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report["sessions"], 1)
        self.assertGreater(report["tools"][0]["output_bytes"], 10_000)

    def test_missing_input_error_does_not_echo_the_path(self) -> None:
        missing = "/private/DO_NOT_ECHO/missing-session.jsonl"
        result = self.run_report(missing)
        self.assertEqual(result.returncode, 2)
        self.assertNotIn(missing, result.stderr)
        self.assertIn("no readable session input", result.stderr)


if __name__ == "__main__":
    unittest.main()
