"""Consumer-side regression for Mini's source-versus-scanout geometry."""

import pytest
from actions import validate_mini_display


def geometry():
    return {"context": {"source_width": 960, "source_height": 540,
                        "scan_width": 1920, "scan_height": 1080,
                        "destination_width": 1920, "destination_height": 1080}}


def test_half_resolution_source_fills_full_scanout():
    result = validate_mini_display(geometry(), {"width": 960, "height": 540})
    assert result["display_geometry"]["destination_width"] == 1920


def test_quarter_screen_destination_fails_even_with_correct_source_capture():
    metrics = geometry()
    metrics["context"].update(destination_width=960, destination_height=540)
    with pytest.raises(AssertionError, match="does not fill"):
        validate_mini_display(metrics, {"width": 960, "height": 540})


def test_stale_or_missing_geometry_fails():
    with pytest.raises(AssertionError, match="differs"):
        validate_mini_display(geometry(), {"width": 640, "height": 480})
    with pytest.raises(AssertionError, match="omitted"):
        validate_mini_display({}, {"width": 960, "height": 540})
