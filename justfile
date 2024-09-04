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

# default recipe to display help information
_default:
    @echo '{{ _docstring }}'
    @just --list


preflight crate="" *cargo_args="": (lint crate cargo_args) (test crate cargo_args)

lint crate="" *cargo_args="": (clippy crate cargo_args) (check-fmt crate cargo_args) (check-docs crate cargo_args)

clippy crate="" *cargo_args="":
    {{ _cargo }} clippy \
        {{ if crate == "" { "--workspace" } else { "-p" } }} {{ crate }} \
        $(just _print_target {{crate}}) \
        {{ _buildstd }} \
        {{ _fmt_clippy }} \
        {{ cargo_args }}

check-fmt crate="" *cargo_args="":
    {{ _cargo }} fmt --check \
        {{ if crate == "" { "--all" } else { "-p" } }} {{ crate }} \
        {{ _fmt }} \
        {{ cargo_args }}

check-docs crate="" *cargo_args="": (build-docs crate cargo_args) (test-docs crate cargo_args)

build-docs crate="" *cargo_args="":
    {{ _rustdoc }} \
        {{ if crate == '' { '--workspace' } else { '--package' } }} {{ crate }} \
        $(just _print_target {{crate}}) \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

test-docs crate="" *cargo_args="":
    {{ _cargo }} test --doc \
        {{ if crate == "" { "--workspace" } else { "--package" } }} {{ crate }} \
        --all-features \
        $(just _print_target {{crate}}) \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

build *cargo_args="":
    {{_cargo}} build \
        -p loader \
        $(just _print_target loader) \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

run *args="":
    {{_cargo}} run \
        -p loader \
        $(just _print_target loader) \
        {{ _buildstd }} \
        {{ _fmt }} \
        -- {{ args }}

test crate="" *cargo_args="":
    {{ error("TODO")  }}

check crate="" *cargo_args="":
    {{ error("TODO")  }}

manual:
    cd manual && mdbook serve

_run_riscv64 binary *args:
    @echo Running {{binary}}
    qemu-system-riscv64 \
        -kernel \
        {{binary}} \
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

@_print_target crate="":
    echo {{ \
        if crate == "loader" { \
          "--target loader/riscv64imac-k23-none-loader.json" \
        } else { \
          "--target kernel/riscv64gc-k23-none-kernel.json" \
        } \
    }}