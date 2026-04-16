load("@toolchains//:qemu.bzl", "QEMUToolchainInfo")

def _qemu_binary(ctx: AnalysisContext) -> list[Provider]:
    cmd = cmd_args(ctx.attrs._qemu_toolchain[QEMUToolchainInfo].qemu)
    cmd.add(ctx.attrs.qemu_args)
    cmd.add(ctx.attrs._qemu_toolchain[QEMUToolchainInfo].qemu_args)
    cmd.add("-kernel", ctx.attrs.binary[DefaultInfo].default_outputs[0])

    return [DefaultInfo(), RunInfo(args = cmd)]

qemu_binary = rule(
    impl = _qemu_binary,
    doc = """
    Runs the provided binary under QEMU.

    The binary must be an ELF executable compatible with the linux kernel ELF boot-process
    (i.e. the devicetree blob is passed as the "first argument" to the ELFs entrypoint) and is passed to QEMU via the -kernel option.
    """,
    attrs = {
        "binary": attrs.option(attrs.dep(providers = [DefaultInfo]), default = None),
        "qemu_args": attrs.list(attrs.string(), default = []),
        "_qemu_toolchain": attrs.toolchain_dep(default = "toolchains//:qemu", providers = [QEMUToolchainInfo])
    }
)
