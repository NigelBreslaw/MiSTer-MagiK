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
        "dropped_frames": 0,
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
        ("dropped_frames", 1),
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


def test_navigation_retains_transition_when_live_capture_changes(monkeypatch, tmp_path):
    state = {"open": False, "reads": 0, "captures": 0}

    class Element:
        @property
        def accessible_description(self):
            state["reads"] += 1
            return "Transitioning" if state["reads"] == 1 else "Ready"

    class Agent:
        def capture_framebuffer(self):
            state["captures"] += 1
            raise actions.AgentError(
                "capture-frame-changed: scanout changed during capture"
            )

    def open_settings(_):
        state["open"] = True
        return actions.time.monotonic()

    monkeypatch.setattr(actions, "_open_settings_card", open_settings)
    monkeypatch.setattr(
        actions, "_settings_element", lambda _: Element() if state["open"] else None
    )
    monkeypatch.setattr(actions, "_settings_open", lambda _: state["open"])
    monkeypatch.setattr(actions, "_press_key", lambda *_: state.update(open=False))
    monkeypatch.setattr(actions, "screenshot", lambda *_: None)
    monkeypatch.setattr(actions.time, "sleep", lambda _: None)
    result = actions.launcher_navigation(
        object(), tmp_path / "settings.png", agent=Agent()
    )
    assert result["transition_observed"]
    assert (
        result["opening_capture_unavailable"]
        == "scanout changed during animation; no retry"
    )
    assert state["captures"] == 1 and not state["open"]


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
    monkeypatch.setattr(actions, "_settings_ready", lambda _: state["open"])
    monkeypatch.setattr(
        actions,
        "_settings_element",
        lambda _: (
            type("Settings", (), {"accessible_description": "Ready"})()
            if state["open"]
            else None
        ),
    )
    monkeypatch.setattr(
        actions,
        "_open_settings_card",
        lambda app: (press(app, "\n"), actions.time.monotonic())[1],
    )
    monkeypatch.setattr(actions, "screenshot", capture)
    with pytest.raises(RuntimeError, match="capture failed"):
        actions.launcher_navigation(object(), tmp_path / "settings.png")
    assert not state["open"]
    assert keys[-1] == "\x1b"


@pytest.mark.parametrize("initial", [0, 2, 5])
def test_settings_navigation_observes_selection_and_retries_unaccepted_keys(
    monkeypatch, initial
):
    keys = []
    cards = ["Arcade", "Consoles", "Computers", "Handhelds", "Favourites", "Settings"]
    selected = initial
    attempts = 0

    def press(_, key):
        nonlocal selected, attempts
        keys.append(key)
        if key == "\uf703":
            attempts += 1
            if attempts > 1:  # First input arrived while the spring was settling.
                selected = (selected + 1) % len(cards)
        if key == "\n":
            assert selected == 5

    monkeypatch.setattr(actions, "_press_key", press)
    monkeypatch.setattr(actions, "_settings_open", lambda _: False)
    monkeypatch.setattr(actions, "_selected_labels", lambda _: [cards[selected]])
    monkeypatch.setattr(actions.time, "sleep", lambda _: None)
    actions._open_settings_card(object())
    assert keys[0] == "\uf729"
    assert keys[-1] == "\n"
    assert selected == 5


def test_settings_navigation_is_bounded_when_input_never_advances(monkeypatch):
    keys = []
    monkeypatch.setattr(actions, "_press_key", lambda _, key: keys.append(key))
    monkeypatch.setattr(actions, "_settings_open", lambda _: False)
    monkeypatch.setattr(actions, "_selected_labels", lambda _: ["Arcade"])
    monkeypatch.setattr(actions.time, "sleep", lambda _: None)
    with pytest.raises(AssertionError, match="within 12 attempts"):
        actions._open_settings_card(object())
    assert keys.count("\uf703") == 12
    assert "\n" not in keys


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


def test_menu_focus_waits_for_navigation_ownership_to_end(monkeypatch):
    from types import SimpleNamespace

    selected = ["Settings"]
    collection = SimpleNamespace(accessible_description="Transitioning")
    checks = 0
    keys = []

    def one(_, label):
        assert label == "Collections"
        return collection

    def wait(predicate, _):
        nonlocal checks
        assert not predicate()
        checks += 1
        collection.accessible_description = "Ready"
        assert predicate()

    def press(_, key):
        assert collection.accessible_description == "Ready"
        keys.append(key)
        selected[:] = ["Arcade"]

    monkeypatch.setattr(actions, "_settings_open", lambda _: False)
    monkeypatch.setattr(actions, "_exists", lambda _, label: label == "Collections")
    monkeypatch.setattr(actions, "one_element", one)
    monkeypatch.setattr(actions, "_selected_labels", lambda _: list(selected))
    monkeypatch.setattr(actions, "_press_key", press)

    def wait_all(predicate, message):
        if "transition" in message and collection.accessible_description != "Ready":
            wait(predicate, message)
        else:
            assert predicate()

    monkeypatch.setattr(actions, "_wait", wait_all)
    actions._focus_label(object(), "Arcade", "right", 2)
    assert checks == 1 and keys == ["right"]


@pytest.mark.parametrize("fail_sleep", [False, True])
def test_held_carousel_requests_a_bounded_hold_and_always_releases(
    monkeypatch, fail_sleep
):
    from types import SimpleNamespace

    events = []
    request = []
    sleeps = []
    evidence = {
        **window(),
        "elapsed_ms": 8000,
        "end_ms": 10000,
        "presentations": 480,
        "card_continuous_presentations": 480,
        "forced_clock_changes": 0,
    }
    replies = iter([{"window": None}, {"sha256": "app", "window": evidence}])
    agent = SimpleNamespace(
        expected_sha256="app",
        metrics=lambda: next(replies),
        _successful=lambda op, fields: request.append((op, fields)),
    )
    application = SimpleNamespace(
        first_window=SimpleNamespace(dispatch_event=events.append)
    )
    monkeypatch.setattr(actions, "_press_key", lambda *_: None)
    monkeypatch.setattr(actions, "_wait", lambda *_: None)

    def sleep(seconds):
        sleeps.append(seconds)
        if fail_sleep and seconds > 1:
            raise RuntimeError("measurement interrupted")

    if fail_sleep:
        with pytest.raises(RuntimeError, match="measurement interrupted"):
            actions.launcher_motion(
                application, agent, held_direction=True, sleep=sleep
            )
    else:
        result = actions.launcher_motion(
            application, agent, held_direction=True, sleep=sleep
        )
        assert result["held_measurement_ms"] == 8000
        assert result["input_events"] == 2
    assert events == []
    assert request[0][1]["launcher_hold"] is True
    assert request[1] == ("measure", {"launcher_hold": "release"})
    assert request[0][1]["duration_ms"] == 8000
    assert sleeps == [1, 10.4]


def test_taps_then_hold_is_device_timed_and_always_releases(monkeypatch):
    from types import SimpleNamespace

    request = []
    sleeps = []
    keys = []
    evidence = {
        **window(),
        "elapsed_ms": 12_500,
        "end_ms": 14_500,
        "presentations": 750,
        "forced_clock_changes": 0,
    }
    replies = iter([{"window": None}, {"sha256": "app", "window": evidence}])
    agent = SimpleNamespace(
        expected_sha256="app",
        metrics=lambda: next(replies),
        _successful=lambda op, fields: request.append((op, fields)),
    )
    application = SimpleNamespace(first_window=SimpleNamespace())
    monkeypatch.setattr(actions, "_press_key", lambda _, key: keys.append(key))
    monkeypatch.setattr(actions, "_wait", lambda *_: None)

    result = actions.launcher_motion(
        application, agent, taps_then_hold=True, sleep=sleeps.append
    )

    # Only the Home key is sent from the host; taps and the hold are device-timed.
    assert keys == [""]
    assert request[0][1]["launcher_sequence"] == {
        "taps": 6,
        "tap_interval_ms": 250,
        "hold_ms": 10_000,
    }
    assert request[0][1]["launcher_hold"] is False
    assert request[0][1]["duration_ms"] == 12_500
    assert request[1] == ("measure", {"launcher_hold": "release"})
    assert sleeps == [1, 14.9]
    assert result["workload"] == "launcher-card-motion-taps-then-hold"
    assert result["input_events"] == 7
    with pytest.raises(ValueError):
        actions.launcher_motion(
            application, agent, taps_then_hold=True, instrumented=True
        )


def test_motion_waits_boundedly_for_a_window_that_started_late(monkeypatch):
    from types import SimpleNamespace

    evidence = {
        **window(),
        "elapsed_ms": 12_500,
        "end_ms": 14_500,
        "presentations": 750,
        "forced_clock_changes": 0,
    }
    replies = iter(
        [{"window": None}, {"sha256": "app", "window": None}]
        + [{"sha256": "app", "window": evidence}]
    )
    sleeps = []
    agent = SimpleNamespace(
        expected_sha256="app",
        metrics=lambda: next(replies),
        _successful=lambda *_: None,
    )
    monkeypatch.setattr(actions, "_press_key", lambda *_: None)
    monkeypatch.setattr(actions, "_wait", lambda *_: None)
    result = actions.launcher_motion(
        SimpleNamespace(first_window=SimpleNamespace()),
        agent,
        taps_then_hold=True,
        sleep=sleeps.append,
    )
    assert result["elapsed_ms"] == 12_500
    assert sleeps == [1, 14.9, 0.25]


@pytest.mark.parametrize("workload", ["held_direction", "taps_then_hold"])
def test_hold_release_waits_for_a_late_window_to_complete(monkeypatch, workload):
    from types import SimpleNamespace

    calls = []
    late = [{"sha256": "app", "window": None}] * 8
    elapsed = 12_500 if workload == "taps_then_hold" else 8_000
    evidence = {
        **window(),
        "elapsed_ms": elapsed,
        "end_ms": elapsed + 2_000,
        "presentations": elapsed * 60 // 1000,
        "card_continuous_presentations": elapsed * 60 // 1000,
        "forced_clock_changes": 0,
    }
    replies = iter([{"window": None}, *late, {"sha256": "app", "window": evidence}])

    def metrics():
        calls.append("metrics")
        return next(replies)

    agent = SimpleNamespace(
        expected_sha256="app",
        metrics=metrics,
        _successful=lambda op, fields: calls.append(fields.get("launcher_hold")),
    )
    monkeypatch.setattr(actions, "_press_key", lambda *_: None)
    monkeypatch.setattr(actions, "_wait", lambda *_: None)
    actions.launcher_motion(
        SimpleNamespace(first_window=SimpleNamespace()),
        agent,
        sleep=lambda _: None,
        **{workload: True},
    )
    # A two-second pickup delay must not cut the hold short: the release is
    # sent only after the completed window has been read.
    assert calls[-1] == "release"
    assert calls[-2] == "metrics"
    assert calls.count("metrics") == 1 + len(late) + 1
