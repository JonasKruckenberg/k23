def k23_image(name, platform, loader, kernel, kernel_load_address = None, **kwargs):
    _k23_image_rule(
        name = name,
        loader = loader,
        kernel = kernel,
        kernel_load_address = kernel_load_address,
        default_target_platform = platform,
        **kwargs
    )

def _k23_image_rule_impl(ctx: AnalysisContext) -> list[Provider]:
    loader_file = ctx.attrs.loader[DefaultInfo].default_outputs[0]
    kernel_file = ctx.attrs.kernel[DefaultInfo].default_outputs[0]

    # TODO this should actually create a proper disk image...
    # but because that is hard (requires a working disk image builder AND a working BIOS/UEFI pipeline AND probably more)
    # this temporary "disk image" is just a concatenation of the two binaries.

    lldb_args = ctx.actions.declare_output("lldb_args")
    ctx.actions.write(
        lldb_args,
        [
            cmd_args("target create", loader_file, delimiter = " "),
            cmd_args("target modules add", kernel_file, delimiter = " "),
            cmd_args("target modules load --file", kernel_file, " -s", str(ctx.attrs.kernel_load_address), delimiter = " ")
        ],
        with_inputs = True,
        allow_args = True
    )

    # Also generate a GDB variant
    gdb_args = ctx.actions.declare_output("gdb_args")
    ctx.actions.write(
        gdb_args,
        [
            cmd_args("file", loader_file, delimiter = " "),
            cmd_args("add-symbol-file", kernel_file, str(ctx.attrs.kernel_load_address), delimiter = " "),
        ],
        with_inputs = True,
        allow_args = True,
    )

    return [DefaultInfo(
        default_output = loader_file,
        sub_targets =  {
            "lldb_args": [DefaultInfo(default_output = lldb_args, other_outputs = [loader_file, kernel_file])],
            "gdb_args": [DefaultInfo(default_output = gdb_args, other_outputs = [loader_file, kernel_file])],
        }
    )]

_k23_image_rule = rule(
    impl = _k23_image_rule_impl,
    attrs = {
        "loader": attrs.dep(providers = [DefaultInfo]),
        "kernel": attrs.dep(providers = [DefaultInfo]),
        "kernel_load_address": attrs.option(attrs.int(), default = None)
    },
)
