load("@toolchains//:qemu.bzl", "QEMUToolchainInfo")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")

def _qemu_binary(ctx: AnalysisContext) -> list[Provider]:
    cmd = cmd_args(ctx.attrs._qemu_toolchain[QEMUToolchainInfo].qemu)
    cmd.add(ctx.attrs.qemu_args)
    cmd.add(ctx.attrs._qemu_toolchain[QEMUToolchainInfo].qemu_args)
    cmd.add("-kernel", ctx.attrs.binary[DefaultInfo].default_outputs[0])
    if ctx.attrs.kernel_args:
        # bootargs land in /chosen/bootargs in the FDT; the kernel reads them there.
        cmd.add("-append", " ".join(ctx.attrs.kernel_args))

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
        "kernel_args": attrs.list(attrs.string(), default = []),
        "_qemu_toolchain": attrs.toolchain_dep(default = "toolchains//:qemu", providers = [QEMUToolchainInfo])
    }
)

def _qemu_test(ctx: AnalysisContext) -> list[Provider]:
    [default_info, run_info] = _qemu_binary(ctx);

    return inject_test_run_info(
            ctx,
            ExternalRunnerTestInfo(
                type = "rust",
                command = [run_info.args],
                labels = ctx.attrs.labels,
                run_from_project_root = True,
                use_project_relative_paths = True,
            ),
        ) + [default_info]

qemu_test = rule(
    impl = _qemu_test,
    attrs = {
        "binary": attrs.option(attrs.dep(providers = [DefaultInfo]), default = None),
        "qemu_args": attrs.list(attrs.string(), default = []),
        "kernel_args": attrs.list(attrs.string(), default = []),
        "labels": attrs.list(attrs.string(), default = []),
        "_qemu_toolchain": attrs.toolchain_dep(default = "toolchains//:qemu", providers = [QEMUToolchainInfo]),
        "_inject_test_env": attrs.default_only(attrs.dep(default = "prelude//test/tools:inject_test_env")),
    }
)
