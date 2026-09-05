from __future__ import annotations

from magik2.build import ensure_arm_probe, needs_build, source_fingerprint, write_build_cache


def test_only_probe_inputs_affect_the_fingerprint(tmp_path) -> None:
    probe = tmp_path / "probe"
    (probe / "src").mkdir(parents=True)
    (probe / "src" / "main.rs").write_text("fn main() {}\n")
    baseline = source_fingerprint(probe)
    (tmp_path / "unrelated.md").write_text("dirty checkout state\n")
    assert source_fingerprint(probe) == baseline
    (probe / "src" / "main.rs").write_text("fn main() { println!(\"changed\"); }\n")
    assert source_fingerprint(probe) != baseline


def test_cache_reuses_exact_probe_inputs(tmp_path) -> None:
    cache = tmp_path / "cache.json"
    (tmp_path / "probe").write_bytes(b"probe")
    write_build_cache(cache, "fingerprint", tmp_path / "probe")
    assert not needs_build(cache, "fingerprint")
    assert needs_build(cache, "different")


def test_build_runs_only_when_a_probe_artifact_is_stale(tmp_path) -> None:
    probe = tmp_path / "probe"
    (probe / "src").mkdir(parents=True)
    (probe / "src" / "main.rs").write_text("fn main() {}\n")
    artifact = probe / "target" / "armv7-unknown-linux-gnueabihf" / "release" / "mister-magik2-probe"
    cache = tmp_path / "cache.json"
    calls: list[list[str]] = []

    class Result:
        returncode = 0

    def runner(command: list[str], *, check: bool) -> Result:
        calls.append(command)
        artifact.parent.mkdir(parents=True)
        artifact.write_bytes(b"probe")
        return Result()

    first = ensure_arm_probe(probe, cache, runner=runner, prepare=lambda *_: "test-builder")
    second = ensure_arm_probe(probe, cache, runner=runner, prepare=lambda *_: "test-builder")

    assert first.rebuilt and not second.rebuilt
    assert len(calls) == 1
    assert calls[0][:2] == ["container", "exec"]
    assert "test-builder" in calls[0]


def test_prebuilt_artifact_bypasses_compilation(monkeypatch, tmp_path) -> None:
    artifact = tmp_path / "prebuilt-probe"
    artifact.write_bytes(b"prebuilt")
    monkeypatch.setenv("MISTER_MAGIK2_PREBUILT_ARTIFACT", str(artifact))

    result = ensure_arm_probe(tmp_path / "probe", tmp_path / "cache.json")

    assert result.artifact == artifact
    assert result.prebuilt and not result.rebuilt


def test_configuration_and_dependency_inputs_but_not_outputs(tmp_path):
    probe = tmp_path / "probe"
    (probe / ".cargo").mkdir(parents=True)
    dep = tmp_path / "dep"
    (dep / "src").mkdir(parents=True)
    (dep / "Cargo.toml").write_text('[package]\nname="dep"\nversion="0.1.0"')
    shared = dep / "src/lib.rs"
    shared.write_text("// A")
    (probe / "Cargo.toml").write_text('[dependencies]\ndep={path="../dep"}')
    config = probe / ".cargo/config.toml"
    config.write_text("# A")
    first = source_fingerprint(probe)
    config.write_text("# B")
    assert source_fingerprint(probe) != first
    first = source_fingerprint(probe)
    shared.write_text("// B")
    assert source_fingerprint(probe) != first
    first = source_fingerprint(probe)
    (probe / "target").mkdir()
    (probe / "target/generated.rs").write_text("generated")
    assert source_fingerprint(probe) == first


def test_replaced_artifact_invalidates_cache(tmp_path):
    artifact = tmp_path / "probe"
    artifact.write_bytes(b"A")
    cache = tmp_path / "cache.json"
    write_build_cache(cache, "inputs", artifact)
    artifact.write_bytes(b"B")
    assert needs_build(cache, "inputs")
