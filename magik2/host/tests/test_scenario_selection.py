from types import SimpleNamespace
from magik2.scenario_runner import pytest_collection_modifyitems


def test_profile_selects_only_profile_and_default_keeps_shared_workload():
    class Item:
        path = SimpleNamespace(name="test_probe.py")

        def __init__(self, profile):
            self.profile = profile

        def get_closest_marker(self, _):
            return object() if self.profile else None

    ordinary = [Item(False), Item(False), Item(False)]
    profiled = Item(True)
    for profile, expected in [(False, ordinary), (True, [profiled])]:
        config = SimpleNamespace(
            getoption=lambda key: "mini-magik" if key == "--magik2-app" else profile,
            hook=SimpleNamespace(pytest_deselected=lambda **_: None),
        )
        items = [*ordinary, profiled]
        pytest_collection_modifyitems(config, items)
        assert items == expected
