"""One exceptional setup operation; Desktop traffic otherwise uses Rust TCP."""

import contextlib
import json
import os
import sys
from pathlib import Path

CAPABILITIES = {
    "status",
    "device-identity-v1",
    "dashboard-status",
    "sd-browser",
    "framebuffer-stream",
    "telemetry-stream",
    "capture-framebuffer",
}


@contextlib.contextmanager
def progress_on_stderr():
    # Redirect inherited child stdout too: Python's redirect_stdout alone does not.
    saved = os.dup(1)
    try:
        sys.stdout.flush()
        os.dup2(2, 1)
        with contextlib.redirect_stdout(sys.stderr):
            yield
    finally:
        os.dup2(saved, 1)
        os.close(saved)


def prepare(run: Path) -> int:
    from .cli import connect_agent
    from .device_profile import DeviceProfile

    try:
        with progress_on_stderr():
            _, status = connect_agent(run, CAPABILITIES)
        profile = DeviceProfile.load()
        if profile is None or status.fields.get("device_identity") != profile.identity:
            raise RuntimeError("Prepared service did not confirm the selected identity")
    except Exception as error:
        print(json.dumps({"outcome": "error", "detail": str(error)}))
        return 2
    print(
        json.dumps(
            {
                "identity": profile.identity,
                "address": profile.address,
                "outcome": "ready",
                "capabilities": sorted(status.capabilities),
            }
        )
    )
    return 0
