def _mdbook_impl(ctx: AnalysisContext) -> list[Provider]:
    staged_files = {src.short_path: src for src in ctx.attrs.srcs}
    for dest, dep in ctx.attrs.extra_srcs.items():
        staged_files[dest] = dep[DefaultInfo].default_outputs[0]

    staged = ctx.actions.copied_dir("source", staged_files)

    out = ctx.actions.declare_output("book", dir = True)
    mdbook = ctx.attrs.mdbook[RunInfo]

    ctx.actions.run(
        cmd_args(mdbook, "build", staged, "--dest-dir", out.as_output()),
        category = "mdbook_build",
        local_only = True,
    )

    return [
        DefaultInfo(
            default_output = out,
            sub_targets = {
                "source": [DefaultInfo(default_output = staged)],
            },
        ),
        RunInfo(args = cmd_args(mdbook, "serve", staged)),
    ]

mdbook = rule(
    impl = _mdbook_impl,
    attrs = {
        "srcs": attrs.list(attrs.source()),
        "extra_srcs": attrs.dict(
            attrs.string(),
            attrs.dep(providers = [DefaultInfo]),
            default = {},
        ),
        "mdbook": attrs.default_only(attrs.exec_dep(
            default = "toolchains//:mdbook",
            providers = [RunInfo],
        )),
    },
)
