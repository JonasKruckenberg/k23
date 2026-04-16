load("@prelude//rust:rust_toolchain.bzl", "PanicRuntime", "RustToolchainInfo", "RustExplicitSysrootDeps")
load("@prelude//rust:link_info.bzl", "RustLinkInfo")

_DEFAULT_TRIPLE = select({
    "config//os:linux": select({
        "config//cpu:arm64": "aarch64-unknown-linux-gnu",
        "config//cpu:x86_64": "x86_64-unknown-linux-gnu",
    }),
    "config//os:macos": select({
        "config//cpu:arm64": "aarch64-apple-darwin",
        "config//cpu:x86_64": "x86_64-apple-darwin",
    }),
    "config//os:windows": select({
        "config//cpu:arm64": select({
            # Rustup's default ABI for the host on Windows is MSVC, not GNU.
            # When you do `rustup install stable` that's the one you get. It
            # makes you opt in to GNU by `rustup install stable-gnu`.
            "DEFAULT": "aarch64-pc-windows-msvc",
            "config//abi:gnu": "aarch64-pc-windows-gnu",
            "config//abi:msvc": "aarch64-pc-windows-msvc",
        }),
        "config//cpu:x86_64": select({
            "DEFAULT": "x86_64-pc-windows-msvc",
            "config//abi:gnu": "x86_64-pc-windows-gnu",
            "config//abi:msvc": "x86_64-pc-windows-msvc",
        }),
    }),
})

def _rust_toolchain_impl(ctx: AnalysisContext) -> list[Provider]:
    return [
        DefaultInfo(),
        RustToolchainInfo(
            compiler = ctx.attrs.rustc[RunInfo],
            clippy_driver = ctx.attrs.clippy[RunInfo],
            rustdoc = ctx.attrs.rustdoc[RunInfo],
            miri_driver = ctx.attrs.miri_driver[RunInfo],
            explicit_sysroot_deps = RustExplicitSysrootDeps(
              core = ctx.attrs.explicit_sysroot_deps.pop("core", None),
              proc_macro = ctx.attrs.explicit_sysroot_deps.pop("proc_macro", None),
              std = ctx.attrs.explicit_sysroot_deps.pop("std", None),
              panic_unwind = ctx.attrs.explicit_sysroot_deps.pop("panic_unwind", None),
              panic_abort = ctx.attrs.explicit_sysroot_deps.pop("panic_abort", None),
              others = ctx.attrs.explicit_sysroot_deps.values(),
            ) if len(ctx.attrs.explicit_sysroot_deps) > 0 else None,
            allow_lints = ctx.attrs.allow_lints,
            clippy_toml = ctx.attrs.clippy_toml[DefaultInfo].default_outputs[0] if ctx.attrs.clippy_toml else None,
            default_edition = ctx.attrs.default_edition,
            panic_runtime = PanicRuntime(ctx.attrs.panic_runtime),
            deny_lints = ctx.attrs.deny_lints,
            doctests = ctx.attrs.doctests,
            miri_sysroot_path = ctx.attrs.miri_sysroot_path[DefaultInfo].default_outputs[0] if ctx.attrs.miri_sysroot_path else None,
            nightly_features = ctx.attrs.nightly_features,
            report_unused_deps = ctx.attrs.report_unused_deps,
            rustc_binary_flags = ctx.attrs.rustc_binary_flags,
            rustc_flags = ctx.attrs.rustc_flags,
            rustc_target_triple = ctx.attrs.rustc_target_triple,
            rust_target_path = ctx.attrs.rust_target_path,
            rustc_test_flags = ctx.attrs.rustc_test_flags,
            rustdoc_flags = ctx.attrs.rustdoc_flags,
            warn_lints = ctx.attrs.warn_lints,
        ),
    ]

rust_toolchain = rule(
    impl = _rust_toolchain_impl,
    attrs = {
        "allow_lints": attrs.list(attrs.string(), default = []),
        "clippy": attrs.exec_dep(providers = [RunInfo]),
        "clippy_toml": attrs.option(attrs.dep(providers = [DefaultInfo]), default = None),
        "default_edition": attrs.option(attrs.string(), default = None),
        "deny_lints": attrs.list(attrs.string(), default = []),
        "doctests": attrs.bool(default = False),
        "explicit_sysroot_deps": attrs.dict(attrs.string(), attrs.dep(providers = [RustLinkInfo]), default = {}),
        "miri_driver": attrs.exec_dep(providers = [RunInfo]),
        "miri_sysroot_path": attrs.option(attrs.dep(providers = [DefaultInfo]), default = None),
        "nightly_features": attrs.bool(default = False),
        "report_unused_deps": attrs.bool(default = False),
        "rustc": attrs.exec_dep(providers = [RunInfo], doc = "the Rust compiler"),
        "rustc_binary_flags": attrs.list(attrs.string(), default = []),
        "rustc_flags": attrs.list(attrs.string(), default = []),
        "rustc_target_triple": attrs.string(default = _DEFAULT_TRIPLE),
        "rust_target_path": attrs.dep(),
        "rustc_test_flags": attrs.list(attrs.string(), default = []),
        "rustdoc": attrs.exec_dep(providers = [RunInfo], doc = "the Rust documentation tool"),
        "rustdoc_flags": attrs.list(attrs.string(), default = []),
        "warn_lints": attrs.list(attrs.string(), default = []),
        "panic_runtime": attrs.enum(["unwind", "abort", "none"], default = "unwind")
    },
    doc = """
    Creates a rust toolchain that is required by all Rust rules.

    ## Examples

    ```starlark
    # expose the Rust toolchain from the Nix flake
    flake.package(
        name = "rust_toolchain",
        package = "rustToolchain",
        binaries = ["rustc", "rustdoc", "clippy-driver", "miri-driver"],
        path = "root//:flake",
    )

    # provide the `rust` toolchain using sub-targets of the flake package
    rust_toolchain(
        name = "rust",
        rustc = ":rust_toolchain[rustc]",
        clippy = ":rust_toolchain[clippy-driver]",
        rustdoc = ":rust_toolchain[rustdoc]",
        miri_driver = ":rust_toolchain[miri-driver]",
        default_edition = "2024",
        visibility = ["PUBLIC"],
    )
    ```
    """,
    is_toolchain_rule = True,
)
