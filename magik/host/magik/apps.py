"""Two ordinary consumers of the same build, delivery and observation workflow."""

from dataclasses import dataclass, replace
import os
from pathlib import Path


@dataclass(frozen=True)
class Application:
    name: str
    package: str
    binary: str
    profile: str = "release"
    features: tuple[str, ...] = ()
    agent_capabilities: frozenset[str] = frozenset()


APPLICATIONS = {
    "mini-magik": Application(
        "mini-magik",
        "magik/probe",
        "mini-magik",
        agent_capabilities=frozenset({"mini-display-plan-v1"}),
    ),
    "magik": Application(
        "magik",
        "apps/mister",
        "mister-magik-fb",
        "release-device-ui-tests",
        ("tooling",),
        frozenset({"main-managed-magik", "card-artwork-v2", "canonical-magik-runtime-v1"}),
    ),
}


def application(name: str = "mini-magik") -> Application:
    app = APPLICATIONS[name]
    if name == "mini-magik" and os.environ.get("MAGIK_MINI_PRODUCTION_BUILD") == "1":
        return replace(app, profile="release-device")
    if name == "magik":
        sampler = os.environ.get("MAGIK_CARD_SAMPLER_AB", "current")
        if sampler not in {"current", "axis"}:
            raise ValueError("MAGIK_CARD_SAMPLER_AB must be current or axis")
        quantiser = os.environ.get("MAGIK_CARD_QUANTISER", "current")
        if quantiser not in {"current", "fast"}:
            raise ValueError("MAGIK_CARD_QUANTISER must be current or fast")
        if sampler == "axis":
            app = replace(app, features=(*app.features, "card-axis-filter"))
        if quantiser == "fast":
            app = replace(app, features=(*app.features, "card-fast-quantisation"))
    return app


def repository() -> Path:
    return Path(__file__).resolve().parents[3]


def validate_renderer_context(context: object) -> None:
    """Require runtime evidence, including for prebuilt or already installed binaries."""
    app = application("magik")
    expected = {
        "card_sampler": "independent-vertical-prefilter"
        if "card-axis-filter" in app.features
        else "current",
        "card_quantiser": "centred-bayer-shifts"
        if "card-fast-quantisation" in app.features
        else "existing-bayer",
    }
    if not isinstance(context, dict) or any(
        context.get(k) != v for k, v in expected.items()
    ):
        raise AssertionError(
            f"launcher renderer does not match requested features: expected={expected}, actual={context!r}"
        )
