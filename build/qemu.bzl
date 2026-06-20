load("@toolchains//:qemu.bzl", "QEMUToolchainInfo")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")

# Carried by image targets to tell the QEMU rules how the image should be booted.
K23BootInfo = provider(fields = {
    "protocol": provider_field(str),  # "uefi" | "flat"
})

_IMAGE_ATTRS = {
    "image": attrs.dep(providers = [DefaultInfo, K23BootInfo]),
    "qemu_args": attrs.list(attrs.string(), default = []),
    "_qemu_toolchain": attrs.toolchain_dep(default = "toolchains//:qemu", providers = [QEMUToolchainInfo]),
}

def _qemu_binary(ctx: AnalysisContext) -> list[Provider]:
    toolchain = ctx.attrs._qemu_toolchain[QEMUToolchainInfo]
    protocol = ctx.attrs.image[K23BootInfo].protocol
    image = ctx.attrs.image[DefaultInfo].default_outputs[0]

    cmd = cmd_args(cmd_args(toolchain.qemu, "bin", toolchain.qemu_binary, delimiter = "/"))
    cmd.add(ctx.attrs.qemu_args)
    cmd.add(toolchain.qemu_args)
    cmd.add("-snapshot")

    if protocol == "uefi":
        # Firmware lives inside the (immutable) nix store output that backs the
        # qemu package symlink, so we reference it directly. Attach the firmware
        # CODE pflash read-only so qemu doesn't try to mutate it, and use
        # `-snapshot` to discard any other writes (NVRAM, ESP changes) at exit —
        # this keeps runs reproducible without needing a per-run VARS.fd copy.
        cmd.add("-drive", cmd_args("if=virtio,format=raw,file=", image, delimiter = ""))

        if toolchain.firmware_code_path != None:
            cmd.add("-drive", cmd_args(
                "if=pflash,format=raw,unit=0,readonly=on,file=",
                cmd_args(toolchain.qemu, toolchain.firmware_code_path, delimiter = "/"),
                delimiter = "",
            ))
        if toolchain.firmware_vars_path != None:
            cmd.add("-drive", cmd_args(
                "if=pflash,format=raw,unit=1,readonly=off,file=",
                cmd_args(toolchain.qemu, toolchain.firmware_vars_path, delimiter = "/"),
                delimiter = "",
            ))
    elif protocol == "flat":
        cmd.add("-kernel", image)
    else:
        fail("unknown boot protocol: " + protocol)

    return [DefaultInfo(), RunInfo(args = cmd)]

qemu_binary = rule(
    impl = _qemu_binary,
    doc = "Runs a k23 image under QEMU. Boot behaviour is determined by the K23BootInfo provider on `image`.",
    attrs = _IMAGE_ATTRS,
)

def _qemu_test(ctx: AnalysisContext) -> list[Provider]:
    [default_info, run_info] = _qemu_binary(ctx)

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
    attrs = _IMAGE_ATTRS | {
        "labels": attrs.list(attrs.string(), default = []),
        "_inject_test_env": attrs.default_only(attrs.dep(default = "prelude//test/tools:inject_test_env")),
    },
)
