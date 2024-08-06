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
    {{ _cargo }} run -p kernel --target targets/riscv64gc-k23-kernel.json {{ _buildstd }} {{ FLAGS }}

preflight *FLAGS: (lint FLAGS)

lint *FLAGS: (clippy FLAGS) (check-fmt FLAGS)

clippy $RUSTFLAGS='-Dwarnings' *FLAGS='':
    # riscv64 checks
    {{ _cargo }} clippy --target targets/riscv64gc-k23-kernel.json {{ _riscv64crates }} {{ _buildstd }} {{ FLAGS }} -- -Dclippy::all -Dclippy::pedantic

# check rustfmt for `crate`
check-fmt *FLAGS:
    {{ _cargo }} fmt --check --all {{ FLAGS }}

check *FLAGS:
    # riscv64 checks
    {{ _cargo }} check --target targets/riscv64gc-k23-kernel.json {{ _riscv64crates }} {{ _buildstd }} {{ FLAGS }}

test-riscv64 *FLAGS:
    {{ _cargo }} test --target targets/riscv64gc-k23-kernel.json -p kernel {{ _buildstd }} {{ FLAGS }}
