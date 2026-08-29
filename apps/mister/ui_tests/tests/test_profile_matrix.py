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
from apps.mister.ui_tests.queries import element_with_label, elements_with_label

DISPLAY_PROFILES = ("hdmi-720p", "hdmi-1080p", "crt-240p", "crt-480p")
ORIENTATIONS = ("normal", "monitor-clockwise")
FEATURES = ("home", "arcade", "settings")

DISPLAY_CONTRACTS = {
    "hdmi-720p": ("hdmi", "hdmi-1280x720p60"),
    "hdmi-1080p": ("hdmi", "hdmi-1920x1080p60"),
    "crt-240p": ("crt-240p60", "crt-240p60"),
    "crt-480p": ("crt-480p60", "crt-480p60"),
}

PROFILE_EXPECTATIONS = {
    "hdmi-720p": ("hdmi", (1280, 720), (1280, 720)),
    "hdmi-1080p": ("hdmi", (1920, 1080), (960, 540)),
    "crt-240p": ("crt-240p60", (640, 240), (640, 240)),
    "crt-480p": ("crt-480p60", (640, 480), (640, 480)),
}
ORIENTATION_LABELS = {
    "normal": "Normal",
    "monitor-clockwise": "Monitor right (clockwise)",
}
FEATURE_LABELS = {
    "home": "MiSTer MagiK Launcher",
    "arcade": "Arcade games",
    "settings": "Settings",
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
        expected_route, expected_output, expected_render = PROFILE_EXPECTATIONS[display]
        semantic = driver.wait_for_semantic(
            lambda state: state.effective_view == feature
            and state.screen_orientation == ORIENTATION_LABELS[orientation]
            and state.output_route == expected_route
            and (state.output_width, state.output_height) == expected_output
            and (state.render_width, state.render_height) == expected_render
        )
        assert semantic.effective_view == feature
        assert elements_with_label(driver, FEATURE_LABELS[feature]), (
            f"feature label {FEATURE_LABELS[feature]!r} was not exposed"
        )


__all__ = ["DISPLAY_PROFILES", "FEATURES", "ORIENTATIONS"]
