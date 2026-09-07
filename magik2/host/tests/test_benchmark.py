import copy
import pytest
from magik2.benchmark import validate_result


def result():
    return {
        "schema_version": 1,
        "workload": "blend",
        "mode": "timing",
        "artifact_sha256": "a" * 64,
        "correctness": "passed",
        "fixture": {"identity": "b" * 64},
        "work_count": 100,
        "samples": [
            {
                "repetition": i,
                "fixture_identity": "b" * 64,
                "work_count": 100,
                "duration_ns": 200,
                "ns_per_pixel": 2,
                "pmu": None,
            }
            for i in range(2)
        ],
        "visual": None,
    }


def check(value):
    return validate_result(value, workload="blend", mode="timing", sha256="a" * 64)


def test_valid_result():
    assert check(result())["work_count"] == 100


@pytest.mark.parametrize(
    "change",
    [
        lambda x: x.update(artifact_sha256="wrong"),
        lambda x: x.update(samples=[]),
        lambda x: x.update(correctness="failed"),
        lambda x: x["samples"][1].update(repetition=0),
        lambda x: x["samples"][1].update(fixture_identity="other"),
        lambda x: x["samples"][1].update(work_count=99),
        lambda x: x["samples"][0].update(duration_ns=float("nan")),
        lambda x: x["samples"][0].update(ns_per_pixel=0),
        lambda x: x["samples"][0].update(pmu={}),
    ],
)
def test_rejects_bad_records(change):
    value = copy.deepcopy(result())
    change(value)
    with pytest.raises(ValueError):
        check(value)


def test_counter_validation_and_zeroes():
    from magik2.benchmark_compare import EVENTS, validate_pmu

    counts = dict.fromkeys(EVENTS["pmu-neon"], 0)
    counts["cycles"] = 100
    pmu = {
        "counter_set": "cortex-a9-neon",
        "counters": {
            "counter_set": "cortex-a9-neon",
            "time_enabled_ns": 100,
            "time_running_ns": 100,
            "counters": counts,
        },
    }
    assert validate_pmu(pmu, "pmu-neon")["l1d-refills"] == 0
    pmu["counters"]["time_running_ns"] = 99
    with pytest.raises(ValueError):
        validate_pmu(pmu, "pmu-neon")
    pmu["counters"]["time_running_ns"] = 100
    del counts["neon-instructions"]
    with pytest.raises(ValueError):
        validate_pmu(pmu, "pmu-neon")


def test_offline_comparison_requires_matching_fixture_and_provenance():
    from magik2.benchmark_compare import compare

    old = result()
    old["provenance"] = {
        "git_revision": "a",
        "git_dirty": False,
        "mister_ip": "device",
        "build_fingerprint": "hash",
    }
    old["build"] = {"target": "arm", "flags": "same"}
    new = copy.deepcopy(old)
    new["provenance"]["git_revision"] = "b"
    assert compare(old, new)["median_change_percent"] == 0
    new["fixture"]["identity"] = "c" * 64
    with pytest.raises(ValueError):
        compare(old, new)
    new = copy.deepcopy(old)
    new["provenance"]["git_dirty"] = True
    with pytest.raises(ValueError):
        compare(old, new)
