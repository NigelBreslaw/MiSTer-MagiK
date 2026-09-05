from __future__ import annotations

from magik2.build import needs_build, source_fingerprint, write_build_cache


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
