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

# default recipe to display help information
_default:
    @echo '{{ _docstring }}'
    @just --list

# run all tests and checks for all targets
preflight *FLAGS: (lint "configs/riscv64-qemu.toml" FLAGS)

# run lints (clippy, rustfmt) for the workspace
lint config *FLAGS: (clippy config FLAGS) (check-fmt config FLAGS)

# run clippy lints for the workspace
clippy config $RUSTFLAGS='-Dwarnings' *CARGO_ARGS='':
    #!/usr/bin/env nu
    let config = open {{config}}

    # run clippy against all crates across the workspace *except the bootloader which has its own target)
    ({{_cargo}} clippy
        --workspace
        --tests
        --benches
        --exclude loader
        --target $config.kernel.target
        --profile {{profile}}
        {{_buildstd}}
        {{CARGO_ARGS}})

    #run clippy against the loader crate and it's specific target
    ({{_cargo}} clippy
            -p loader
            --tests
            --benches
            --target $config.kernel.target
            --profile {{profile}}
            {{_buildstd}}
            {{CARGO_ARGS}})

# run checks for the workspace
check config $RUSTFLAGS='' *CARGO_ARGS='':
    #!/usr/bin/env nu
    let config = open {{config}}
    def check_crate [crate, target, ...args] {
        ({{_cargo}} check -p $crate --target $target --profile {{profile}} {{_buildstd}} {{CARGO_ARGS}})
    }

    check_crate "kernel" $config.kernel.target
    check_crate "loader" $config.loader.target

# check rustfmt for `crate`
check-fmt *FLAGS:
    {{ _cargo }} fmt --check --all {{ FLAGS }}

# Builds the kernel using the given config and runs it using QEMU
run config *CARGO_ARGS="": (build config CARGO_ARGS) (_run config "target/k23/bootimg.bin")

# Builds the kernel using the given config
build config *CARGO_ARGS="": && (_make_bootimg config "target/k23/payload" CARGO_ARGS)
    #!/usr/bin/env nu
    let config = open {{config}}
    let target = try { $config | get kernel.target } catch { $config | get target }

    let out_dir = "{{_target_dir}}" | path join "k23"
    mkdir $out_dir

    let cargo_out = ({{_cargo}} build
        -p kernel
        --target $target
        --profile {{profile}}
        --message-format=json
        {{_buildstd}}
        {{CARGO_ARGS}})
    cp ($cargo_out | from json --objects | last 2 | get 0.executable) ($out_dir | path join payload)

# Runs the tests for the kernel
test config *CARGO_ARGS="" :
    #!/usr/bin/env nu
    let config = open {{config}}
    let target = try { $config.kernel.target } catch { $config.target }
    let triple = try { $config.kernel.target-triple } catch { $config.target-triple }

    # CARGO_TARGET_<triple>_RUNNER
    $env.CARGO_TARGET_RISCV64GC_K23_NONE_KERNEL_RUNNER = "just profile={{profile}} _runner {{config}}"

    {{ _cargo }} test -p kernel --target $target {{ _buildstd }} {{ CARGO_ARGS }}

# This is a helper recipe designed to be used as a cargo *target runner*
# When running tests, the `cargo test` command will produce potentially many executables.
# The paths to these files are not known ahead of time, so we use this runner trick to package each executable
# into a bootable image and run it using QEMU.
_runner config binary *ARGS: (_make_bootimg config binary) (_run config "target/k23/bootimg.bin" ARGS)

# Runs the given binary that has been built using the config using QEMU
#
# This recipe is designed to be used as a dependency of other, user-facing recipes (such as `run` and `test`)
_run config binary *ARGS:
    #!/usr/bin/env nu
    let config = open {{ config }}
    let runner = $config.runner

    let cpu = match $runner {
      "qemu-system-riscv64" => "rv64"
    }

    (run-external $runner
        "-kernel"
        {{binary}}
        "-machine" "virt"
        "-cpu" $cpu
        "-smp" "1"
        "-m" "512M"
        "-d" "guest_errors,int"
        "-nographic"
        "-semihosting-config"
        "enable=on,target=native"
        #"-monitor"
        #"unix:qemu-monitor-socket,server,nowait"
        )

# This takes in the given config and payload and creates a bootable image. This is the "magic" build step that makes it all work!
#
# The boot image is created by:
# 1. Building the bootloader
# 2. Compressing the payload
# 3. Signing the compressed payload
# 4. Embedding the public key, signature and compressed payload in the bootloader
#
# This recipe is designed to be used as a dependency of other, user-facing recipes
_make_bootimg config payload *CARGO_ARGS="":
    #!/usr/bin/env nu
    let config = open {{config}}
    let target = try { $config.loader.target } catch { $config.target }

    let out_dir = "{{_target_dir}}" | path join "k23"
    mkdir $out_dir

    let loader_path = ($out_dir | path join loader)
    let secret_key_path = ($out_dir | path join secret.der)
    let public_key_path = ($out_dir | path join pubkey.bin)
    let signature_path = ($out_dir | path join signature.bin)
    let bootimg_path = ($out_dir | path join bootimg.bin)

    # Step 1: Compress the payload
    let payload_lz4_path = "{{payload}}.lz4"
    {{_cargo}} run -p lz4-block-compress {{payload}} $payload_lz4_path

    # Step 2: Sign the compressed payload
    # Write ed25519 key pair
    echo "{{_signing_key}}" | openssl pkey -outform DER -out $secret_key_path
    # Do the actual signing
    openssl pkeyutl -sign -inkey $secret_key_path -out $signature_path -rawin -in $payload_lz4_path
    # Extract the 32-byte public key
    openssl pkey -in $secret_key_path -pubout -outform DER | tail -c 32 | save -f $public_key_path

    # Assign environment variables so we can pick it up in the loader build script
    $env.K23_VERIFYING_KEY_PATH = $public_key_path
    $env.K23_SIGNATURE_PATH = $signature_path
    $env.K23_PAYLOAD_PATH = $payload_lz4_path
    $env.K23_PAYLOAD_SIZE = (stat -c %s {{payload}})

    # Step 3: Build the bootloader
    let cargo_out = ({{_cargo}} build
        -p loader
        --target $target
        --profile {{profile}}
        --message-format=json
        {{_buildstd}}
        {{CARGO_ARGS}})
    cp ($cargo_out | from json --objects | last 2 | get 0.executable) $bootimg_path