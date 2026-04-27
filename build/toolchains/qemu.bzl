QEMUToolchainInfo = provider(fields = {
    "qemu": provider_field(RunInfo),
    "qemu_args": provider_field(list[typing.Any], default = []),
})

def _qemu_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        QEMUToolchainInfo(
            qemu = ctx.attrs.qemu[RunInfo],
            qemu_args = ctx.attrs.qemu_args
        )
    ]

qemu_toolchain = rule(
    impl = _qemu_toolchain_impl,
    attrs = {
        "qemu": attrs.exec_dep(),
        "qemu_args": attrs.list(attrs.string(), default = []),
    },
    is_toolchain_rule = True,
)
