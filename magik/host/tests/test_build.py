from __future__ import annotations

import json
from types import SimpleNamespace

import pytest

from magik.build import TARGET, ensure_arm_application


@pytest.fixture
def build_case(tmp_path, monkeypatch):
    monkeypatch.delenv("MISTER_MAGIK2_PREBUILT_ARTIFACT", raising=False)
    package = tmp_path / "magik/probe"
    (package / "src").mkdir(parents=True)
    artifact = package / "target" / TARGET / "release/mini-magik"
    artifact.parent.mkdir(parents=True)
    artifact.write_bytes(b"old binary")
    record = {
        "reason": "compiler-artifact",
        "target": {"name": "mini-magik", "kind": ["bin"]},
        "executable": f"/workspace/magik/probe/target/{TARGET}/release/mini-magik",
        "fresh": False,
    }
    return package, artifact, record


def test_cargo_decides_freshness_even_with_existing_binary_and_cache(
    build_case, monkeypatch
):
    package, artifact, record = build_case
    monkeypatch.setenv("RUSTFLAGS", "-C debuginfo=1")
    cache = artifact.parents[2] / "magik-build.json"
    cache.write_text('{"fingerprint":"old","artifact":"cached"}')
    calls = []

    def runner(command, *, check, stdout):
        calls.append(command)
        stdout.write(json.dumps(record) + "\n")
        return SimpleNamespace(returncode=0)

    def build():
        return ensure_arm_application(
            package, runner=runner, prepare=lambda *_: "builder"
        )

    assert build().rebuilt
    record["fresh"] = True
    assert not build().rebuilt
    for name in ("kernel.c", "kernel.h"):
        (package / "src" / name).write_text("changed native input")
        record["fresh"] = False
        assert build().rebuilt
    assert len(calls) == 4
    assert all("--message-format=json-render-diagnostics" in call for call in calls)
    assert all("RUSTFLAGS=-C debuginfo=1" in call for call in calls)
    assert cache.read_text() == '{"fingerprint":"old","artifact":"cached"}'


@pytest.mark.parametrize(
    "failure", ["exit", "missing_record", "other_binary", "other_path", "missing_file"]
)
def test_failed_build_never_returns_a_stale_binary(build_case, failure, capsys):
    package, artifact, record = build_case
    if failure == "other_binary":
        record["target"]["name"] = "dependency"
    if failure == "other_path":
        record["executable"] = "/workspace/another/mini-magik"
    if failure == "missing_file":
        artifact.unlink()

    def runner(command, *, check, stdout):
        stdout.write(
            json.dumps(
                {
                    "reason": "compiler-message",
                    "message": {"rendered": "error: native build failed\n"},
                }
            )
            + "\n"
        )
        if failure != "missing_record":
            stdout.write(json.dumps(record) + "\n")
        return SimpleNamespace(returncode=1 if failure == "exit" else 0)

    with pytest.raises(RuntimeError, match="build failed"):
        ensure_arm_application(package, runner=runner, prepare=lambda *_: "builder")
    assert "error: native build failed" in capsys.readouterr().err


def test_dependency_freshness_does_not_override_selected_binary(build_case):
    package, artifact, record = build_case
    record["fresh"] = True

    def runner(command, *, check, stdout):
        stdout.write(json.dumps(record) + "\n")
        stdout.write(
            json.dumps(
                {
                    **record,
                    "target": {"name": "dependency", "kind": ["lib"]},
                    "executable": None,
                    "fresh": False,
                }
            )
            + "\n"
        )
        return SimpleNamespace(returncode=0)

    result = ensure_arm_application(
        package, runner=runner, prepare=lambda *_: "builder"
    )
    assert result.artifact == artifact
    assert not result.rebuilt


def test_prebuilt_artifact_bypasses_compilation(monkeypatch, tmp_path):
    artifact = tmp_path / "prebuilt-probe"
    artifact.write_bytes(b"prebuilt")
    monkeypatch.setenv("MISTER_MAGIK2_PREBUILT_ARTIFACT", str(artifact))
    result = ensure_arm_application(tmp_path / "probe")
    assert result.artifact == artifact
    assert result.prebuilt and not result.rebuilt


def test_real_launcher_sampler_feature_is_explicit_and_mini_is_unchanged(monkeypatch):
    from magik.apps import application

    monkeypatch.delenv("MAGIK_CARD_SAMPLER_AB", raising=False)
    baseline = application("magik")
    assert "card-axis-filter" not in baseline.features
    monkeypatch.setenv("MAGIK_CARD_SAMPLER_AB", "axis")
    candidate = application("magik")
    assert candidate.features == (*baseline.features, "card-axis-filter")
    assert candidate.profile == baseline.profile
    assert candidate.agent_capabilities == baseline.agent_capabilities
    assert "card-axis-filter" not in application("mini-magik").features
    monkeypatch.setenv("MAGIK_CARD_SAMPLER_AB", "invalid")
    with pytest.raises(ValueError, match="current or axis"):
        application("magik")


def test_quantiser_and_sampler_features_are_independent(monkeypatch):
    from magik.apps import application

    monkeypatch.delenv("MAGIK_CARD_SAMPLER_AB", raising=False)
    monkeypatch.setenv("MAGIK_CARD_QUANTISER", "fast")
    quantiser_only = application("magik")
    assert "card-fast-quantisation" in quantiser_only.features
    assert "card-axis-filter" not in quantiser_only.features
    monkeypatch.setenv("MAGIK_CARD_SAMPLER_AB", "axis")
    candidate = application("magik")
    assert "card-axis-filter" in candidate.features
    assert "card-fast-quantisation" in candidate.features
    assert "card-fast-quantisation" not in application("mini-magik").features
    monkeypatch.setenv("MAGIK_CARD_QUANTISER", "invalid")
    with pytest.raises(ValueError, match="current or fast"):
        application("magik")


@pytest.mark.parametrize("sampler", ["current", "axis"])
@pytest.mark.parametrize("quantiser", ["current", "fast"])
def test_renderer_context_requires_actual_compiled_features(
    monkeypatch, sampler, quantiser
):
    from magik.apps import validate_renderer_context

    monkeypatch.setenv("MAGIK_CARD_SAMPLER_AB", sampler)
    monkeypatch.setenv("MAGIK_CARD_QUANTISER", quantiser)
    context = {
        "card_sampler": "independent-vertical-prefilter"
        if sampler == "axis"
        else "current",
        "card_quantiser": "centred-bayer-shifts"
        if quantiser == "fast"
        else "existing-bayer",
    }
    validate_renderer_context(context)
    for field in context:
        with pytest.raises(AssertionError, match="does not match"):
            validate_renderer_context({**context, field: "wrong"})
    with pytest.raises(AssertionError, match="does not match"):
        validate_renderer_context({})


def test_unset_renderer_selectors_require_baseline_evidence(monkeypatch):
    from magik.apps import validate_renderer_context

    monkeypatch.delenv("MAGIK_CARD_SAMPLER_AB", raising=False)
    monkeypatch.delenv("MAGIK_CARD_QUANTISER", raising=False)
    validate_renderer_context(
        {"card_sampler": "current", "card_quantiser": "existing-bayer"}
    )
    with pytest.raises(AssertionError, match="does not match"):
        validate_renderer_context(
            {"card_sampler": "current", "card_quantiser": "centred-bayer-shifts"}
        )
