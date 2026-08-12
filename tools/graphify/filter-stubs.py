#!/usr/bin/env python3
"""Strip sourceless stdlib/external type-stub nodes from graphify-out/graph.json.

BACKGROUND (evidence bd adr-fmt-636cr Q2)
graphify's Rust extractor (extract.py:9194-9216, ensure_named_node) emits a
node with source_file == "" and source_location == "" whenever a type/name
is referenced but not defined in the current file (cross-file or
external/stdlib reference: Result, Vec, Option, self, Arc, HashMap, ...).
This is deliberate upstream behaviour with no suppression flag, env var, or
config key (grepped the installed package; only comments matched). A
post-processing filter we own is therefore the only mechanism that does not
require patching, forking, or vendoring the third-party graphifyy package.

This script:
  - loads graphify-out/graph.json (plain networkx node_link_data JSON:
    top-level keys directed/multigraph/graph/nodes/links/hyperedges/
    built_at_commit; nodes is a flat list of dicts, links is a flat list of
    {source,target,...} dicts)
  - selects nodes where BOTH source_file and source_location are empty or
    absent
  - drops those nodes and every link with a source/target endpoint in the
    dropped-id set
  - writes atomically (temp file in the same directory + os.replace, never
    truncate-in-place)
  - refuses to write if the result would have 0 nodes, or if the removal
    would exceed 40% of the current node count (safety rail)
  - is idempotent: a second run against an already-filtered graph removes 0
    nodes, because no remaining node satisfies the empty/empty predicate
  - supports --dry-run (report counts, do not write)

COMMUNITY ASSIGNMENTS ARE NOT RECOMPUTED.
Per-node community fields in graph.json, and the community-index-to-name
mapping in .graphify_labels.json (index-keyed, content-agnostic to which
nodes are in each community), are left exactly as the most recent rebuild
clustered them. Removing stub nodes may leave community boundaries stale
relative to the reduced node set. This is an accepted, documented
limitation of this filter, not a defect: re-clustering is explicitly out of
scope (see the graphify-improve-03 sub-mission contract) and this script
must not attempt it.

Python 3 standard library only. No third-party dependencies.
"""

from __future__ import annotations

import argparse
import json
import os
import sys
from pathlib import Path

DEFAULT_GRAPH_PATH = Path("graphify-out/graph.json")
MAX_REMOVAL_FRACTION = 0.40


def is_stub(node: dict) -> bool:
    return not node.get("source_file") and not node.get("source_location")


def node_id(node: dict) -> object:
    return node.get("id")


def filter_graph(graph: dict) -> tuple[dict, int, int]:
    nodes = graph.get("nodes", [])
    links = graph.get("links", [])

    original_node_count = len(nodes)
    stub_ids = {node_id(n) for n in nodes if is_stub(n)}

    kept_nodes = [n for n in nodes if node_id(n) not in stub_ids]
    kept_links = [
        link
        for link in links
        if link.get("source") not in stub_ids and link.get("target") not in stub_ids
    ]

    removed_node_count = original_node_count - len(kept_nodes)

    new_graph = dict(graph)
    new_graph["nodes"] = kept_nodes
    new_graph["links"] = kept_links
    return new_graph, original_node_count, removed_node_count


def write_atomically(graph_path: Path, graph: dict) -> None:
    tmp_path = graph_path.with_suffix(graph_path.suffix + ".tmp")
    with open(tmp_path, "w", encoding="utf-8") as fh:
        json.dump(graph, fh)
    os.replace(tmp_path, graph_path)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--graph",
        type=Path,
        default=DEFAULT_GRAPH_PATH,
        help=f"path to graph.json (default: {DEFAULT_GRAPH_PATH})",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="report what would be removed without writing",
    )
    args = parser.parse_args(argv)

    if not args.graph.exists():
        print(f"error: {args.graph} does not exist", file=sys.stderr)
        return 1

    with open(args.graph, "r", encoding="utf-8") as fh:
        graph = json.load(fh)

    new_graph, original_node_count, removed_node_count = filter_graph(graph)
    kept_node_count = original_node_count - removed_node_count
    removal_fraction = (
        removed_node_count / original_node_count if original_node_count else 0.0
    )

    print(
        f"nodes before: {original_node_count}\n"
        f"nodes removed: {removed_node_count} ({removal_fraction:.1%})\n"
        f"nodes after: {kept_node_count}\n"
        f"links before: {len(graph.get('links', []))}\n"
        f"links after: {len(new_graph['links'])}"
    )

    if args.dry_run:
        print("dry-run: no write performed")
        return 0

    if kept_node_count == 0:
        print(
            "refusing to write: result would have 0 nodes",
            file=sys.stderr,
        )
        return 1

    if removal_fraction > MAX_REMOVAL_FRACTION:
        print(
            f"refusing to write: removal fraction {removal_fraction:.1%} "
            f"exceeds safety rail of {MAX_REMOVAL_FRACTION:.0%}",
            file=sys.stderr,
        )
        return 1

    write_atomically(args.graph, new_graph)
    print(f"wrote {args.graph}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
