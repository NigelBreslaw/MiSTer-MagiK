import hashlib
import json
from types import SimpleNamespace
from magik.fpga_evidence import capture


def test_partial_capture_preserves_diagnostics_raw_frame_and_hashes(tmp_path):
    calls = []

    def operation(name):
        calls.append(name)
        return {"samples": [{"raw_ack_high": [23, 75, 2409, 15306]}]}

    def frame():
        return {"source": "wrong-source"}, b"raw"

    agent = SimpleNamespace(device_operation=operation, capture_framebuffer=frame)
    assert capture(agent, tmp_path, framebuffer=True) == 1
    folder = tmp_path / "fpga-incident"
    assert calls == ["fpga-evidence", "fpga-evidence"]
    assert (folder / "framebuffer.rgb565").read_bytes() == b"raw"
    timeline = json.loads((folder / "timeline.json").read_text())
    assert [v["status"] for v in timeline] == ["saved", "failed", "saved"]
    for line in (folder / "SHA256SUMS").read_text().splitlines():
        digest, name = line.split("  ")
        assert hashlib.sha256((folder / name).read_bytes()).hexdigest() == digest


def test_poll_bounds_first_event_and_keeps_output_out_of_fingerprint(
    monkeypatch, tmp_path
):
    from magik import fpga_evidence

    def report(valid, output):
        decoded = {
            "schema": 24,
            "first_selected": True,
            "crc_valid": True,
            "record_valid": valid,
            "cause": 2 if valid else 0,
            "ledger_valid": True,
            "physical_depth": 0,
            "physical_phase": 0,
            "production_depth": 0,
            "production_phase": 0,
            "output_state": output,
        }
        return {
            "before": {"boot_id": "boot-a"},
            "after": {"boot_id": "boot-a"},
            "samples": [{"decoded": decoded}],
        }

    replies = iter(
        [report(False, 0), report(False, 1), report(True, 2), report(True, 3)]
    )
    agent = SimpleNamespace(device_operation=lambda _: next(replies))
    monkeypatch.setattr(fpga_evidence.time, "sleep", lambda _: None)
    args = SimpleNamespace(
        poll_count=2, poll_interval=1, framebuffer=False, usb_seconds=None
    )
    assert fpga_evidence.collect(agent, SimpleNamespace(fields={}), tmp_path, args) == 0
    index = json.loads((tmp_path / "poll-index.json").read_text())
    assert index[1]["first_seen_interval_monotonic_ns"] == [
        index[0]["started_monotonic_ns"],
        index[1]["ended_monotonic_ns"],
    ]
    assert fpga_evidence.first_source(report(True, 2)) == fpga_evidence.first_source(
        report(True, 3)
    )


def test_inconsistent_first_reads_do_not_become_a_valid_event():
    from magik.fpga_evidence import first_source

    records = [
        {"schema": 24, "first_selected": True, "crc_valid": True, "record_valid": v}
        for v in (False, True)
    ]
    assert first_source({"samples": [{"decoded": r} for r in records]}) is None
