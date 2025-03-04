##!/usr/bin/env just --justfile

set windows-shell := ["powershell.exe", "-c"]

# Overrides the default Rust toolchain set in `rust-toolchain.toml`.
toolchain := ""

# configures what profile to use for builds.
profile := env_var_or_default("K23_PROFILE", "dev")
export K23_PROFILE := profile

_cargo := "cargo" + if toolchain != "" { " +" + toolchain } else { "" }
_rustflags := env_var_or_default("RUSTFLAGS", "")
_buildstd := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"
_rustdoc := _cargo + " doc --no-deps --all-features"

_loader_artifact := "target/riscv64imac-k23-none-loader" / (if profile == "dev" { "debug" } else { profile }) / "loader"
_kernel_artifact := "target/riscv64gc-k23-none-kernel" / (if profile == "dev" { "debug" } else { profile }) / "kernel"

# If we're running in Github Actions and cargo-action-fmt is installed, then add
# a command suffix that formats errors.
_fmt_clippy := if env_var_or_default("GITHUB_ACTIONS", "") != "true" { "" } else {
    ```
    if command -v cargo-action-fmt >/dev/null 2>&1; then
        echo "--message-format=json -- -Dwarnings | cargo-action-fmt"
    fi
    ```
}

_fmt := if env_var_or_default("GITHUB_ACTIONS", "") != "true" { "" } else {
    ```
    if command -v cargo-action-fmt >/dev/null 2>&1; then
        echo "--message-format=json | cargo-action-fmt"
    fi
    ```
}

_docstring := "
justfile for k23
see https://just.systems/man/en/

Available variables:
    toolchain       # overrides the default Rust toolchain set in the
                    # rust-toolchain.toml file.
    profile         # configures what Cargo profile (release or debug) to use
                    # for builds.

Variables can be set using `just VARIABLE=VALUE ...` or
`just --set VARIABLE VALUE ...`.
"

# env var to set the cargo runner for the riscv64 target
export CARGO_TARGET_RISCV64GC_K23_NONE_KERNEL_RUNNER := "just _run_riscv64"
# as of recent Rust nightly versions the old `CARGO_RUSTC_CURRENT_DIR` we used to locate the kernel artifact from the
# loader build script got removed :/ This is a stopgap until they come up with a replacement.
# https://github.com/rust-lang/cargo/issues/3946
export __K23_CARGO_RUSTC_CURRENT_DIR := `dirname "$(cargo locate-project --workspace --message-format plain)"`

# default recipe to display help information
_default:
    @echo '{{ _docstring }}'
    @just --list

# run the OS
run cargo_args="" *args="":
    {{ _cargo }} run \
        -p kernel \
        --target kernel/riscv64gc-k23-none-kernel.json \
        --locked \
        --profile {{ profile }} \
        {{ _buildstd }} \
        {{ cargo_args }} \
        -- {{ args }}

# quick check for development
check crate="" *cargo_args="":
    {{ _cargo }} check \
        {{ if crate == "" { "--workspace --exclude loader" } else { "-p" } }} {{ crate }} \
        --target kernel/riscv64gc-k23-none-kernel.json \
        --locked \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}
    KERNEL=Cargo.toml {{ _cargo }} check \
        -p loader \
        --target loader/riscv64imac-k23-none-loader.json \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

# run all tests and checks
preflight crate="" *cargo_args="": (lint crate cargo_args)

# run lints (clippy, rustfmt, docs) for a crate or the entire for the workspace.
lint crate="" *cargo_args="": (clippy crate cargo_args) (check-fmt crate cargo_args) (check-docs crate cargo_args)

# run clippy on a crate or the entire workspace.
clippy crate="" *cargo_args="":
    {{ _cargo }} clippy \
        {{ if crate == "" { "--workspace --exclude loader" } else { "-p" } }} {{ crate }} \
        --target kernel/riscv64gc-k23-none-kernel.json \
        --locked \
        {{ _buildstd }} \
        {{ _fmt_clippy }} \
        {{ cargo_args }}
    KERNEL=Cargo.toml {{ _cargo }} clippy \
            -p loader \
            --target loader/riscv64imac-k23-none-loader.json \
            --locked \
            {{ _buildstd }} \
            {{ _fmt_clippy }} \
            {{ cargo_args }}

# check formatting for a crate or the entire workspace.
check-fmt crate="" *cargo_args="":
    {{ _cargo }} fmt --check \
        {{ if crate == "" { "--all" } else { "-p" } }} {{ crate }} \
        {{ _fmt }} \
        {{ cargo_args }}

# check documentation for a crate or the entire workspace.
check-docs crate="" *cargo_args="": (build-docs crate cargo_args) (test-docs crate cargo_args)

# build documentation for a crate or the entire workspace.
build-docs crate="" *cargo_args="":
    {{ _rustdoc }} \
        {{ if crate == '' { '--workspace --exclude loader --exclude wast' } else { '--package' } }} {{ crate }} \
        --target kernel/riscv64gc-k23-none-kernel.json \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}
    KERNEL=Cargo.toml {{ _rustdoc }} \
            -p loader \
            --target loader/riscv64imac-k23-none-loader.json \
            {{ _buildstd }} \
            {{ _fmt }} \
            {{ cargo_args }}

# test documentation for a crate or the entire workspace.
test-docs crate="" *cargo_args="":
    {{ _cargo }} test --doc \
        {{ if crate == "" { "--workspace --exclude loader" } else { "--package" } }} {{ crate }} \
        --target kernel/riscv64gc-k23-none-kernel.json \
        --locked \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

# run all tests
test $K23_PROFILE=(profile) cargo_args="" *args="": && (test-docs cargo_args)
    {{ _cargo }} test \
        -p kernel \
        --locked \
        --target kernel/riscv64gc-k23-none-kernel.json \
        --profile {{ profile }} \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }} \
        -- {{ args }}

build: && (_build_bootimg _kernel_artifact)
    {{_cargo}} build \
        -p kernel \
        --locked \
        --target kernel/riscv64gc-k23-none-kernel.json \
        --profile {{ profile }} \
        {{ _buildstd }} \
        {{ _fmt }}

# open the manual in development mode
manual:
    cd manual && mdbook serve --open

# This default configuration produces a 8-cpu system with a NUMA topology like this:
#  _____________      _____________
# |             |    |             |
# | Node 0      |    | Node 1      |
# | cpu 0,1,2,3 |-20-| cpu 4,5,6,7 |
# |_____________|    |_____________|
#
_run_riscv64 binary *args: (_build_bootimg binary)
    @echo Running {{binary}}
    qemu-system-riscv64 \
        -kernel \
        {{_loader_artifact}} \
        -machine virt \
        -cpu rv64 \
        -m 256M \
        -d guest_errors \
        -display none \
        -serial mon:stdio \
        -semihosting-config \
        enable=on,target=native \
        -smp cpus=8 \
        -object memory-backend-ram,size=128M,id=m0 \
        -object memory-backend-ram,size=128M,id=m1 \
        -numa node,cpus=0-3,nodeid=0,memdev=m0 \
        -numa node,cpus=4-7,nodeid=1,memdev=m1 \
        -numa dist,src=0,dst=1,val=20 \
        -monitor unix:qemu-monitor-socket,server,nowait \
        {{args}}

_build_bootimg $KERNEL:
    {{_cargo}} build \
        -p loader \
        --locked \
        --target loader/riscv64imac-k23-none-loader.json \
        --profile {{ profile }} \
        {{ _buildstd }} \
        {{ _fmt }}