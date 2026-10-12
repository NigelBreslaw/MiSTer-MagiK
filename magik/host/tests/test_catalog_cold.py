import json
from types import SimpleNamespace
from unittest.mock import Mock

import pytest
from magik import catalog_cold
from magik.client import AgentError

TIMELINE = (
    "t_ms\tutime\tstime\tbuild_utime\tbuild_stime\tdisk_read_ms\tdisk_read_sectors"
    "\tdisk_write_ms\tdisk_write_sectors\tcpu_user\tcpu_nice\tcpu_system\tcpu_idle\tcpu_iowait"
    "\tcpu_irq\tcpu_softirq\n"
    "0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\t0\n"
    "1000\t60\t20\t10\t0\t400\t4096\t100\t0\t100\t0\t40\t40\t20\t0\t0\n"
    "2000\t60\t20\t10\t0\t400\t4096\t100\t2048\t200\t0\t80\t80\t40\t0\t0\n"
)
INSPECT = (
    "catalog_v3_summary_tsv\tvalid=1\tschema=2\tgeneration=1\ttotal_games=7\t"
    + "\t".join(f"{key}={key[:2] * 32}" for key in catalog_cold.DIGESTS)
)


def test_catalog_digests_reject_missing_or_invalid_summaries():
    assert catalog_cold.catalog_digests(INSPECT)["total_games"] == "7"
    with pytest.raises(ValueError, match="single summary"):
        catalog_cold.catalog_digests("other\tline")
    with pytest.raises(ValueError, match="not valid"):
        catalog_cold.catalog_digests(INSPECT.replace("valid=1", "valid=0"))


def test_differing_digests_name_each_changed_output():
    base = catalog_cold.catalog_digests(INSPECT)
    changed = {**base, "search_sha256": "0" * 64}
    assert catalog_cold.differing_digests(base, base) == []
    assert catalog_cold.differing_digests(base, changed) == ["search_sha256"]


def test_milestones_keep_the_first_time():
    marks = "t_ms\tname\n5\tplan-ready\n9\tplan-ready\n12\tpublished\n"
    assert catalog_cold.milestones(marks) == {"plan-ready": 5, "published": 12}


def test_phase_report_measures_each_window_between_milestones():
    marks = {"system-prepared:arcade": 1000, "plan-ready": 2000}
    report = catalog_cold.phase_report({"clock_ticks_per_second": 100}, TIMELINE, marks)
    assert [phase["phase"] for phase in report] == [
        "start to Arcade ready",
        "scan other systems",
    ]
    first, second = report
    assert first["wall_ms"] == 1000
    # The process used 80 ticks in a second; the build thread 10 of them.
    assert first["build_cpu_percent"] == 10
    assert first["other_threads_cpu_percent"] == 70
    assert first["sd_busy_percent"] == 50
    assert first["sd_read_mib"] == pytest.approx(2.0)
    # Busy ticks over all ticks: (100 + 40) of 200, then (100 + 40) of 200.
    assert first["all_cpus_busy_percent"] == 70
    assert second["sd_write_mib"] == pytest.approx(1.0)


def test_phase_report_keeps_the_scan_when_the_card_has_no_arcade():
    report = catalog_cold.phase_report(
        {"clock_ticks_per_second": 100}, TIMELINE, {"plan-ready": 2000}
    )
    assert [(phase["phase"], phase["wall_ms"]) for phase in report] == [
        ("scan other systems", 2000)
    ]


class FakeAgent:
    def __init__(self, evidence, files):
        self.evidence, self.files, self.calls = evidence, files, []

    def read_profile_artifact(self, profile_id, name):
        assert profile_id == catalog_cold.PROFILE_ID
        self.calls.append(name)
        if name == "evidence.json":
            return self.evidence.pop(0)()
        return self.files[name]


def test_wait_for_evidence_polls_until_the_completion_marker_exists():
    def missing():
        raise AgentError("artifact-unavailable: not yet")

    agent = FakeAgent([missing, missing, lambda: b"{}"], {})
    sleeps = []
    assert catalog_cold.wait_for_evidence(agent, sleep=sleeps.append) == b"{}"
    assert sleeps == [5, 5]


def test_wait_for_evidence_times_out_and_surfaces_other_errors():
    def missing():
        raise AgentError("artifact-unavailable: not yet")

    clock = iter([0, catalog_cold.EVIDENCE_TIMEOUT_SECONDS + 1])
    with pytest.raises(AgentError, match="Development tooling build"):
        catalog_cold.wait_for_evidence(
            FakeAgent([missing], {}), sleep=lambda _: None, clock=lambda: next(clock)
        )

    def denied():
        raise AgentError("unauthorized")

    with pytest.raises(AgentError, match="unauthorized"):
        catalog_cold.wait_for_evidence(FakeAgent([denied], {}))


def setup_cold_build(monkeypatch, tmp_path, outcome="published", inspect=INSPECT):
    order = []
    evidence = {
        "outcome": outcome,
        "wall_ms": 2000,
        "kernel": "Linux version test",
        "clock_ticks_per_second": 100,
        "task_interval_ms": 10,
    }
    files = {
        "timeline.tsv": TIMELINE.encode(),
        "tasks.tsv": b"t_ms\ttid\tcomm\tstate\twchan\tsyscall\tdetail\n",
        "stacks.tsv": b"tid\tpc\tlr\tframes\n",
        "maps.txt": b"",
        "marks.tsv": b"t_ms\tname\n1000\tsystem-prepared:arcade\n2000\tplan-ready\n",
    }
    agent = FakeAgent([lambda: json.dumps(evidence).encode()], files)
    agent.status = Mock(return_value=SimpleNamespace(fields={"artifacts": {}}))
    monkeypatch.setattr("magik.cli.connect_agent", Mock(return_value=(agent, None)))

    def operation(_agent, _run, fields, name="catalog"):
        order.append(("operation", fields["action"]))
        return SimpleNamespace(fields={}), inspect.encode()

    monkeypatch.setattr("magik.device.catalog_operation", operation)
    monkeypatch.setattr(
        "magik.device.reboot_device",
        lambda arguments, run, *, agent: order.append(("reboot", arguments.attended)),
    )
    return order


def arguments(tmp_path, baseline=None):
    return SimpleNamespace(
        attended=True, baseline=baseline, symbols=tmp_path / "missing-elf"
    )


def test_cold_build_purges_reboots_once_then_inspects(monkeypatch, tmp_path, capsys):
    order = setup_cold_build(monkeypatch, tmp_path)
    assert catalog_cold.cold_build(arguments(tmp_path), tmp_path) == 0
    assert order == [
        ("operation", "purge"),
        ("reboot", True),
        ("operation", "inspect"),
    ]
    summary = json.loads((tmp_path / "summary.json").read_text())
    assert summary["catalog"]["total_games"] == "7"
    assert "skipped" in summary["profile"]
    assert "cold build published in 2.0 s" in capsys.readouterr().out


def test_cold_build_gates_on_identical_output_against_a_baseline(monkeypatch, tmp_path):
    setup_cold_build(monkeypatch, tmp_path)
    baseline = tmp_path / "baseline"
    baseline.mkdir()
    changed = INSPECT.replace("identity_sha256=id", "identity_sha256=xx")
    (baseline / "summary.json").write_text(
        json.dumps({"wall_ms": 3000, "catalog": catalog_cold.catalog_digests(changed)})
    )
    run = tmp_path / "run"
    run.mkdir()
    status = catalog_cold.cold_build(arguments(tmp_path, baseline), run)
    summary = json.loads((run / "summary.json").read_text())
    assert status == 1
    assert summary["output_differs"] == ["identity_sha256"]
    assert summary["baseline_wall_ms"] == 3000


def test_cold_build_reports_a_failed_build_without_inspecting(monkeypatch, tmp_path):
    order = setup_cold_build(monkeypatch, tmp_path, outcome="failed")
    with pytest.raises(RuntimeError, match="failed"):
        catalog_cold.cold_build(arguments(tmp_path), tmp_path)
    assert ("operation", "inspect") not in order
    assert (tmp_path / "evidence.json").exists()
