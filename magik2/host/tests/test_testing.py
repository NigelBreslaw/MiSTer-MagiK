from types import SimpleNamespace

from magik2.testing import one_element


def test_one_element_searches_the_full_accessibility_tree() -> None:
    target = SimpleNamespace(accessible_label="build-label")

    class Query:
        searched_descendants = False

        def match_descendants(self):
            self.searched_descendants = True
            return self

        def find_all(self):
            assert self.searched_descendants
            return [target]

    query = Query()
    root = SimpleNamespace(accessible_label="root", query_descendants=lambda: query)
    application = SimpleNamespace(first_window=SimpleNamespace(root_element=root))

    assert one_element(application, "build-label") is target
