"""Small typed boundary around the private Slint system-test client."""

from __future__ import annotations

import importlib
from collections.abc import Mapping
from types import ModuleType
from typing import Protocol, Self, cast


class SlintQuery(Protocol):
    """The query operations used by MagiK tests."""

    def match_accessible_role(self, role: object) -> Self:
        """Restrict results to an accessibility role."""

    def find_all(self) -> list[SlintElement]:
        """Return all matching descendants."""


class SlintElement(Protocol):
    """Accessibility properties exposed by the system-test protocol."""

    accessible_checked: bool
    accessible_description: str
    accessible_enabled: bool
    accessible_item_selected: bool
    accessible_label: str
    accessible_value: str

    def query_descendants(self) -> SlintQuery:
        """Start a descendant query."""


class SlintWindow(Protocol):
    """Window operations used by the MagiK test oracle."""

    root_element: SlintElement

    def grab_window_as_png(self) -> bytes:
        """Return a PNG snapshot of the window."""


class SlintApplication(Protocol):
    """Lifecycle surface of ``slint_testing.Application``."""

    first_window: SlintWindow | None

    def __enter__(self) -> Self:
        """Start the application and wait for its test connection."""

    def __exit__(
        self,
        exception_type: type[BaseException] | None,
        exception: BaseException | None,
        traceback: object | None,
    ) -> bool | None:
        """Close the test connection and terminate the application."""


class SlintApplicationFactory(Protocol):
    """Constructor shape used by the device bridge."""

    def __call__(
        self,
        arguments: list[str],
        *,
        env: Mapping[str, str],
        launch_timeout: float,
    ) -> SlintApplication:
        """Construct a system-test application."""


def load_application_factory() -> SlintApplicationFactory:
    """Load the private client only when a device run has been requested."""

    try:
        module = importlib.import_module("slint_testing")
    except ModuleNotFoundError as error:
        raise RuntimeError(
            "slint-testing==0.3 is required for device UI tests; "
            "install the optional device-ui-tests dependency group"
        ) from error

    application = getattr(module, "Application", None)
    if not callable(application):
        raise RuntimeError("installed slint-testing package has no Application client")
    return cast(SlintApplicationFactory, application)


def require_window(application: SlintApplication) -> SlintWindow:
    """Return the first window or fail with a useful lifecycle error."""

    window = application.first_window
    if window is None:
        raise RuntimeError("MagiK UI test application exposed no Slint window")
    return window
