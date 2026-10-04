"""Describe sampling evidence without mistaking leaf symbols for call stacks."""

from collections import Counter


def summarize_profile(folded: str, expected_samples: int) -> dict:
    depths = Counter()
    threads = Counter()
    leaves = Counter()
    total = 0
    for line in folded.splitlines():
        stack, count = line.rsplit(" ", 1)
        frames = stack.split(";")
        count = int(count)
        if count <= 0 or len(frames) < 2 or not all(frames):
            raise ValueError("Invalid sampled profile row")
        total += count
        depths[len(frames) - 1] += count
        threads[frames[0]] += count
        leaves[frames[-1]] += count
    if total != expected_samples or total <= 0:
        raise ValueError("Folded profile sample count does not match profile receipt")
    leaf_only = set(depths) == {1}
    return {
        "schema": "magik-profile-quality-v1",
        "samples": total,
        "attribution": "leaf-only" if leaf_only else "multi-symbol",
        "symbol_depth_samples": dict(sorted(depths.items())),
        "symbol_depth_scope": "flattened symbols; inline frames may increase depth",
        "thread_samples": dict(threads.most_common()),
        "top_leaf_symbols": [
            {"symbol": symbol, "samples": count, "percent": count * 100 / total}
            for symbol, count in leaves.most_common(20)
        ],
        "thread_sample_share_is_calibrated_cpu_share": False,
        "sample_timestamps_available": False,
        "per_drop_stack_attribution_available": False,
        "limitations": [
            "Samples are aggregated over the measurement window, not timestamped.",
            "CPU sampling cannot measure time sleeping or blocked.",
            "Compare thread CPU clocks: sample shares are not calibrated CPU shares.",
            *(
                ["All samples contain one symbol: no caller-chain evidence."]
                if leaf_only
                else []
            ),
        ],
    }
