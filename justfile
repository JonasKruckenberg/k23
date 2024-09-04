##!/usr/bin/env just --justfile

# Overrides the default Rust toolchain set in `rust-toolchain.toml`.
toolchain := ""

# configures what profile to use for builds.
profile := "dev"

_cargo := "cargo" + if toolchain != "" { " +" + toolchain } else { "" }
_rustflags := env_var_or_default("RUSTFLAGS", "")
_buildstd := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"
_rustdoc := _cargo + " doc --no-deps --all-features"

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

export CARGO_TARGET_RISCV64GC_K23_NONE_KERNEL_RUNNER := "just _run_riscv64"

# default recipe to display help information
_default:
    @echo '{{ _docstring }}'
    @just --list

# run the OS
run cargo_args *args="":
    {{ _cargo }} run \
        -p kernel \
        --target kernel/riscv64gc-k23-none-kernel.json \
        --profile {{ profile }} \
        {{ _buildstd }} \
        {{ cargo_args }} \
        -- {{ args }}

# run all tests and checks
preflight crate="" *cargo_args="": (lint crate cargo_args)

# run lints (clippy, rustfmt, docs) for a crate or the entire for the workspace.
lint crate="" *cargo_args="": (clippy crate cargo_args) (check-fmt crate cargo_args) (check-docs crate cargo_args)

# run clippy on a crate or the entire workspace.
clippy crate="" *cargo_args="":
    {{ _cargo }} clippy \
        {{ if crate == "" { "--workspace --exclude loader --exclude panic" } else { "-p" } }} {{ crate }} \
        --target kernel/riscv64gc-k23-none-kernel.json \
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
        {{ if crate == '' { '--workspace --exclude panic --exclude loader --exclude wast' } else { '--package' } }} {{ crate }} \
        --target kernel/riscv64gc-k23-none-kernel.json \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

# test documentation for a crate or the entire workspace.
test-docs crate="" *cargo_args="":
    {{ _cargo }} test --doc \
        {{ if crate == "" { "--workspace --exclude panic --exclude loader" } else { "--package" } }} {{ crate }} \
        --target kernel/riscv64gc-k23-none-kernel.json \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

# run all tests
test cargo_args="" *args="": && (test-docs cargo_args)
    {{ _cargo }} test \
        -p kernel \
        --target kernel/riscv64gc-k23-none-kernel.json \
        --profile {{ profile }} \
        {{ _buildstd }} \
        {{ cargo_args }} \
        -- {{ args }}

# open the manual in development mode
manual:
    cd manual && mdbook serve --open

_run_riscv64 binary *args: (_build_bootimg binary)
    @echo Running {{binary}}
    qemu-system-riscv64 \
        -kernel \
        target/riscv64imac-k23-none-loader/{{ if profile == "dev" { "debug" } else { profile } }}/loader \
        -machine virt \
        -cpu rv64 \
        -smp 1 \
        -m 512M \
        -d guest_errors,int \
        -display none \
        -serial stdio \
        -semihosting-config \
        enable=on,target=native \
        {{args}}

_build_bootimg $KERNEL:
    {{_cargo}} build \
        -p loader \
        --target loader/riscv64imac-k23-none-loader.json \
        --profile {{ profile }} \
        {{ _buildstd }} \
        {{ _fmt }}