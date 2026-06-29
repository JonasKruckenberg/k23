# riscv64 has no upstream UEFI target. We compile for
# `riscv64gc-unknown-none-elf` and convert the resulting ELF shared object
# to PE via `//build/riscv-elf2efi`.

def _elf_to_efi_impl(ctx: AnalysisContext) -> list[Provider]:
    elf = ctx.attrs.src[DefaultInfo].default_outputs[0]
    efi = ctx.actions.declare_output(ctx.label.name + ".efi")
    ctx.actions.run(
        cmd_args(
            ctx.attrs._riscv_elf2efi[RunInfo].args,
            "--input",
            elf,
            "--output",
            efi.as_output(),
        ),
        category = "riscv_elf2efi",
    )
    return [DefaultInfo(
        default_outputs = [efi],
        sub_targets = {"elf": [DefaultInfo(default_outputs = [elf])]},
    )]

elf_to_efi = rule(
    impl = _elf_to_efi_impl,
    attrs = {
        "src": attrs.dep(providers = [DefaultInfo]),
        "_riscv_elf2efi": attrs.exec_dep(
            default = "root//build/riscv-elf2efi:riscv-elf2efi",
            providers = [RunInfo],
        ),
    },
)
