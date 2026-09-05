from magik2.scenarios import _measurement


def test_measurement_requires_complete_device_timing_fields() -> None:
    complete = {
        "elapsed_ms": 1,
        "presentations": 2,
        "render_us_total": 3,
        "last_render_us": 4,
        "vsync_hits": 5,
        "vsync_misses": 0,
    }
    assert _measurement(complete) == complete


def test_measurement_rejects_missing_or_non_integer_evidence() -> None:
    incomplete = {"elapsed_ms": 1}
    try:
        _measurement(incomplete)
    except AssertionError as error:
        assert "incomplete" in str(error)
    else:  # pragma: no cover - assertion documents the failure contract
        raise AssertionError("incomplete measurements must fail")
