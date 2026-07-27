"""The kernel's build-time configuration.

Set on the command line with `--config kernel.KEY=VALUE`.
"""

load("//build:kcfg.bzl", "kcfg")

LogLevel = enum("error", "warn", "info", "debug", "trace")

LOG_LEVEL = kcfg.declare(
    section = "kernel",
    key = "log_level",
    type = kcfg.enum(LogLevel, default = LogLevel("warn")),
    doc = "Verbosity of the kernel log output.",
)

STACK_SIZE = kcfg.declare(
    section = "kernel",
    key = "stack_size_kb",
    type = kcfg.int(default = 512),
    doc = "Default thread stack size in kilobytes.",
)
