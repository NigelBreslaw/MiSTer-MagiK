"""Reject stale/probe measurements and distinguish physical repeats from drops."""

import pytest
from actions import validate_launcher_motion


def measurement():
    return {"sha256": "expected", "context": {
        "workload": "launcher-slide", "launcher_owned_vblanks": 300,
        "launcher_presented_vblanks": 300, "launcher_repeated_vblanks": 0,
        "launcher_ownership_losses": 0,
    }, "window": {
        "start_ms": 2000, "end_ms": 7000, "elapsed_ms": 5000,
        "width": 960, "height": 540, "instrumented": False,
        "presentations": 300, "render_us_total": 900000,
        "render_to_present_us_total": 4990000, "physical_latch_posts": 300,
        "physical_latch_flips": 300, "physical_drops": 0, "latch_rejections": 0,
        "evidence_error": None, "drop_baseline_available": True,
    }}


def test_valid_launcher_measurement():
    assert validate_launcher_motion(measurement(), "expected")["no_physical_repeats"]


def test_repeats_are_reported_not_mislabelled_as_no_drops():
    metrics = measurement()
    metrics["context"].update(launcher_presented_vblanks=299, launcher_repeated_vblanks=1)
    assert not validate_launcher_motion(metrics, "expected")["no_physical_repeats"]


@pytest.mark.parametrize("field,value", [("workload", "probe-motion"),
                                         ("launcher_ownership_losses", 1),
                                         ("launcher_owned_vblanks", 301)])
def test_invalid_or_wrong_workload_rejected(field, value):
    metrics = measurement()
    metrics["context"][field] = value
    with pytest.raises(AssertionError):
        validate_launcher_motion(metrics, "expected")


def test_stale_artifact_and_missing_telemetry_rejected():
    with pytest.raises(AssertionError):
        validate_launcher_motion(measurement(), "other")
    metrics = measurement()
    del metrics["context"]["launcher_repeated_vblanks"]
    with pytest.raises(AssertionError):
        validate_launcher_motion(metrics, "expected")
