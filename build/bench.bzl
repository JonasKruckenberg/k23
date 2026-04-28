load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")

_DEFAULT_MODIFIERS = [
    "constraints//:opt-level[3]",
    "constraints//:debuginfo[line-tables-only]",
    "constraints//:strip[debuginfo]",
]

def _rust_benchmark_runner_impl(ctx: AnalysisContext) -> list[Provider]:
    run_info = ctx.attrs.binary[RunInfo]
    criterion_home = ctx.actions.declare_output("criterion", dir = True)
    cmd = cmd_args(run_info.args, "--bench", hidden = criterion_home.as_output())

    return inject_test_run_info(
            ctx,
            ExternalRunnerTestInfo(
                type = "rust",
                command = [cmd],
                env = {"CRITERION_HOME": cmd_args(criterion_home.as_output())},
                labels = ctx.attrs.labels,
                # contacts = ctx.attrs.contacts,
                # default_executor = re_executor,
                # executor_overrides = executor_overrides,
                run_from_project_root = True,
                use_project_relative_paths = True,
            ),
        ) + [ctx.attrs.binary[DefaultInfo]]

_rust_benchmark_runner = rule(
    impl = _rust_benchmark_runner_impl,
    attrs = {
        "binary": attrs.dep(providers = [RunInfo]),
        "labels": attrs.list(attrs.string(), default = []),
        "_inject_test_env": attrs.default_only(attrs.dep(default = "prelude//test/tools:inject_test_env")),
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
        labels = ["micro_benchmark"],
        visibility = visibility,
    )
