from magik2.acceptance import summarize


def test_failed_attempts_remain_in_the_timings_and_failures():
    attempts = [
        {"elapsed_ms": 3000, "exit_code": 2},
        {"elapsed_ms": 4000, "exit_code": 0},
    ]
    result = summarize(attempts, 1000)
    assert result == {
        "attempts": 2,
        "failures": 1,
        "slowest_ms": 4000,
        "target_ms": 1000,
        "target_met": False,
    }


def test_two_successful_attempts_are_sufficient():
    rows = [{"elapsed_ms": 500, "exit_code": 0}, {"elapsed_ms": 900, "exit_code": 0}]
    assert summarize(rows, 1000)["target_met"]
    assert summarize(rows, 1000)["slowest_ms"] == 900
    assert not summarize(rows[:1], 1000)["target_met"]


def test_cancellation_retains_attempt_and_restores_sources(monkeypatch, tmp_path):
    import json
    from types import SimpleNamespace
    import pytest
    from magik2 import acceptance

    root = tmp_path / "repo"
    probe = root / "magik2/probe"
    (probe / "src").mkdir(parents=True)
    (probe / "ui").mkdir()
    rust = probe / "src/main.rs"
    rust.write_text("original Rust")
    (probe / "ui/probe.slint").write_text("original Slint")
    artifact = (
        probe / "target/armv7-unknown-linux-gnueabihf/release/mister-magik2-probe"
    )
    artifact.parent.mkdir(parents=True)
    artifact.write_bytes(b"probe")
    monkeypatch.setattr(
        acceptance, "__file__", str(root / "magik2/host/magik2/acceptance.py")
    )
    calls = []

    def execute(*args, **kwargs):
        calls.append(args)
        if len(calls) == 2:
            raise KeyboardInterrupt
        return SimpleNamespace(returncode=0)

    monkeypatch.setattr(acceptance.subprocess, "run", execute)
    run = tmp_path / "run"
    run.mkdir()
    with pytest.raises(KeyboardInterrupt):
        acceptance.run_delivery_matrix(run)
    result = json.loads((run / "acceptance.json").read_text())
    assert result["cases"]["no-op"]["attempts"][0]["exit_code"] == 130
    assert result["restoration"]["exit_code"] == 0
    assert len(calls) == 3
    assert rust.read_text() == "original Rust"
