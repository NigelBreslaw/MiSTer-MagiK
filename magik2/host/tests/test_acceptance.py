from magik2.acceptance import summarize


def test_failed_attempts_remain_in_the_percentile_and_failures():
    attempts = [{"elapsed_ms": 100, "exit_code": 0}] * 18 + [
        {"elapsed_ms": 3000, "exit_code": 2},
        {"elapsed_ms": 4000, "exit_code": 0},
    ]
    result = summarize(attempts, 1000)
    assert result == {
        "attempts": 20,
        "failures": 1,
        "p95_ms": 3000,
        "target_ms": 1000,
        "target_met": False,
    }
