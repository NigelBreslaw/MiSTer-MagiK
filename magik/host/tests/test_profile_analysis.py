import pytest

from magik.profile_analysis import summarize_profile


def test_leaf_samples_are_not_reported_as_caller_stacks():
    result = summarize_profile("ui;memcpy 4\nhelper;draw 6", 10)
    assert result["attribution"] == "leaf-only"
    assert result["symbol_depth_samples"] == {1: 10}
    assert result["thread_samples"] == {"helper": 6, "ui": 4}
    assert not result["per_drop_stack_attribution_available"]


def test_mixed_depth_is_counted_without_claiming_temporal_attribution():
    result = summarize_profile("ui;caller;draw 3\nui;memcpy 7", 10)
    assert result["attribution"] == "multi-symbol"
    assert result["symbol_depth_samples"] == {1: 7, 2: 3}
    assert not result["sample_timestamps_available"]


@pytest.mark.parametrize(
    "folded,count", [("ui;draw 2", 3), ("ui;draw 0", 0), ("ui 1", 1)]
)
def test_invalid_profile_does_not_pass_quality_validation(folded, count):
    with pytest.raises(ValueError):
        summarize_profile(folded, count)
