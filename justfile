##!/usr/bin/env just --justfile

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

# Overrides the default Rust toolchain set in `rust-toolchain.toml`.
toolchain := ""

# configures what profile to use for builds.
profile := "dev"

_cargo := "cargo" + if toolchain != "" { " +" + toolchain } else { "" }
_rustflags := env_var_or_default("RUSTFLAGS", "")
_buildstd := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"
_target_dir := env_var_or_default("CARGO_TARGET_DIR", justfile_directory() / "target")

_signing_key := env_var_or_default("SIGNING_KEY", `openssl genpkey -algorithm Ed25519 -outform pem`)
_kernel_target := "./configs/riscv64gc-k23-none-kernel.json"
_loader_target := "./configs/riscv64imac-k23-none-loader.json"

# default recipe to display help information
_default:
    @echo '{{ _docstring }}'
    @just --list

# run all tests and checks for all targets
preflight *FLAGS: (lint "configs/riscv64-qemu.toml" FLAGS)

# run lints (clippy, rustfmt) for the workspace
lint *FLAGS: (clippy FLAGS) (check-fmt FLAGS)

# run clippy lints for the workspace
clippy $RUSTFLAGS='-Dwarnings' *CARGO_ARGS='':
    # check kernel and dependencies
    {{_cargo}} clippy \
        -p kernel \
        --target {{_kernel_target}} \
        --profile {{profile}} \
        {{_buildstd}} \
        {{CARGO_ARGS}}

    # check loader and dependencies
    {{_cargo}} clippy \
        -p loader \
        --target {{_kernel_target}} \
        --profile {{profile}} \
        {{_buildstd}} \
        {{CARGO_ARGS}}

# run checks for the workspace
check $RUSTFLAGS='' *CARGO_ARGS='':
    # check kernel and dependencies
    {{_cargo}} check \
        -p kernel \
        --target {{_kernel_target}} \
        --profile {{profile}} \
        {{_buildstd}} \
        {{CARGO_ARGS}}

    # check loader and dependencies
    {{_cargo}} check \
        -p loader \
        --target {{_loader_target}} \
        --profile {{profile}} \
        {{_buildstd}} \
        {{CARGO_ARGS}}

# check rustfmt for `crate`
check-fmt *FLAGS:
    {{ _cargo }} fmt --check --all {{ FLAGS }}

# Builds the kernel using the given config and runs it using QEMU
run CARGO_ARGS="" *ARGS="": (build CARGO_ARGS) (_run "target/k23/bootimg.bin" ARGS)

# Builds the kernel using the given config
build *CARGO_ARGS="": && (_make_bootimg "target/k23/kernel" CARGO_ARGS)
    #!/usr/bin/env nu
    let out_dir = "{{_target_dir}}" | path join "k23"
    mkdir $out_dir

    let cargo_out = ({{_cargo}} build
        -p kernel
        --target {{_kernel_target}}
        --profile {{profile}}
        --message-format=json
        {{_buildstd}}
        {{CARGO_ARGS}})
    cp ($cargo_out | from json --objects | last 2 | get 0.executable) ($out_dir | path join kernel)

# Runs the tests for the kernel
test *CARGO_ARGS="" :
    #!/usr/bin/env nu
    # CARGO_TARGET_<triple>_RUNNER
    $env.CARGO_TARGET_RISCV64GC_K23_NONE_KERNEL_RUNNER = "just profile={{profile}} _runner"

    {{ _cargo }} test -p kernel --target {{_kernel_target}} {{ _buildstd }} {{ CARGO_ARGS }}

# This is a helper recipe designed to be used as a cargo *target runner*
# When running tests, the `cargo test` command will produce potentially many executables.
# The paths to these files are not known ahead of time, so we use this runner trick to package each executable
# into a bootable image and run it using QEMU.
_runner binary *ARGS: (_make_bootimg binary) (_run "target/k23/bootimg.bin" ARGS)

# Runs the given binary that has been built using the config using QEMU
#
# This recipe is designed to be used as a dependency of other, user-facing recipes (such as `run` and `test`)
_run binary *ARGS:
    #!/usr/bin/env nu
    print {{binary}}
    (qemu-system-riscv64
        "-kernel"
        {{binary}}
        "-machine" "virt"
        "-cpu" "rv64"
        "-smp" "1"
        "-m" "512M"
        "-d" "guest_errors,int"
        "-display" "none"
        "-serial" "stdio"
        "-semihosting-config"
        "enable=on,target=native"
        {{ARGS}}
        #"-monitor"
        #"unix:qemu-monitor-socket,server,nowait"
        )

# This takes in the given config and kernel and creates a bootable image. This is the "magic" build step that makes it all work!
#
# The boot image is created by:
# 1. Building the bootloader
# 2. Compressing the kernel
# 3. Signing the compressed kernel
# 4. Embedding the public key, signature and compressed kernel in the bootloader
#
# This recipe is designed to be used as a dependency of other, user-facing recipes
_make_bootimg kernel *CARGO_ARGS="":
    #!/usr/bin/env nu
    let out_dir = "{{_target_dir}}" | path join "k23"
    mkdir $out_dir

    let loader_path = ($out_dir | path join loader)
    let bootimg_path = ($out_dir | path join bootimg.bin)

    # Step 1: Compress the kernel
    print "Compressing the kernel..."
    let kernel_lz4_path = "{{kernel}}.lz4"
    {{_cargo}} run -p lz4-block-compress {{kernel}} $kernel_lz4_path

    $env.K23_KERNEL_PATH = $kernel_lz4_path

    # Step 3: Build the bootloader
    print "Building the bootloader..."
    let cargo_out = ({{_cargo}} build
        -p loader
        --target {{_loader_target}}
        --profile {{profile}}
        --message-format=json
        {{_buildstd}}
        {{CARGO_ARGS}})
    cp ($cargo_out | from json --objects | last 2 | get 0.executable) $bootimg_path