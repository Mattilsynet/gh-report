#!/usr/bin/env python3
"""Build and mechanically assert the gh-report release SBOM.

Two subcommands, both stdlib-only:

  shipset  cargo-metadata document -> sorted `name@version` shipped closure.
  merge    cargo BOM + image BOM + shipset -> one CycloneDX 1.5 JSON document.

Shipset rule (the exclusion rule, mechanical):

  Start at the workspace package whose name is --root. Follow only resolve
  edges whose dep_kinds carry kind null or "normal". A package with a target
  of kind "proc-macro" is not shipped: it is recorded as excluded and never
  traversed, because a proc-macro's own dependency subtree is build-time
  only. Packages reachable only through a proc-macro therefore drop out.

Merge rule:

  The cargo document's metadata.component (the application) is itself a
  shipped component and joins components[], and its bom-ref is a child of
  the OCI image root. Cargo components absent from the shipset are dropped
  and reported; image components without a `pkg:` purl are dropped and
  counted. Dependency entries are kept for retained components only, and
  every dependsOn target must resolve to a retained component, so the
  exclusion rule holds in the graph as well as in components[].

  The OCI root is purl == bom-ref:
  pkg:oci/gh-report@sha256%3A<hex>?repository_url=<percent-encoded>&tag=<tag>

Determinism: no serialNumber, no metadata.timestamp, no metadata.tools, no
signature; components and dependencies sorted; JSON written with
`indent=2, sort_keys=True` and a trailing newline.
"""

from __future__ import annotations

import argparse
import copy
import json
import re
import sys
import urllib.parse

DIGEST_RE = re.compile(r"^sha256:[0-9a-f]{64}$")
BANNED_NAMES = frozenset({"proptest", "insta", "dhat", "basic-toml", "paste"})
MIN_COMPONENTS = 200
NORMAL_KINDS = (None, "normal")


class UsageError(Exception):
    """Bad arguments, unreadable input, or unparseable JSON (exit 2)."""


class DataError(Exception):
    """The inputs cannot produce a truthful document (exit 1)."""

    def __init__(self, code, detail):
        super().__init__(detail)
        self.code = code
        self.detail = detail


def _load_json(path):
    try:
        with open(path, "r", encoding="utf-8") as handle:
            return json.load(handle)
    except OSError as exc:
        raise UsageError("cannot read %s: %s" % (path, exc))
    except ValueError as exc:
        raise UsageError("%s is not valid JSON: %s" % (path, exc))


def _name_version(component):
    return "%s@%s" % (component.get("name", ""), component.get("version", ""))


def _is_proc_macro(package):
    return any(
        "proc-macro" in (target.get("kind") or [])
        for target in package.get("targets", [])
    )


def _normal_edge(dep):
    return any(
        kind.get("kind") in NORMAL_KINDS for kind in dep.get("dep_kinds", [])
    )


def shipped_shipset(metadata, root_name):
    """Return (sorted shipped lines, sorted excluded lines) for the closure."""
    packages = {package["id"]: package for package in metadata.get("packages", [])}
    nodes = {
        node["id"]: node
        for node in (metadata.get("resolve") or {}).get("nodes", [])
    }
    roots = [
        package
        for package in packages.values()
        if package.get("name") == root_name and not package.get("source")
    ]
    if len(roots) != 1:
        raise UsageError(
            "expected exactly one workspace package named %r, found %d"
            % (root_name, len(roots))
        )
    shipped = set()
    excluded = set()
    seen = set()
    pending = [roots[0]["id"]]
    while pending:
        package_id = pending.pop()
        if package_id in seen:
            continue
        seen.add(package_id)
        package = packages.get(package_id)
        if package is None:
            raise UsageError("resolve graph references unknown package %r" % package_id)
        if _is_proc_macro(package):
            excluded.add(_name_version(package))
            continue
        shipped.add(_name_version(package))
        node = nodes.get(package_id)
        if node is None:
            raise UsageError("resolve graph has no node for %r" % package_id)
        for dep in node.get("deps", []):
            if _normal_edge(dep):
                pending.append(dep["pkg"])
    return sorted(shipped), sorted(excluded)


def percent_encode(value):
    return urllib.parse.quote(value, safe="")


def oci_purl(tag, digest, repository_url):
    return "pkg:oci/gh-report@%s?repository_url=%s&tag=%s" % (
        percent_encode(digest),
        percent_encode(repository_url),
        percent_encode(tag),
    )


def purl_version(purl):
    without_subpath = purl.split("#", 1)[0]
    without_qualifiers = without_subpath.split("?", 1)[0]
    if "@" not in without_qualifiers:
        return ""
    return without_qualifiers.rsplit("@", 1)[1]


def _strip_signatures(value):
    if isinstance(value, dict):
        return {
            key: _strip_signatures(item)
            for key, item in value.items()
            if key != "signature"
        }
    if isinstance(value, list):
        return [_strip_signatures(item) for item in value]
    return value


def _require_purl(component):
    purl = component.get("purl") or ""
    if not purl:
        component["purl"] = "pkg:cargo/%s@%s" % (
            component.get("name", ""),
            component.get("version", ""),
        )
    if not component.get("bom-ref"):
        component["bom-ref"] = component["purl"]


def _dedupe_bom_refs(sources):
    seen = {}
    for source, component in sources:
        ref = component["bom-ref"]
        if ref in seen:
            raise DataError(
                "duplicate-bom-ref",
                "%s appears in both %s and %s" % (ref, seen[ref], source),
            )
        seen[ref] = source


def build_document(cargo_doc, image_doc, shipset, repository_url, digest, tag):
    """Return (document, excluded list, dropped image component count)."""
    shipset_set = set(shipset)
    cargo_metadata = cargo_doc.get("metadata") or {}
    app_root = cargo_metadata.get("component")
    if not isinstance(app_root, dict):
        raise DataError("cargo-bom", "cargo document has no metadata.component")
    cargo_root_ref = app_root.get("bom-ref")
    if not cargo_root_ref:
        raise DataError("cargo-bom", "cargo metadata.component has no bom-ref")

    entries = []
    excluded = []
    for component in cargo_doc.get("components", []):
        if _name_version(component) in shipset_set:
            entries.append(("cargo", copy.deepcopy(component)))
        else:
            excluded.append(_name_version(component))

    if _name_version(app_root) in shipset_set:
        entries.append(("cargo", copy.deepcopy(app_root)))
    else:
        raise DataError(
            "missing-shipset-component",
            "%s is the cargo BOM root but is not in the shipset" % _name_version(app_root),
        )

    present = {_name_version(component) for _, component in entries}
    missing = sorted(shipset_set - present)
    if missing:
        raise DataError("missing-shipset-component", ", ".join(missing))

    dropped_image = 0
    image_refs = []
    for component in image_doc.get("components", []):
        purl = component.get("purl") or ""
        if not purl.startswith("pkg:"):
            dropped_image += 1
            continue
        cloned = copy.deepcopy(component)
        image_refs.append(cloned)
        entries.append(("image", cloned))

    for _, component in entries:
        _require_purl(component)
    _dedupe_bom_refs(entries)

    root_purl = oci_purl(tag, digest, repository_url)
    dependencies = {}
    root_depends = []
    for entry in cargo_doc.get("dependencies", []):
        ref = entry.get("ref")
        if ref == cargo_root_ref:
            root_depends.extend(entry.get("dependsOn") or [])
            continue
        dependencies.setdefault(ref, set()).update(entry.get("dependsOn") or [])

    retained = {component["bom-ref"] for _, component in entries}
    root_depends = [ref for ref in root_depends if ref in retained]
    root_depends.append(app_root["bom-ref"])
    root_depends.extend(component["bom-ref"] for component in image_refs)

    pruned = {
        ref: sorted(target for target in targets if target in retained)
        for ref, targets in dependencies.items()
        if ref in retained
    }
    pruned[root_purl] = root_depends

    document = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": {
                "type": "container",
                "bom-ref": root_purl,
                "name": "gh-report",
                "version": tag,
                "purl": root_purl,
            }
        },
        "components": [
            component for _, component in sorted(entries, key=lambda item: item[1]["bom-ref"])
        ],
        "dependencies": [
            {"ref": ref, "dependsOn": sorted(set(targets))}
            for ref, targets in sorted(pruned.items())
        ],
    }
    return _strip_signatures(document), sorted(excluded), dropped_image


def check_document(document, shipset, digest):
    """Return a list of (check, detail) failures; empty means the shape holds."""
    failures = []
    if document.get("bomFormat") != "CycloneDX":
        failures.append(("bom-format", repr(document.get("bomFormat"))))
    if document.get("specVersion") != "1.5":
        failures.append(("spec-version", repr(document.get("specVersion"))))

    root = (document.get("metadata") or {}).get("component") or {}
    root_purl = root.get("purl") or ""
    if root.get("type") != "container":
        failures.append(("root-component", "type is %r" % (root.get("type"),)))
    if not root_purl.startswith("pkg:oci/"):
        failures.append(("root-component", "purl is %r" % (root_purl,)))
    else:
        decoded = urllib.parse.unquote(purl_version(root_purl))
        if decoded != digest:
            failures.append(
                ("digest-mismatch", "%s does not match %s" % (decoded, digest))
            )
        if root.get("bom-ref") != root_purl:
            failures.append(("root-component", "bom-ref is not the purl"))

    components = document.get("components") or []
    present = set()
    for component in components:
        name = component.get("name") or ""
        purl = component.get("purl") or ""
        present.add(_name_version(component))
        if not purl.startswith("pkg:"):
            failures.append(("component-purl", name))
        if component.get("type") == "file":
            failures.append(("component-type", name))
        for reference in (purl, component.get("bom-ref") or ""):
            if reference.startswith("pkg:github"):
                failures.append(("github-reference", reference))
        if name in BANNED_NAMES:
            failures.append(("banned-component", _name_version(component)))

    for entry in document.get("dependencies") or []:
        for reference in [entry.get("ref") or ""] + list(entry.get("dependsOn") or []):
            if reference.startswith("pkg:github"):
                failures.append(("github-reference", reference))

    for key in sorted(shipset):
        if key not in present:
            failures.append(("missing-shipset-component", key))

    if len(components) < MIN_COMPONENTS:
        failures.append(
            ("component-count", "%d components (floor %d)" % (len(components), MIN_COMPONENTS))
        )
    return failures


def write_document(document, path):
    try:
        with open(path, "w", encoding="utf-8", newline="\n") as handle:
            json.dump(document, handle, indent=2, sort_keys=True)
            handle.write("\n")
    except OSError as exc:
        raise UsageError("cannot write %s: %s" % (path, exc))


def _read_shipset(path):
    try:
        with open(path, "r", encoding="utf-8") as handle:
            lines = [line.strip() for line in handle]
    except OSError as exc:
        raise UsageError("cannot read %s: %s" % (path, exc))
    return [line for line in lines if line]


def _run_shipset(args):
    metadata = _load_json(args.metadata)
    shipped, excluded = shipped_shipset(metadata, args.root)
    if args.out:
        try:
            with open(args.out, "w", encoding="utf-8", newline="\n") as handle:
                handle.write("".join(line + "\n" for line in shipped))
        except OSError as exc:
            raise UsageError("cannot write %s: %s" % (args.out, exc))
    else:
        sys.stdout.write("".join(line + "\n" for line in shipped))
    sys.stderr.write("excluded-components: %s\n" % ", ".join(excluded))
    return 0


def _run_merge(args):
    if not DIGEST_RE.match(args.digest):
        raise UsageError("--digest must be sha256:<64 lowercase hex>")
    cargo_doc = _load_json(args.cargo_bom)
    image_doc = _load_json(args.image_bom)
    shipset = _read_shipset(args.shipset)
    document, excluded, dropped_image = build_document(
        cargo_doc, image_doc, shipset, args.repository_url, args.digest, args.tag
    )
    write_document(document, args.out)
    if args.check:
        try:
            with open(args.out, "r", encoding="utf-8") as handle:
                written = json.load(handle)
        except (OSError, ValueError) as exc:
            raise UsageError("cannot re-read %s: %s" % (args.out, exc))
        failures = check_document(written, shipset, args.digest)
        if failures:
            for check, detail in failures:
                print("::error::%s: %s" % (check, detail))
            return 1
    print("shipped-components: %d" % len(document["components"]))
    print("excluded-components: %s" % ", ".join(excluded))
    print("dropped-image-components: %d" % dropped_image)
    return 0


def _parser():
    parser = argparse.ArgumentParser(prog="sbom_merge.py")
    sub = parser.add_subparsers(dest="command", required=True)

    ship = sub.add_parser("shipset", help="cargo metadata -> shipped closure")
    ship.add_argument("--metadata", required=True)
    ship.add_argument("--root", required=True)
    ship.add_argument("--out")

    merge = sub.add_parser("merge", help="cargo BOM + image BOM -> release BOM")
    merge.add_argument("--cargo-bom", required=True)
    merge.add_argument("--image-bom", required=True)
    merge.add_argument("--shipset", required=True)
    merge.add_argument("--repository-url", required=True)
    merge.add_argument("--digest", required=True)
    merge.add_argument("--tag", required=True)
    merge.add_argument("--out", required=True)
    merge.add_argument("--check", action="store_true")
    return parser


def main(argv=None):
    args = _parser().parse_args(argv)
    try:
        if args.command == "shipset":
            return _run_shipset(args)
        return _run_merge(args)
    except UsageError as exc:
        print("::error::usage: %s" % exc, file=sys.stderr)
        return 2
    except DataError as exc:
        print("::error::%s: %s" % (exc.code, exc.detail))
        return 1


if __name__ == "__main__":
    sys.exit(main())
