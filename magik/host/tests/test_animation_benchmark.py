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


def test_completed_window_is_rejected_even_when_motion_has_settled(monkeypatch):
    monkeypatch.setattr(animation_benchmark.time, "monotonic", lambda: 0)
    with pytest.raises(AssertionError, match="window ended"):
        animation_benchmark._settled_metrics(
            lambda: {
                "elapsed_ms": 3_500,
                "ui_motion": False,
                "window": {"end_ms": 3_400},
            },
            1_000,
            10,
        )


@pytest.mark.parametrize("value", ["0", "11", "invalid"])
def test_campaign_cannot_silently_run_zero_or_unbounded_repetitions(monkeypatch, value):
    monkeypatch.setenv("MAGIK_ANIMATION_REPETITIONS", value)
    with pytest.raises(ValueError):
        animation_benchmark.animation_repetitions()


def test_campaign_defaults_to_three_and_supports_one_diagnostic(monkeypatch):
    monkeypatch.delenv("MAGIK_ANIMATION_REPETITIONS", raising=False)
    assert list(animation_benchmark.animation_repetitions()) == [0, 1, 2]
    monkeypatch.setenv("MAGIK_ANIMATION_REPETITIONS", "1")
    assert list(animation_benchmark.animation_repetitions()) == [0]


def test_campaign_routes_preserve_order_and_reject_unknown_routes(monkeypatch):
    monkeypatch.setenv("MAGIK_ANIMATION_ROUTES", "settings, root")
    assert animation_benchmark.animation_routes() == ["settings", "root"]
    monkeypatch.setenv("MAGIK_ANIMATION_ROUTES", "root,unknown")
    with pytest.raises(ValueError, match="Unknown animation routes"):
        animation_benchmark.animation_routes()
