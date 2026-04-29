load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")

# Wrapper invoked by `buck2 run` / `buck2 test` for any rust_fuzz target.
#
# Two responsibilities:
#
# 1. Route libfuzzer's crash artifacts (`crash-*`, `leak-*`, `oom-*`, `slow-unit-*`)
#    into a per-target subdirectory under `fuzz/artifacts/` instead of the
#    project root, so they're easy to gitignore and to upload as a CI artifact
#    bundle. We inject `-artifact_prefix=...` *before* user args so explicit
#    user overrides still win.
#
# 2. On a non-zero exit, scan libfuzzer's stderr for the
#       "Test unit written to <path>"
#    line, then re-run the binary with that single input under
#    RUST_LIBFUZZER_DEBUG_PATH so libfuzzer-sys's `fuzz_target!` macro emits
#    the typed input via its Debug impl (libfuzzer-sys/src/lib.rs:341).
#    Mirrors what `cargo fuzz fmt` does — but inline, so a crash reproduces
#    with structured output by default instead of raw bytes.
_FUZZ_WRAPPER = """#!/usr/bin/env bash
set -uo pipefail
NAME="$1"
shift
BIN="$1"
shift

ARTIFACT_DIR="fuzz/artifacts/$NAME"
mkdir -p "$ARTIFACT_DIR"

LOG="$(mktemp)"
trap 'rm -f "$LOG"' EXIT

# Capture stderr through a real pipe (not process substitution) so tee
# is guaranteed to flush $LOG before we read it below. fd 3 carries the
# binary's stdout past tee; ${PIPESTATUS[0]} recovers the binary's exit
# code instead of tee's.
{ "$BIN" "-artifact_prefix=$ARTIFACT_DIR/" "$@" 2>&1 1>&3 3>&- | tee -a "$LOG" >&2; } 3>&1
rc=${PIPESTATUS[0]}

if [ "$rc" -ne 0 ]; then
    CRASH=$(sed -n 's/.*Test unit written to //p' "$LOG" | tail -1)
    if [ -n "$CRASH" ] && [ -f "$CRASH" ]; then
        DBG="$(mktemp)"
        RUST_LIBFUZZER_DEBUG_PATH="$DBG" "$BIN" "$CRASH" >/dev/null 2>&1 || true
        printf '\\n========== Failing input (%s) ==========\\n' "$CRASH" >&2
        cat "$DBG" >&2
        printf '================================================\\n' >&2
        rm -f "$DBG"
    fi
fi

exit "$rc"
"""

def _rust_fuzz_runner_impl(ctx: AnalysisContext) -> list[Provider]:
    wrapper = ctx.actions.declare_output("fuzz_wrapper.sh")
    ctx.actions.write(wrapper, _FUZZ_WRAPPER, is_executable = True)

    cmd = cmd_args(wrapper, ctx.label.name, ctx.attrs.binary[RunInfo].args)
    if ctx.attrs.max_total_time != None:
        cmd.add("-max_total_time={}".format(ctx.attrs.max_total_time))

    return inject_test_run_info(
            ctx,
            ExternalRunnerTestInfo(
                type = "rust",
                command = [cmd],
                labels = ctx.attrs.labels,
                run_from_project_root = True,
                use_project_relative_paths = True,
            ),
        ) + [ctx.attrs.binary[DefaultInfo]]

_rust_fuzz_runner = rule(
    impl = _rust_fuzz_runner_impl,
    attrs = {
        "binary": attrs.dep(providers = [RunInfo]),
        "labels": attrs.list(attrs.string(), default = []),
        "max_total_time": attrs.option(attrs.int(), default = None),
        "_inject_test_env": attrs.default_only(attrs.dep(default = "prelude//test/tools:inject_test_env")),
    },
)

def rust_fuzz(name, srcs, crate_root, deps = [], visibility = None, max_total_time = None, **kwargs):
    bin_name = name + "_bin"
    native.rust_binary(
        name = bin_name,
        srcs = srcs,
        crate_root = crate_root,
        deps = deps,
        rustc_flags = [
            "--cfg=fuzzing"
        ],
        incoming_transition = "root//build:fuzz",
        visibility = visibility,
        **kwargs
    )
    _rust_fuzz_runner(
        name = name,
        binary = ":" + bin_name,
        labels = ["fuzz"],
        max_total_time = max_total_time,
        visibility = visibility,
    )
