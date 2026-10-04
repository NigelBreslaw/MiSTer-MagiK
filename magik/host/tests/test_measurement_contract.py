from unittest.mock import Mock

import pytest

from magik.measurement_contract import (
    has_native_card_helpers,
    measurement_metrics,
    verify_display,
)
from magik.results import create_run


def test_geometry_drift_or_stale_embedded_window_is_rejected(monkeypatch):
    monkeypatch.setenv("MAGIK_EXPECT_RENDER_SIZE", "640x240")
    for value in (
        {"width": 960, "height": 540},
        {"width": 640, "height": 240, "window": {"width": 960, "height": 540}},
    ):
        with pytest.raises(AssertionError, match="output changed"):
            measurement_metrics(Mock(metrics=Mock(return_value=value)))
    good = {"width": 640, "height": 240, "window": {"width": 640, "height": 240}}
    assert measurement_metrics(Mock(metrics=Mock(return_value=good))) is good


def test_mode_guard_records_failure_without_changing_device(monkeypatch, tmp_path):
    monkeypatch.setenv("MAGIK_EXPECT_DISPLAY_MODE", "crt-240p60")
    agent = Mock()
    agent.device_operation.return_value = {
        "reply": "ok active=hdmi-1920x1080p60 pending=none"
    }
    run = create_run(tmp_path, "check", {})
    with pytest.raises(AssertionError, match="Expected crt-240p60"):
        verify_display(agent, run, "before-session")
    agent.device_operation.assert_called_once_with("display-status")
    assert '"outcome":"failed"' in (run / "events.jsonl").read_text()


def test_pending_mode_cannot_pass_even_if_active_mode_matches(monkeypatch, tmp_path):
    monkeypatch.setenv("MAGIK_EXPECT_DISPLAY_MODE", "crt-240p60")
    agent = Mock()
    agent.device_operation.return_value = {
        "reply": "ok active=crt-240p60 pending=hdmi-1920x1080p60"
    }
    with pytest.raises(AssertionError):
        verify_display(agent, create_run(tmp_path, "check", {}), "after-session")


def test_unpinned_measurements_reuse_existing_service_without_extra_operations(
    monkeypatch,
):
    monkeypatch.delenv("MAGIK_EXPECT_DISPLAY_MODE", raising=False)
    monkeypatch.delenv("MAGIK_EXPECT_RENDER_SIZE", raising=False)
    agent = Mock(metrics=Mock(return_value={"existing": True}))
    verify_display(agent, None, "before-session")
    assert measurement_metrics(agent) == {"existing": True}
    agent.device_operation.assert_not_called()


@pytest.mark.parametrize(
    "value,expected",
    [
        ("native-tricks-v1", True),
        ("native-browse-tricks-v2", True),
        ("disabled", False),
        (None, False),
    ],
)
def test_helper_checks_apply_only_to_advertised_native_renderer(value, expected):
    assert (
        has_native_card_helpers({"context": {"card_helper_ahead": value}}) is expected
    )
