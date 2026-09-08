"""Bounded workload delivery and one shared, versioned result contract."""

from __future__ import annotations

import hashlib
import json
import math
import re
import time

from .apps import repository
from .build import ensure_arm_application
from .results import append_event


def positive(value):
    return type(value) in (int, float) and math.isfinite(value) and value > 0


def validate_result(value, *, workload, mode, sha256):
    if not isinstance(value, dict):
        raise ValueError("benchmark result must be an object")
    for key, expected in (
        ("schema_version", 1),
        ("workload", workload),
        ("mode", mode),
        ("artifact_sha256", sha256),
        ("correctness", "passed"),
    ):
        if value.get(key) != expected:
            raise ValueError(f"benchmark result mismatch: {key}")
    if mode not in ("timing", "visual", "pmu-neon", "pmu-memory"):
        raise ValueError("invalid benchmark mode")
    if not isinstance(sha256, str) or not re.fullmatch("[0-9a-f]{64}", sha256):
        raise ValueError("invalid artifact hash")
    fixture = value.get("fixture", {})
    if not isinstance(fixture, dict):
        raise ValueError("invalid fixture")
    identity = fixture.get("identity", "")
    if not isinstance(identity, str) or not re.fullmatch("[0-9a-f]{64}", identity):
        raise ValueError("missing fixture identity")
    count = value.get("work_count")
    if type(count) is not int or count <= 0:
        raise ValueError("invalid work count")
    samples = value.get("samples")
    expected_count = 0 if mode == "visual" else 2 if mode == "timing" else 1
    if not isinstance(samples, list) or len(samples) != expected_count:
        raise ValueError("incorrect sample count")
    for index, sample in enumerate(samples):
        if not isinstance(sample, dict):
            raise ValueError("invalid sample")
        if (
            sample.get("repetition") != index
            or sample.get("fixture_identity") != identity
            or sample.get("work_count") != count
        ):
            raise ValueError("sample identity or work count mismatch")
        if not positive(sample.get("duration_ns")) or not positive(
            sample.get("ns_per_pixel")
        ):
            raise ValueError("invalid timing")
        if not math.isclose(
            sample["ns_per_pixel"], sample["duration_ns"] / count, rel_tol=1e-9
        ):
            raise ValueError("inconsistent normalized timing")
        if mode.startswith("pmu-"):
            from .benchmark_compare import validate_pmu

            validate_pmu(sample.get("pmu"), mode)
        if mode == "timing" and sample.get("pmu") is not None:
            raise ValueError("instrumented sample in timing result")
    if mode == "visual":
        visual = value.get("visual", {})
        if not isinstance(visual, dict):
            raise ValueError("invalid visual result")
        if not positive(visual.get("duration_ms")) or not positive(
            visual.get("presentations")
        ):
            raise ValueError("incomplete visual result")
        if visual.get("rejections") != 0 or visual.get("physical_drops") != 0:
            raise ValueError("visual presentation failed")
    elif value.get("visual") is not None:
        raise ValueError("presentation data in kernel benchmark")
    return value


def run_benchmark(arguments, run):
    from .cli import connect_agent

    started = time.monotonic()
    mode = (
        "visual"
        if arguments.visual
        else f"pmu-{arguments.counters}"
        if arguments.counters
        else "timing"
    )
    agent, status = connect_agent(run, {"status", "upload-v1", "run-benchmark-v2"})
    package = repository() / "magik/probe"
    phase = time.monotonic()
    built = ensure_arm_application(package)
    append_event(
        run,
        {
            "phase": "build",
            "elapsed_ms": (time.monotonic() - phase) * 1000,
            "reused": not built.rebuilt,
            "prebuilt": built.prebuilt,
        },
    )
    payload = built.artifact.read_bytes()
    digest = hashlib.sha256(payload).hexdigest()
    append_event(run, {"phase": "artifact", "sha256": digest})
    phase = time.monotonic()
    skipped = status.fields.get("artifacts", {}).get("mini-magik") == digest
    if not skipped:
        agent.upload("mini-magik", payload)
    append_event(
        run,
        {
            "phase": "upload",
            "skipped": skipped,
            "elapsed_ms": (time.monotonic() - phase) * 1000,
        },
    )
    phase = time.monotonic()
    metadata, output = agent.run_benchmark(digest, arguments.workload, mode)
    (run / "benchmark-raw.json").write_bytes(output)
    (run / "benchmark-process.json").write_text(json.dumps(metadata, indent=2) + "\n")
    append_event(
        run, {"phase": "execution", "elapsed_ms": (time.monotonic() - phase) * 1000}
    )
    if (
        metadata.get("error")
        or metadata.get("exit_code") != 0
        or metadata.get("launcher_resumed") is not True
        or metadata.get("sha256") != digest
    ):
        raise RuntimeError(f"benchmark process failed: {metadata}")
    result = validate_result(
        json.loads(output), workload=arguments.workload, mode=mode, sha256=digest
    )
    result["provenance"] = {
        **json.loads((run / "run.json").read_text())["source"],
    }
    (run / "benchmark.json").write_text(json.dumps(result, indent=2) + "\n")
    for sample in result["samples"]:
        print(
            f"Run {sample['repetition'] + 1}: {sample['duration_ns'] / 1e6:.3f} ms; {sample['ns_per_pixel']:.3f} ns/pixel; correctness passed"
        )
    if mode == "visual":
        print(f"Visual pass: {result['visual']}")
    append_event(
        run,
        {
            "phase": "benchmark-complete",
            "elapsed_ms": (time.monotonic() - started) * 1000,
        },
    )
    print(f"Results: {run.resolve()}")
    return 0
