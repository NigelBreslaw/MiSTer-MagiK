"""Cross-product smoke coverage for supported display/layout profiles."""

from __future__ import annotations

import os
import shlex

import pytest

from apps.mister.ui_tests.driver import (
    DriverConfig,
    MagiKDriver,
    environment_for_application,
)
from apps.mister.ui_tests.queries import element_with_label

DISPLAY_PROFILES = ("hdmi-720p", "hdmi-1080p", "crt-240p", "crt-480p")
ORIENTATIONS = ("normal", "monitor-clockwise")
FEATURES = ("home", "arcade", "settings")

DISPLAY_CONTRACTS = {
    "hdmi-720p": ("hdmi", "hdmi-1280x720p60"),
    "hdmi-1080p": ("hdmi", "hdmi-1920x1080p60"),
    "crt-240p": ("crt-240p60", "crt-240p60"),
    "crt-480p": ("crt-480p60", "crt-480p60"),
}


@pytest.mark.parametrize("display", DISPLAY_PROFILES)
@pytest.mark.parametrize("orientation", ORIENTATIONS)
@pytest.mark.parametrize("feature", FEATURES)
def test_profile_feature_matrix(
    display: str,
    orientation: str,
    feature: str,
) -> None:
    command_text = os.environ.get("MISTER_UI_TEST_COMMAND")
    if not command_text:
        pytest.skip("set MISTER_UI_TEST_COMMAND for attended device UI tests")
    output_route, display_mode = DISPLAY_CONTRACTS[display]
    environment = environment_for_application()
    environment.update(
        {
            "MISTER_UI_TEST_DISPLAY": display,
            "MISTER_UI_TEST_ORIENTATION": orientation,
            "MISTER_UI_TEST_FEATURE": feature,
            "MISTER_MAGIK_RUNTIME_SETTINGS_V1": f"schema=1&output={output_route}",
            "MISTER_MAGIK_RUNTIME_DISPLAY_V1": f"schema=1&mode={display_mode}",
        }
    )
    config = DriverConfig(
        command=tuple(shlex.split(command_text)),
        environment=environment,
        launch_timeout=float(os.environ.get("MISTER_UI_TEST_LAUNCH_TIMEOUT", "20")),
    )
    with MagiKDriver.start(config) as driver:
        launcher = element_with_label(driver, "MiSTer MagiK Launcher")
        assert launcher.accessible_enabled


__all__ = ["DISPLAY_PROFILES", "FEATURES", "ORIENTATIONS"]
