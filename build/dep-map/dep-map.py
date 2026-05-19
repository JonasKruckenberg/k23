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
          markdown summary plus a Graphviz SVG highlighting added/removed
          /version-changed crates.
"""

from __future__ import annotations

import argparse
import datetime as _dt
import json
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
TEMPLATE = Path(__file__).resolve().parent / "template.html"

CRATES_IO_API = "https://crates.io/api/v1/crates"
USER_AGENT = "k23-dep-map (https://github.com/JonasKruckenberg/k23)"

MAINT_GREEN_DAYS = 365
MAINT_YELLOW_DAYS = 365 * 2

# Drop reindeer's auxiliary buildscript targets — they swamp the graph and
# aren't deps a human cares about when judging risk.
DEP_FILTER = re.compile(r"-(build_script(_build|_run)?(-[^/]+)?|pre-build-script)$")


# ---------------------------------------------------------------------------
# Cargo.lock — canonical (name, version, source) for every external crate.
# ---------------------------------------------------------------------------


def parse_cargo_lock(path: Path) -> dict[str, dict[str, Any]]:
    """Returns {`name-version`: {name, version, source, git_url}}."""
    data = tomllib.loads(path.read_text())
    out: dict[str, dict[str, Any]] = {}
    for pkg in data.get("package", []):
        src = pkg.get("source")
        if src is None:
            kind, git_url = "path", None
        elif src.startswith("registry+"):
            kind, git_url = "registry", None
        elif src.startswith("git+"):
            kind = "git"
            git_url = src[len("git+") :].split("?", 1)[0].split("#", 1)[0]
        else:
            kind, git_url = "other", None
        slug = f"{pkg['name']}-{pkg['version']}"
        out[slug] = {
            "name": pkg["name"],
            "version": pkg["version"],
            "source": kind,
            "git_url": git_url,
        }
    return out


# ---------------------------------------------------------------------------
# buck2 uquery — fetch the (target → deps) graph and alias map in one shot.
# ---------------------------------------------------------------------------


def _buck2_uquery(query: str, attrs: str) -> dict[str, dict[str, Any]]:
    cmd = ["buck2", "uquery", "--output-format", "json",
           "--output-attribute", attrs, query]
    proc = subprocess.run(cmd, cwd=REPO_ROOT, check=True,
                          capture_output=True, text=True)
    return json.loads(proc.stdout) if proc.stdout.strip() else {}


def extract_buck_graph() -> tuple[dict[str, dict[str, Any]], dict[str, str]]:
    """Returns (graph: {target: {kind, deps}}, aliases: {alias_label: actual_label})."""
    universe = "set(//lib/... //sys/... //third-party/...)"
    raw = _buck2_uquery(
        f'kind("^(rust_(library|binary|test)|alias)$", {universe})',
        "^(buck.type|deps|named_deps|actual)$",
    )
    graph: dict[str, dict[str, Any]] = {}
    aliases: dict[str, str] = {}
    for tgt, attrs in raw.items():
        kind = attrs.get("buck.type") or ""
        if kind == "alias":
            actual = attrs.get("actual", "").lstrip(":")
            if actual and "//" not in actual:
                # Same-package alias like `:foo-1.2.3` → expand to full label.
                actual = f"{tgt.rsplit(':', 1)[0]}:{actual}"
            if actual:
                aliases[tgt] = actual
            continue
        deps = list(attrs.get("deps") or [])
        named = attrs.get("named_deps") or {}
        if isinstance(named, dict):
            deps.extend(named.values())
        graph[tgt] = {
            "kind": kind,
            "deps": [d for d in deps if not DEP_FILTER.search(d)],
        }
    return graph, aliases


# ---------------------------------------------------------------------------
# crates.io enrichment.
# ---------------------------------------------------------------------------


def _http_get_json(url: str, retries: int = 3) -> dict[str, Any]:
    last_err: Exception | None = None
    for i in range(retries):
        req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
        try:
            with urllib.request.urlopen(req, timeout=20) as resp:
                return json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as e:
            if e.code == 429 or 500 <= e.code < 600:
                time.sleep(2 ** (i + (1 if e.code == 429 else 0)))
                last_err = e
                continue
            raise
        except (urllib.error.URLError, TimeoutError) as e:
            time.sleep(2**i)
            last_err = e
    raise RuntimeError(f"GET {url} failed after {retries} tries: {last_err}")


def _maintenance(days: int | None) -> str:
    if days is None:
        return "unknown"
    if days <= MAINT_GREEN_DAYS:
        return "green"
    if days <= MAINT_YELLOW_DAYS:
        return "yellow"
    return "red"


def fetch_crate_metrics(name: str, version: str,
                        cache: dict[str, Any]) -> dict[str, Any]:
    key = f"{name}@{version}"
    if key in cache:
        return cache[key]

    metrics: dict[str, Any] = {"maintenance": "unknown"}
    try:
        data = _http_get_json(f"{CRATES_IO_API}/{name}")
    except Exception as e:
        metrics["fetch_error"] = str(e)
        cache[key] = metrics
        return metrics

    crate = data.get("crate") or {}
    versions = data.get("versions") or []
    selected = next((v for v in versions if v.get("num") == version), None)
    if selected is None:
        try:
            selected = _http_get_json(
                f"{CRATES_IO_API}/{name}/{version}"
            ).get("version") or {}
        except Exception:
            selected = {}

    ref_ts = (versions[0].get("updated_at") if versions else None) \
        or selected.get("updated_at") or selected.get("created_at")
    days = None
    if ref_ts:
        try:
            ts = _dt.datetime.fromisoformat(ref_ts.replace("Z", "+00:00"))
            days = (_dt.datetime.now(tz=_dt.timezone.utc) - ts).days
        except ValueError:
            pass

    metrics.update({
        "license": selected.get("license"),
        "repository": crate.get("repository"),
        "homepage": crate.get("homepage"),
        "description": (crate.get("description") or "").strip() or None,
        "downloads_total": crate.get("downloads"),
        "downloads_recent": crate.get("recent_downloads"),
        "crate_size": selected.get("crate_size"),
        "latest_stable_version": crate.get("max_stable_version")
                                 or crate.get("max_version"),
        "days_since_last_release": days,
        "maintenance": _maintenance(days),
    })
    cache[key] = metrics
    return metrics


# ---------------------------------------------------------------------------
# Graph assembly.
# ---------------------------------------------------------------------------


def _short(target: str) -> str:
    return target.rsplit(":", 1)[1] if ":" in target else target


def _category(target: str) -> str:
    if target.startswith("//third-party:"):
        return "external"
    if target.startswith("//lib/"):
        return "lib"
    return "system"


def build_graph(buck_graph: dict[str, dict[str, Any]],
                aliases: dict[str, str],
                lock_by_slug: dict[str, dict[str, Any]],
                cache: dict[str, Any],
                skip_crates_io: bool) -> dict[str, Any]:
    resolve = lambda label: aliases.get(label, label)

    def make_node(target: str, kind: str | None) -> dict[str, Any] | None:
        category = _category(target)
        if category == "external":
            locked = lock_by_slug.get(_short(target))
            if not locked:
                return None  # alias or unknown third-party target
            return {
                "id": target, "label": _short(target), "category": "external",
                "buck_kind": kind, "metrics": {},
                "crate_name": locked["name"], "version": locked["version"],
                "source": locked["source"], "git_url": locked["git_url"],
            }
        return {"id": target, "label": _short(target), "category": category,
                "buck_kind": kind}

    nodes: dict[str, dict[str, Any]] = {}
    for tgt, attrs in buck_graph.items():
        node = make_node(tgt, attrs.get("kind"))
        if node:
            nodes[tgt] = node

    edges: set[tuple[str, str]] = set()
    for src, attrs in buck_graph.items():
        if src not in nodes:
            continue  # aliases / non-package sources
        for dep in attrs["deps"]:
            tgt = resolve(dep)
            if tgt in nodes:
                edges.add((src, tgt))

    if not skip_crates_io:
        externals = [n for n in nodes.values()
                     if n["category"] == "external" and n["source"] == "registry"]
        misses = sum(1 for n in externals if f"{n['crate_name']}@{n['version']}" not in cache)
        if misses:
            print(f"[dep-map] fetching crates.io metadata for {misses}/{len(externals)} crates...",
                  file=sys.stderr)
        for n in externals:
            n["metrics"] = fetch_crate_metrics(n["crate_name"], n["version"], cache)

    fan_in: dict[str, int] = {t: 0 for t in nodes}
    for _, tgt in edges:
        fan_in[tgt] += 1
    for tid, n in nodes.items():
        n["fan_in"] = fan_in[tid]

    return {
        "generated_at": _dt.datetime.now(tz=_dt.timezone.utc)
                          .isoformat(timespec="seconds").replace("+00:00", "Z"),
        "nodes": list(nodes.values()),
        "edges": [{"source": s, "target": t} for s, t in sorted(edges)],
    }


# ---------------------------------------------------------------------------
# Rendering.
# ---------------------------------------------------------------------------


def render_html(graph: dict[str, Any], out_path: Path) -> None:
    payload = json.dumps(graph, separators=(",", ":")) \
        .replace("</script", "<\\/script")
    out_path.write_text(TEMPLATE.read_text().replace("__GRAPH_JSON__", payload))


MAINT_EMOJI = {"green": "🟢", "yellow": "🟡", "red": "🔴"}


def _crate_link(node: dict[str, Any]) -> str:
    name, ver = node.get("crate_name", node["label"]), node.get("version", "")
    if node.get("source") == "registry":
        return f"[`{name} {ver}`](https://crates.io/crates/{name}/{ver})"
    if node.get("source") == "git":
        return f"`{name} {ver}` (git: {node.get('git_url','?')})"
    return f"`{name} {ver}`"


def _fmt_metrics(node: dict[str, Any]) -> str:
    m = node.get("metrics") or {}
    bits = []
    if m.get("license"):
        bits.append(f"license: `{m['license']}`")
    if m.get("downloads_recent") is not None:
        bits.append(f"90d-dl: `{m['downloads_recent']:,}`")
    if m.get("crate_size") is not None:
        bits.append(f"size: `{m['crate_size']/1024:.1f} KB`")
    if m.get("maintenance"):
        emoji = MAINT_EMOJI.get(m["maintenance"], "⚪")
        d = m.get("days_since_last_release")
        bits.append(f"{emoji} last release: {d}d ago" if d is not None else emoji)
    return " · ".join(bits) if bits else "—"


def render_diff_markdown(base: dict[str, Any], head: dict[str, Any]) -> str:
    def split(graph):
        ext, internal = {}, {}
        for n in graph["nodes"]:
            (ext if n["category"] == "external" else internal)[
                n.get("crate_name", n["label"]) if n["category"] == "external"
                else n["id"]
            ] = n
        return ext, internal

    b_ext, b_int = split(base)
    h_ext, h_int = split(head)
    added_ext = sorted(set(h_ext) - set(b_ext))
    removed_ext = sorted(set(b_ext) - set(h_ext))
    changed_ext = [(n, b_ext[n]["version"], h_ext[n]["version"])
                   for n in sorted(set(h_ext) & set(b_ext))
                   if b_ext[n].get("version") != h_ext[n].get("version")]
    added_int = sorted(set(h_int) - set(b_int))
    removed_int = sorted(set(b_int) - set(h_int))

    lines = ["<!-- dep-map-diff -->", "## 📦 Dependency map diff", ""]
    if not (added_ext or removed_ext or changed_ext or added_int or removed_int):
        return "\n".join(lines + ["No dependency changes detected.", ""])

    lines.append(
        f"**External crates:** {len(b_ext)} → {len(h_ext)} (`{len(h_ext)-len(b_ext):+d}`) · "
        f"**Internal targets:** {len(b_int)} → {len(h_int)} (`{len(h_int)-len(b_int):+d}`)"
    )
    lines.append("")

    def table(title: str, header: str, rows: list[str]) -> None:
        if not rows:
            return
        cols = header.count("|") - 1
        lines.extend([f"### {title}", "", header,
                      "|" + "---|" * cols, *rows, ""])

    def listing(title: str, rows: list[str]) -> None:
        if not rows:
            return
        lines.extend([f"### {title}", "", *rows, ""])

    table(f"➕ Added external crates ({len(added_ext)})",
          "| crate | metrics |",
          [f"| {_crate_link(h_ext[n])} | {_fmt_metrics(h_ext[n])} |"
           for n in added_ext])
    listing(f"➖ Removed external crates ({len(removed_ext)})",
            [f"- {_crate_link(b_ext[n])}" for n in removed_ext])
    table(f"🔀 Version changes ({len(changed_ext)})",
          "| crate | base | head | metrics |",
          [f"| `{n}` | `{bv}` | `{hv}` | {_fmt_metrics(h_ext[n])} |"
           for n, bv, hv in changed_ext])
    listing(f"🧩 Added internal targets ({len(added_int)})",
            [f"- `{t}`" for t in added_int])
    listing(f"🗑 Removed internal targets ({len(removed_int)})",
            [f"- `{t}`" for t in removed_int])

    lines.append(
        "Full graph SVG and interactive HTML are attached as workflow artifacts. "
        "The live map (main branch) lives at `dep-map.html` in the manual."
    )
    return "\n".join(lines) + "\n"


def render_diff_dot(base: dict[str, Any], head: dict[str, Any]) -> str:
    b_nodes = {n["id"] for n in base["nodes"]}
    h_nodes = {n["id"] for n in head["nodes"]}
    b_edges = {(e["source"], e["target"]) for e in base["edges"]}
    h_edges = {(e["source"], e["target"]) for e in head["edges"]}
    all_nodes = {n["id"]: n for n in (*base["nodes"], *head["nodes"])}

    def fill(nid: str) -> str:
        if nid in b_nodes and nid in h_nodes: return "#1f2937"
        return "#1f5132" if nid in h_nodes else "#5a1f24"

    def color(edge: tuple[str, str]) -> str:
        in_b, in_h = edge in b_edges, edge in h_edges
        if in_b and in_h: return "#30364080"
        return "#3fb950" if in_h else "#f85149"

    lines = [
        "digraph dep_map {",
        '  graph [bgcolor="#0e1117", overlap=false, splines=true, rankdir=LR];',
        '  node  [style="filled", fontname="Helvetica", fontsize=10,'
        ' fontcolor="#c9d1d9", color="#30363d"];',
        '  edge  [color="#3036407f"];',
    ]
    for nid, n in all_nodes.items():
        shape = "ellipse" if n["category"] == "external" else "box"
        label = n["label"].replace('"', "")
        lines.append(f'  "{nid}" [label="{label}", fillcolor="{fill(nid)}", shape={shape}];')
    for edge in sorted(b_edges | h_edges):
        lines.append(f'  "{edge[0]}" -> "{edge[1]}" [color="{color(edge)}"];')
    lines.append("}")
    return "\n".join(lines)


# ---------------------------------------------------------------------------
# CLI.
# ---------------------------------------------------------------------------


def cmd_gen(args: argparse.Namespace) -> int:
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)
    # Cache lives outside out_dir so it doesn't leak into mdbook's published
    # output (mdbook copies every non-`.md` file under src/).
    cache_path = Path(args.cache).resolve() if args.cache \
        else REPO_ROOT / ".dep-map-cache.json"
    cache: dict[str, Any] = {}
    if not args.no_cache:
        try:
            cache = json.loads(cache_path.read_text())
        except (FileNotFoundError, json.JSONDecodeError):
            pass

    if args.graph_input:
        buck_graph, aliases = json.loads(Path(args.graph_input).read_text())
    else:
        print("[dep-map] running buck2 uquery...", file=sys.stderr)
        buck_graph, aliases = extract_buck_graph()
        if args.graph_output:
            Path(args.graph_output).write_text(json.dumps([buck_graph, aliases]))

    lock_by_slug = parse_cargo_lock(CARGO_LOCK)
    graph = build_graph(buck_graph, aliases, lock_by_slug, cache,
                        skip_crates_io=args.skip_crates_io)

    (out_dir / "dep-map.json").write_text(json.dumps(graph, indent=2))
    render_html(graph, out_dir / "dep-map.html")
    print(f"[dep-map] wrote {out_dir/'dep-map.json'} + dep-map.html",
          file=sys.stderr)

    if not args.no_cache:
        cache_path.parent.mkdir(parents=True, exist_ok=True)
        cache_path.write_text(json.dumps(cache, indent=2))
    return 0


def cmd_diff(args: argparse.Namespace) -> int:
    base = json.loads(Path(args.base).read_text())
    head = json.loads(Path(args.head).read_text())
    out_dir = Path(args.out_dir).resolve()
    out_dir.mkdir(parents=True, exist_ok=True)

    (out_dir / "dep-map-diff.md").write_text(render_diff_markdown(base, head))
    dot = render_diff_dot(base, head)
    (out_dir / "dep-map-diff.dot").write_text(dot)
    try:
        subprocess.run(["dot", "-Tsvg", "-o", str(out_dir / "dep-map-diff.svg")],
                       input=dot, text=True, check=True, capture_output=True)
    except FileNotFoundError:
        print("[dep-map] graphviz `dot` not found; skipping SVG", file=sys.stderr)
    except subprocess.CalledProcessError as e:
        print(f"[dep-map] graphviz failed: {e.stderr}; skipping SVG", file=sys.stderr)
    print(f"[dep-map] wrote {out_dir/'dep-map-diff.md'} + .dot + .svg",
          file=sys.stderr)
    return 0


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser(prog="dep-map", description=__doc__)
    sub = p.add_subparsers(dest="cmd", required=True)

    g = sub.add_parser("gen", help="generate dep-map.{json,html}")
    g.add_argument("--out-dir", default=str(REPO_ROOT / "manual" / "src"))
    g.add_argument("--graph-input", help="replay a saved buck2 dump")
    g.add_argument("--graph-output", help="save buck2 output for later replay")
    g.add_argument("--skip-crates-io", action="store_true")
    g.add_argument("--no-cache", action="store_true")
    g.add_argument("--cache", help="crates.io cache path (default: <repo>/.dep-map-cache.json)")
    g.set_defaults(func=cmd_gen)

    d = sub.add_parser("diff", help="markdown + SVG diff between two dep-map.json files")
    d.add_argument("--base", required=True)
    d.add_argument("--head", required=True)
    d.add_argument("--out-dir", default=str(REPO_ROOT / "dep-map-diff"))
    d.set_defaults(func=cmd_diff)

    args = p.parse_args(argv)
    return args.func(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
