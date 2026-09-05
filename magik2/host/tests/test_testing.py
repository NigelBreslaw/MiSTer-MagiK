from types import SimpleNamespace

from magik2.testing import one_element


def test_one_element_uses_a_stable_id_and_checks_its_accessibility_label() -> None:
    target = SimpleNamespace(accessible_label="build-label")

    class Window:
        def find_elements_by_id(self, element_id: str):
            assert element_id == "Probe::build-label-text"
            return [target]

    application = SimpleNamespace(first_window=Window())

    assert one_element(application, "build-label") is target
