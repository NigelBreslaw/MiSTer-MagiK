"""Optional campaign invariants; a changed output invalidates a measurement."""

import os
import re

from .results import append_event


def expected_display_mode():
    value = os.environ.get("MAGIK_EXPECT_DISPLAY_MODE")
    if value:
        from .device import DISPLAY_MODES

        if value == "auto" or value not in DISPLAY_MODES:
            raise ValueError(
                "MAGIK_EXPECT_DISPLAY_MODE must be an explicit supported mode"
            )
    return value


def verify_display(agent, run, boundary):
    expected = expected_display_mode()
    if not expected:
        return
    reply = agent.device_operation("display-status").get("reply", "")
    fields = dict(word.split("=", 1) for word in reply.split() if "=" in word)
    valid = fields.get("active") == expected and fields.get("pending") == "none"
    append_event(
        run,
        {
            "phase": "display-contract",
            "boundary": boundary,
            "expected": expected,
            "reply": reply,
            "outcome": "passed" if valid else "failed",
        },
    )
    if not valid:
        raise AssertionError(
            f"Expected {expected} without a pending transaction: {reply}"
        )


def measurement_metrics(agent):
    value = agent.metrics()
    expected = os.environ.get("MAGIK_EXPECT_RENDER_SIZE")
    if expected:
        match = re.fullmatch(r"([1-9][0-9]*)x([1-9][0-9]*)", expected)
        if not match:
            raise ValueError("MAGIK_EXPECT_RENDER_SIZE must be WIDTHxHEIGHT")
        dimensions = tuple(map(int, match.groups()))
        snapshots = [value]
        if value.get("window") is not None:
            snapshots.append(value["window"])
        for snapshot in snapshots:
            actual = (snapshot.get("width"), snapshot.get("height"))
            if actual != dimensions:
                raise AssertionError(
                    f"Measurement output changed: expected {dimensions}, got {actual}"
                )
    return value


def has_native_card_helpers(window):
    return window.get("context", {}).get("card_helper_ahead") in {
        "native-tricks-v1",
        "native-browse-tricks-v2",
    }
