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

    return [DefaultInfo(
        default_output = loader_file,
    )]

_k23_image_rule = rule(
    impl = _k23_image_rule_impl,
    attrs = {
        "loader": attrs.dep(providers = [DefaultInfo]),
        "kernel": attrs.dep(providers = [DefaultInfo]),
        "kernel_debuginfo": attrs.dep(providers = [DefaultInfo]),
        "kernel_load_address": attrs.option(attrs.int(), default = None)
    },
)
