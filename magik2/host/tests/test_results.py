from __future__ import annotations

import json

from magik2.results import append_event, create_run


def test_run_bundle_has_small_machine_readable_index(tmp_path) -> None:
    run = create_run(tmp_path, "motion", {"dirty": True})
    append_event(run, {"phase": "complete", "outcome": "passed"})
    assert json.loads((run / "run.json").read_text())["operation"] == "motion"
    assert "passed" in (run / "events.jsonl").read_text()


def test_final_result_retains_exact_artifact_failure_and_logs(tmp_path):
    from magik2.results import finalize, retain_diagnostics

    class Agent:
        def diagnostics(self):
            return {"probe_log": "startup failed", "agent_log": "native detail"}

    run = create_run(tmp_path, "deploy", {"git_dirty": True})
    append_event(
        run,
        {
            "phase": "artifact",
            "sha256": "exact-bytes",
            "source_fingerprint": "exact-inputs",
        },
    )
    append_event(
        run, {"phase": "failed", "outcome": "failed", "error": "startup rejected"}
    )
    retain_diagnostics(run, Agent())
    finalize(run, 2, 123)
    result = json.loads((run / "run.json").read_text())
    assert result["artifact"]["sha256"] == "exact-bytes"
    assert result["outcome"] == "failed" and result["elapsed_ms"] == 123
    assert result["failures"][0]["error"] == "startup rejected"
    assert "startup failed" in (run / "logs.txt").read_text()
