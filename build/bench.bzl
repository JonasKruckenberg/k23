_DEFAULT_MODIFIERS = [
    "constraints//:opt-level[3]",
    "constraints//:debuginfo[line-tables-only]",
    "constraints//:strip[debuginfo]",
]

def _rust_benchmark_runner_impl(ctx: AnalysisContext) -> list[Provider]:
    run_info = ctx.attrs.binary[RunInfo]
    cmd = cmd_args(run_info.args, "--bench")
    return [DefaultInfo(), RunInfo(args = cmd)]

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
