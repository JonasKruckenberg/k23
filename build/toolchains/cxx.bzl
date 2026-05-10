load(
    "@prelude//cxx:cxx_toolchain_types.bzl",
    "BinaryUtilitiesInfo",
    "CCompilerInfo",
    "CxxCompilerInfo",
    "CxxInternalTools",
    "CxxPlatformInfo",
    "CxxToolchainInfo",
    "LinkerInfo",
    "LinkerType",
    "PicBehavior",
    "ShlibInterfacesMode",
    "RuntimeDependencyHandling"
)
load("@prelude//cxx:headers.bzl", "HeaderMode")
load("@prelude//cxx:linker.bzl", "is_pdb_generated")
load("@prelude//linking:link_info.bzl", "LinkOrdering", "LinkStyle")
load("@prelude//linking:lto.bzl", "LtoMode")
load("@prelude//decls:common.bzl", "buck")
load("@prelude//os_lookup:defs.bzl", "Os", "OsLookup")

def _clang_toolchain(ctx: AnalysisContext) -> list[Provider]:
    clang = ctx.attrs.clang[DefaultInfo].sub_targets
    os = ctx.attrs._target_os_type[OsLookup].os

    compiler = clang["cc"][RunInfo]
    cxx_compiler = clang["c++"][RunInfo]

    compiler_type = "clang"
    archiver = clang["ar"][RunInfo]
    archiver_type = "gnu"
    archiver_supports_argfiles = True
    asm_compiler = compiler
    asm_compiler_type = compiler_type
    binary_extension = ""
    object_file_extension = "o"
    static_library_extension = "a"
    shared_library_name_default_prefix = "lib"
    shared_library_name_format = "{}.so"
    shared_library_versioned_name_format = "{}.so.{}"
    llvm_link = RunInfo(args = ["llvm-link"])

    if ctx.attrs.linker_dispatch == "driver":
        # Drive linking through clang and force it to invoke the supplied
        # linker via --ld-path. Use this for hosted targets where rustc emits
        # cc-driver flags (sysroot, -arch, -lSystem, ...). Pinning --ld-path
        # makes the linker binary reproducible regardless of system PATH.
        linker = cxx_compiler
        additional_linker_flags = [
            cmd_args(ctx.attrs.linker[RunInfo].args, format = "--ld-path={}"),
        ]

        # --ld-path bypasses clang's bintools-wrapper, so bake an rpath to
        # the C++ runtime ourselves.
        if ctx.attrs.cxx_runtime_lib:
            additional_linker_flags.append(
                cmd_args(ctx.attrs.cxx_runtime_lib[DefaultInfo].default_outputs[0], format = "-Wl,-rpath,{}/lib"),
            )
    else:
        # Invoke the linker directly. Use this for freestanding targets whose
        # rustc target spec sets linker-flavor=gnu-lld (raw lld flags only).
        linker = ctx.attrs.linker[RunInfo]
        additional_linker_flags = []

    if os == Os("macos"):
        linker_type = LinkerType("darwin")
        pic_behavior = PicBehavior("always_enabled")
    else:
        linker_type = LinkerType("gnu")
        pic_behavior = PicBehavior("supported")

    return [
        DefaultInfo(),
        CxxToolchainInfo(
            internal_tools = ctx.attrs._internal_tools[CxxInternalTools],
            linker_info = LinkerInfo(
                linker = RunInfo(args = linker),
                linker_flags = additional_linker_flags + ctx.attrs.link_flags,
                archiver = archiver,
                archiver_type = archiver_type,
                archiver_supports_argfiles = archiver_supports_argfiles,
                generate_linker_maps = False,
                lto_mode = LtoMode("none"),
                type = linker_type,
                link_binaries_locally = True,
                archive_objects_locally = True,
                use_archiver_flags = True,
                static_dep_runtime_ld_flags = [],
                static_pic_dep_runtime_ld_flags = [],
                shared_dep_runtime_ld_flags = [],
                independent_shlib_interface_linker_flags = [],
                shlib_interfaces = ShlibInterfacesMode("disabled"),
                link_style = LinkStyle(ctx.attrs.link_style),
                link_weight = 1,
                binary_extension = binary_extension,
                object_file_extension = object_file_extension,
                shared_library_name_default_prefix = shared_library_name_default_prefix,
                shared_library_name_format = shared_library_name_format,
                shared_library_versioned_name_format = shared_library_versioned_name_format,
                static_library_extension = static_library_extension,
                force_full_hybrid_if_capable = False,
                is_pdb_generated = is_pdb_generated(linker_type, ctx.attrs.link_flags),
                link_ordering = ctx.attrs.link_ordering,
            ),
            bolt_enabled = False,
            binary_utilities_info = BinaryUtilitiesInfo(
                nm = clang["nm"][RunInfo],
                objcopy = clang["objcopy"][RunInfo],
                ranlib = clang["ranlib"][RunInfo],
                strip = clang["strip"][RunInfo],
                dwp = None,
                bolt_msdk = None,
            ),
            cxx_compiler_info = CxxCompilerInfo(
                compiler = RunInfo(args = [cxx_compiler]),
                preprocessor_flags = [],
                compiler_flags = ctx.attrs.cxx_flags,
                compiler_type = compiler_type,
            ),
            c_compiler_info = CCompilerInfo(
                compiler = RunInfo(args = [compiler]),
                preprocessor_flags = [],
                compiler_flags = ctx.attrs.c_flags,
                compiler_type = compiler_type,
            ),
            as_compiler_info = CCompilerInfo(
                compiler = RunInfo(args = [compiler]),
                compiler_type = compiler_type,
            ),
            asm_compiler_info = CCompilerInfo(
                compiler = RunInfo(args = [asm_compiler]),
                compiler_type = asm_compiler_type,
            ),
            header_mode = HeaderMode("symlink_tree_only"),
            cpp_dep_tracking_mode = ctx.attrs.cpp_dep_tracking_mode,
            pic_behavior = pic_behavior,
            llvm_link = llvm_link,
            runtime_dependency_handling = RuntimeDependencyHandling("no_symlink"),
        ),
        CxxPlatformInfo(name = "aarch64" if host_info().arch.is_aarch64 else "x86_64"),
    ]

clang_toolchain = rule(
    impl = _clang_toolchain,
    attrs = {
        "_internal_tools": attrs.default_only(attrs.exec_dep(providers = [CxxInternalTools], default = "prelude//cxx/tools:internal_tools")),
        "c_flags": attrs.list(attrs.string(), default = []),
        "cpp_dep_tracking_mode": attrs.string(default = "makefile"),
        "cxx_flags": attrs.list(attrs.string(), default = []),
        "link_ordering": attrs.option(attrs.enum(LinkOrdering.values()), default = None),
        "link_flags": attrs.list(attrs.string(), default = []),
        "link_style": attrs.string(default = "shared"),
        "clang": attrs.exec_dep(),
        "linker": attrs.exec_dep(),
        "linker_dispatch": attrs.enum(["direct", "driver"]),
        "cxx_runtime_lib": attrs.option(attrs.dep(), default = None),
        "_target_os_type": buck.target_os_type_arg(),
    },
    doc = """
    Creates a cxx toolchain backed by clang plus an explicit linker binary.

    `linker_dispatch` selects how `linker` is invoked:
    - `"direct"`: invoked as-is with raw linker flags. Use for freestanding
      rustc targets whose target spec already speaks raw linker (e.g.
      `linker-flavor: "gnu-lld"`).
    - `"driver"`: linking goes through clang, which is forced to dispatch
      to `linker` via `--ld-path`. Use for hosted targets where rustc emits
      cc-driver flags (sysroot, -arch, -lSystem, ...).
    """,
    is_toolchain_rule = True,
)
