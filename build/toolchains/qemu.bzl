QEMUToolchainInfo = provider(fields = {
    "qemu": provider_field(typing.Any),
    "qemu_binary": provider_field(str),
    "qemu_args": provider_field(list[typing.Any], default = []),
    "firmware_code_path": provider_field(str | None),
    "firmware_vars_path": provider_field(str | None)
})

def _qemu_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        QEMUToolchainInfo(
            qemu = ctx.attrs.qemu[DefaultInfo].default_outputs[0],
            qemu_binary = ctx.attrs.qemu_binary,
            qemu_args = ctx.attrs.qemu_args,
            firmware_code_path = ctx.attrs.firmware_code_path,
            firmware_vars_path = ctx.attrs.firmware_vars_path
        )
    ]

qemu_toolchain = rule(
    impl = _qemu_toolchain_impl,
    attrs = {
        "qemu": attrs.exec_dep(providers = [DefaultInfo]),
        "qemu_binary": attrs.string(),
        "qemu_args": attrs.list(attrs.string(), default = []),
        "firmware_code_path": attrs.option(attrs.string()),
        "firmware_vars_path": attrs.option(attrs.string()),
    },
    is_toolchain_rule = True,
)
