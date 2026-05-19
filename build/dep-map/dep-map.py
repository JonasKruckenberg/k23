#!/usr/bin/env python3
# Copyright 2025 Jonas Kruckenberg
#
# Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
# http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
# http://opensource.org/licenses/MIT>, at your option. This file may not be
# copied, modified, or distributed except according to those terms.
"""
dep-map: build, render, and diff k23's dependency graph.

Subcommands:
  gen   — Run buck2 to extract the dep graph, enrich third-party crates with
          crates.io metadata, and emit dep-map.json plus a single-file
          interactive Cytoscape.js viz (dep-map.html).
  diff  — Compare two dep-map.json files (e.g. base vs HEAD) and emit a
          markdown summary plus, if Graphviz is available, an SVG that
          highlights additions/removals/version changes.

The buck2 step requires a populated nix shell (see flake.nix #ci). The
crates.io fetch needs outbound HTTPS; results are cached in
.dep-map-cache.json under the workdir so reruns are cheap.
"""

from __future__ import annotations

import argparse
import dataclasses
import datetime as _dt
import json
import os
import re
import subprocess
import sys
import time
import tomllib
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
CARGO_LOCK = REPO_ROOT / "third-party" / "Cargo.lock"
THIRD_PARTY_BUCK = REPO_ROOT / "third-party" / "BUCK"

CRATES_IO_API = "https://crates.io/api/v1/crates"
USER_AGENT = "k23-dep-map (https://github.com/JonasKruckenberg/k23)"

# Thresholds for the maintenance traffic-light (days since last release).
MAINT_GREEN_DAYS = 365
MAINT_YELLOW_DAYS = 365 * 2

# Buck2 query universes. Internal nodes include both libraries and binaries
# (kernel/loader); third-party are libraries only (proc-macros and build
# scripts are reachable but excluded as noise — see DEP_FILTERS).
INTERNAL_UNIVERSE = "set(//lib/... //sys/...)"
THIRDPARTY_UNIVERSE = "//third-party/..."

# Drop edges to these auxiliary buck targets — they swamp the graph and
# aren't deps a human cares about when judging risk.
DEP_FILTERS = (
    re.compile(r"-build_script(_build|_run)?(-[^/]+)?$"),
    re.compile(r"-pre-build-script$"),
    re.compile(r"^toolchains//"),
    re.compile(r"^prelude//"),
)


# ---------------------------------------------------------------------------
# Cargo.lock parsing — canonical (name, version, source) for every external.
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class LockedCrate:
    name: str
    version: str
    source: str  # "registry" | "git" | "path"
    git_url: str | None = None
    checksum: str | None = None


def parse_cargo_lock(path: Path) -> list[LockedCrate]:
    data = tomllib.loads(path.read_text())
    out: list[LockedCrate] = []
    for pkg in data.get("package", []):
        src = pkg.get("source")
        if src is None:
            kind = "path"
            git_url = None
        elif src.startswith("registry+"):
            kind = "registry"
            git_url = None
        elif src.startswith("git+"):
            kind = "git"
            git_url = src[len("git+") :].split("?", 1)[0].split("#", 1)[0]
        else:
            kind = "other"
            git_url = None
        out.append(
            LockedCrate(
                name=pkg["name"],
                version=pkg["version"],
                source=kind,
                git_url=git_url,
                checksum=pkg.get("checksum"),
            )
        )
    return out


# ---------------------------------------------------------------------------
# third-party/BUCK parsing — alias resolution for `//third-party:cfg-if` →
# `//third-party:cfg-if-1.0.4`. We grep the generated file rather than rely
# on a buck2 attribute query: aliases don't carry a `deps` edge.
# ---------------------------------------------------------------------------

_ALIAS_RE = re.compile(
    r"""alias\(\s*
        name\s*=\s*"(?P<name>[^"]+)"\s*,\s*
        actual\s*=\s*"(?P<actual>[^"]+)"\s*,
    """,
    re.VERBOSE,
)


def parse_third_party_aliases(path: Path) -> dict[str, str]:
    aliases: dict[str, str] = {}
    text = path.read_text()
    for m in _ALIAS_RE.finditer(text):
        name = m.group("name")
        actual = m.group("actual").lstrip(":")
        aliases[f"//third-party:{name}"] = f"//third-party:{actual}"
    return aliases


# ---------------------------------------------------------------------------
# buck2 uquery — fetch the (target → deps) graph.
# ---------------------------------------------------------------------------


def run_buck2_uquery(query: str) -> dict[str, dict[str, Any]]:
    # `deps` covers the canonical edge attribute; `named_deps` is what reindeer
    # uses for renamed crates (the dict values are the target labels we want).
    cmd = [
        "buck2",
        "uquery",
        "--output-format",
        "json",
        "--output-attribute",
        "^(deps|named_deps)$",
        query,
    ]
    proc = subprocess.run(
        cmd, cwd=REPO_ROOT, check=True, capture_output=True, text=True
    )
    return json.loads(proc.stdout) if proc.stdout.strip() else {}


def normalize_target(label: str) -> str:
    # Strip configuration hashes that cquery would add. uquery shouldn't
    # produce them but be defensive.
    return label.split(" (", 1)[0]


def filter_deps(deps: list[str]) -> list[str]:
    out: list[str] = []
    for d in deps:
        d = normalize_target(d)
        if any(p.search(d) for p in DEP_FILTERS):
            continue
        out.append(d)
    return out


def _flatten_dep_attrs(attrs: dict[str, Any]) -> list[str]:
    out: list[str] = []
    deps = attrs.get("deps") or []
    if isinstance(deps, list):
        out.extend(deps)
    named = attrs.get("named_deps") or {}
    if isinstance(named, dict):
        out.extend(named.values())
    elif isinstance(named, list):
        # Some buck2 versions surface named_deps as a list of pairs.
        for entry in named:
            if isinstance(entry, (list, tuple)) and len(entry) == 2:
                out.append(entry[1])
            elif isinstance(entry, str):
                out.append(entry)
    return out


def extract_buck_graph() -> dict[str, dict[str, Any]]:
    """Returns {target: {kind, deps: [target, ...]}} merged across queries."""
    internal_q = f'kind("^rust_(library|binary|test)$", {INTERNAL_UNIVERSE})'
    thirdparty_q = f'kind("^rust_library$", {THIRDPARTY_UNIVERSE})'

    raw: dict[str, dict[str, Any]] = {}
    for q in (internal_q, thirdparty_q):
        for tgt, attrs in run_buck2_uquery(q).items():
            tgt = normalize_target(tgt)
            kind = attrs.get("buck.type") or attrs.get("type") or "unknown"
            deps = filter_deps(_flatten_dep_attrs(attrs))
            existing = raw.get(tgt)
            if existing is None:
                raw[tgt] = {"kind": kind, "deps": deps}
            else:
                # Merge — same target may surface across queries with
                # slightly different selectors expanded.
                existing["deps"] = sorted(set(existing["deps"]) | set(deps))
    return raw


# ---------------------------------------------------------------------------
# crates.io enrichment.
# ---------------------------------------------------------------------------


@dataclasses.dataclass
class CrateMeta:
    name: str
    version: str
    license: str | None = None
    repository: str | None = None
    homepage: str | None = None
    description: str | None = None
    downloads_total: int | None = None
    downloads_recent: int | None = None
    crate_size: int | None = None  # bytes of the .crate tarball for `version`
    latest_stable_version: str | None = None
    last_updated: str | None = None  # ISO-8601 of the version we depend on
    last_updated_latest: str | None = None  # ISO-8601 of latest published
    days_since_last_release: int | None = None
    fetch_error: str | None = None

    def maintenance(self) -> str:
        d = self.days_since_last_release
        if d is None:
            return "unknown"
        if d <= MAINT_GREEN_DAYS:
            return "green"
        if d <= MAINT_YELLOW_DAYS:
            return "yellow"
        return "red"


def _http_get_json(url: str, retries: int = 3) -> dict[str, Any]:
    last_err: Exception | None = None
    for i in range(retries):
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        try:
            with urllib.request.urlopen(req, timeout=20) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as e:
            if e.code == 429:
                time.sleep(2 ** (i + 1))
                last_err = e
                continue
            if 500 <= e.code < 600:
                time.sleep(2**i)
                last_err = e
                continue
            raise
        except (urllib.error.URLError, TimeoutError) as e:
            time.sleep(2**i)
            last_err = e
    raise RuntimeError(f"GET {url} failed after {retries} tries: {last_err}")


def fetch_crate_meta(name: str, version: str, cache: dict[str, Any]) -> CrateMeta:
    key = f"{name}@{version}"
    if key in cache:
        # Tolerate schema drift: drop unknown keys, default missing ones.
        known = {f.name for f in dataclasses.fields(CrateMeta)}
        entry = {k: v for k, v in cache[key].items() if k in known}
        try:
            return CrateMeta(**entry)
        except TypeError:
            del cache[key]  # corrupted; refetch

    meta = CrateMeta(name=name, version=version)
    try:
        data = _http_get_json(f"{CRATES_IO_API}/{name}")
    except Exception as e:
        meta.fetch_error = str(e)
        cache[key] = dataclasses.asdict(meta)
        return meta

    crate = data.get("crate", {}) or {}
    meta.repository = crate.get("repository")
    meta.homepage = crate.get("homepage")
    meta.description = (crate.get("description") or "").strip() or None
    meta.downloads_total = crate.get("downloads")
    meta.downloads_recent = crate.get("recent_downloads")
    meta.latest_stable_version = crate.get("max_stable_version") or crate.get(
        "max_version"
    )

    # crates.io's `/crates/{name}` returns up to N recent versions; if our
    # version isn't there we fall back to the per-version endpoint.
    versions: list[dict[str, Any]] = data.get("versions") or []
    selected = next((v for v in versions if v.get("num") == version), None)
    if selected is None:
        try:
            ver_data = _http_get_json(f"{CRATES_IO_API}/{name}/{version}")
            selected = ver_data.get("version") or {}
        except Exception:
            selected = {}

    meta.crate_size = selected.get("crate_size")
    meta.last_updated = selected.get("updated_at") or selected.get("created_at")
    # License lives on the version, not the crate, in newer crates.io API.
    meta.license = selected.get("license")

    if versions:
        meta.last_updated_latest = versions[0].get(
            "updated_at"
        ) or versions[0].get("created_at")

    ref_ts = meta.last_updated_latest or meta.last_updated
    if ref_ts:
        try:
            ts = _dt.datetime.fromisoformat(ref_ts.replace("Z", "+00:00"))
            meta.days_since_last_release = (
                _dt.datetime.now(tz=_dt.timezone.utc) - ts
            ).days
        except ValueError:
            pass

    cache[key] = dataclasses.asdict(meta)
    return meta


# ---------------------------------------------------------------------------
# Graph assembly.
# ---------------------------------------------------------------------------


def _kind_of(target: str, buck_kind: str | None, has_kernel_root: bool) -> str:
    if target.startswith("//third-party:"):
        return "external"
    if target.startswith("//sys:") or target.startswith("//sys/"):
        return "binary" if (buck_kind or "").endswith("_binary") else "system"
    if target.startswith("//lib/"):
        return "lib"
    return "other"


def short_name(target: str) -> str:
    # //third-party:foo-1.2.3 → foo-1.2.3
    # //lib/spin:spin → spin
    # //sys/kernel:kernel → kernel
    if ":" in target:
        return target.rsplit(":", 1)[1]
    return target


def build_graph(
    buck_graph: dict[str, dict[str, Any]],
    aliases: dict[str, str],
    locked: list[LockedCrate],
    cache: dict[str, Any],
    skip_crates_io: bool = False,
) -> dict[str, Any]:
    # Reverse Cargo.lock by reindeer's `name-version` target slug.
    lock_by_slug: dict[str, LockedCrate] = {
        f"{c.name}-{c.version}": c for c in locked
    }

    # Resolve every dep label: aliases collapse to canonical versioned target.
    def resolve(label: str) -> str:
        return aliases.get(label, label)

    # Build nodes
    nodes: dict[str, dict[str, Any]] = {}
    edges: list[dict[str, str]] = []

    def add_node(target: str, buck_kind: str | None) -> None:
        if target in nodes:
            return
        slug = short_name(target)
        category = (
            "external"
            if target.startswith("//third-party:")
            else "lib"
            if target.startswith("//lib/")
            else "system"
        )
        node: dict[str, Any] = {
            "id": target,
            "label": slug,
            "category": category,
            "buck_kind": buck_kind,
        }
        if category == "external":
            locked_meta = lock_by_slug.get(slug)
            if locked_meta:
                node["crate_name"] = locked_meta.name
                node["version"] = locked_meta.version
                node["source"] = locked_meta.source
                node["git_url"] = locked_meta.git_url
            else:
                node["crate_name"] = slug
                node["source"] = "unknown"
        nodes[target] = node

    for tgt, attrs in buck_graph.items():
        if tgt.startswith("//third-party:"):
            # Skip alias targets — we want the versioned canonical ones.
            slug = short_name(tgt)
            if slug not in lock_by_slug:
                # alias or non-package; ignore as a node, but it might be a
                # canonical we missed (e.g. a -build target). Skip.
                continue
        add_node(tgt, attrs.get("kind"))

    # Add edges (resolving alias → canonical) and ensure target nodes exist.
    for src, attrs in buck_graph.items():
        src_resolved = resolve(src)
        if src_resolved not in nodes:
            # Drop edges originating from non-package targets (e.g. aliases)
            continue
        for d in attrs.get("deps", []):
            d_resolved = resolve(d)
            if d_resolved not in nodes:
                # Allow only deps that resolve to a tracked node. Surfaces
                # alias-only third-party targets that lack a Cargo.lock entry
                # (rare, but stays defensive).
                if d_resolved.startswith("//third-party:"):
                    slug = short_name(d_resolved)
                    if slug in lock_by_slug:
                        add_node(d_resolved, "rust_library")
                    else:
                        continue
                else:
                    continue
            edges.append({"source": src_resolved, "target": d_resolved})

    # Deduplicate edges
    seen = set()
    deduped: list[dict[str, str]] = []
    for e in edges:
        key = (e["source"], e["target"])
        if key in seen:
            continue
        seen.add(key)
        deduped.append(e)

    # Default an empty metrics dict so the HTML viz never NPEs on missing keys.
    for n in nodes.values():
        if n["category"] == "external":
            n.setdefault("metrics", {})

    # Enrich external nodes with crates.io metadata.
    if not skip_crates_io:
        externals = [
            n
            for n in nodes.values()
            if n["category"] == "external" and n.get("source") == "registry"
        ]
        print(
            f"[dep-map] fetching crates.io metadata for {len(externals)} crates...",
            file=sys.stderr,
        )
        for i, node in enumerate(externals, 1):
            meta = fetch_crate_meta(node["crate_name"], node["version"], cache)
            node["metrics"] = {
                "license": meta.license,
                "repository": meta.repository,
                "homepage": meta.homepage,
                "description": meta.description,
                "downloads_total": meta.downloads_total,
                "downloads_recent": meta.downloads_recent,
                "crate_size": meta.crate_size,
                "latest_stable_version": meta.latest_stable_version,
                "last_updated": meta.last_updated,
                "last_updated_latest": meta.last_updated_latest,
                "days_since_last_release": meta.days_since_last_release,
                "maintenance": meta.maintenance(),
                "fetch_error": meta.fetch_error,
            }
            if i % 25 == 0:
                print(f"[dep-map]   {i}/{len(externals)}", file=sys.stderr)

    # Compute incoming-edge counts as a "fan-in" weight for layout / sizing.
    fan_in: dict[str, int] = {n: 0 for n in nodes}
    for e in deduped:
        fan_in[e["target"]] = fan_in.get(e["target"], 0) + 1
    for tid, n in nodes.items():
        n["fan_in"] = fan_in.get(tid, 0)

    return {
        "schema_version": 1,
        "generated_at": _dt.datetime.now(tz=_dt.timezone.utc)
        .isoformat(timespec="seconds")
        .replace("+00:00", "Z"),
        "nodes": list(nodes.values()),
        "edges": deduped,
    }


# ---------------------------------------------------------------------------
# HTML rendering — single-file Cytoscape.js viz.
# ---------------------------------------------------------------------------


HTML_TEMPLATE = r"""<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>k23 dependency map</title>
<meta name="viewport" content="width=device-width,initial-scale=1">
<style>
  :root {
    --bg: #0e1117;
    --panel: #161b22;
    --border: #30363d;
    --fg: #c9d1d9;
    --muted: #8b949e;
    --accent: #58a6ff;
    --green: #3fb950;
    --yellow: #d29922;
    --red: #f85149;
    --system: #d2a8ff;
    --lib: #58a6ff;
    --external: #f0883e;
    --external-git: #a371f7;
  }
  * { box-sizing: border-box; }
  body, html { margin: 0; padding: 0; height: 100%; background: var(--bg);
    color: var(--fg); font: 14px/1.4 -apple-system, BlinkMacSystemFont,
    "Segoe UI", system-ui, sans-serif; }
  #app { display: grid; grid-template-columns: 1fr 360px; height: 100vh; }
  #cy { background: var(--bg); }
  #sidebar { background: var(--panel); border-left: 1px solid var(--border);
    overflow-y: auto; padding: 18px; }
  header { padding: 12px 18px; border-bottom: 1px solid var(--border);
    background: var(--panel); display: flex; justify-content: space-between;
    align-items: center; gap: 16px; }
  header h1 { font-size: 16px; margin: 0; }
  header .meta { color: var(--muted); font-size: 12px; }
  #controls { display: flex; gap: 12px; flex-wrap: wrap; align-items: center; }
  #controls label { color: var(--muted); font-size: 12px; }
  #controls input[type=text] { background: var(--bg); color: var(--fg);
    border: 1px solid var(--border); padding: 4px 8px; border-radius: 4px;
    width: 180px; }
  .legend { display: flex; gap: 14px; flex-wrap: wrap; font-size: 12px;
    color: var(--muted); }
  .legend .swatch { display: inline-block; width: 10px; height: 10px;
    border-radius: 50%; margin-right: 4px; vertical-align: middle; }
  .swatch.system { background: var(--system); }
  .swatch.lib { background: var(--lib); }
  .swatch.ext { background: var(--external); }
  .swatch.ext-git { background: var(--external-git); }
  .swatch.maint-green { background: var(--green); }
  .swatch.maint-yellow { background: var(--yellow); }
  .swatch.maint-red { background: var(--red); }
  #sidebar h2 { font-size: 14px; margin: 0 0 8px; }
  #sidebar h3 { font-size: 12px; margin: 14px 0 4px; color: var(--muted);
    text-transform: uppercase; letter-spacing: 0.04em; }
  #sidebar .row { display: flex; justify-content: space-between; gap: 8px;
    padding: 2px 0; border-bottom: 1px dotted var(--border); }
  #sidebar .row .k { color: var(--muted); }
  #sidebar a { color: var(--accent); text-decoration: none; word-break: break-all; }
  #sidebar a:hover { text-decoration: underline; }
  .badge { display: inline-block; padding: 1px 6px; border-radius: 10px;
    background: var(--border); color: var(--fg); font-size: 11px; }
  .badge.green { background: rgba(63,185,80,0.16); color: var(--green); }
  .badge.yellow { background: rgba(210,153,34,0.16); color: var(--yellow); }
  .badge.red { background: rgba(248,81,73,0.16); color: var(--red); }
  .dep-list { font-size: 12px; max-height: 200px; overflow-y: auto; }
  .dep-list .item { padding: 2px 0; cursor: pointer; }
  .dep-list .item:hover { color: var(--accent); }
  .empty-hint { color: var(--muted); font-size: 12px; padding: 20px 0; }
  button.tab { background: transparent; color: var(--muted); border: 1px solid
    var(--border); padding: 4px 10px; border-radius: 4px; cursor: pointer;
    font-size: 12px; }
  button.tab.active { color: var(--fg); border-color: var(--accent); }
  .checks { display: flex; gap: 8px; }
</style>
</head>
<body>
<header>
  <div>
    <h1>k23 dependency map</h1>
    <div class="meta" id="meta"></div>
  </div>
  <div id="controls">
    <input type="text" id="search" placeholder="Filter (substring)…">
    <div class="checks">
      <button class="tab active" data-layout="cose-bilkent">force</button>
      <button class="tab" data-layout="breadthfirst">layered</button>
      <button class="tab" data-layout="concentric">concentric</button>
    </div>
  </div>
</header>
<div id="app">
  <div id="cy"></div>
  <div id="sidebar">
    <h2>Legend</h2>
    <div class="legend">
      <span><span class="swatch system"></span>system / kernel</span>
      <span><span class="swatch lib"></span>internal lib</span>
      <span><span class="swatch ext"></span>external (crates.io)</span>
      <span><span class="swatch ext-git"></span>external (git/fork)</span>
    </div>
    <h3>Maintenance signal (external)</h3>
    <div class="legend">
      <span><span class="swatch maint-green"></span>≤ 1 year</span>
      <span><span class="swatch maint-yellow"></span>≤ 2 years</span>
      <span><span class="swatch maint-red"></span>&gt; 2 years</span>
    </div>
    <div id="detail">
      <p class="empty-hint">
        Click a node to inspect it. Node size reflects fan-in (how many other
        crates depend on it). Border ring reflects maintenance signal for
        external crates.
      </p>
    </div>
  </div>
</div>
<script src="https://cdn.jsdelivr.net/npm/cytoscape@3.30.4/dist/cytoscape.min.js"></script>
<script src="https://cdn.jsdelivr.net/npm/layout-base@2.0.1/layout-base.js"></script>
<script src="https://cdn.jsdelivr.net/npm/cose-base@2.2.0/cose-base.js"></script>
<script src="https://cdn.jsdelivr.net/npm/cytoscape-cose-bilkent@4.1.0/cytoscape-cose-bilkent.js"></script>
<script id="graph-data" type="application/json">__GRAPH_JSON__</script>
<script>
(() => {
  const data = JSON.parse(document.getElementById('graph-data').textContent);
  document.getElementById('meta').textContent =
    `${data.nodes.length} nodes · ${data.edges.length} edges · generated ${data.generated_at}`;

  const colorFor = (n) => {
    if (n.category === 'external') {
      return n.source === 'git' ? '#a371f7' : '#f0883e';
    }
    if (n.category === 'lib') return '#58a6ff';
    return '#d2a8ff';
  };
  const borderFor = (n) => {
    const m = n.metrics && n.metrics.maintenance;
    if (m === 'green')  return '#3fb950';
    if (m === 'yellow') return '#d29922';
    if (m === 'red')    return '#f85149';
    return '#30363d';
  };
  const sizeFor = (n) => {
    const f = n.fan_in || 0;
    return Math.max(18, Math.min(70, 18 + Math.sqrt(f) * 10));
  };

  const elements = [
    ...data.nodes.map(n => ({
      data: {
        id: n.id, label: n.label, category: n.category, source: n.source,
        color: colorFor(n), border: borderFor(n), size: sizeFor(n),
        node: n,
      },
    })),
    ...data.edges.map((e, i) => ({
      data: { id: 'e' + i, source: e.source, target: e.target },
    })),
  ];

  const cy = cytoscape({
    container: document.getElementById('cy'),
    elements,
    wheelSensitivity: 0.2,
    style: [
      { selector: 'node', style: {
        'background-color': 'data(color)',
        'border-width': 2.5,
        'border-color': 'data(border)',
        'label': 'data(label)',
        'color': '#c9d1d9',
        'text-outline-color': '#0e1117',
        'text-outline-width': 2,
        'font-size': 9,
        'width': 'data(size)',
        'height': 'data(size)',
        'text-valign': 'bottom',
        'text-margin-y': 2,
      }},
      { selector: 'edge', style: {
        'width': 0.7,
        'line-color': '#30363d',
        'target-arrow-color': '#30363d',
        'target-arrow-shape': 'triangle',
        'arrow-scale': 0.6,
        'curve-style': 'bezier',
        'opacity': 0.55,
      }},
      { selector: 'node.dim',  style: { 'opacity': 0.12 }},
      { selector: 'edge.dim',  style: { 'opacity': 0.05 }},
      { selector: 'node.hi',   style: { 'border-width': 4 }},
      { selector: 'edge.hi',   style: {
        'line-color': '#58a6ff', 'target-arrow-color': '#58a6ff',
        'opacity': 1, 'width': 1.6,
      }},
    ],
    layout: { name: 'cose-bilkent', idealEdgeLength: 90,
              nodeRepulsion: 4500, edgeElasticity: 0.45,
              animate: false, randomize: true, fit: true },
  });

  function highlight(node) {
    cy.elements().addClass('dim').removeClass('hi');
    if (!node) return;
    const me = cy.getElementById(node.id);
    const neigh = me.closedNeighborhood();
    neigh.removeClass('dim').addClass('hi');
  }

  function renderDetail(n) {
    const d = document.getElementById('detail');
    if (!n) { d.innerHTML = '<p class="empty-hint">Click a node to inspect it.</p>'; return; }
    const m = n.metrics || {};
    const rows = [];
    const row = (k, v) => v == null ? '' :
      `<div class="row"><span class="k">${k}</span><span>${v}</span></div>`;
    const link = (label, url) => url ? `<a href="${url}" target="_blank" rel="noopener">${label}</a>` : '';
    rows.push(row('target', `<code>${n.id}</code>`));
    rows.push(row('category', n.category + (n.source ? ` · ${n.source}` : '')));
    rows.push(row('fan-in', n.fan_in));
    if (n.category === 'external') {
      rows.push(row('crate', n.crate_name));
      rows.push(row('version', n.version));
      if (m.latest_stable_version && m.latest_stable_version !== n.version) {
        rows.push(row('latest stable', m.latest_stable_version));
      }
      rows.push(row('license', m.license));
      if (m.crate_size != null) {
        const kb = (m.crate_size / 1024).toFixed(1);
        rows.push(row('crate size', `${kb} KB`));
      }
      rows.push(row('downloads (90d)', m.downloads_recent?.toLocaleString()));
      rows.push(row('downloads (total)', m.downloads_total?.toLocaleString()));
      if (m.days_since_last_release != null) {
        const cls = m.maintenance || 'unknown';
        rows.push(row('last release',
          `<span class="badge ${cls}">${m.days_since_last_release} days ago</span>`));
      }
      rows.push(row('crates.io',
        link(n.crate_name, `https://crates.io/crates/${n.crate_name}/${n.version}`)));
      rows.push(row('repository', link(m.repository || '', m.repository)));
      if (n.git_url) rows.push(row('git source', link(n.git_url, n.git_url)));
      if (m.description) rows.push(`<p style="margin:8px 0; color:var(--muted)">${m.description}</p>`);
      if (m.fetch_error) rows.push(row('crates.io error', `<span class="badge red">${m.fetch_error}</span>`));
    }
    const directDeps = data.edges.filter(e => e.source === n.id).map(e => e.target);
    const dependents = data.edges.filter(e => e.target === n.id).map(e => e.source);
    let html = `<h2>${n.label}</h2>` + rows.join('');
    if (directDeps.length) {
      html += `<h3>Direct deps (${directDeps.length})</h3><div class="dep-list">`;
      for (const id of directDeps.slice(0, 200)) {
        html += `<div class="item" data-jump="${id}">${id}</div>`;
      }
      html += '</div>';
    }
    if (dependents.length) {
      html += `<h3>Dependents (${dependents.length})</h3><div class="dep-list">`;
      for (const id of dependents.slice(0, 200)) {
        html += `<div class="item" data-jump="${id}">${id}</div>`;
      }
      html += '</div>';
    }
    d.innerHTML = html;
    d.querySelectorAll('[data-jump]').forEach(el => {
      el.addEventListener('click', () => {
        const target = cy.getElementById(el.dataset.jump);
        if (target.length) {
          cy.animate({ center: { eles: target }, zoom: 1.4, duration: 250 });
          const node = data.nodes.find(x => x.id === el.dataset.jump);
          highlight(node);
          renderDetail(node);
        }
      });
    });
  }

  cy.on('tap', 'node', evt => {
    const n = evt.target.data('node');
    highlight(n);
    renderDetail(n);
  });
  cy.on('tap', evt => {
    if (evt.target === cy) {
      cy.elements().removeClass('dim').removeClass('hi');
      renderDetail(null);
    }
  });

  document.getElementById('search').addEventListener('input', e => {
    const q = e.target.value.trim().toLowerCase();
    if (!q) {
      cy.elements().removeClass('dim');
      return;
    }
    cy.nodes().forEach(node => {
      const n = node.data('node');
      const hit = n.id.toLowerCase().includes(q)
        || (n.crate_name || '').toLowerCase().includes(q);
      node[hit ? 'removeClass' : 'addClass']('dim');
    });
    cy.edges().forEach(edge => {
      const s = edge.source().hasClass('dim');
      const t = edge.target().hasClass('dim');
      edge[(s || t) ? 'addClass' : 'removeClass']('dim');
    });
  });

  document.querySelectorAll('button.tab').forEach(btn => {
    btn.addEventListener('click', () => {
      document.querySelectorAll('button.tab').forEach(b => b.classList.remove('active'));
      btn.classList.add('active');
      const name = btn.dataset.layout;
      const opts = name === 'cose-bilkent'
        ? { name, idealEdgeLength: 90, nodeRepulsion: 4500, edgeElasticity: 0.45,
            animate: false, randomize: true, fit: true }
        : { name, animate: false, fit: true, spacingFactor: 1.1 };
      cy.layout(opts).run();
    });
  });
})();
</script>
</body>
</html>
"""


def render_html(graph: dict[str, Any], out_path: Path) -> None:
    payload = json.dumps(graph, separators=(",", ":"))
    # JSON inside a <script type="application/json"> only needs to avoid
    # the literal string `</script` (case-insensitive). The standard escape
    # is to break it with a backslash. We also escape forward-slash defensively.
    payload = payload.replace("</", "<\\/")
    out = HTML_TEMPLATE.replace("__GRAPH_JSON__", payload)
    out_path.write_text(out)


# ---------------------------------------------------------------------------
# Diff mode — base vs head JSONs → markdown + (optional) SVG.
# ---------------------------------------------------------------------------


def _index_externals(graph: dict[str, Any]) -> dict[str, dict[str, Any]]:
    out: dict[str, dict[str, Any]] = {}
    for n in graph["nodes"]:
        if n["category"] == "external":
            out[n.get("crate_name", n["label"])] = n
    return out


def _index_internals(graph: dict[str, Any]) -> dict[str, dict[str, Any]]:
    return {n["id"]: n for n in graph["nodes"] if n["category"] != "external"}


def render_diff_markdown(base: dict[str, Any], head: dict[str, Any]) -> str:
    b_ext = _index_externals(base)
    h_ext = _index_externals(head)
    b_int = _index_internals(base)
    h_int = _index_internals(head)

    added_ext = sorted(set(h_ext) - set(b_ext))
    removed_ext = sorted(set(b_ext) - set(h_ext))
    changed_ext: list[tuple[str, str, str]] = []
    for name in sorted(set(h_ext) & set(b_ext)):
        bv = b_ext[name].get("version")
        hv = h_ext[name].get("version")
        if bv != hv:
            changed_ext.append((name, bv or "?", hv or "?"))

    added_int = sorted(set(h_int) - set(b_int))
    removed_int = sorted(set(b_int) - set(h_int))

    lines: list[str] = []
    lines.append("<!-- dep-map-diff -->")
    lines.append("## 📦 Dependency map diff")
    lines.append("")
    if not (added_ext or removed_ext or changed_ext or added_int or removed_int):
        lines.append("No dependency changes detected.")
        return "\n".join(lines) + "\n"

    base_counts = (
        len([n for n in base["nodes"] if n["category"] == "external"]),
        len([n for n in base["nodes"] if n["category"] != "external"]),
    )
    head_counts = (
        len([n for n in head["nodes"] if n["category"] == "external"]),
        len([n for n in head["nodes"] if n["category"] != "external"]),
    )
    lines.append(
        f"**External crates:** {base_counts[0]} → {head_counts[0]} "
        f"(`{head_counts[0] - base_counts[0]:+d}`) · "
        f"**Internal targets:** {base_counts[1]} → {head_counts[1]} "
        f"(`{head_counts[1] - base_counts[1]:+d}`)"
    )
    lines.append("")

    def crate_link(node: dict[str, Any]) -> str:
        name = node.get("crate_name") or node["label"]
        ver = node.get("version") or ""
        if node.get("source") == "registry":
            return f"[`{name} {ver}`](https://crates.io/crates/{name}/{ver})"
        if node.get("source") == "git":
            return f"`{name} {ver}` (git: {node.get('git_url','?')})"
        return f"`{name} {ver}`"

    def fmt_metrics(node: dict[str, Any]) -> str:
        m = node.get("metrics") or {}
        bits = []
        if m.get("license"):
            bits.append(f"license: `{m['license']}`")
        if m.get("downloads_recent") is not None:
            bits.append(f"90d-dl: `{m['downloads_recent']:,}`")
        if m.get("crate_size") is not None:
            bits.append(f"size: `{m['crate_size']/1024:.1f} KB`")
        if m.get("maintenance"):
            emoji = {"green": "🟢", "yellow": "🟡", "red": "🔴"}.get(
                m["maintenance"], "⚪"
            )
            d = m.get("days_since_last_release")
            bits.append(f"{emoji} last release: {d}d ago" if d is not None else f"{emoji}")
        return " · ".join(bits) if bits else "—"

    if added_ext:
        lines.append(f"### ➕ Added external crates ({len(added_ext)})")
        lines.append("")
        lines.append("| crate | metrics |")
        lines.append("|---|---|")
        for name in added_ext:
            n = h_ext[name]
            lines.append(f"| {crate_link(n)} | {fmt_metrics(n)} |")
        lines.append("")

    if removed_ext:
        lines.append(f"### ➖ Removed external crates ({len(removed_ext)})")
        lines.append("")
        for name in removed_ext:
            n = b_ext[name]
            lines.append(f"- {crate_link(n)}")
        lines.append("")

    if changed_ext:
        lines.append(f"### 🔀 Version changes ({len(changed_ext)})")
        lines.append("")
        lines.append("| crate | base | head | metrics |")
        lines.append("|---|---|---|---|")
        for name, bv, hv in changed_ext:
            n = h_ext[name]
            lines.append(
                f"| `{name}` | `{bv}` | `{hv}` | {fmt_metrics(n)} |"
            )
        lines.append("")

    if added_int:
        lines.append(f"### 🧩 Added internal targets ({len(added_int)})")
        lines.append("")
        for t in added_int:
            lines.append(f"- `{t}`")
        lines.append("")

    if removed_int:
        lines.append(f"### 🗑 Removed internal targets ({len(removed_int)})")
        lines.append("")
        for t in removed_int:
            lines.append(f"- `{t}`")
        lines.append("")

    lines.append(
        "Full graph SVG and interactive HTML are attached as workflow artifacts. "
        "The live map (main branch) lives at `dep-map.html` in the manual."
    )
    return "\n".join(lines) + "\n"


def render_diff_dot(base: dict[str, Any], head: dict[str, Any]) -> str:
    """Graphviz DOT showing additions (green), removals (red), and shared (grey)."""
    b_nodes = {n["id"] for n in base["nodes"]}
    h_nodes = {n["id"] for n in head["nodes"]}
    b_edges = {(e["source"], e["target"]) for e in base["edges"]}
    h_edges = {(e["source"], e["target"]) for e in head["edges"]}

    all_nodes = {n["id"]: n for n in (*base["nodes"], *head["nodes"])}
    lines = [
        "digraph dep_map {",
        '  graph [bgcolor="#0e1117", overlap=false, splines=true, rankdir=LR];',
        '  node  [style="filled", fontname="Helvetica", fontsize=10, '
        '         fontcolor="#c9d1d9", color="#30363d"];',
        '  edge  [color="#3036407f"];',
    ]
    for nid, n in all_nodes.items():
        if nid in b_nodes and nid in h_nodes:
            fill = "#1f2937"
        elif nid in h_nodes:
            fill = "#1f5132"  # added
        else:
            fill = "#5a1f24"  # removed
        shape = "ellipse" if n["category"] == "external" else "box"
        label = n["label"].replace('"', "")
        lines.append(f'  "{nid}" [label="{label}", fillcolor="{fill}", shape={shape}];')
    for s, t in sorted(b_edges | h_edges):
        if (s, t) in b_edges and (s, t) in h_edges:
            color = "#30364080"
        elif (s, t) in h_edges:
            color = "#3fb950"
        else:
            color = "#f85149"
        lines.append(f'  "{s}" -> "{t}" [color="{color}"];')
    lines.append("}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# CLI.
# ---------------------------------------------------------------------------


def cmd_gen(args: argparse.Namespace) -> int:
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    # Keep the cache outside `out-dir` so it doesn't leak into mdbook's
    # published output (mdbook copies every non-`.md` file under src/).
    cache_path = (
        Path(args.cache).resolve()
        if args.cache
        else REPO_ROOT / ".dep-map-cache.json"
    )
    cache: dict[str, Any] = {}
    if cache_path.exists() and not args.no_cache:
        try:
            cache = json.loads(cache_path.read_text())
        except json.JSONDecodeError:
            cache = {}

    if args.graph_input:
        buck_graph = json.loads(Path(args.graph_input).read_text())
    else:
        print("[dep-map] running buck2 uquery...", file=sys.stderr)
        buck_graph = extract_buck_graph()
        if args.graph_output:
            Path(args.graph_output).write_text(json.dumps(buck_graph, indent=2))

    aliases = parse_third_party_aliases(THIRD_PARTY_BUCK)
    locked = parse_cargo_lock(CARGO_LOCK)
    graph = build_graph(
        buck_graph, aliases, locked, cache, skip_crates_io=args.skip_crates_io
    )

    json_out = out_dir / "dep-map.json"
    json_out.write_text(json.dumps(graph, indent=2))
    print(f"[dep-map] wrote {json_out}", file=sys.stderr)

    html_out = out_dir / "dep-map.html"
    render_html(graph, html_out)
    print(f"[dep-map] wrote {html_out}", file=sys.stderr)

    if not args.no_cache:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        cache_path.write_text(json.dumps(cache, indent=2))

    return 0


def cmd_diff(args: argparse.Namespace) -> int:
    base = json.loads(Path(args.base).read_text())
    head = json.loads(Path(args.head).read_text())
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    md = render_diff_markdown(base, head)
    md_path = out_dir / "dep-map-diff.md"
    md_path.write_text(md)
    print(f"[dep-map] wrote {md_path}", file=sys.stderr)

    dot = render_diff_dot(base, head)
    (out_dir / "dep-map-diff.dot").write_text(dot)

    # Try to render SVG via Graphviz if available; non-fatal otherwise.
    try:
        proc = subprocess.run(
            ["dot", "-Tsvg", "-o", str(out_dir / "dep-map-diff.svg")],
            input=dot,
            text=True,
            check=True,
            capture_output=True,
        )
        print(f"[dep-map] wrote {out_dir / 'dep-map-diff.svg'}", file=sys.stderr)
    except FileNotFoundError:
        print("[dep-map] graphviz `dot` not found; skipping SVG", file=sys.stderr)
    except subprocess.CalledProcessError as e:
        print(
            f"[dep-map] graphviz failed: {e.stderr}; skipping SVG", file=sys.stderr
        )

    return 0


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(prog="dep-map", description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    g = sub.add_parser("gen", help="generate dep-map.{json,html}")
    g.add_argument("--out-dir", default=str(REPO_ROOT / "manual" / "src"),
                   help="where dep-map.{json,html} land (default: manual/src)")
    g.add_argument("--graph-input", default=None,
                   help="read pre-recorded buck2 output instead of running buck2")
    g.add_argument("--graph-output", default=None,
                   help="also save the raw buck2 output for later replay")
    g.add_argument("--skip-crates-io", action="store_true",
                   help="don't reach out to crates.io (offline mode)")
    g.add_argument("--no-cache", action="store_true",
                   help="don't read/write the crates.io response cache")
    g.add_argument("--cache", default=None,
                   help="crates.io response cache path (default: <repo>/.dep-map-cache.json)")
    g.set_defaults(func=cmd_gen)

    d = sub.add_parser("diff", help="emit a markdown + SVG diff between two dep-map.json files")
    d.add_argument("--base", required=True)
    d.add_argument("--head", required=True)
    d.add_argument("--out-dir", default=str(REPO_ROOT / "dep-map-diff"))
    d.set_defaults(func=cmd_diff)

    args = p.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
