"""Build-time configuration owned by `//sys/cpus`.

Set on the command line with `--config kernel.KEY=VALUE`. The section is the
user-facing namespace; the declaration lives with the crate that consumes it.
"""

load("//build:kcfg.bzl", "kcfg")

MAX_CPUS = kcfg.declare(
    section = "kernel",
    key = "max_cpus",
    type = kcfg.int(default = 64),
    doc = "Upper bound on the number of CPUs the kernel supports. Every per-CPU " +
          "table is sized from this, so raising it costs memory whether or not the " +
          "CPUs exist. At 64 or below a CPU set fits one machine word, which keeps " +
          "idle/online bitmaps to a single atomic and lets an SBI hart mask reach " +
          "the whole system in one call.",
)
