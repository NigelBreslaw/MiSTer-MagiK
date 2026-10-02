from magik.concepts import validate
import pytest


def sample():
    return {
        "sha256": "abc",
        "motion_started_ms": 2000,
        "window": dict(
            width=960,
            height=540,
            instrumented=False,
            start_ms=2000,
            end_ms=32000,
            elapsed_ms=30000,
            context={
                "concept": "diagnostic",
                "preset": "default",
                "route": "hdmi",
                "animation_clock": "frame",
                "animation_period_ns": 16_666_667,
                "animation_window_start_ms": 0,
                "animation_elapsed_ms": 30000,
            },
            process_cpu_percent=75,
            peak_rss_bytes=4_000_000,
            refresh_hz=60,
            drop_baseline_available=True,
            presentations=1800,
            physical_latch_posts=1800,
            physical_latch_flips=1800,
            presented_vblanks=1800,
            owned_vblanks=1800,
            dropped_frames=0,
            latch_drops=0,
            latch_rejections=0,
        ),
    }


def test_short_window_requires_explicit_duration_and_preserves_drop_gate():
    data = sample()
    w = data["window"]
    w.update(
        elapsed_ms=10000,
        end_ms=12000,
        presentations=600,
        physical_latch_posts=600,
        physical_latch_flips=600,
        owned_vblanks=600,
        presented_vblanks=600,
    )
    assert validate(data, "abc", "diagnostic", "default", quick=True)["qualified"]
    with pytest.raises(ValueError, match="boundaries"):
        validate(data, "abc", "diagnostic", "default")
    w["dropped_frames"] = 1
    assert not validate(data, "abc", "diagnostic", "default", quick=True)["qualified"]


def test_valid_window():
    assert validate(sample(), "abc", "diagnostic", "default")["qualified"]


@pytest.mark.parametrize(
    "key,value",
    [
        ("dropped_frames", 1),
        ("latch_drops", 1),
        ("process_cpu_percent", 150),
        ("peak_rss_bytes", 134217729),
        ("presented_vblanks", 1799),
    ],
)
def test_failed_gates_retained(key, value):
    data = sample()
    data["window"][key] = value
    assert not validate(data, "abc", "diagnostic", "default")["qualified"]


def test_unknown_evidence_is_not_zero():
    data = sample()
    data["window"]["process_cpu_percent"] = None
    with pytest.raises(ValueError):
        validate(data, "abc", "diagnostic", "default")


def test_identity_mismatch():
    with pytest.raises(ValueError):
        validate(sample(), "wrong", "diagnostic", "default")


@pytest.mark.parametrize(
    "field",
    [
        "width",
        "height",
        "process_cpu_percent",
        "peak_rss_bytes",
        "refresh_hz",
        "drop_baseline_available",
    ],
)
def test_required_evidence_cannot_be_omitted(field):
    data = sample()
    del data["window"][field]
    with pytest.raises(ValueError):
        validate(data, "abc", "diagnostic", "default")


def test_profile_cannot_qualify():
    data = sample()
    data["window"].update(instrumented=True, elapsed_ms=10000, end_ms=12000)
    assert not validate(data, "abc", "diagnostic", "default", profile=True)["qualified"]


def test_accessible_actions_reuse_stable_handles(monkeypatch):
    from magik import concepts
    from unittest.mock import Mock

    app = Mock()
    handle = Mock(accessible_value="17")
    lookup = Mock(return_value=handle)
    monkeypatch.setattr(concepts, "one_element", lookup)
    assert concepts.value(app, "frame") == "17"
    assert concepts.value(app, "frame") == "17"
    lookup.assert_called_once_with(app, "concept-frame")


def test_same_name_preset_selection_waits_for_new_generation(monkeypatch):
    from magik import concepts
    from unittest.mock import Mock

    generations = iter(["7", "7", "8"])
    monkeypatch.setattr(
        concepts,
        "value",
        lambda _, name: next(generations) if name == "generation" else "",
    )
    invoke = Mock()
    monkeypatch.setattr(concepts, "action", invoke)
    concepts.select(object(), "light-sweep", "reduced")
    invoke.assert_called_once_with(
        invoke.call_args.args[0], "select-light-sweep-reduced"
    )


@pytest.mark.parametrize(
    "effect", ["launcher-cards", "arcade-transition", "settings-transition"]
)
@pytest.mark.parametrize("preset", ["default"])
def test_render_labs_require_production_build_for_qualification(effect, preset):
    data = sample()
    data["window"]["context"].update(
        concept=effect, preset=preset, build_profile="release"
    )
    assert not validate(data, "abc", effect, preset)["qualified"]
    data["window"]["context"]["build_profile"] = "release-device"
    assert validate(data, "abc", effect, preset)["qualified"]
    data["window"]["dropped_frames"] = 1
    assert not validate(data, "abc", effect, preset)["qualified"]


def test_rendering_presets_are_scoped_to_rendering_labs():
    from magik.concepts import supported

    assert supported("launcher-cards", "default")
    assert supported("arcade-transition", "default")
    assert not supported("launcher-cards", "reduced")
    assert supported("starfield", "default")
    assert supported("starfield", "reduced")


def test_production_build_is_mini_only(monkeypatch):
    from magik.apps import application

    monkeypatch.delenv("MAGIK_MINI_PRODUCTION_BUILD", raising=False)
    assert application("mini-magik").profile == "release"
    full_profile = application("magik").profile
    monkeypatch.setenv("MAGIK_MINI_PRODUCTION_BUILD", "1")
    assert application("mini-magik").profile == "release-device"
    assert application("magik").profile == full_profile


def test_lab_requires_full_window_cadence_and_exact_geometry():
    data = sample()
    data["window"]["context"].update(
        concept="launcher-cards", build_profile="release-device"
    )
    for field in (
        "presentations",
        "physical_latch_posts",
        "physical_latch_flips",
        "presented_vblanks",
        "owned_vblanks",
    ):
        data["window"][field] = 900
    assert not validate(data, "abc", "launcher-cards", "default")["qualified"]
    data["window"]["height"] = 600
    with pytest.raises(ValueError, match="geometry"):
        validate(data, "abc", "launcher-cards", "default")


@pytest.mark.parametrize("preset", ["rgb888", "scanline"])
def test_retired_rendering_presets_are_unavailable(preset):
    from magik.concepts import supported

    assert not supported("arcade-transition", preset)
    assert not supported("launcher-cards", preset)
    assert not supported("starfield", preset)


@pytest.mark.parametrize("phase", [15000, 29900, 30000, 30100, 31000, None])
def test_motion_cannot_qualify_with_a_stretched_animation_clock(phase):
    data = sample()
    data["window"]["context"].update(
        concept="launcher-cards",
        build_profile="release-device",
        animation_elapsed_ms=phase,
    )
    result = validate(data, "abc", "launcher-cards", "default")
    assert result["qualified"] is (phase is not None and 29900 <= phase <= 30100)


@pytest.mark.parametrize("field", ["running", "ready", "artifact", "running_sha256"])
def test_installed_concept_requires_the_requested_ready_artifact(field):
    from magik.concepts import verify_installed

    fields = dict(
        running=True, ready=True, artifact="mini-magik", running_sha256="a" * 64
    )
    verify_installed(fields, "a" * 64)
    fields[field] = None
    with pytest.raises(ValueError, match="artifact"):
        verify_installed(fields, "a" * 64)


def test_motion_evidence_accounts_for_device_warmup():
    data = sample()
    data["window"]["context"].update(
        concept="launcher-cards",
        build_profile="release-device",
        animation_window_start_ms=2000,
        animation_elapsed_ms=32000,
    )
    assert validate(data, "abc", "launcher-cards", "default")["motion_qualified"]


def test_motion_follows_rendered_frames_not_wall_time():
    # 59.5 Hz for 30 s is 1,785 frames and 29,750 ms of animation: valid.
    data = sample()
    w = data["window"]
    w.update(
        refresh_hz=59.5,
        presentations=1785,
        physical_latch_posts=1785,
        physical_latch_flips=1785,
        presented_vblanks=1785,
        owned_vblanks=1785,
    )
    w["context"].update(
        concept="launcher-cards",
        build_profile="release-device",
        animation_elapsed_ms=29750,
    )
    fps = 1785 * 1000 / 30000
    assert abs(fps - 59.5) <= 0.1
    assert validate(data, "abc", "launcher-cards", "default")["qualified"]
    # The same frames with animation that fell behind them are not.
    w["context"]["animation_elapsed_ms"] = 29750 - 200
    assert not validate(data, "abc", "launcher-cards", "default")["motion_qualified"]
    # A missing frame period or window start cannot qualify.
    for key in ("animation_period_ns", "animation_window_start_ms"):
        w["context"]["animation_elapsed_ms"] = 29750
        w["context"].pop(key)
        assert not validate(data, "abc", "launcher-cards", "default")["motion_qualified"]
        w["context"][key] = 16_666_667 if key == "animation_period_ns" else 0


@pytest.mark.parametrize("sampled", [True, False])
@pytest.mark.parametrize("identity_matches", [True, False])
def test_preparation_profile_waits_for_attributed_current_artifact(
    tmp_path, monkeypatch, identity_matches, sampled
):
    from magik import concepts
    from unittest.mock import Mock

    app = Mock()
    agent = Mock(expected_sha256="a" * 64)
    raw = {
        "sha256": "a" * 64 if identity_matches else "b" * 64,
        "evidence_error": None,
        "latch_rejections": 0,
        "presentations": 2,
        "physical_latch_posts": 2,
        "physical_latch_flips": 2,
        "context": {
            "concept": "launcher-cards",
            "preset": "default",
            "build_profile": "release-device",
            "preparation_ms": 2026,
            "preparation_profile": {"complete": True, "process_cpu_us": 2_000_000},
            "startup": {"preparation_to_first_confirmed_present_us": 2_050_000},
            "concept_generation": 7,
            "preparation_benchmark": True,
        },
    }
    import copy

    stale = copy.deepcopy(raw)
    stale["context"]["concept_generation"] = 6
    agent.metrics.side_effect = [
        {"context": None},
        {"context": {"preparation_profile": None}},
        stale,
        raw,
    ]
    order = []
    monkeypatch.setattr(concepts, "value", lambda app, name: "7")
    captured = Mock()
    monkeypatch.setattr(concepts, "capture", captured)
    monkeypatch.setattr(concepts, "action", lambda app, name: order.append(name))
    monkeypatch.setattr(
        concepts, "select", lambda app, name, preset: order.append((name, preset))
    )
    if identity_matches:
        assert (
            concepts.profile_preparation(
                app, agent, tmp_path, "launcher-cards", "default", sampled=sampled
            )
            == 0
        )
        assert (tmp_path / "preparation-raw.json").exists()
    else:
        with pytest.raises(ValueError, match="identity"):
            concepts.profile_preparation(
                app, agent, tmp_path, "launcher-cards", "default", sampled=sampled
            )
        assert not (tmp_path / "preparation-raw.json").exists()
    assert order == [
        "profile-preparation" if sampled else "bench-preparation",
        ("launcher-cards", "default"),
    ]
    assert captured.call_count == (1 if identity_matches and not sampled else 0)
