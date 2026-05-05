# HOW TO USE THIS MODULE:
#
#    load("@nix/flake.bzl", "flake")
#
#    flake.package(name = "pkg", path = "path/to/flake/dir", ...)

load("@prelude//decls/common.bzl", "buck")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

## ---------------------------------------------------------------------------------------------------------------------
def __flake_package_impl(
        ctx: AnalysisContext,
        path: Artifact,
        package: str,
        output: str,
        binary: str | None,
        binaries: list[str],
        sub_targets: list[str],
        exec_os_type: OsLookup) -> list[Provider]:
    # calls nix build path:<path>#package.<arch-os>.<package>

    if exec_os_type.os == Os("linux"):
        os = "linux"
    elif exec_os_type.os == Os("macos"):
        os = "darwin"
    else:
        fail("host os not supported: {}".format(exec_os_type.os))

    if exec_os_type.cpu == "x86_64":
        cpu = "x86_64"
    elif exec_os_type.cpu == "arm64":
        cpu = "aarch64"
    else:
        fail("host arch is not supported: {}".format(exec_os_type.cpu))

    system = "{cpu}-{os}".format(os = os, cpu = cpu)

    attribute = "packages" + "." + system + "." + package + "." + output

    # nix will build the first output by default, but we do not know what the first output is called.
    # That's why we build the "out" output by default.
    # Note, nix does not append a suffix to the out-link for the "out" output.
    out_link = ctx.actions.declare_output("out.link" if output == "out" else "out.link-" + output)

    nix_build = cmd_args([
        "env",
        "--",  # this is needed to avoid "Spawning executable `nix` failed: Failed to spawn a process"
        "nix",
        "--extra-experimental-features",
        "nix-command flakes",
        "build",
        #"--show-trace",         # for debugging
        cmd_args("--out-link", cmd_args(out_link.as_output(), parent = 1, absolute_suffix = "/out.link")),
        cmd_args(cmd_args(path, format = "path:{}"), attribute, delimiter = "#"),
    ])
    ctx.actions.run(nix_build, category = "nix_flake", local_only = True)

    run_info = []
    if binary:
        run_info.append(
            RunInfo(
                args = cmd_args(out_link, "bin", ctx.attrs.binary, delimiter = "/"),
            ),
        )

    sub_targets = {
        bin: [DefaultInfo(default_output = out_link), RunInfo(args = cmd_args(out_link, "bin", bin, delimiter = "/"))]
        for bin in binaries
    }

    for path in ctx.attrs.sub_targets:
        sub_targets[path] = [DefaultInfo(default_output = out_link.project(path))]

    return [
        DefaultInfo(
            default_output = out_link,
            sub_targets = sub_targets,
        ),
    ] + run_info

__common_attrs = {
    "binary": attrs.option(attrs.string(), default = None, doc = """
      specify the default binary of this package

      This provides `RunInfo` for a binary in the `bin` directory of the package.
    """),
    "binaries": attrs.list(attrs.string(), default = [], doc = """
      add auxiliary binaries for this package

      These can be accessed as sub-targets with the given name in dependent rules.
    """),
    "path": attrs.source(allow_directory = True, doc = "the path to the flake"),
    "output": attrs.string(default = "out", doc = """
      specify the output to build instead of the default

      (optional, default: `"out"`)
    """),
    "package": attrs.option(attrs.string(), doc = """
      name of the flake output

      (optional, default: same as `name`)
    """, default = None),
    "sub_targets": attrs.list(attrs.string(), default = []),
    "_exec_os_type": buck.exec_os_type_arg(),
}

__flake_package = rule(
    impl = lambda ctx: __flake_package_impl(
        ctx,
        ctx.attrs.path,
        ctx.attrs.package or ctx.label.name,
        ctx.attrs.output,
        ctx.attrs.binary,
        ctx.attrs.binaries,
        ctx.attrs.sub_targets,
        ctx.attrs._exec_os_type[OsLookup],
    ),
    attrs = __common_attrs,
    doc = """
    A `flake.package()` rule builds a nix package of a given flake.

    ## Examples

    ```starlark
    flake.package(
        name = "curl",
        path = "nix",
        output = "bin",
        binary = "curl",
    )
    ```

    This creates a target called `curl` from the nix flake in `./nix`, building `path:nix#packages.<system>.curl.bin`.
    """,
)

## ---------------------------------------------------------------------------------------------------------------------

flake = struct(
    package = __flake_package,
)
