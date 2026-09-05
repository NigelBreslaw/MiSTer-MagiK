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

    first = ensure_arm_probe(probe, cache, runner=runner)
    second = ensure_arm_probe(probe, cache, runner=runner)

    assert first.rebuilt and not second.rebuilt
    assert len(calls) == 1
    assert calls[0][:3] == ["container", "exec", "magik2-arm-build"]


def test_prebuilt_artifact_bypasses_compilation(monkeypatch, tmp_path) -> None:
    artifact = tmp_path / "prebuilt-probe"
    artifact.write_bytes(b"prebuilt")
    monkeypatch.setenv("MISTER_MAGIK2_PREBUILT_ARTIFACT", str(artifact))

    result = ensure_arm_probe(tmp_path / "probe", tmp_path / "cache.json")

    assert result.artifact == artifact
    assert result.prebuilt and not result.rebuilt
