##!/usr/bin/env just --justfile

_default:
    @echo "justfile for k23"
    @echo "see https://just"
    @echo ""
    @just --list

# Overrides the default Rust toolchain set in `rust-toolchain.toml`.
toolchain := ""

_cargo := "cargo" + if toolchain != "" { " +" + toolchain } else { "" }
_rustflags := env_var_or_default("RUSTFLAGS", "")
_buildstd := "-Z build-std=core,alloc -Z build-std-features=compiler-builtins-mem"
_target_dir := env_var_or_default("CARGO_TARGET_DIR", justfile_directory() / "target")

_signing_key := env_var_or_default("SIGNING_KEY", `openssl genpkey -algorithm Ed25519 -outform pem`)

test config profile="dev" *CARGO_ARGS="":
    #!/usr/bin/env nu
    let config = open {{config}}
    let target = try { $config | get kernel.target } catch { $config | get target }
    let triple = try { $config | get kernel.target-triple } catch { $config | get target-triple }

    # CARGO_TARGET_<triple>_RUNNER
    let var = $"CARGO_TARGET_($triple | str upcase | str replace --all "-" "_")_RUNNER"
    $env.$var = "just _runner {{config}} {{profile}}"

    {{ _cargo }} test -p kernel --target $target {{ _buildstd }} {{ CARGO_ARGS }}

build config profile="dev" *CARGO_ARGS="": && (_make_bootimg config "target/k23/payload" profile CARGO_ARGS)
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

run config profile="dev" *CARGO_ARGS="": (build config profile CARGO_ARGS)
    #!/usr/bin/env nu
    let config = open {{ config }}
    let runner = $config | get runner

    let cpu = match $runner {
      "qemu-system-riscv64" => "rv64"
    }

    (run-external $runner
        "-kernel"
        "./target/k23/bootimg.bin"
        "-machine" "virt"
        "-cpu" $cpu
        "-smp" "1"
        "-m" "512M"
        "-d" "guest_errors,int"
        "-nographic"
        "-semihosting-config"
        "enable=on,target=native"
        "-monitor"
        "unix:qemu-monitor-socket,server,nowait")

_runner config profile binary *ARGS: (_make_bootimg config binary profile)
    error "{{binary}}"

_make_bootimg config payload profile="dev" *CARGO_ARGS="":
    #!/usr/bin/env nu
    let config = open {{config}}
    let target = try { $config | get loader.target } catch { $config | get target }

    let out_dir = "{{_target_dir}}" | path join "k23"
    mkdir $out_dir

    let loader_path = ($out_dir | path join loader)
    let secret_key_path = ($out_dir | path join secret.der)
    let public_key_path = ($out_dir | path join pubkey.bin)
    let signature_path = ($out_dir | path join signature.bin)
    let bootimg_path = ($out_dir | path join bootimg.bin)

    # Step 1: Build the bootloader
    let cargo_out = ({{_cargo}} build
        -p loader
        --target $target
        --profile {{profile}}
        --message-format=json
        {{_buildstd}}
        {{CARGO_ARGS}})
    cp ($cargo_out | from json --objects | last 2 | get 0.executable) $loader_path

    # Step 2: Compress the payload
    lz4 -f -9 "{{payload}}"
    let payload_lz4_path = "{{payload}}.lz4"

    # Step 3: Write ed25519 key pair
    echo "{{_signing_key}}" | openssl pkey -outform DER -out $secret_key_path
    echo $secret_key_path

    # Step 4: Sign the compressed payload
    openssl pkeyutl -sign -inkey $secret_key_path -out $signature_path -rawin -in $payload_lz4_path

    # Step 5: Extract the 32-byte public key
    tail -c 32 $secret_key_path | save -f $public_key_path

    # Step 6: Embed the public key, signature and compressed payload in the bootloader
    (objcopy
      --add-section=.k23_pubkey=($public_key_path)
      --add-section=.k23_siganture=($signature_path)
      --add-section=.k23_payload=($payload_lz4_path)
      $loader_path
      $bootimg_path
    )

