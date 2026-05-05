load("@toolchains//:qemu.bzl", "QEMUToolchainInfo")
load("@prelude//test:inject_test_run_info.bzl", "inject_test_run_info")

def _qemu_binary(ctx: AnalysisContext) -> list[Provider]:
    toolchain = ctx.attrs._qemu_toolchain[QEMUToolchainInfo]

    cmd = cmd_args(cmd_args(toolchain.qemu, "bin", toolchain.qemu_binary, delimiter = "/"))
    cmd.add(ctx.attrs.qemu_args)
    cmd.add(toolchain.qemu_args)

    # TODO: route ctx.attrs.kernel_args into /chosen/bootargs (e.g. via a
    # synthesised -dtb override). UEFI boot has no equivalent of
    # `-kernel`/`-append`, so the attribute is currently accepted but ignored.

    image = ctx.attrs.image[DefaultInfo].default_outputs[0]

    # Firmware lives inside the (immutable) nix store output that backs the
    # qemu package symlink, so we reference it directly. Attach the firmware
    # CODE pflash read-only so qemu doesn't try to mutate it, and use
    # `-snapshot` to discard any other writes (NVRAM, ESP changes) at exit —
    # this keeps runs reproducible without needing a per-run VARS.fd copy.
    cmd.add(
        "-nographic",
        "-snapshot",
        "-drive",
        cmd_args("if=virtio,format=raw,file=", image, delimiter = ""),
    )

    if toolchain.firmware_code_path != None:
        cmd.add(
            "-drive",
            cmd_args(
                "if=pflash,format=raw,unit=0,readonly=on,file=",
                cmd_args(toolchain.qemu, toolchain.firmware_code_path, delimiter = "/"),
                delimiter = "",
            )
        )

    return [DefaultInfo(), RunInfo(args = cmd)]

qemu_binary = rule(
    impl = _qemu_binary,
    doc = """
    Runs the provided UEFI disk image under QEMU.

    The `image` must be a UEFI-bootable ISO-9660 image (as produced by
    `k23_image`). QEMU is configured with an EDK2 UEFI firmware as pflash and
    the image attached as a virtio block device — no BIOS boot path.
    """,
    attrs = {
        "image": attrs.dep(providers = [DefaultInfo]),
        "qemu_args": attrs.list(attrs.string(), default = []),
        "kernel_args": attrs.list(attrs.string(), default = []),
        "_qemu_toolchain": attrs.toolchain_dep(default = "toolchains//:qemu", providers = [QEMUToolchainInfo]),
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
        "image": attrs.dep(providers = [DefaultInfo]),
        "qemu_args": attrs.list(attrs.string(), default = []),
        "kernel_args": attrs.list(attrs.string(), default = []),
        "labels": attrs.list(attrs.string(), default = []),
        "_qemu_toolchain": attrs.toolchain_dep(default = "toolchains//:qemu", providers = [QEMUToolchainInfo]),
        "_inject_test_env": attrs.default_only(attrs.dep(default = "prelude//test/tools:inject_test_env")),
    }
)
