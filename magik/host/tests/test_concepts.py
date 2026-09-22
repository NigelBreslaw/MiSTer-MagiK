from magik.concepts import validate
import pytest


def sample():
    return {
        "sha256": "abc",
        "window": dict(
            width=960,
            height=540,
            instrumented=False,
            start_ms=2000,
            end_ms=32000,
            elapsed_ms=30000,
            context={"concept": "diagnostic", "preset": "default", "route": "hdmi"},
            process_cpu_percent=75,
            peak_rss_bytes=4_000_000,
            refresh_hz=60,
            drop_baseline_available=True,
            presentations=1800,
            physical_latch_posts=1800,
            physical_latch_flips=1800,
            presented_vblanks=1800,
            owned_vblanks=1800,
            physical_drops=0,
            latch_drops=0,
            latch_rejections=0,
        ),
    }


def test_valid_window():
    assert validate(sample(), "abc", "diagnostic", "default")["qualified"]


@pytest.mark.parametrize(
    "key,value",
    [
        ("physical_drops", 1),
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
