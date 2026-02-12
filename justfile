##!/usr/bin/env just --justfile

set windows-shell := ["powershell.exe", "-c"]

# Overrides the default Rust toolchain set in `rust-toolchain.toml`.
toolchain := ""

# disables cargo nextest
no-nextest := ''

_docstring := "
justfile for k23
see https://just.systems/man/en/

Available variables:
    toolchain       # overrides the default Rust toolchain set in the
                    # rust-toolchain.toml file.
    profile         # configures what Cargo profile (release or debug) to use
                    # for builds.
    no-nextest      # disable running tests with cargo-nextest, use the regular test runner instead.

Variables can be set using `just VARIABLE=VALUE ...` or
`just --set VARIABLE VALUE ...`.
"

# default recipe to display help information
_default:
    @echo '{{ _docstring }}'
    @just --list

# Alias for `cargo xtask qemu`
run configuration args="" *qemu_args="":
    {{ _cargo }} xtask run {{ configuration }} {{ args }} {{ qemu_args }}

# Alias for `cargo xtask build`
build configuration args="" *qemu_args="":
    {{ _cargo }} xtask build {{ configuration }} {{ args }} {{ qemu_args }}

# quick check for development
check crate="" *cargo_args="":
    RUSTFLAGS=-Dwarnings {{ _cargo }} check \
        {{ if crate == "" { "--workspace --exclude loader --exclude xtask --exclude toml-patch" } else { "-p" } }} {{ crate }} \
        --target configurations/riscv64/riscv64gc-k23-none-kernel.json \
        --locked \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}
    RUSTFLAGS=-Dwarnings KERNEL=Cargo.toml {{ _cargo }} check \
        -p loader \
        --target riscv64gc-unknown-none-elf \
        {{ _buildstd }} \
        {{ _fmt }} \
        {{ cargo_args }}

# run all tests and checks
preflight crate="" *cargo_args="": (lint crate cargo_args) (test crate cargo_args) (miri crate cargo_args) (loom crate cargo_args)
    typos

# run lints (clippy, rustfmt, docs) for a crate or the entire for the workspace.
lint crate="" *cargo_args="": (clippy crate cargo_args) (check-fmt crate cargo_args)

# run clippy on a crate or the entire workspace.
clippy crate="" *cargo_args="":
    RUSTFLAGS=-Dwarnings {{ _cargo }} clippy \
        {{ if crate == "" { "--workspace --exclude loader --exclude xtask --exclude toml-patch" } else { "-p" } }} {{ crate }} \
        --target configurations/riscv64/riscv64gc-k23-none-kernel.json \
        --locked \
        {{ _buildstd }} \
        {{ _fmt_clippy }} \
        {{ cargo_args }}
    RUSTFLAGS=-Dwarnings KERNEL=Cargo.toml {{ _cargo }} clippy \
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

# ==============================================================================
# Hosted Testing
# ==============================================================================

# crates that have hosted tests
_hosted_crates := "-p kaddr2line -p kmem -p kcpu-local -p kfastrand -p kfdt -p kasync --features counters -p ksharded-slab -p kspin -p kwast -p wavltree"
# run hosted tests
test crate="" *cargo_args="": _get-nextest
    RUSTFLAGS=-Dwarnings {{ _cargo }} {{ _testcmd }} \
            {{ if crate == "" { _hosted_crates } else { "-p" + crate } }} \
            --lib \
            --no-fail-fast \
            {{ cargo_args }}

# crates that have miri tests
_miri_crates := "-p kasync --features counters -p ksharded-slab -p kspin -p wavltree"
# run hosted tests under miri
miri crate="" *cargo_args="": _get-nextest
    MIRIFLAGS="{{ env_var_or_default("MIRIFLAGS", "-Zmiri-strict-provenance -Zmiri-disable-isolation") }} -Zmiri-env-forward=RUST_BACKTRACE -Zmiri-env-forward=RUST_LOG" \
        RUSTFLAGS="{{ env_var_or_default("RUSTFLAGS", "-Dwarnings -Zrandomize-layout") }}" \
        {{ _cargo }} miri {{ _testcmd }} \
            {{ if crate == "" { _miri_crates } else { "-p" + crate } }} \
            --lib \
            --no-fail-fast \
            {{ cargo_args }}

# crates that have loom tests
_loom_crates := "-p kasync --features counters -p kspin"
# run hosted tests under loom
loom crate="" *cargo_args='': _get-nextest
    #!/usr/bin/env bash
    set -euo pipefail
    source "./util/shell.sh"

    export RUSTFLAGS="--cfg loom ${RUSTFLAGS:-}"
    export LOOM_LOG="${LOOM_LOG:-kasync=trace,cordyceps=trace,debug}"
    export LOOM_MAX_PREEMPTIONS=2

    # if logging is enabled, also enable location tracking.
    if [[ "${LOOM_LOG}" != "off" ]]; then
        export LOOM_LOCATION=true
        status "Enabled" "logging, LOOM_LOG=${LOOM_LOG}"
    else
        status "Disabled" "logging and location tracking"
    fi

    if [[ "${LOOM_CHECKPOINT_FILE:-}" ]]; then
        export LOOM_CHECKPOINT_FILE="${LOOM_CHECKPOINT_FILE:-}"
        export LOOM_CHECKPOINT_INTERVAL="${LOOM_CHECKPOINT_INTERVAL:-100}"
        status "Saving" "checkpoints to ${LOOM_CHECKPOINT_FILE} every ${LOOM_CHECKPOINT_INTERVAL} iterations"
    fi

    # if the loom tests fail, we still want to be able to print the checkpoint
    # location before exiting.
    set +e

    # run loom tests
    {{ _cargo }} {{ _testcmd }} \
        {{ _loom-profile }} \
        {{ if crate == "" { _loom_crates } else { "-p" + crate } }} \
        --lib \
        --no-fail-fast \
        {{ cargo_args }}
    status="$?"

    if [[ "${LOOM_CHECKPOINT_FILE:-}" ]]; then
        status "Checkpoints" "saved to ${LOOM_CHECKPOINT_FILE}"
    fi

    exit "$status"

# ==============================================================================
# On-Target Testing
# ==============================================================================

# run on-target tests for RISCV 64-bit
test-riscv64 *args='':
    cargo xtask test configurations/riscv64/qemu.toml --release {{ args }}

# ==============================================================================
# Documentation
# ==============================================================================

# open the manual in development mode
manual:
    cd manual && mdbook serve --open

## build documentation for a crate or the entire workspace.
#build-docs crate="" *cargo_args="":
#    {{ _rustdoc }} \
#        {{ if crate == "" { _hosted_crates } else { "-p" + crate } }} \
#        --target configurations/riscv64/riscv64gc-k23-none-kernel.json \
#        {{ _buildstd }} \
#        {{ _fmt }} \
#        {{ cargo_args }}
#    KERNEL=Cargo.toml {{ _rustdoc }} \
#            -p loader \
#            --target riscv64gc-unknown-none-elf \
#            {{ _buildstd }} \
#            {{ _fmt }} \
#            {{ cargo_args }}
#
## check documentation for a crate or the entire workspace.
#check-docs crate="" *cargo_args="": (build-docs crate cargo_args) (test-docs crate cargo_args)
#
## test documentation for a crate or the entire workspace.
#test-docs crate="" *cargo_args="":
#    {{ _cargo }} test --doc \
#        {{ if crate == "" { _hosted_crates } else { "-p" + crate } }} \
#        --locked \
#        {{ _buildstd }} \
#        {{ _fmt }} \
#        {{ cargo_args }}

# ==============================================================================
# Private state and commands
# ==============================================================================

# configures what profile to use for builds.
_cargo := "cargo" + if toolchain != "" { " +" + toolchain } else { "" }
_buildstd := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"
_rustdoc := _cargo + " doc --no-deps --all-features"

# as of recent Rust nightly versions the old `CARGO_RUSTC_CURRENT_DIR` we used to locate the kernel artifact from the
# loader build script got removed :/ This is a stopgap until they come up with a replacement.
# https://github.com/rust-lang/cargo/issues/3946
export __K23_CARGO_RUSTC_CURRENT_DIR := `dirname "$(cargo locate-project --workspace --message-format plain)"`

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
_testcmd := if no-nextest == "" { "nextest run" } else { "test" }
_loom-profile := if no-nextest == '' { '--cargo-profile loom' } else { '--profile loom' }

_get-nextest:
    #!/usr/bin/env bash
    set -euo pipefail
    source "./util/shell.sh"

    if [ -n "{{ no-nextest }}" ]; then
        status "Configured" "not to use cargo nextest"
        exit 0
    fi

    if {{ _cargo }} --list | grep -q 'nextest'; then
        status "Found" "cargo nextest"
        exit 0
    fi

    err "missing cargo-nextest executable"
    if confirm "      install it?"; then
        cargo install cargo-nextest
    fi
