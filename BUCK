filegroup(
    name = "flake",
    srcs = [
        "flake.lock",
        "flake.nix",
        "rust-toolchain.toml" # required by rust-overlay
    ],
    visibility = ["PUBLIC"],
)

filegroup(
    name = "wast_tests",
    srcs = glob(["tests/**/*.wast"]),
    visibility = ["PUBLIC"],
)
