import importlib.util
from pathlib import Path
import pytest

spec = importlib.util.spec_from_file_location(
    "probe_actions", Path(__file__).resolve().parents[2] / "scenarios/actions.py"
)
actions = importlib.util.module_from_spec(spec)
spec.loader.exec_module(actions)


def window():
    return {
        "start_ms": 2000,
        "end_ms": 7000,
        "elapsed_ms": 5000,
        "width": 960,
        "height": 540,
        "presentations": 300,
        "render_us_total": 1000,
        "render_to_present_us_total": 5000000,
        "physical_latch_posts": 300,
        "physical_latch_flips": 300,
        "physical_drops": 0,
        "latch_rejections": 0,
        "drop_baseline_available": True,
        "instrumented": False,
        "evidence_error": None,
    }


def test_device_window_is_validated():
    assert actions.validate_window(window(), instrumented=False, seconds=5)[
        "physical_evidence_valid"
    ]


@pytest.mark.parametrize(
    "key,value",
    [
        ("physical_drops", 1),
        ("latch_rejections", 1),
        ("drop_baseline_available", False),
        ("evidence_error", "unavailable"),
        ("instrumented", True),
        ("elapsed_ms", 100),
    ],
)
def test_missing_or_invalid_physical_evidence_fails(key, value):
    evidence = window()
    evidence[key] = value
    with pytest.raises(AssertionError):
        actions.validate_window(evidence, instrumented=False, seconds=5)


def test_development_paths_reject_production_or_missing_evidence():
    root = "/media/fat/mister-magik-dev"
    context = {"data_root": root, "main": "/media/fat/MiSTer_MagiKDev"}
    context.update(
        {
            name: f"{root}/{name}"
            for name in (
                "settings",
                "controllers",
                "catalog",
                "library",
                "user_state",
                "assets",
            )
        }
    )
    assert actions.validate_development_paths(context) == context
    for bad in (
        None,
        {},
        {**context, "assets": "/media/fat/mister-magik/assets"},
        {**context, "assets": f"{root}/../mister-magik/assets"},
        {**context, "main": "/media/fat/MiSTer_MagiK"},
    ):
        with pytest.raises(AssertionError):
            actions.validate_development_paths(bad)


def test_idle_cannot_reuse_a_previous_completed_window(monkeypatch):
    from types import SimpleNamespace

    evidence = {"sha256": "app", "pid": 1, "window": window()}
    agent = SimpleNamespace(
        expected_sha256="app", metrics=lambda: evidence, _successful=lambda _: None
    )
    monkeypatch.setattr(actions.time, "sleep", lambda _: None)
    with pytest.raises(AssertionError, match="previous window"):
        actions.launcher_idle(SimpleNamespace(first_window=object()), agent)


def test_navigation_returns_from_settings_when_capture_fails(monkeypatch, tmp_path):
    state = {"open": False}
    keys = []

    def press(_, key):
        keys.append(key)
        if key == "\n":
            state["open"] = True
        elif key in {"\x1b", "\uf729"}:
            state["open"] = False

    def capture(*_):
        raise RuntimeError("capture failed")

    monkeypatch.setattr(actions, "_press_key", press)
    monkeypatch.setattr(actions, "_settings_open", lambda _: state["open"])
    monkeypatch.setattr(actions, "screenshot", capture)
    with pytest.raises(RuntimeError, match="capture failed"):
        actions.launcher_navigation(object(), tmp_path / "settings.png")
    assert not state["open"]
    assert keys[-1] == "\x1b"


def test_settings_button_is_not_mistaken_for_the_open_screen():
    from types import SimpleNamespace

    elements = [
        SimpleNamespace(
            accessible_label="Settings", accessible_role=SimpleNamespace(name="Button")
        )
    ]
    query = SimpleNamespace(match_inherits=lambda _: query, find_all=lambda: elements)
    app = SimpleNamespace(
        first_window=SimpleNamespace(
            root_element=SimpleNamespace(query_descendants=lambda: query)
        )
    )
    assert not actions._settings_open(app)
    elements.append(
        SimpleNamespace(
            accessible_label="Settings", accessible_role=SimpleNamespace(name="Main")
        )
    )
    assert actions._settings_open(app)
