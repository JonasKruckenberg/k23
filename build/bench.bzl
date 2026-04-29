_DEFAULT_MODIFIERS = [
    "constraints//:opt-level[3]",
    "constraints//:debuginfo[line-tables-only]",
    "constraints//:strip[debuginfo]",
]

def _rust_benchmark_runner_impl(ctx: AnalysisContext) -> list[Provider]:
    bin_run = ctx.attrs.binary[RunInfo]

    script = cmd_args(
        "#!/bin/sh",
        "set -e",
        'export CRITERION_HOME="$(pwd)/bench-artifacts"',
        'mkdir -p "$CRITERION_HOME"',
        cmd_args(bin_run.args, format = 'exec {} --bench "$@"'),
        delimiter = "\n",
    )

    wrapper, hidden = ctx.actions.write(
        "run_bench.sh",
        script,
        is_executable = True,
        allow_args = True,
        with_inputs = True
    )

    return [
        DefaultInfo(default_output = wrapper, other_outputs = hidden),
        RunInfo(args = cmd_args(wrapper, hidden = hidden)),
    ]

_rust_benchmark_runner = rule(
    impl = _rust_benchmark_runner_impl,
    attrs = {
        "binary": attrs.dep(providers = [RunInfo]),
    },
)

def rust_benchmark(name, modifiers = [], visibility = None, **kwargs):
    bin_name = name + "_bin"
    native.rust_binary(
        name = bin_name,
        modifiers = _DEFAULT_MODIFIERS + modifiers,
        visibility = visibility,
        **kwargs
    )
    _rust_benchmark_runner(
        name = name,
        binary = ":" + bin_name,
        visibility = visibility,
    )
