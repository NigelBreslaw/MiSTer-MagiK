from __future__ import annotations

import json

from magik2.results import append_event, create_run


def test_run_bundle_has_small_machine_readable_index(tmp_path) -> None:
    run = create_run(tmp_path, "motion", {"dirty": True})
    append_event(run, {"phase": "complete", "outcome": "passed"})
    assert json.loads((run / "run.json").read_text())["operation"] == "motion"
    assert "passed" in (run / "events.jsonl").read_text()
