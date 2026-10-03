from unittest.mock import Mock

import pytest

from magik import animation_benchmark


def test_late_animation_and_stale_idle_sample_do_not_allow_the_next_input(monkeypatch):
    sleep = Mock()
    monkeypatch.setattr(animation_benchmark.time, "sleep", sleep)
    monkeypatch.setattr(animation_benchmark.time, "monotonic", lambda: 0)
    snapshots = iter(
        [
            {"elapsed_ms": 2_900, "ui_motion": False},
            {"elapsed_ms": 3_100, "ui_motion": True},
            {"elapsed_ms": 3_500, "ui_motion": False},
        ]
    )
    assert animation_benchmark._settled_metrics(lambda: next(snapshots), 1_000, 10) == {
        "elapsed_ms": 3_500,
        "ui_motion": False,
    }
    assert sleep.call_count == 2


def test_stalled_animation_fails_before_the_measurement_window_can_end(monkeypatch):
    monkeypatch.setattr(animation_benchmark.time, "monotonic", lambda: 10)
    with pytest.raises(AssertionError, match="did not settle"):
        animation_benchmark._settled_metrics(
            lambda: {"elapsed_ms": 3_100, "ui_motion": True},
            1_000,
            10,
        )


def test_missing_motion_evidence_cannot_be_treated_as_idle():
    with pytest.raises(AssertionError, match="Missing UI motion"):
        animation_benchmark._settled_metrics(lambda: {"elapsed_ms": 3_100}, 1_000, 10)
