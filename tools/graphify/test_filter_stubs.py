#!/usr/bin/env python3
"""Unit tests for tools/graphify/filter-stubs.py."""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

FILTER_STUBS_PATH = Path(__file__).parent / "filter-stubs.py"

spec = importlib.util.spec_from_file_location("filter_stubs", FILTER_STUBS_PATH)
filter_stubs = importlib.util.module_from_spec(spec)
spec.loader.exec_module(filter_stubs)


class TestFilterStubs(unittest.TestCase):
    def test_is_stub(self) -> None:
        self.assertTrue(filter_stubs.is_stub({}))
        self.assertTrue(filter_stubs.is_stub({"source_file": "", "source_location": ""}))
        self.assertFalse(filter_stubs.is_stub({"source_file": "src/lib.rs", "source_location": ""}))
        self.assertFalse(filter_stubs.is_stub({"source_file": "", "source_location": "10:1"}))
        self.assertFalse(filter_stubs.is_stub({"source_file": "src/lib.rs", "source_location": "10:1"}))

    def test_filter_graph_removes_stubs_and_incident_links(self) -> None:
        graph = {
            "directed": False,
            "multigraph": False,
            "graph": {},
            "nodes": [
                {"id": "node-1", "source_file": "src/lib.rs", "source_location": "1:1"},
                {"id": "node-2", "source_file": "src/main.rs", "source_location": "2:1"},
                {"id": "stub-1", "source_file": "", "source_location": ""},
            ],
            "links": [
                {"source": "node-1", "target": "node-2", "relation": "calls"},
                {"source": "node-1", "target": "stub-1", "relation": "references"},
            ],
        }

        filtered, original_count, removed_count = filter_stubs.filter_graph(graph)

        self.assertEqual(original_count, 3)
        self.assertEqual(removed_count, 1)
        self.assertEqual(len(filtered["nodes"]), 2)
        self.assertEqual(len(filtered["links"]), 1)
        self.assertEqual(filtered["links"][0]["source"], "node-1")
        self.assertEqual(filtered["links"][0]["target"], "node-2")

    def test_filter_graph_idempotent(self) -> None:
        graph = {
            "nodes": [
                {"id": "node-1", "source_file": "src/lib.rs", "source_location": "1:1"},
                {"id": "stub-1", "source_file": "", "source_location": ""},
            ],
            "links": [
                {"source": "node-1", "target": "stub-1"},
            ],
        }

        first_pass, _, removed_first = filter_stubs.filter_graph(graph)
        self.assertEqual(removed_first, 1)

        second_pass, count_second, removed_second = filter_stubs.filter_graph(first_pass)
        self.assertEqual(removed_second, 0)
        self.assertEqual(count_second, 1)
        self.assertEqual(len(second_pass["nodes"]), 1)
        self.assertEqual(len(second_pass["links"]), 0)

    def test_filter_graph_empty(self) -> None:
        filtered, original_count, removed_count = filter_stubs.filter_graph({})
        self.assertEqual(original_count, 0)
        self.assertEqual(removed_count, 0)
        self.assertEqual(filtered.get("nodes"), [])
        self.assertEqual(filtered.get("links"), [])


if __name__ == "__main__":
    unittest.main()
