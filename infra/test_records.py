"""infra/records.py: what two exports of one tracker must agree on, whatever order each lists members in."""
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import records


class Canonical(unittest.TestCase):
    def test_the_checkout_path_is_dropped_and_lists_are_ordered(self):
        written = {"id": "x-a", "source_repo_path": "/one", "labels": ["b", "a"],
                   "dependencies": [{"depends_on_id": "x-c", "type": "blocks"}, {"depends_on_id": "x-b", "type": "blocks"}]}
        again = {"id": "x-a", "source_repo_path": "/two", "labels": ["a", "b"],
                 "dependencies": [{"depends_on_id": "x-b", "type": "blocks"}, {"depends_on_id": "x-c", "type": "blocks"}]}
        self.assertEqual(records.canonical(written), records.canonical(again))
        self.assertNotIn("source_repo_path", records.canonical(written))

    def test_dependencies_alike_but_for_a_member_are_ordered_by_all_of_it(self):
        # the same target and type twice, told apart only by when: ordered whole, never by the first two
        early = {"depends_on_id": "x-b", "type": "blocks", "created_at": "2026-01-01T00:00:00Z"}
        late = {"depends_on_id": "x-b", "type": "blocks", "created_at": "2026-02-01T00:00:00Z"}
        self.assertEqual(records.canonical({"id": "x-a", "dependencies": [late, early]}),
                         records.canonical({"id": "x-a", "dependencies": [early, late]}))

    def test_the_record_given_is_left_as_it_was(self):
        record = {"id": "x-a", "labels": ["b", "a"], "source_repo_path": "/one"}
        records.canonical(record)
        self.assertEqual(record, {"id": "x-a", "labels": ["b", "a"], "source_repo_path": "/one"})


if __name__ == "__main__":
    unittest.main()
