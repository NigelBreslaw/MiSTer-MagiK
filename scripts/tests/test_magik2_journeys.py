# Copyright (C) 2026 Nigel Breslaw
# SPDX-License-Identifier: GPL-3.0-or-later

"""Host-only regressions for the application's shared 2.0 journeys."""

import importlib.util
from pathlib import Path

import pytest


@pytest.fixture
def actions(monkeypatch):
    repository = Path(__file__).resolve().parents[2]
    monkeypatch.syspath_prepend(str(repository / "magik2/host"))
    spec = importlib.util.spec_from_file_location(
        "journey_actions", repository / "magik2/scenarios/actions.py"
    )
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


@pytest.mark.parametrize("capture_fails", [False, True])
def test_setting_restores_original_after_capture_failure(
    actions, monkeypatch, tmp_path, capture_fails
):
    from types import SimpleNamespace

    state = {"open": False, "value": "On"}

    def press(_, key):
        if key == "\uf729":
            state["open"] = False
        elif key == "\n":
            if state["open"]:
                state["value"] = "Off" if state["value"] == "On" else "On"
            else:
                state["open"] = True

    def capture(*_):
        if capture_fails:
            raise RuntimeError("capture failed")

    monkeypatch.setattr(actions, "_press_key", press)
    monkeypatch.setattr(actions, "_focus_label", lambda *_: None)
    monkeypatch.setattr(actions, "_settings_open", lambda _: state["open"])
    monkeypatch.setattr(
        actions,
        "one_element",
        lambda *_: SimpleNamespace(accessible_description=state["value"]),
    )
    monkeypatch.setattr(actions, "screenshot", capture)
    if capture_fails:
        with pytest.raises(RuntimeError, match="capture failed"):
            actions.launcher_setting(object(), tmp_path / "setting.png")
    else:
        assert actions.launcher_setting(object(), tmp_path / "setting.png")["restored"]
    assert state == {"open": False, "value": "On"}


def test_focus_accepts_target_on_last_allowed_step(actions, monkeypatch):
    state = {"focus": "first"}
    monkeypatch.setattr(actions, "_selected_labels", lambda _: [state["focus"]])
    monkeypatch.setattr(actions, "_press_key", lambda *_: state.update(focus="target"))
    actions._focus_label(object(), "target", "down", 1)


@pytest.mark.parametrize("initial,capture_fails", [(1, False), (2, False), (2, True)])
def test_catalog_moves_at_either_boundary_and_restores_selection(
    actions, monkeypatch, tmp_path, initial, capture_fails
):
    state = {"selection": initial, "open": False}

    class Games:
        accessible_enabled = True
        accessible_description = "2 games"

        @property
        def accessible_value(self):
            return str(state["selection"])

    def press(_, key):
        if key == "\n":
            state["open"] = True
        elif key == "\uf729":
            state["open"] = False
        elif key == "\uf700":
            state["selection"] = max(1, state["selection"] - 1)
        elif key == "\uf701":
            state["selection"] = min(2, state["selection"] + 1)

    def capture(*_):
        assert state["selection"] != initial
        if capture_fails:
            raise RuntimeError("capture failed")

    monkeypatch.setattr(actions, "_press_key", press)
    monkeypatch.setattr(actions, "_focus_label", lambda *_: None)
    monkeypatch.setattr(actions, "_exists", lambda *_: state["open"])
    monkeypatch.setattr(actions, "one_element", lambda *_: Games())
    monkeypatch.setattr(actions, "screenshot", capture)
    if capture_fails:
        with pytest.raises(RuntimeError, match="capture failed"):
            actions.launcher_catalog(object(), tmp_path / "catalog.png")
    else:
        actions.launcher_catalog(object(), tmp_path / "catalog.png")
    assert state == {"selection": initial, "open": False}
