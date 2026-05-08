#!/usr/bin/env python3
# Copyright 2025 Jonas Kruckenberg
#
# Licensed under the Apache License, Version 2.0, <LICENSE-APACHE or
# http://apache.org/licenses/LICENSE-2.0> or the MIT license <LICENSE-MIT or
# http://opensource.org/licenses/MIT>, at your option. This file may not be
# copied, modified, or distributed except according to those terms.

"""Post-process k23 alloc_trace dumps.

Reads a kernel log containing `ALLOC,<size>,<align>,<pc>...` lines
(possibly mangled by the tracing subscriber, which rewrites commas as
spaces inside the message payload), and reports the top callstacks by
allocation count and by total bytes. Optionally symbolizes via
addr2line.

Usage:
    alloc_trace_analyze.py LOG [--elf KERNEL_ELF] [--top N] [--frames N]
"""

import argparse
import collections
import re
import shutil
import subprocess
import sys

# Strip ANSI escapes from terminal-captured logs.
ANSI = re.compile(r"\x1b\[[0-9;]*[a-zA-Z]")
# Match an ALLOC record. Both `,` and whitespace work as separators
# because the tracing subscriber rewrites commas. We greedily eat all
# trailing hex addresses; the structured tracing fields that follow
# (e.g. `log.target=...`) start with a non-hex token so they stop the
# match naturally.
ALLOC = re.compile(
    r"ALLOC[\s,]+(\d+)[\s,]+(\d+)((?:[\s,]+0x[0-9a-fA-F]+)+)"
)
HEX = re.compile(r"0x[0-9a-fA-F]+")


def parse(path):
    records = []
    with open(path, "r", errors="replace") as f:
        for line in f:
            line = ANSI.sub("", line)
            m = ALLOC.search(line)
            if not m:
                continue
            size = int(m.group(1))
            align = int(m.group(2))
            frames = tuple(int(x, 16) for x in HEX.findall(m.group(3)))
            records.append((size, align, frames))
    return records


def symbolize(elf, pcs):
    pcs = sorted(set(pcs))
    if not elf:
        return {pc: f"0x{pc:x}" for pc in pcs}
    if not shutil.which("addr2line"):
        print("warning: addr2line not on PATH; emitting raw PCs", file=sys.stderr)
        return {pc: f"0x{pc:x}" for pc in pcs}
    args = ["addr2line", "-e", elf, "-f", "-C"] + [f"0x{pc:x}" for pc in pcs]
    r = subprocess.run(args, capture_output=True, text=True, check=False)
    if r.returncode != 0:
        print(f"addr2line failed: {r.stderr}", file=sys.stderr)
        return {pc: f"0x{pc:x}" for pc in pcs}
    lines = r.stdout.splitlines()
    out = {}
    for i, pc in enumerate(pcs):
        func = lines[i * 2] if i * 2 < len(lines) else "??"
        loc = lines[i * 2 + 1] if i * 2 + 1 < len(lines) else "??:?"
        out[pc] = f"{func}\n             at {loc}"
    return out


def fmt_size_dist(sizes, n=4):
    parts = [f"{s}B×{c}" for s, c in sorted(sizes.items())[:n]]
    if len(sizes) > n:
        parts.append(f"+{len(sizes) - n} more")
    return ", ".join(parts)


def report(title, ranking, by_stack, bytes_by_stack, sizes_by_stack, syms):
    print(f"\n=== {title} ===\n")
    for stack, _ in ranking:
        count = by_stack[stack]
        total = bytes_by_stack[stack]
        sizes = sizes_by_stack[stack]
        print(
            f"{count:>8} allocs  {total:>12} B  "
            f"[{fmt_size_dist(sizes)}]"
        )
        for pc in stack:
            print(f"             {syms.get(pc, f'0x{pc:x}')}")
        print()


def main():
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("log", help="kernel log file containing the alloc_trace dump")
    ap.add_argument("--elf", help="kernel ELF for addr2line symbolization")
    ap.add_argument("--top", type=int, default=20, help="how many stacks to show")
    ap.add_argument(
        "--frames",
        type=int,
        default=4,
        help="number of leaf frames used as the callstack key",
    )
    args = ap.parse_args()

    records = parse(args.log)
    print(f"parsed {len(records)} ALLOC records", file=sys.stderr)
    if not records:
        sys.exit(1)

    total_allocs = len(records)
    total_bytes = sum(r[0] for r in records)
    print(
        f"total: {total_allocs} allocations, {total_bytes} B "
        f"({total_bytes / 1024 / 1024:.2f} MiB)",
        file=sys.stderr,
    )

    by_stack = collections.Counter()
    bytes_by_stack = collections.Counter()
    sizes_by_stack = collections.defaultdict(collections.Counter)
    for size, _align, frames in records:
        key = frames[: args.frames]
        by_stack[key] += 1
        bytes_by_stack[key] += size
        sizes_by_stack[key][size] += 1

    pcs = {pc for k in by_stack for pc in k}
    syms = symbolize(args.elf, pcs)

    report(
        f"Top {args.top} callstacks by allocation count",
        by_stack.most_common(args.top),
        by_stack, bytes_by_stack, sizes_by_stack, syms,
    )
    report(
        f"Top {args.top} callstacks by total bytes",
        bytes_by_stack.most_common(args.top),
        by_stack, bytes_by_stack, sizes_by_stack, syms,
    )


if __name__ == "__main__":
    main()
