import importlib.util
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "scope", Path(__file__).resolve().parents[2] / "scripts/check_tooling_scope.py"
)
scope = importlib.util.module_from_spec(spec)
spec.loader.exec_module(scope)


def test_consumers_remain_independent_and_label_cannot_hide_mixed_core_work():
    assert scope.scope_error(["magik2/scenarios/test_probe.py"], False) is None
    assert scope.scope_error(["scripts/magik2"], False)
    assert scope.scope_error(
        ["magik2/host/magik2/client.py", "apps/mister/ui.slint"], True
    )
    assert (
        scope.scope_error(
            ["magik2/host/magik2/client.py", "magik2/scenarios/test_probe.py"], True
        )
        is None
    )
