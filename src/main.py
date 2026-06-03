"""Entry point for the MiSTer Slint UI prototype.

On a desktop this opens a normal window. On the MiSTer it renders straight to
``/dev/fb0`` through Slint's LinuxKMS backend in its legacy-framebuffer mode
(``SLINT_BACKEND=linuxkms-skia-software`` + ``SLINT_BACKEND_LINUXFB=1``), set up
by the launcher script.

Modes (selected by environment variable):

* ``MISTER_MAGIC_CHECK=1`` - headless self-test: load the component, exercise
  the property and callback bindings, then exit. Needs no display, so it is
  used for CI and for validating the install on a headless machine.
* ``MISTER_MAGIC_SMOKE=1`` - start the real event loop, then quit after
  ``MISTER_MAGIC_SMOKE_DELAY`` seconds (default 3). Used to confirm a frame can
  actually be rendered on a machine that has a display/framebuffer.
* default - run the UI normally until the process is told to quit.
"""

from __future__ import annotations

import os
import sys
import threading
from pathlib import Path

import slint

# Make the .slint files discoverable by ``slint.loader`` regardless of the
# working directory the launcher happens to use.
UI_DIR = Path(__file__).resolve().parent.parent / "ui"
sys.path.insert(0, str(UI_DIR))


# ``slint.loader.app_window`` resolves ``app-window.slint`` in ``sys.path``
# (the dash is matched automatically).
class App(slint.loader.app_window.AppWindow):  # type: ignore[name-defined]
    @slint.callback
    def request_increase_value(self) -> None:
        self.counter = self.counter + 1


def _run_check() -> int:
    app = App()
    app.counter = 41
    if app.counter != 41:
        raise AssertionError(f"property set/get failed: {app.counter}")
    app.request_increase_value()
    if app.counter != 42:
        raise AssertionError(f"callback binding failed: {app.counter}")
    print("[mister-magic] check OK: component loads, property + callback bindings work")
    return 0


def _arm_smoke_timer() -> None:
    delay = float(os.environ.get("MISTER_MAGIC_SMOKE_DELAY", "3.0"))
    print(f"[mister-magic] smoke test: quitting event loop after {delay:.1f}s", flush=True)

    def stop() -> None:
        try:
            slint.quit_event_loop()
        finally:
            # Safety net: some platforms do not interrupt a running native
            # event loop from another thread, so force the process to exit
            # shortly after asking nicely.
            threading.Timer(1.0, lambda: os._exit(0)).start()

    threading.Timer(delay, stop).start()


def main() -> int:
    if os.environ.get("MISTER_MAGIC_CHECK") == "1":
        return _run_check()

    app = App()
    if os.environ.get("MISTER_MAGIC_SMOKE") == "1":
        _arm_smoke_timer()
    app.run()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
