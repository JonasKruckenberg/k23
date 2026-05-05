def _k23_image(ctx: AnalysisContext) -> list[Provider]:
    loader_file = ctx.attrs.loader[DefaultInfo].default_outputs[0]
    kernel_file = ctx.attrs.kernel[DefaultInfo].default_outputs[0]

    esp = ctx.actions.declare_output("esp")
    image = ctx.actions.declare_output(ctx.label.name + ".iso")
    ctx.actions.run(
        cmd_args(
            ctx.attrs._mkdisk_img[RunInfo].args,
            "--loader", loader_file,
            "--kernel", kernel_file,
            "--arch", ctx.attrs._mkdisk_img_arch,
            "--esp-out", esp.as_output(),
            "--output", image.as_output(),
        ),
        category = "mkdisk_img",
    )

    return [DefaultInfo(
        default_output = image,
        other_outputs = [loader_file, kernel_file, esp]
    )]

k23_image = rule(
    impl = _k23_image,
    doc = """
    Builds a UEFI-bootable ISO-9660 image containing an embedded FAT ESP with
    the loader (at `\\EFI\\BOOT\\BOOT<arch>.EFI`) and the kernel (at
    `\\EFI\\k23\\kernel.elf`), wired up via an El Torito UEFI boot catalog.

    On riscv64 the loader ELF shared object is first converted to a PE `.efi`
    by `elf_to_efi` (see `sys/loader/defs.bzl`); aarch64 and x86_64 build
    the loader directly against the native `*-unknown-uefi` rustc targets
    so no conversion is needed.

    The output is suitable for attachment to QEMU as a virtio-scsi CD-ROM
    alongside an EDK2 UEFI firmware.
    """,
    attrs = {
        "loader": attrs.dep(providers = [DefaultInfo]),
        "kernel": attrs.dep(providers = [DefaultInfo]),
        "kernel_debuginfo": attrs.dep(providers = [DefaultInfo]),
        "_mkdisk_img_arch": attrs.string(default = select({
            "prelude//cpu:riscv64": "riscv64",
            "prelude//cpu:arm64": "aarch64",
            "prelude//cpu:x86_64": "x86_64",
        })),
        "_mkdisk_img": attrs.exec_dep(default = "root//build/mkdisk-img:mkdisk-img", providers = [RunInfo]),
    },
)
