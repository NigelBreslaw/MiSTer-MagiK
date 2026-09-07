"""Offline comparisons only: never launch or repeat a workload."""

import statistics

EVENTS = {
    "pmu-neon": {
        "cycles",
        "speculative-instructions",
        "neon-instructions",
        "neon-clock-cycles",
        "data-dependent-stall-cycles",
        "l1d-accesses",
        "l1d-refills",
    },
    "pmu-memory": {
        "cycles",
        "l1d-accesses",
        "l1d-refills",
        "data-dependent-stall-cycles",
        "data-evictions",
        "tlb-stall-cycles",
        "issue-stall-cycles",
    },
}


def validate_pmu(pmu, mode):
    if not isinstance(pmu, dict):
        raise ValueError("missing PMU record")
    reading = pmu.get("counters", {})
    if not isinstance(reading, dict):
        raise ValueError("invalid PMU reading")
    expected = "cortex-a9-" + mode.removeprefix("pmu-")
    if pmu.get("counter_set") != expected or reading.get("counter_set") != expected:
        raise ValueError("incorrect PMU group")
    enabled = reading.get("time_enabled_ns")
    if (
        type(enabled) is not int
        or enabled <= 0
        or reading.get("time_running_ns") != enabled
    ):
        raise ValueError("PMU counters unavailable or multiplexed")
    counts = reading.get("counters", {})
    if not isinstance(counts, dict):
        raise ValueError("invalid PMU counts")
    if (
        set(counts) != EVENTS[mode]
        or any(type(v) is not int or v < 0 for v in counts.values())
        or counts["cycles"] == 0
    ):
        raise ValueError("missing or invalid PMU counters")
    return counts


def compare(baseline, candidate):
    from .benchmark import validate_result

    for record in (baseline, candidate):
        validate_result(
            record,
            workload=record["workload"],
            mode=record["mode"],
            sha256=record["artifact_sha256"],
        )
        provenance = record.get("provenance", {})
        for key in ("git_revision", "mister_ip", "build_fingerprint"):
            if not provenance.get(key):
                raise ValueError(f"missing comparison provenance: {key}")
        if provenance.get("git_dirty") is not False:
            raise ValueError("freeze benchmark sources before comparing versions")
        if not record.get("build"):
            raise ValueError("missing build flags and target")
    for key in ("workload", "mode", "fixture", "work_count", "build"):
        if baseline[key] != candidate[key]:
            raise ValueError(f"incompatible benchmark results: {key}")
    if baseline["provenance"]["mister_ip"] != candidate["provenance"]["mister_ip"]:
        raise ValueError("different benchmark devices")
    mode = baseline["mode"]
    if mode == "visual":
        raise ValueError("visual inspection is not a speed comparison")
    if mode == "timing":
        old = [s["duration_ns"] for s in baseline["samples"]]
        new = [s["duration_ns"] for s in candidate["samples"]]
        return {
            "baseline_ns": old,
            "candidate_ns": new,
            "median_change_percent": (
                statistics.median(new) / statistics.median(old) - 1
            )
            * 100,
            "both_pairs_improved": all(b < a for a, b in zip(old, new)),
            "note": "Two samples per version; not statistical proof or GUI performance.",
        }
    old = validate_pmu(baseline["samples"][0]["pmu"], mode)
    new = validate_pmu(candidate["samples"][0]["pmu"], mode)
    return {
        "events": {
            event: {
                "baseline_per_pixel": old[event] / baseline["work_count"],
                "candidate_per_pixel": new[event] / candidate["work_count"],
                "change_percent": (new[event] / old[event] - 1) * 100
                if old[event]
                else None,
            }
            for event in sorted(old)
        },
        "note": "One diagnostic pass per version. NEON clock-enabled is not utilization; cache refills are not DRAM misses.",
        "environment_changed": baseline["samples"][0]["pmu"].get("environment_before")
        != candidate["samples"][0]["pmu"].get("environment_before"),
    }
