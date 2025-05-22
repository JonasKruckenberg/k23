##!/usr/bin/env just --justfile

set windows-shell := ["powershell.exe", "-c"]

# Overrides the default Rust toolchain set in `rust-toolchain.toml`.

toolchain := ""

# configures what profile to use for builds.

_cargo := "cargo" + if toolchain != "" { " +" + toolchain } else { "" }
_buildstd := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"
_rustdoc := _cargo + " doc --no-deps --all-features"

# If we're running in Github Actions and cargo-action-fmt is installed, then add
# a command suffix that formats errors.

_fmt_clippy := if env_var_or_default("GITHUB_ACTIONS", "") != "true" { "" } else { ```
    if command -v cargo-action-fmt >/dev/null 2>&1; then
        echo "--message-format=json -- -Dwarnings | cargo-action-fmt"
    fi
    ``` }
_fmt := if env_var_or_default("GITHUB_ACTIONS", "") != "true" { "" } else { ```
    if command -v cargo-action-fmt >/dev/null 2>&1; then
        echo "--message-format=json | cargo-action-fmt"
    fi
    ``` }
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

# as of recent Rust nightly versions the old `CARGO_RUSTC_CURRENT_DIR` we used to locate the kernel artifact from the
# loader build script got removed :/ This is a stopgap until they come up with a replacement.
# https://github.com/rust-lang/cargo/issues/3946
export __K23_CARGO_RUSTC_CURRENT_DIR := `dirname "$(cargo locate-project --workspace --message-format plain)"`

# default recipe to display help information
_default:
    @echo '{{ _docstring }}'
    @just --list

# Alias for `cargo xtask qemu`
run profile args="" *qemu_args="":
    {{ _cargo }} xtask run {{ profile }} {{ args }} -- {{ qemu_args }}

# Alias for `cargo xtask build`
build profile args="" *qemu_args="":
    {{ _cargo }} xtask build {{ profile }} {{ args }} -- {{ qemu_args }}

# quick check for development
check crate="" *cargo_args="":
    {{ _cargo }} check \
        {{ if crate == "" { "--workspace --exclude loader --exclude xtask --exclude toml-patch" } else { "-p" } }} {{ crate }} \
        --target profile/riscv64/riscv64gc-k23-none-kernel.json \
        --locked \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}
    KERNEL=Cargo.toml {{ _cargo }} check \
        -p loader \
        --target riscv64gc-unknown-none-elf \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

# run all tests and checks
preflight crate="" *cargo_args="": (lint crate cargo_args)
    typos

# run lints (clippy, rustfmt, docs) for a crate or the entire for the workspace.
lint crate="" *cargo_args="": (clippy crate cargo_args) (check-fmt crate cargo_args) (check-docs crate cargo_args)

# run clippy on a crate or the entire workspace.
clippy crate="" *cargo_args="":
    {{ _cargo }} clippy \
        {{ if crate == "" { "--workspace --exclude loader --exclude xtask --exclude toml-patch" } else { "-p" } }} {{ crate }} \
        --target profile/riscv64/riscv64gc-k23-none-kernel.json \
        --locked \
        {{ _buildstd }} \
        {{ _fmt_clippy }} \
        {{ cargo_args }}
    KERNEL=Cargo.toml {{ _cargo }} clippy \
            -p loader \
            --target riscv64gc-unknown-none-elf \
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
        {{ if crate == '' { '--workspace --exclude loader --exclude wast --exclude xtask --exclude toml-patch --exclude async-exec' } else { '--package' } }} {{ crate }} \
        --target profile/riscv64/riscv64gc-k23-none-kernel.json \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}
    KERNEL=Cargo.toml {{ _rustdoc }} \
            -p loader \
            --target riscv64gc-unknown-none-elf \
            {{ _buildstd }} \
            {{ _fmt }} \
            {{ cargo_args }}

# test documentation for a crate or the entire workspace.
test-docs crate="" *cargo_args="":
    {{ _cargo }} test --doc \
        {{ if crate == "" { "--workspace --exclude loader --exclude xtask --exclude toml-patch --exclude fiber --exclude fastrand --exclude async-exec" } else { "--package" } }} {{ crate }} \
        --target profile/riscv64/riscv64gc-k23-none-kernel.json \
        --locked \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

# run all tests
test cargo_args="" *args="":
    {{ _cargo }} test \
        -p kernel \
        --target profile/riscv64/riscv64gc-k23-none-kernel.json \
        --locked \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }} \
        -- {{ args }}

# open the manual in development mode
manual:
    cd manual && mdbook serve --open
