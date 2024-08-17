{
    inputs = {
        nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
        flake-utils.url = "github:numtide/flake-utils";
        naersk = {
            url = "github:nix-community/naersk";
            inputs.nixpkgs.follows = "nixpkgs";
        };
        rust-overlay = {
            url = "github:oxalica/rust-overlay";
            inputs.nixpkgs.follows = "nixpkgs";
        };
    };
    outputs = { nixpkgs, flake-utils, rust-overlay, naersk, ... }:
        flake-utils.lib.eachDefaultSystem (localSystem:
            let
                target = "riscv64gc-unknown-none-elf";

                pkgs = import nixpkgs {
                    inherit localSystem;
                    overlays = [ (import rust-overlay) ];
                };
                inherit (pkgs) lib stdenv writeShellScript symlinkJoin;

                rust-toolchain = pkgs.rust-bin.fromRustupToolchainFile ./rust-toolchain.toml;

                naersk' = naersk.lib.${localSystem}.override {
                    cargo = rust-toolchain;
                    rustc = rust-toolchain;
                };

                fileSetForCrate = crate: lib.fileset.toSource {
                        root = ./.;
                        fileset = lib.fileset.unions [
                            ./Cargo.toml
                            ./Cargo.lock
                            ./kernel
                            ./loader
                            crate
                        ];
                    };

                naerskBuildPackage = target: args:
                naersk'.buildPackage (
                    args // {
                        strictDeps = true;
                        doCheck = false;
                        additionalCargoLock = "${rust-toolchain.availableComponents.rust-src}/lib/rustlib/src/rust/library/Cargo.lock";
                        CARGO_BUILD_TARGET = target;
                        cargoBuildOptions = x: x ++ [ "-Zbuild-std=core,alloc" "-Zbuild-std-features=compiler-builtins-mem" ];
                    }
                );

                kernel = naerskBuildPackage "riscv64gc-unknown-none-elf" {
                    pname = "kernel";
                    src = fileSetForCrate ./kernel;
                };

                loader = naerskBuildPackage "riscv64gc-unknown-none-elf" {
                    pname = "loader";
                    src = fileSetForCrate ./loader;
                };

                bootimg = stdenv.mkDerivation {
                    name = "bootimg";
                    nativeBuildInputs = with pkgs; [ lz4 openssl clang_19 coreutils ];
                    PATH = lib.makeBinPath (with pkgs; [ lz4 openssl clang_19 coreutils ]);
                    builder = writeShellScript "builder.sh" ''
                        # Step 1: Compress the payload
                        lz4 -9 ${kernel}/bin/kernel payload.bin

                        # Step 2: Generate a Ed25519 key pair
                        openssl genpkey -algorithm Ed25519 -out secret.der -outform der

                        # Step 3: Sign the compressed payload
                        openssl pkeyutl -sign -inkey secret.der -out signature.bin -rawin -in payload.bin

                        # Step 4: Extract the 32-byte public key
                        tail -c 32 secret.der > pubkey.bin

                        # Step 5: Embed the public key, signature and compressed payload in the bootloader

                        mkdir $out
                        mkdir $out/bin
                        cp ${loader}/bin/loader $out/bin/loader
                        cp ${kernel}/bin/kernel $out/bin/kernel

                        objcopy --add-section=.k23_pubkey=pubkey.bin --add-section=.k23_siganture=signature.bin --add-section=.k23_payload=payload.bin  ${loader}/bin/loader $out/bin/bootimg
                    '';
                };
            in
            {
                packages = {
                    inherit kernel loader bootimg;

                };
            }
        );
}