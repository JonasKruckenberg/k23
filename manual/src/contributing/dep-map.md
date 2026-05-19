# Dependency Map

k23's dependency graph is a perpetual liability surface: every external crate
is code that the kernel will end up linking. To keep that surface visible, the
repo ships an always-updated **interactive dependency map** that doubles as a
high-level map of the project itself.

> 🗺️ **[Open the interactive map →](../dep-map.html)**

The page above is regenerated on every push to `main` and bundled alongside
this manual.

## What it shows

Each node is a buck2 `rust_library` (or `rust_binary` for the system
top-levels), colored by where it comes from:

| Color  | Meaning                                          |
| ------ | ------------------------------------------------ |
| 🟣 purple | `//sys/...` — kernel, loader, async runtime   |
| 🔵 blue   | `//lib/...` — first-party libraries           |
| 🟠 orange | `//third-party/...` — registry crates         |
| 🟪 violet | `//third-party/...` — git / forked sources    |

Each external node carries a ring whose color is a **maintenance
traffic-light**:

- 🟢 a release was published in the last year
- 🟡 a release was published within the last two years
- 🔴 no release in over two years (or upstream archived)

Node size is proportional to **fan-in** — how many other crates pull this one
in. Big nodes are the load-bearing dependencies; small leaves are easy to
swap.

Click a node to see crates.io metrics (license, popularity, .crate file size,
last release, repository link) and walk its direct deps / dependents.

## Regenerating locally

```sh
just dep-map                       # full regen, hits crates.io
just dep-map --skip-crates-io      # offline; reuses cached metrics
```

The script writes `manual/src/dep-map.html` and `manual/src/dep-map.json`,
plus a `.dep-map-cache.json` at the repo root for the crates.io responses.
All three are gitignored — they're CI-built artifacts.

## CI behavior

- **Push to `main`**: the manual workflow regenerates the map before running
  mdbook, so the published site at the manual's URL always reflects the tip.
- **Pull request**: the `dep-map` workflow builds the map at both `base` and
  `HEAD`, diffs them, and posts a sticky comment listing added / removed /
  version-changed crates with key metrics. The full graph HTML and a colorized
  Graphviz SVG are attached as workflow artifacts.

## Limitations

- Aliases in `third-party/BUCK` (`//third-party:cfg-if` →
  `//third-party:cfg-if-1.0.4`) are resolved during ingestion, so the graph
  shows the *canonical versioned* node. The bare alias name never appears as
  a node.
- crates.io is the only data source for external metrics. For git-sourced
  crates (e.g. our `wasmtime` fork) only the repo URL is shown.
- The buck2 query is *unconfigured* (`uquery`), so all platform-conditional
  deps appear in the graph regardless of target. This is intentional: the
  map is about *which crates we ship anywhere*, not *which crates ship for
  riscv64*.
