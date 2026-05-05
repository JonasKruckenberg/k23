load("@toolchains//:qemu.bzl", "QEMUToolchainInfo")

def _qemu_binary(ctx: AnalysisContext) -> list[Provider]:
    toolchain = ctx.attrs._qemu_toolchain[QEMUToolchainInfo]

    cmd = cmd_args(toolchain.qemu)
    cmd.add(ctx.attrs.qemu_args)
    cmd.add(toolchain.qemu_args)

    image = ctx.attrs.image[DefaultInfo].default_outputs[0]

    if toolchain.firmware_code == None:
        fail("qemu_binary: no firmware_code configured on the qemu toolchain for this target cpu")

    # Firmware and image are both in the buck-out read-only cache. Attach the
    # firmware CODE pflash read-only so qemu doesn't try to mutate it, and use
    # `-snapshot` to discard any other writes (NVRAM, ESP changes) at exit —
    # this keeps runs reproducible without needing a per-run VARS.fd copy.
    cmd.add(
        "-nographic",
        "-snapshot",
        "-drive",
        cmd_args(
            "if=pflash,format=raw,unit=0,readonly=on,file=",
            toolchain.firmware_code,
            delimiter = "",
        ),
        "-drive",
        cmd_args("if=virtio,format=raw,file=", image, delimiter = ""),
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
        "_qemu_toolchain": attrs.toolchain_dep(default = "toolchains//:qemu", providers = [QEMUToolchainInfo]),
    }
)
