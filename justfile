#!/usr/bin/env just --justfile

# default recipe to display help information
default:
    @echo "justfile for k23"
    @echo "see https://just.systems/man/en/ for more details"
    @echo ""
    @just --list

# Overrides the default Rust toolchain set in `rust-toolchain.toml`.
toolchain := ""

_cargo := "cargo" + if toolchain != "" { " +" + toolchain } else { "" }

_rustflags := env_var_or_default("RUSTFLAGS", "")

_riscv64crates := "-p kernel -p loader -p kstd -p vmm"

_buildstd := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"

run-riscv64 *FLAGS:
    {{ _cargo }} run -p kernel --target riscv64gc-unknown-none-elf {{ _buildstd }} {{ FLAGS }}

preflight *FLAGS: (lint FLAGS)

lint *FLAGS: (clippy FLAGS) (check-fmt FLAGS)

clippy *FLAGS:
    # riscv64 checks
    {{ _cargo }} clippy --target riscv64gc-unknown-none-elf {{ _riscv64crates }} {{ _buildstd }} {{ FLAGS }}

# check rustfmt for `crate`
check-fmt *FLAGS:
    {{ _cargo }} fmt --check --all {{ FLAGS }}
