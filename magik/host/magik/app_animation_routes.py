"""Reversible app navigation routes, using the round-trip measurement contract."""

import re
import time

from .animation_benchmark import _focus, _key, _selected, _tree


def _vertical_focus(app, label):
    for _ in range(9):
        if _selected(_tree(app), "^" + re.escape(label) + "$"):
            return
        _key(app, "\uf701")
        time.sleep(0.25)
    raise AssertionError(f"Cannot focus {label}: {_tree(app)}")


def navigate(app, step, route):
    def key(name, value, **expected):
        step(name, lambda: _key(app, value), **expected)

    if route == "root":
        for direction, value in [("right", "\uf703"), ("left", "\uf702")]:
            for _ in range(6):
                before = _tree(app)["selected"]
                key(f"Root {before} → {direction}", value, browsing=True)
                assert _tree(app)["selected"] != before, "Root card did not move"
        assert _selected(_tree(app), r"^Arcade$")
    elif route in {"computers", "handhelds"}:
        category, family, system = (
            ("Computers", "Sinclair", "ZX Spectrum")
            if route == "computers"
            else ("Handhelds", "Nintendo", "Game Boy")
        )
        key(f"Root → {category}", "\n", menu=category, browsing=True)
        step(
            f"Browse to {family}",
            lambda: _focus(app, "^" + family + "$"),
            expected="^" + family + "$",
            browsing=True,
        )
        key(f"{category} → {family}", "\n", menu=family, browsing=True)
        system_pattern = (
            r"^ZX[ -]Spectrum$" if route == "computers" else r"^Game[ -]?Boy$"
        )
        step(
            f"Browse to {system}",
            lambda: _focus(app, system_pattern),
            expected=system_pattern,
            browsing=True,
        )
        key(f"{family} → {system} hub", "\n", expected=r"^GAMES$")
        key(f"{system} hub → Games", "\n", games=True)
        key(f"Games → {system} hub (Select)", "\t", expected=r"^GAMES$")
        key(f"{system} hub → {family}", "\x1b", menu=family, browsing=True)
        key(f"{family} → {category}", "\x1b", menu=category, browsing=True)
        key(f"{category} → Root", "\x1b", expected="^" + category + "$", browsing=True)
    elif route == "arcade":
        key("Root → Arcade hub", "\n", expected=r"^GAMES$")

        def sections():
            for value, selected in [
                ("\uf703", "RECENT"),
                ("\uf703", "FAVOURITES"),
                ("\uf702", "RECENT"),
                ("\uf702", "GAMES"),
            ]:
                _key(app, value)
                time.sleep(0.8)
                assert _selected(_tree(app), "^" + selected + "$"), _tree(app)

        step(
            "Arcade hub: Games → Recent → Favourites → Games",
            sections,
            expected=r"^GAMES$",
        )
        key("Arcade hub → Games", "\n", games=True)
        key("Arcade list scroll down", "\uf701", games=True)
        key("Arcade list scroll up", "\uf700", games=True)
        key("Arcade list → alphabet drawer", "\uf702", element="Games A-Z")
        key("Alphabet drawer → filters", "\uf702", element="Filters")
        key("Filters → Games (Games A-Z)", "\n", games=True)
        key("Arcade games → hub (Select)", "\t", expected=r"^GAMES$")
        key("Arcade hub → Root", "\x1b", expected=r"^Arcade$", browsing=True)
    elif route == "favourites":
        key("Root → global Favourites", "\n", games=True)
        key("Global Favourites → Root", "\x1b", expected=r"^Favourites$", browsing=True)
    elif route == "settings":
        key("Root → Settings", "\n", element="Settings")
        key("Settings → display choices", "\n", element="Settings")
        key("Display choices → Settings (cancel)", "\x1b", element="Settings")
        step(
            "Settings focus → About",
            lambda: _vertical_focus(app, "About"),
            expected=r"^About$",
        )
        key("Settings → About", "\n", element="About")
        key("About → Licenses", "\n", element="Licenses")
        key("Licenses → license text", "\n", element="License text")
        key("License text → Licenses", "\x1b", element="Licenses")
        key("Licenses → About", "\x1b", element="About")
        key("About → Settings", "\x1b", element="Settings")
        key("Settings → Root", "\x1b", expected=r"^Settings$", browsing=True)
    else:
        raise ValueError(f"Unknown animation route: {route}")
