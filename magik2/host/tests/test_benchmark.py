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
