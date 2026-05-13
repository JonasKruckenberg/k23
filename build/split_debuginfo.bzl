def _split_debuginfo_impl(ctx: AnalysisContext) -> list[Provider]:
    src = ctx.attrs.binary[DefaultInfo].default_outputs[0]
    debug = ctx.actions.declare_output(ctx.attrs.name + ".debug")
    stripped = ctx.actions.declare_output(ctx.attrs.name)

    ctx.actions.run(
        cmd_args(ctx.attrs._objcopy[RunInfo], "--only-keep-debug", src, debug.as_output()),
        category = "objcopy_only_keep_debug",
    )

    ctx.actions.run(
        cmd_args(
            ctx.attrs._objcopy[RunInfo],
            "--strip-unneeded",
            cmd_args(debug, format = "--add-gnu-debuglink={}"),
            src,
            stripped.as_output(),
        ),
        category = "objcopy_strip",
    )

    return [DefaultInfo(
        default_output = stripped,
        sub_targets = {"debuginfo": [DefaultInfo(default_output = debug)]},
    )]

split_debuginfo = rule(
    impl = _split_debuginfo_impl,
    doc = """
    Split an incoming debuginfo-containing ELF file into a stripped ELF file and standalone debuginfo ELF file.

    This is effectively the same as Rusts `-Csplit-debuginfo` setting but works more reliably across different architectures.
    """,
    attrs = {
        "binary": attrs.dep(),
        "_objcopy": attrs.exec_dep(default = "toolchains//:llvm_bintools[llvm-objcopy]"),
    },
)
